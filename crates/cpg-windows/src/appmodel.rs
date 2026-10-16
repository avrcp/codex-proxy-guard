//! Native AppModel package discovery — no PowerShell, no fallback. Enumerates
//! the supported package families with `FindPackagesByPackageFamily`, derives
//! identity through `PackageIdFromFullName`, resolves install paths and
//! parses `AppxManifest.xml` applications through the Appx COM reader.

use crate::cancellation::Cancellation;
use cpg_domain::error::{Error, Result};
use cpg_domain::json::Json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Storage::Packaging::Appx::{
    APPX_PACKAGE_ARCHITECTURE_ARM64, APPX_PACKAGE_ARCHITECTURE_X64, AppxFactory,
    FindPackagesByPackageFamily, GetPackagePathByFullName, IAppxFactory, IAppxManifestApplication,
    IAppxManifestApplicationsEnumerator, IAppxManifestReader, PACKAGE_FILTER_HEAD, PACKAGE_ID,
    PACKAGE_VERSION, PackageIdFromFullName,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Com::{STGM_READ, STGM_SHARE_DENY_NONE};
use windows::Win32::System::SystemInformation::{
    GetNativeSystemInfo, PROCESSOR_ARCHITECTURE_AMD64, PROCESSOR_ARCHITECTURE_ARM64, SYSTEM_INFO,
};
use windows::Win32::UI::Shell::SHCreateStreamOnFileEx;

const SUPPORTED: &[(&str, &str)] = &[
    ("OpenAI.Codex_2p2nqsd0c76g0", "OpenAI.Codex"),
    (
        "OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0",
        "OpenAI.ChatGPT-Desktop",
    ),
];

fn fail<T>(code: &str, message: &str) -> Result<T> {
    Err(Error::new(code, message))
}

/// RAII COM apartment (per-thread).
pub struct ComScope {
    result: windows::core::HRESULT,
}

impl ComScope {
    pub fn new() -> Result<ComScope> {
        // SAFETY: initializes COM on this thread exactly once per scope; the
        // matching CoUninitialize runs in Drop when initialization succeeded.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Ok(ComScope { result })
    }

    pub fn hr(&self) -> windows::core::HRESULT {
        self.result
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.result.is_ok() {
            // SAFETY: paired with the successful CoInitializeEx above.
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

struct PackageIdentity {
    name: String,
    full_name: String,
    version: PACKAGE_VERSION,
    architecture: u32,
}

fn package_id_of(full_name: &str) -> Option<PackageIdentity> {
    let wide: Vec<u16> = full_name.encode_utf16().chain([0]).collect();
    let mut size: u32 = 0;
    const PACKAGE_INFORMATION_FULL: u32 = 0;
    // SAFETY: sizing call into a caller-owned buffer contract.
    let status = unsafe {
        PackageIdFromFullName(
            windows::core::PCWSTR(wide.as_ptr()),
            PACKAGE_INFORMATION_FULL,
            &mut size,
            None,
        )
    };
    if status.0 != 122 || size < std::mem::size_of::<PACKAGE_ID>() as u32 || size > 65536 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    // SAFETY: exact-sized buffer for the returned PACKAGE_ID plus strings.
    let status = unsafe {
        PackageIdFromFullName(
            windows::core::PCWSTR(wide.as_ptr()),
            PACKAGE_INFORMATION_FULL,
            &mut size,
            Some(buffer.as_mut_ptr()),
        )
    };
    if status.0 != 0 || (size as usize) < std::mem::size_of::<PACKAGE_ID>() {
        return None;
    }
    // SAFETY: the buffer was returned by PackageIdFromFullName with a
    // length at least the PACKAGE_ID header; reading it is the documented
    // contract of that API.
    let identifier = &unsafe { *(buffer.as_ptr() as *const PACKAGE_ID) };
    let read_wide = |pointer: windows::core::PWSTR| -> String {
        if pointer.is_null() {
            return String::new();
        }
        // SAFETY: the pointer references a null-terminated string inside the
        // same caller-owned buffer that is still alive here.
        unsafe {
            let mut length = 0usize;
            while *pointer.0.add(length) != 0 {
                length += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(pointer.0, length))
        }
    };
    Some(PackageIdentity {
        name: read_wide(identifier.name),
        full_name: full_name.to_owned(),
        version: identifier.version,
        architecture: identifier.processorArchitecture,
    })
}

fn package_path_of(full_name: &str) -> Option<String> {
    let wide: Vec<u16> = full_name.encode_utf16().chain([0]).collect();
    let mut length: u32 = 0;
    // SAFETY: sizing call.
    let status = unsafe {
        GetPackagePathByFullName(windows::core::PCWSTR(wide.as_ptr()), &mut length, None)
    };
    if status.0 != 122 || length == 0 || length > 32768 {
        return None;
    }
    let mut buffer = vec![0u16; length as usize];
    // SAFETY: exact-sized buffer.
    let status = unsafe {
        GetPackagePathByFullName(
            windows::core::PCWSTR(wide.as_ptr()),
            &mut length,
            Some(windows::core::PWSTR(buffer.as_mut_ptr())),
        )
    };
    if status.0 != 0 || length == 0 || length as usize > buffer.len() {
        return None;
    }
    buffer.truncate(length as usize - 1); // drop the written NUL
    Some(String::from_utf16_lossy(&buffer))
}

fn architecture_name(architecture: u32) -> &'static str {
    match architecture {
        0 => "X86",
        5 => "Arm", // APPX_PACKAGE_ARCHITECTURE_ARM
        6 => "X64",
        12 => "Arm64",
        11 => "Neutral",
        _ => "Unknown",
    }
}

fn package_full_names(
    family: &str,
    cancel: &Cancellation,
    deadline: Instant,
) -> Result<Vec<String>> {
    let wide: Vec<u16> = family.encode_utf16().chain([0]).collect();
    for _ in 0..5 {
        cancel.check()?;
        if Instant::now() >= deadline {
            return fail(
                "APPX_DISCOVERY_TIMEOUT",
                "Package discovery exceeded its deadline",
            );
        }
        let mut count: u32 = 0;
        let mut length: u32 = 0;
        // SAFETY: sizing call with null buffers.
        let status = unsafe {
            FindPackagesByPackageFamily(
                windows::core::PCWSTR(wide.as_ptr()),
                PACKAGE_FILTER_HEAD,
                &mut count,
                None,
                &mut length,
                None,
                None,
            )
        };
        const APPMODEL_ERROR_NO_PACKAGE: u32 = 15700;
        if status.0 == APPMODEL_ERROR_NO_PACKAGE || (status.0 == 0 && count == 0) {
            return Ok(Vec::new());
        }
        if count > 16 || length > 65536 {
            return fail(
                "APPX_DISCOVERY_LIMIT",
                "Package enumeration exceeded its capacity limit",
            );
        }
        if status.0 == 122 {
            if count == 0 || length == 0 {
                return fail(
                    "APPX_DISCOVERY_FAILED",
                    "Package enumeration returned invalid capacity",
                );
            }
            let mut names = vec![windows::core::PWSTR::null(); count as usize];
            let mut buffer = vec![0u16; length as usize];
            let mut properties = vec![0u32; count as usize];
            // SAFETY: exact-sized name pointer array and buffer.
            let status = unsafe {
                FindPackagesByPackageFamily(
                    windows::core::PCWSTR(wide.as_ptr()),
                    PACKAGE_FILTER_HEAD,
                    &mut count,
                    Some(names.as_mut_ptr()),
                    &mut length,
                    Some(windows::core::PWSTR(buffer.as_mut_ptr())),
                    Some(properties.as_mut_ptr()),
                )
            };
            if status.0 != 0 || count as usize > names.len() || length as usize > buffer.len() {
                return fail("APPX_DISCOVERY_FAILED", "Package family enumeration failed");
            }
            let mut result = Vec::with_capacity(count as usize);
            let base = buffer.as_ptr() as usize;
            let end = base + buffer.len() * 2;
            for name in &names[..count as usize] {
                let address = name.0 as usize;
                if address < base || address >= end || !(address - base).is_multiple_of(2) {
                    return fail(
                        "APPX_DISCOVERY_FAILED",
                        "Package enumeration returned an invalid name",
                    );
                }
                let offset = (address - base) / 2;
                let tail = &buffer[offset..];
                let name_length = tail
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(tail.len());
                if name_length == 0 {
                    return fail(
                        "APPX_DISCOVERY_FAILED",
                        "Package enumeration returned an invalid name",
                    );
                }
                result.push(String::from_utf16_lossy(&tail[..name_length]));
            }
            return Ok(result);
        }
        if status.0 != 0 {
            return fail("APPX_DISCOVERY_FAILED", "Package family enumeration failed");
        }
    }
    fail(
        "APPX_DISCOVERY_UNSTABLE",
        "Package enumeration did not stabilize within its retry budget",
    )
}

fn manifest_string(
    application: &IAppxManifestApplication,
    name: &windows::core::HSTRING,
) -> Result<Json> {
    // SAFETY: COM call on a live interface pointer; the returned string is
    // owned by us and freed through CoTaskMemFree.
    let value = match unsafe { application.GetStringValue(name) } {
        Ok(value) => value,
        Err(status) => {
            let code = if status.code() == windows::core::HRESULT(0x8007_0057u32 as i32) {
                "APPX_MANIFEST_ATTRIBUTE_UNSUPPORTED"
            } else {
                "APPX_MANIFEST_READ_FAILED"
            };
            return Err(Error::new(
                code,
                format!(
                    "Cannot read the supported manifest attribute (HRESULT {})",
                    status.code().0
                ),
            ));
        }
    };
    let text = if value.is_null() {
        String::new()
    } else {
        // SAFETY: value references a CoTaskMem string owned by this call.
        unsafe { value.to_string() }.unwrap_or_default()
    };
    // SAFETY: freeing the CoTaskMem allocation from GetStringValue.
    unsafe { CoTaskMemFree(Some(value.as_ptr() as *const core::ffi::c_void)) };
    if text.is_empty() {
        return Ok(Json::Null);
    }
    Ok(Json::from(text.replace('\\', "/")))
}

fn manifest_applications(
    manifest_path: &Path,
    cancel: &Cancellation,
    deadline: Instant,
) -> Result<Json> {
    // SAFETY: stream creation on a real manifest path; COM factory creation.
    unsafe {
        let wide: Vec<u16> = manifest_path
            .to_string_lossy()
            .encode_utf16()
            .chain([0])
            .collect();
        let stream = match SHCreateStreamOnFileEx(
            windows::core::PCWSTR(wide.as_ptr()),
            (STGM_READ | STGM_SHARE_DENY_NONE).0,
            0,
            false,
            None,
        ) {
            Ok(stream) => stream,
            Err(_) => return fail("APPX_DISCOVERY_FAILED", "Cannot open the package manifest"),
        };
        let factory: IAppxFactory = match CoCreateInstance(&AppxFactory, None, CLSCTX_INPROC_SERVER)
        {
            Ok(factory) => factory,
            Err(_) => {
                return fail(
                    "APPX_DISCOVERY_FAILED",
                    "Cannot create the manifest reader factory",
                );
            }
        };
        let reader: IAppxManifestReader = match factory.CreateManifestReader(&stream) {
            Ok(reader) => reader,
            Err(_) => return fail("APPX_DISCOVERY_FAILED", "Cannot read the package manifest"),
        };
        let applications: IAppxManifestApplicationsEnumerator = match reader.GetApplications() {
            Ok(applications) => applications,
            Err(_) => {
                return fail(
                    "APPX_DISCOVERY_FAILED",
                    "Cannot enumerate manifest applications",
                );
            }
        };
        let mut has_current = match applications.GetHasCurrent() {
            Ok(value) => value.as_bool(),
            Err(_) => {
                return fail(
                    "APPX_DISCOVERY_FAILED",
                    "Cannot enumerate manifest applications",
                );
            }
        };
        let mut records: Vec<Json> = Vec::new();
        while has_current {
            cancel.check()?;
            if Instant::now() >= deadline {
                return fail(
                    "APPX_DISCOVERY_TIMEOUT",
                    "Package discovery exceeded its deadline",
                );
            }
            let application: IAppxManifestApplication = match applications.GetCurrent() {
                Ok(application) => application,
                Err(_) => {
                    return fail(
                        "APPX_DISCOVERY_FAILED",
                        "Cannot read a manifest application",
                    );
                }
            };
            let record = Json::object_from([
                (
                    "application_id",
                    manifest_string(&application, windows::core::h!("Id"))?,
                ),
                (
                    "manifest_executable",
                    manifest_string(&application, windows::core::h!("Executable"))?,
                ),
                (
                    "entry_point",
                    manifest_string(&application, windows::core::h!("EntryPoint"))?,
                ),
                ("runtime_behavior", Json::Null),
                ("trust_level", Json::Null),
            ]);
            records.push(record);
            if records.len() > 16 {
                return fail("APPX_DISCOVERY_FAILED", "Package has too many applications");
            }
            match applications.MoveNext() {
                Ok(value) => has_current = value.as_bool(),
                Err(_) => {
                    return fail(
                        "APPX_DISCOVERY_FAILED",
                        "Cannot enumerate manifest applications",
                    );
                }
            }
        }
        Ok(Json::Array(records))
    }
}

fn version_parts(version: &PACKAGE_VERSION) -> (u16, u16, u16, u16) {
    // SAFETY: the union holds a plain u64 bit layout; reading the anonymous
    // struct view is the documented representation.
    let parts = unsafe { version.Anonymous.Anonymous };
    (parts.Major, parts.Minor, parts.Build, parts.Revision)
}

fn version_less(a: &PACKAGE_VERSION, b: &PACKAGE_VERSION) -> bool {
    version_parts(a) < version_parts(b)
}

/// Native discovery envelope (`{"schema_version":1,"records":[…]}`).
pub fn discover_packages_native(cancel: &Cancellation) -> Result<Json> {
    cancel.check()?;
    let com = ComScope::new()?;
    if com.hr().is_err() {
        return fail(
            "APPX_DISCOVERY_COM_FAILED",
            &format!(
                "Cannot initialize package discovery COM (HRESULT {})",
                com.hr().0
            ),
        );
    }
    cancel.check()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut records: Vec<Json> = Vec::new();
    for (family, package_name) in SUPPORTED {
        cancel.check()?;
        let full_names = package_full_names(family, cancel, deadline)?;
        if full_names.is_empty() {
            continue;
        }
        // Highest version wins; ties prefer the native architecture.
        let mut best: Option<PackageIdentity> = None;
        // SAFETY: native system info query into a local struct.
        let mut info = SYSTEM_INFO::default();
        unsafe { GetNativeSystemInfo(&mut info) };
        let architecture = unsafe { info.Anonymous.Anonymous }.wProcessorArchitecture;
        let native_machine: u32 = if architecture == PROCESSOR_ARCHITECTURE_AMD64 {
            APPX_PACKAGE_ARCHITECTURE_X64.0 as u32
        } else if architecture == PROCESSOR_ARCHITECTURE_ARM64 {
            APPX_PACKAGE_ARCHITECTURE_ARM64.0 as u32
        } else {
            0xffff_ffff
        };
        for full_name in &full_names {
            cancel.check()?;
            let Some(identity) = package_id_of(full_name) else {
                continue;
            };
            if identity.name != *package_name {
                continue; // allowlist drift
            }
            let newer = best
                .as_ref()
                .map(|current| version_less(&current.version, &identity.version))
                .unwrap_or(false);
            let native_swap = best.as_ref().is_some_and(|current| {
                !version_less(&identity.version, &current.version)
                    && identity.architecture == native_machine
                    && current.architecture != native_machine
            });
            if best.is_none() || newer || native_swap {
                best = Some(identity);
            }
        }
        let Some(best) = best else {
            continue;
        };
        let install_location = match package_path_of(&best.full_name) {
            Some(path) => path,
            None => {
                return fail(
                    "APPX_DISCOVERY_FAILED",
                    "Package install location is unavailable",
                );
            }
        };
        let (major, minor, build, revision) = version_parts(&best.version);
        let version = format!("{major}.{minor}.{build}.{revision}");
        let record = Json::object_from([
            ("package_name", Json::from(best.name.as_str())),
            ("package_full_name", Json::from(best.full_name.as_str())),
            ("package_family_name", Json::from(*family)),
            ("package_version", Json::from(version.as_str())),
            (
                "architecture",
                Json::from(architecture_name(best.architecture)),
            ),
            ("install_location", Json::from(install_location.as_str())),
            (
                "applications",
                manifest_applications(
                    &PathBuf::from(&install_location).join("AppxManifest.xml"),
                    cancel,
                    deadline,
                )?,
            ),
        ]);
        records.push(record);
    }
    Ok(Json::object_from([
        ("schema_version", Json::Int(1)),
        ("records", Json::Array(records)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_supported_packages_on_this_machine() {
        // Read-only AppModel enumeration; safe on a developer machine.
        let cancel = Cancellation::new();
        let envelope = discover_packages_native(&cancel).expect("native discovery");
        let records = envelope.at("records").as_array().expect("records");
        for record in records {
            let name = record.string_at("package_name");
            assert!(
                name == "OpenAI.Codex" || name == "OpenAI.ChatGPT-Desktop",
                "only allowlisted families are recorded, got {name}"
            );
            assert!(!record.string_at("package_full_name").is_empty());
            assert!(record.string_at("install_location").starts_with('C'));
            let apps = record.at("applications").as_array().expect("applications");
            assert!(!apps.is_empty());
            for app in apps {
                assert!(app.get("application_id").is_some());
                assert!(app.get("manifest_executable").is_some());
            }
        }
    }
}

//! Application-model activation: `IApplicationActivationManager::ActivateApplication`
//! with `AO_NONE` on a dynamically resolved AUMID, followed by handle-based
//! identity verification of what was actually activated. Runs inside the
//! short-lived activation worker only.

use crate::elevation::process_elevated;
use crate::handles::OwnedHandle;
use crate::paths::normalized_windows_path;
use cpg_domain::json::Json;
use cpg_domain::worker::empty_receipt;
use std::path::Path;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Foundation::WAIT_EVENT;
use windows::Win32::Storage::FileSystem::SYNCHRONIZE as SYNCHRONIZE_ACCESS;
use windows::Win32::Storage::Packaging::Appx::{GetApplicationUserModelId, GetPackageFullName};
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    AO_NONE, ApplicationActivationManager, IApplicationActivationManager,
};
use windows::core::HSTRING;

fn ticks(time: &FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// AppModel string query outcome: `None` models "missing", an empty string
/// models "query_failed", anything else is the observed value.
fn query_appmodel_string(
    process: windows::Win32::Foundation::HANDLE,
    application: bool,
) -> Option<String> {
    let mut size: u32 = 0;
    // SAFETY: sizing call with a null buffer.
    let status = if application {
        unsafe { GetApplicationUserModelId(process, &mut size, None) }
    } else {
        unsafe { GetPackageFullName(process, &mut size, None) }
    };
    const APPMODEL_ERROR_NO_PACKAGE: u32 = 15700;
    const APPMODEL_ERROR_NO_APPLICATION: u32 = 15703;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    if status.0 == APPMODEL_ERROR_NO_PACKAGE
        || (application && status.0 == APPMODEL_ERROR_NO_APPLICATION)
    {
        return None;
    }
    if status.0 != ERROR_INSUFFICIENT_BUFFER || !(2..=2048).contains(&size) {
        return Some(String::new());
    }
    let mut buffer = vec![0u16; size as usize];
    // SAFETY: exact-sized buffer for the returned string.
    let status = if application {
        unsafe {
            GetApplicationUserModelId(
                process,
                &mut size,
                Some(windows::core::PWSTR(buffer.as_mut_ptr())),
            )
        }
    } else {
        unsafe {
            GetPackageFullName(
                process,
                &mut size,
                Some(windows::core::PWSTR(buffer.as_mut_ptr())),
            )
        }
    };
    if status.0 != 0 || size < 2 || size as usize > buffer.len() || buffer[(size - 1) as usize] != 0
    {
        return Some(String::new());
    }
    buffer.truncate((size - 1) as usize);
    let text = String::from_utf16(&buffer).ok()?;
    if text.contains('\0') {
        return Some(String::new());
    }
    Some(text)
}

fn observe_identity(
    receipt: &mut Json,
    process: windows::Win32::Foundation::HANDLE,
    expected: &str,
    state_field: &str,
    observed_field: &str,
    application: bool,
) {
    match query_appmodel_string(process, application) {
        None => {
            receipt.insert(state_field, Json::from("missing"));
        }
        Some(text) if text.is_empty() => {
            receipt.insert(state_field, Json::from("query_failed"));
        }
        Some(text) => {
            let state = if text == expected {
                "matched"
            } else {
                "mismatch"
            };
            receipt.insert(observed_field, Json::from(text.as_str()));
            receipt.insert(state_field, Json::from(state));
        }
    }
}

/// Port of `classifyInstance`.
pub fn classify_instance(creation: u64, submission: u64) -> &'static str {
    if creation == 0 || submission == 0 || creation == submission {
        "unknown"
    } else if creation < submission {
        "reused"
    } else {
        "created"
    }
}

fn observe_image(receipt: &mut Json, actual: &str, expected: &str) {
    if actual.is_empty() {
        receipt.insert(
            "observation_failure",
            Json::from("APPX_TARGET_IMAGE_QUERY_FAILED: Cannot query activated executable"),
        );
    } else if normalized_windows_path(Path::new(actual))
        != normalized_windows_path(Path::new(expected))
    {
        receipt.insert(
            "observation_failure",
            Json::from(
                "APPX_TARGET_IMAGE_MISMATCH: Activated executable differs from selected Desktop",
            ),
        );
    }
}

fn image_path_of(process: windows::Win32::Foundation::HANDLE) -> String {
    let mut buffer = [0u16; 32768];
    let mut size = buffer.len() as u32;
    // SAFETY: owned handle; sized buffer with its length passed in and out.
    if unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    }
    .is_err()
    {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..size as usize])
}

/// Port of `activateAndObserve`: performs the activation and fills the
/// receipt. All failures are recorded as receipt facts, not exceptions.
pub fn activate_and_observe(request: &Json) -> Json {
    let mut receipt = empty_receipt();
    let com = match crate::appmodel::ComScope::new() {
        Ok(com) => com,
        Err(_) => {
            receipt.insert(
                "failure",
                Json::from("APPX_ACTIVATION_COM_INIT_FAILED: Cannot initialize COM"),
            );
            return receipt;
        }
    };
    if com.hr().is_err() {
        receipt.insert(
            "failure",
            Json::from("APPX_ACTIVATION_COM_INIT_FAILED: Cannot initialize COM"),
        );
        return receipt;
    }
    // SAFETY: COM creation on this thread's apartment; the interface pointer
    // is released by its wrapper.
    let manager: IApplicationActivationManager =
        match unsafe { CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER) }
        {
            Ok(manager) => manager,
            Err(_) => {
                receipt.insert(
                    "failure",
                    Json::from(
                        "APPX_ACTIVATION_MANAGER_UNAVAILABLE: Cannot create activation manager",
                    ),
                );
                return receipt;
            }
        };
    // SAFETY: returns the current filetime by value.
    let submitted: FILETIME = unsafe { GetSystemTimeAsFileTime() };
    let aumid = request.string_at("aumid");
    let arguments = request.string_at("arguments");
    // SAFETY: ActivateApplication with AO_NONE; the PID returns by value.
    let activated = unsafe {
        manager.ActivateApplication(&HSTRING::from(&aumid), &HSTRING::from(&arguments), AO_NONE)
    };
    let pid = match activated {
        Ok(pid) => pid,
        Err(error) => {
            let text = format!("0x{:08X}", (error.code().0 as u32));
            receipt.insert("activation_hresult", Json::from(text.as_str()));
            return receipt;
        }
    };
    receipt.insert("activation_returned", Json::Bool(true));
    receipt.insert("pid", Json::from(u64::from(pid)));
    // SAFETY: opening the activated process by the returned PID; the handle
    // is owned for all observations below.
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_ACCESS_RIGHTS(SYNCHRONIZE_ACCESS.0),
            false,
            pid,
        )
    }
    .ok()
    .and_then(OwnedHandle::new);
    let Some(process) = process else {
        receipt.insert("package_identity", Json::from("query_failed"));
        receipt.insert("aumid", Json::from("query_failed"));
        receipt.insert(
            "observation_failure",
            Json::from("APPX_TARGET_VERIFY_FAILED: Cannot open activated process"),
        );
        return receipt;
    };
    observe_identity(
        &mut receipt,
        process.raw(),
        &request.string_at("expected_package_full_name"),
        "package_identity",
        "observed_package_full_name",
        false,
    );
    observe_identity(
        &mut receipt,
        process.raw(),
        &aumid,
        "aumid",
        "observed_aumid",
        true,
    );
    match process_elevated(process.raw()) {
        Some(elevated) => {
            receipt.insert("elevation", Json::Bool(elevated));
        }
        None => {
            receipt.insert(
                "observation_failure",
                Json::from("APPX_TARGET_ELEVATION_QUERY_FAILED: Cannot query target elevation"),
            );
        }
    }
    let mut creation = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: owned handle; four local out-structs.
    if unsafe {
        GetProcessTimes(
            process.raw(),
            &mut creation,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .is_ok()
    {
        receipt.insert(
            "instance",
            Json::from(classify_instance(ticks(&creation), ticks(&submitted))),
        );
    } else {
        receipt.insert(
            "observation_failure",
            Json::from("APPX_TARGET_TIME_QUERY_FAILED: Cannot query process creation time"),
        );
    }
    let actual_path = image_path_of(process.raw());
    observe_image(
        &mut receipt,
        &actual_path,
        &request.string_at("expected_executable"),
    );
    // SAFETY: owned handle; 150 ms lifetime probe. WAIT_TIMEOUT == 258.
    let wait: WAIT_EVENT = unsafe { WaitForSingleObject(process.raw(), 150) };
    if wait == WAIT_EVENT(0) {
        let mut code: u32 = 0;
        // SAFETY: owned handle.
        if unsafe { GetExitCodeProcess(process.raw(), &mut code) }.is_ok() {
            receipt.insert("early_exit_code", Json::from(u64::from(code)));
        } else {
            receipt.insert(
                "observation_failure",
                Json::from("APPX_TARGET_EXIT_QUERY_FAILED: Cannot query early exit"),
            );
        }
    } else if wait != WAIT_EVENT(258) {
        receipt.insert(
            "observation_failure",
            Json::from("APPX_TARGET_WAIT_FAILED: Cannot observe target lifetime"),
        );
    }
    receipt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_classification_boundaries() {
        assert_eq!(classify_instance(0, 5), "unknown");
        assert_eq!(classify_instance(5, 0), "unknown");
        assert_eq!(classify_instance(5, 5), "unknown");
        assert_eq!(classify_instance(4, 5), "reused");
        assert_eq!(classify_instance(6, 5), "created");
    }

    #[test]
    fn com_initializes_on_this_thread() {
        // Apartment-threaded init and clean teardown; RPC_E_CHANGED_MODE
        // (0x80010106) is tolerated exactly like the C++ ComScope.
        let scope = crate::appmodel::ComScope::new().expect("COM initializes");
        assert!(scope.hr().is_ok() || scope.hr() == windows::core::HRESULT(0x8001_0106u32 as i32));
    }
}

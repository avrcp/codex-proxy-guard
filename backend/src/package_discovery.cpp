#include "package_discovery.h"
#include "package_discovery_p.h"
#include "cpg/win_handle.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <appmodel.h>
#include <appxpackaging.h>
#include <shlwapi.h>

#include <algorithm>
#include <cstdint>
#include <cwchar>
#include <optional>
#include <string>
#include <vector>

namespace cpg {
namespace {

[[noreturn]] void fail(const std::string &code, const std::string &message)
{
    throw Error(code, message);
}

struct SupportedPackage {
    const wchar_t *familyName;
    const wchar_t *packageName;
};

// Package family allowlist. The publisher id (2p2nqsd0c76g0) is measured
// from the installed OpenAI.Codex package; a family name is
// <Name>_<PublisherId> by construction, and the same publisher therefore
// yields the Classic spelling below. A stale allowlist can only produce
// zero records (CODEX_NOT_INSTALLED, fail closed) — package identity is
// re-derived from PackageIdFromFullName and compared before any launch.
const SupportedPackage supportedPackages[] = {
    {L"OpenAI.Codex_2p2nqsd0c76g0", L"OpenAI.Codex"},
    {L"OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0", L"OpenAI.ChatGPT-Desktop"},
};

struct ComScope {
    const HRESULT result;
    ComScope() : result(CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED)) {}
    ~ComScope() { if (SUCCEEDED(result)) CoUninitialize(); }
    ComScope(const ComScope &) = delete;
    ComScope &operator=(const ComScope &) = delete;
};

template <typename T>
struct ComPtr {
    T *value = nullptr;
    ComPtr() = default;
    ~ComPtr() { if (value) value->Release(); }
    ComPtr(const ComPtr &) = delete;
    ComPtr &operator=(const ComPtr &) = delete;
    T **out() { return &value; }
    T *operator->() const { return value; }
};

std::string architectureName(const UINT32 architecture)
{
    // ProcessorArchitecture.ToString() spellings, matching the values the
    // PowerShell baseline emitted for Get-AppxPackage's Architecture field.
    // The numeric values are the APPX_PACKAGE_ARCHITECTURE ones.
    switch (architecture) {
    case APPX_PACKAGE_ARCHITECTURE_X86: return "X86";
    case APPX_PACKAGE_ARCHITECTURE_ARM: return "Arm";
    case APPX_PACKAGE_ARCHITECTURE_X64: return "X64";
    case APPX_PACKAGE_ARCHITECTURE_ARM64: return "Arm64";
    case APPX_PACKAGE_ARCHITECTURE_NEUTRAL: return "Neutral";
    default: return "Unknown";
    }
}

// Owned copy of PackageIdFromFullName's result: the raw PACKAGE_ID points
// into a caller-owned buffer, so strings are copied into std::wstring.
struct PackageIdentity {
    std::wstring name;
    std::wstring publisherId;
    std::wstring fullName;
    PACKAGE_VERSION version{};
    UINT32 processorArchitecture = 0xffffffffu;
};

std::optional<PackageIdentity> packageIdOf(const std::wstring &fullName)
{
    UINT32 size = 0;
    LONG status = PackageIdFromFullName(fullName.c_str(), PACKAGE_INFORMATION_FULL,
                                        &size, nullptr);
    if (status != ERROR_INSUFFICIENT_BUFFER || size < sizeof(PACKAGE_ID) || size > 65536) return std::nullopt;
    std::vector<BYTE> buffer(size);
    status = PackageIdFromFullName(fullName.c_str(), PACKAGE_INFORMATION_FULL,
                                   &size, buffer.data());
    if (status != ERROR_SUCCESS || size < sizeof(PACKAGE_ID) || size > buffer.size()) return std::nullopt;
    PACKAGE_ID identifier{};
    std::memcpy(&identifier, buffer.data(), sizeof(PACKAGE_ID));
    PackageIdentity identity;
    identity.name = identifier.name ? identifier.name : L"";
    identity.publisherId = identifier.publisherId ? identifier.publisherId : L"";
    identity.fullName = fullName;
    identity.version = identifier.version;
    identity.processorArchitecture = identifier.processorArchitecture;
    return identity;
}

std::wstring packagePathOf(const std::wstring &fullName)
{
    UINT32 length = 0;
    LONG status = GetPackagePathByFullName(fullName.c_str(), &length, nullptr);
    if (status != ERROR_INSUFFICIENT_BUFFER || length == 0 || length > 32768) return {};
    std::wstring path(length, L'\0');
    status = GetPackagePathByFullName(fullName.c_str(), &length, path.data());
    if (status != ERROR_SUCCESS || length == 0 || length > path.size() || path[length - 1] != L'\0') return {};
    path.resize(length - 1);
    return path;
}

// Read one manifest string attribute; missing attributes read as null (the
// same protocol state the PowerShell baseline emitted for absent
// RuntimeBehavior/TrustLevel). The Appx reader canonicalizes path separators
// to backslashes; Executable is normalized to the forward-slash spelling the
// baseline and every receipt consumer use.
Json manifestString(IAppxManifestApplication *application, const wchar_t *name, bool normalizeSeparators)
{
    LPWSTR value = nullptr;
    const HRESULT status = application->GetStringValue(name, &value);
    if (FAILED(status)) {
        if (value) CoTaskMemFree(value);
        fail(status == E_INVALIDARG ? "APPX_MANIFEST_ATTRIBUTE_UNSUPPORTED" : "APPX_MANIFEST_READ_FAILED",
             "Cannot read the supported manifest attribute (HRESULT " + std::to_string(static_cast<long>(status)) + ")");
    }
    std::wstring text(value ? value : L"");
    if (value) CoTaskMemFree(value);
    if (text.empty()) return nullptr;
    if (normalizeSeparators)
        for (wchar_t &character : text)
            if (character == L'\\') character = L'/';
    return wideToUtf8(text);
}

Json manifestApplications(const std::filesystem::path &manifestPath, const Cancellation &cancel,
    std::chrono::steady_clock::time_point deadline)
{
    ComPtr<IStream> stream;
    if (FAILED(SHCreateStreamOnFileEx(manifestPath.c_str(), STGM_READ | STGM_SHARE_DENY_NONE,
                                      0, FALSE, nullptr, stream.out())))
        fail("APPX_DISCOVERY_FAILED", "Cannot open the package manifest");
    ComPtr<IAppxFactory> factory;
    const HRESULT created = CoCreateInstance(CLSID_AppxFactory, nullptr, CLSCTX_INPROC_SERVER,
                                             IID_PPV_ARGS(factory.out()));
    if (FAILED(created) || !factory.value)
        fail("APPX_DISCOVERY_FAILED", "Cannot create the manifest reader factory");
    ComPtr<IAppxManifestReader> reader;
    if (FAILED(factory->CreateManifestReader(stream.value, reader.out())) || !reader.value)
        fail("APPX_DISCOVERY_FAILED", "Cannot read the package manifest");
    ComPtr<IAppxManifestApplicationsEnumerator> applications;
    if (FAILED(reader->GetApplications(applications.out())) || !applications.value)
        fail("APPX_DISCOVERY_FAILED", "Cannot enumerate manifest applications");
    BOOL hasCurrent = FALSE;
    if (FAILED(applications->GetHasCurrent(&hasCurrent)))
        fail("APPX_DISCOVERY_FAILED", "Cannot enumerate manifest applications");
    Json records = Json::array();
    while (hasCurrent) {
        cancel.check();
        if (std::chrono::steady_clock::now() >= deadline)
            fail("APPX_DISCOVERY_TIMEOUT", "Package discovery exceeded its deadline");
        ComPtr<IAppxManifestApplication> application;
        if (FAILED(applications->GetCurrent(application.out())) || !application.value)
            fail("APPX_DISCOVERY_FAILED", "Cannot read a manifest application");
        Json record{{"application_id", manifestString(application.value, L"Id", false)},
                    {"manifest_executable", manifestString(application.value, L"Executable", true)},
                    {"entry_point", manifestString(application.value, L"EntryPoint", false)},
                    {"runtime_behavior", nullptr}, {"trust_level", nullptr}};
        records.push_back(std::move(record));
        if (records.size() > 16)
            fail("APPX_DISCOVERY_FAILED", "Package has too many applications");
        if (FAILED(applications->MoveNext(&hasCurrent)))
            fail("APPX_DISCOVERY_FAILED", "Cannot enumerate manifest applications");
    }
    return records;
}

bool versionLess(const PACKAGE_VERSION &a, const PACKAGE_VERSION &b)
{
    if (a.Major != b.Major) return a.Major < b.Major;
    if (a.Minor != b.Minor) return a.Minor < b.Minor;
    if (a.Build != b.Build) return a.Build < b.Build;
    return a.Revision < b.Revision;
}

} // namespace

std::vector<std::wstring> detail::packageFullNames(const Cancellation &cancel,
    const PackageFamilyQuery &query, std::chrono::steady_clock::time_point deadline)
{
    UINT32 count = 0, length = 0;
    std::vector<PWSTR> names;
    std::vector<wchar_t> buffer;
    std::vector<UINT32> properties;
    for (int attempt = 0; attempt < 5; ++attempt) {
        cancel.check();
        if (std::chrono::steady_clock::now() >= deadline)
            fail("APPX_DISCOVERY_TIMEOUT", "Package discovery exceeded its deadline");
        const LONG status = query(&count, names.empty() ? nullptr : names.data(),
            &length, buffer.empty() ? nullptr : buffer.data(), properties.empty() ? nullptr : properties.data());
        if (status == APPMODEL_ERROR_NO_PACKAGE || (status == ERROR_SUCCESS && count == 0)) return {};
        if (count > 16 || length > 65536)
            fail("APPX_DISCOVERY_LIMIT", "Package enumeration exceeded its capacity limit");
        if (status == ERROR_INSUFFICIENT_BUFFER) {
            if (!count || !length) fail("APPX_DISCOVERY_FAILED", "Package enumeration returned invalid capacity");
            names.resize(count); buffer.resize(length); properties.resize(count);
            continue;
        }
        if (status != ERROR_SUCCESS || count > names.size() || length > buffer.size())
            fail("APPX_DISCOVERY_FAILED", "Package family enumeration failed");
        std::vector<std::wstring> result;
        for (UINT32 index = 0; index < count; ++index) {
            const auto address = reinterpret_cast<std::uintptr_t>(names[index]);
            const auto begin = reinterpret_cast<std::uintptr_t>(buffer.data());
            const auto end = begin + buffer.size() * sizeof(wchar_t);
            if (address < begin || address >= end || (address - begin) % sizeof(wchar_t))
                fail("APPX_DISCOVERY_FAILED", "Package enumeration returned an invalid name");
            const auto offset = (address - begin) / sizeof(wchar_t);
            const auto first = buffer.begin() + static_cast<std::ptrdiff_t>(offset);
            const auto last = std::find(first, buffer.end(), L'\0');
            if (last == buffer.end() || first == last)
                fail("APPX_DISCOVERY_FAILED", "Package enumeration returned an invalid name");
            result.emplace_back(first, last);
        }
        return result;
    }
    fail("APPX_DISCOVERY_UNSTABLE", "Package enumeration did not stabilize within its retry budget");
}

Json discoverPackagesNative(const Cancellation &cancel)
{
    cancel.check();
    ComScope com;
    if (FAILED(com.result))
        fail("APPX_DISCOVERY_COM_FAILED", "Cannot initialize package discovery COM (HRESULT "
            + std::to_string(static_cast<long>(com.result)) + ")");
    cancel.check();
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    Json records = Json::array();
    for (const SupportedPackage &supported : supportedPackages) {
        cancel.check();
        const std::vector<std::wstring> fullNames = detail::packageFullNames(cancel,
            [&](UINT32 *count, PWSTR *names, UINT32 *length, PWSTR buffer, UINT32 *properties) {
                return FindPackagesByPackageFamily(supported.familyName, PACKAGE_FILTER_HEAD,
                    count, names, length, buffer, properties);
            }, deadline);
        if (fullNames.empty()) continue;
        // Highest version wins, mirroring Sort-Object Version -Descending |
        // Select-Object -First 1; ties prefer the native architecture.
        std::optional<PackageIdentity> best;
        SYSTEM_INFO info{};
        GetNativeSystemInfo(&info);
        const UINT32 nativeMachine = info.wProcessorArchitecture == PROCESSOR_ARCHITECTURE_AMD64
            ? static_cast<UINT32>(APPX_PACKAGE_ARCHITECTURE_X64)
            : info.wProcessorArchitecture == PROCESSOR_ARCHITECTURE_ARM64
                ? static_cast<UINT32>(APPX_PACKAGE_ARCHITECTURE_ARM64)
                : 0xffffffffu;
        for (const std::wstring &fullName : fullNames) {
            cancel.check();
            std::optional<PackageIdentity> identifier = packageIdOf(fullName);
            if (!identifier) continue;
            if (identifier->name != supported.packageName) continue; // allowlist drift
            if (!best) {
                best = std::move(identifier);
                continue;
            }
            const bool newer = versionLess(best->version, identifier->version);
            const bool nativeSwap = !versionLess(identifier->version, best->version)
                && identifier->processorArchitecture == nativeMachine
                && best->processorArchitecture != nativeMachine;
            if (newer || nativeSwap) best = std::move(identifier);
        }
        if (!best) continue;
        const std::wstring installLocation = packagePathOf(best->fullName);
        if (installLocation.empty())
            fail("APPX_DISCOVERY_FAILED", "Package install location is unavailable");
        const std::string version = std::to_string(best->version.Major) + "."
            + std::to_string(best->version.Minor) + "."
            + std::to_string(best->version.Build) + "."
            + std::to_string(best->version.Revision);
        Json record{{"package_name", wideToUtf8(best->name)},
                    {"package_full_name", wideToUtf8(best->fullName)},
                    {"package_family_name", wideToUtf8(supported.familyName)},
                    {"package_version", version},
                    {"architecture", architectureName(best->processorArchitecture)},
                    {"install_location", wideToUtf8(installLocation)},
                    {"applications", manifestApplications(std::filesystem::path(installLocation)
                                                             / L"AppxManifest.xml", cancel, deadline)}};
        records.push_back(std::move(record));
    }
    return Json{{"schema_version", 1}, {"records", records}};
}

} // namespace cpg

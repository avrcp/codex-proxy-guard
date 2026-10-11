#include "platform.h"
#include "cpg/build_info.h"
#include "cpg/path.h"
#include "cpg/win_handle.h"
#include "package_discovery.h"
#include "platform_protocol_p.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <cctype>
#include <cstdio>
#include <cstring>
#include <limits>
#include <vector>

#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <appmodel.h>
#include <shobjidl.h>
#include <tlhelp32.h>

namespace cpg {
namespace {
constexpr std::size_t requestLimit = 16 * 1024;
constexpr std::size_t receiptLimit = 64 * 1024;
const char *discoveryError = "APPX_DISCOVERY_PROTOCOL_INVALID";
const char *activationError = "APPX_ACTIVATION_PROTOCOL_INVALID";

[[noreturn]] void fail(const std::string &code, const std::string &message)
{
    throw Error(code, message);
}

const Json &field(const Json &object, const char *key)
{
    static const Json nullValue = Json();
    const auto entry = object.find(key);
    return entry == object.end() ? nullValue : *entry;
}

bool isNullish(const Json &value)
{
    return value.is_null();
}

bool asciiEquals(const std::string &left, const std::string &right)
{
    if (left.size() != right.size()) return false;
    for (std::size_t index = 0; index < left.size(); ++index)
        if (std::tolower(static_cast<unsigned char>(left[index]))
            != std::tolower(static_cast<unsigned char>(right[index])))
            return false;
    return true;
}

std::string asciiTrimmedUtf8(std::string_view value)
{
    std::size_t begin = 0, end = value.size();
    while (begin < end && std::isspace(static_cast<unsigned char>(value[begin]))) ++begin;
    while (end > begin && std::isspace(static_cast<unsigned char>(value[end - 1]))) --end;
    return std::string(value.substr(begin, end - begin));
}

// Qt QJsonValue::toString() semantics: non-strings read as empty.
std::string stringValue(const Json &value)
{
    return value.is_string() ? value.get<std::string>() : std::string();
}

std::string stringField(const Json &object, const char *key, const char *code,
                        bool optional = false, bool nullable = false)
{
    const Json &value = field(object, key);
    if ((optional && object.find(key) == object.end()) || (nullable && isNullish(value))) return {};
    if (!value.is_string()) fail(code, std::string("Invalid string field: ") + key);
    const std::string result = value.get<std::string>();
    if (result.find('\0') != std::string::npos) fail(code, std::string("Embedded NUL in field: ") + key);
    return result;
}

void exactKeys(const Json &object, std::initializer_list<const char *> allowed, const char *code)
{
    for (auto entry = object.begin(); entry != object.end(); ++entry) {
        const std::string &key = entry.key();
        bool known = false;
        for (const char *candidate : allowed)
            if (key == candidate) { known = true; break; }
        if (!known) fail(code, "Unexpected protocol field");
    }
}

std::filesystem::path canonicalFile(const std::filesystem::path &path)
{
    if (!path.is_absolute() || !std::filesystem::exists(path) || !std::filesystem::is_regular_file(path))
        fail("CODEX_EXECUTABLE_MISSING", "Desktop executable is not an existing absolute file");
    const auto canonical = canonicalPathOf(path, false);
    if (!canonical) fail("CODEX_EXECUTABLE_INVALID", "Cannot resolve Desktop executable");
    return *canonical;
}

bool supported(const std::string &name)
{
    return name == "OpenAI.Codex" || name == "OpenAI.ChatGPT-Desktop";
}

std::string runtime(const Json &app)
{
    const std::string entry = stringField(app, "entry_point", discoveryError, true);
    if (asciiEquals(entry, "Windows.FullTrustApplication")) return "full_trust_desktop";
    if (asciiTrimmedUtf8(entry).empty()
        && asciiTrimmedUtf8(stringField(app, "runtime_behavior", discoveryError, true, true)).empty()
        && asciiTrimmedUtf8(stringField(app, "trust_level", discoveryError, true, true)).empty())
        return "unknown";
    return "app_container";
}

std::string toForwardSlashes(std::string path)
{
    for (char &character : path) if (character == '\\') character = '/';
    return path;
}

std::string baseName(const std::string &forwardPath)
{
    const std::size_t slash = forwardPath.rfind('/');
    return slash == std::string::npos ? forwardPath : forwardPath.substr(slash + 1);
}

void validateRecord(const Json &record)
{
    for (const char *key : {"package_name", "package_version", "install_location"})
        (void)stringField(record, key, discoveryError);
    for (const char *key : {"package_full_name", "package_family_name", "architecture"})
        (void)stringField(record, key, discoveryError, true);
    const auto applications = record.find("applications");
    if (applications != record.end()) {
        if (!applications->is_array()) fail(discoveryError, "Applications must be an array");
        if (applications->size() > 16) fail(discoveryError, "Too many application records");
        for (const auto &value : *applications) {
            if (!value.is_object()) fail(discoveryError, "Application must be an object");
            for (const char *key : {"application_id", "manifest_executable", "entry_point"})
                (void)stringField(value, key, discoveryError, true);
            for (const char *key : {"runtime_behavior", "trust_level"})
                (void)stringField(value, key, discoveryError, true, true);
        }
    }
}

Desktop registeredDesktop(const Json &record)
{
    Desktop desktop;
    desktop.packageName = stringField(record, "package_name", discoveryError);
    desktop.product = desktop.packageName == "OpenAI.Codex" ? "chat_gpt" : "chat_gpt_classic";
    desktop.displayName = desktop.packageName == "OpenAI.Codex" ? "ChatGPT Desktop" : "ChatGPT Desktop (Classic)";
    desktop.packageVersion = stringField(record, "package_version", discoveryError);
    desktop.architecture = asciiTrimmedUtf8(stringField(record, "architecture", discoveryError, true));
    if (desktop.architecture.empty()) desktop.architecture = "unknown";
    desktop.packageFullName = asciiTrimmedUtf8(stringField(record, "package_full_name", discoveryError, true));
    desktop.packageFamilyName = asciiTrimmedUtf8(stringField(record, "package_family_name", discoveryError, true));
    if (desktop.packageFullName.empty() || desktop.packageFamilyName.empty())
        fail("APPX_METADATA_INCOMPLETE", "Package identity metadata is missing");
    const std::filesystem::path root(utf8ToWide(stringField(record, "install_location", discoveryError)));
    if (!root.is_absolute() || !std::filesystem::is_directory(root))
        fail("APPX_INSTALL_LOCATION_INVALID", "Package installation directory is unavailable");
    const auto canonicalRoot = canonicalPathOf(root, true);
    if (!canonicalRoot) fail("APPX_INSTALL_LOCATION_INVALID", "Package installation directory is unavailable");
    desktop.installLocation = *canonicalRoot;
    std::vector<Json> candidates;
    const Json &apps = field(record, "applications");
    if (apps.is_array()) {
        for (const auto &value : apps) {
            const std::string manifest = stringField(value, "manifest_executable", discoveryError, true);
            const std::string relative = toForwardSlashes(manifest);
            const bool trusted = desktop.packageName == "OpenAI.Codex"
                ? stringField(value, "application_id", discoveryError, true) == "App"
                    && asciiEquals(relative, "app/ChatGPT.exe")
                : runtime(value) == "full_trust_desktop"
                    && asciiEquals(baseName(relative), "ChatGPT.exe");
            if (trusted) candidates.push_back(value);
        }
    }
    if (candidates.empty()) fail("APPX_APPLICATION_MISSING", "No trusted Desktop application was found");
    if (candidates.size() != 1) fail("APPX_APPLICATION_AMBIGUOUS", "Multiple trusted Desktop applications were found");
    const Json &app = candidates.front();
    desktop.applicationId = asciiTrimmedUtf8(stringField(app, "application_id", discoveryError, true));
    desktop.manifestExecutable = asciiTrimmedUtf8(stringField(app, "manifest_executable", discoveryError, true));
    if (desktop.applicationId.empty() || desktop.manifestExecutable.empty())
        fail("APPX_METADATA_INCOMPLETE", "Application identity metadata is missing");
    const std::string relative = toForwardSlashes(desktop.manifestExecutable);
    if (!relative.empty() && relative.front() == '/')
        fail("APPX_EXECUTABLE_INVALID", "Manifest executable must remain within the package directory");
    if (relative.find(':') != std::string::npos)
        fail("APPX_EXECUTABLE_INVALID", "Manifest executable must remain within the package directory");
    bool dotDot = false;
    for (std::size_t position = 0; position + 1 < relative.size(); ++position)
        if (relative[position] == '.' && relative[position + 1] == '.'
            && (position + 2 == relative.size() || relative[position + 2] == '/'))
            dotDot = true;
    if (dotDot) fail("APPX_EXECUTABLE_INVALID", "Manifest executable must remain within the package directory");
    desktop.executable = canonicalFile(desktop.installLocation / utf8ToWide(relative));
    if (!pathWithin(desktop.installLocation, desktop.executable))
        fail("APPX_EXECUTABLE_INVALID", "Manifest executable resolves outside the package directory");
    desktop.aumid = desktop.packageFamilyName + '!' + desktop.applicationId;
    desktop.runtimeKind = runtime(app);
    desktop.registered = true;
    desktop.discoverySource = "appx_manifest";
    return desktop;
}

std::string tokenElevation(HANDLE process)
{
    HANDLE raw = nullptr;
    if (!OpenProcessToken(process, TOKEN_QUERY, &raw)) return "unknown";
    const UniqueHandle token(raw);
    TOKEN_ELEVATION result{};
    DWORD size = 0;
    if (!GetTokenInformation(token.get(), TokenElevation, &result, sizeof(result), &size)
        || size != sizeof(result)) return "unknown";
    return result.TokenIsElevated ? "elevated" : "not_elevated";
}

std::wstring imagePath(HANDLE process)
{
    std::array<wchar_t, 32768> buffer{};
    DWORD size = static_cast<DWORD>(buffer.size());
    if (!QueryFullProcessImageNameW(process, 0, buffer.data(), &size)) return {};
    return std::wstring(buffer.data(), size);
}

std::uint64_t ticks(const FILETIME &time)
{
    return (static_cast<std::uint64_t>(time.dwHighDateTime) << 32) | time.dwLowDateTime;
}

void observeIdentity(HANDLE process, Json &receipt, const std::string &expected,
                     const char *stateField, const char *observedField, bool appId)
{
    UINT32 size = 0;
    auto query = appId ? GetApplicationUserModelId : GetPackageFullName;
    LONG status = query(process, &size, nullptr);
    if (status == APPMODEL_ERROR_NO_PACKAGE || (appId && status == APPMODEL_ERROR_NO_APPLICATION)) {
        receipt[stateField] = "missing";
        return;
    }
    if (status != ERROR_INSUFFICIENT_BUFFER || size < 2 || size > 2048) {
        receipt[stateField] = "query_failed";
        return;
    }
    std::vector<wchar_t> buffer(size);
    const auto capacity = size;
    status = query(process, &size, buffer.data());
    if (status != ERROR_SUCCESS || size < 2 || size > capacity || buffer[size - 1] != L'\0') {
        receipt[stateField] = "query_failed";
        return;
    }
    const std::string text = wideToUtf8(std::wstring(buffer.data(), size - 1));
    if (text.find('\0') != std::string::npos || !strictUtf8(text)) {
        receipt[stateField] = "query_failed";
        return;
    }
    receipt[observedField] = text;
    receipt[stateField] = text == expected ? "matched" : "mismatch";
}

Json emptyReceipt()
{
    return Json{{"version", 1}, {"failure", nullptr}, {"activation_returned", false},
                {"activation_hresult", nullptr}, {"pid", nullptr},
                {"instance", "unknown"}, {"package_identity", "not_queried"},
                {"observed_package_full_name", nullptr}, {"aumid", "not_queried"},
                {"observed_aumid", nullptr}, {"elevation", nullptr},
                {"early_exit_code", nullptr}, {"observation_failure", nullptr}};
}

Json activateAndObserve(const Json &request)
{
    Json receipt = emptyReceipt();
    const HRESULT initialized = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    if (FAILED(initialized)) {
        receipt["failure"] = "APPX_ACTIVATION_COM_INIT_FAILED: Cannot initialize COM";
        return receipt;
    }
    struct ComScope { ~ComScope() { CoUninitialize(); } } comScope;
    IApplicationActivationManager *manager = nullptr;
    const HRESULT created = CoCreateInstance(CLSID_ApplicationActivationManager, nullptr,
                                             CLSCTX_LOCAL_SERVER, IID_PPV_ARGS(&manager));
    if (FAILED(created) || !manager) {
        receipt["failure"] = "APPX_ACTIVATION_MANAGER_UNAVAILABLE: Cannot create activation manager";
        return receipt;
    }
    struct ManagerScope { IApplicationActivationManager *value; ~ManagerScope() { value->Release(); } } managerScope{manager};
    FILETIME submitted{};
    GetSystemTimeAsFileTime(&submitted);
    const std::string aumid = request.value("aumid", "");
    const std::string arguments = request.value("arguments", "");
    DWORD pid = 0;
    const std::wstring wideAumid = utf8ToWide(aumid);
    const std::wstring wideArguments = utf8ToWide(arguments);
    const HRESULT activated = manager->ActivateApplication(wideAumid.c_str(), wideArguments.c_str(),
                                                           AO_NONE, &pid);
    if (FAILED(activated)) {
        char buffer[16] = {};
        std::snprintf(buffer, sizeof buffer, "0x%08X", static_cast<unsigned>(activated));
        std::string text(buffer);
        for (char &character : text) character = static_cast<char>(std::toupper(static_cast<unsigned char>(character)));
        receipt["activation_hresult"] = text;
        return receipt;
    }
    receipt["activation_returned"] = true;
    receipt["pid"] = static_cast<std::uint64_t>(pid);
    const UniqueHandle process(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, FALSE, pid));
    if (!process.valid()) {
        receipt["package_identity"] = "query_failed";
        receipt["aumid"] = "query_failed";
        receipt["observation_failure"] = "APPX_TARGET_VERIFY_FAILED: Cannot open activated process";
        return receipt;
    }
    observeIdentity(process.get(), receipt, request.value("expected_package_full_name", ""),
                    "package_identity", "observed_package_full_name", false);
    observeIdentity(process.get(), receipt, aumid, "aumid", "observed_aumid", true);
    const std::string elevated = tokenElevation(process.get());
    if (elevated != "unknown") receipt["elevation"] = elevated == "elevated";
    FILETIME creation{}, exited{}, kernel{}, user{};
    if (GetProcessTimes(process.get(), &creation, &exited, &kernel, &user))
        receipt["instance"] = ticks(creation) + 250ULL * 10000ULL < ticks(submitted) ? "reused" : "created";
    else receipt["observation_failure"] = "APPX_TARGET_TIME_QUERY_FAILED: Cannot query process creation time";
    const std::wstring actualPath = imagePath(process.get());
    if (!actualPath.empty()
        && normalizedWindowsPath(actualPath) != normalizedWindowsPath(utf8ToWide(request.value("expected_executable", ""))))
        receipt["observation_failure"] = "APPX_TARGET_IMAGE_MISMATCH: Activated executable differs from selected Desktop";
    const DWORD wait = WaitForSingleObject(process.get(), 150);
    if (wait == WAIT_OBJECT_0) {
        DWORD code = 0;
        if (GetExitCodeProcess(process.get(), &code)) receipt["early_exit_code"] = static_cast<std::uint64_t>(code);
        else receipt["observation_failure"] = "APPX_TARGET_EXIT_QUERY_FAILED: Cannot query early exit";
    } else if (wait != WAIT_TIMEOUT) {
        receipt["observation_failure"] = "APPX_TARGET_WAIT_FAILED: Cannot observe target lifetime";
    }
    return receipt;
}

bool unsignedNumber(const Json &value, bool zeroAllowed)
{
    if (value.is_number_unsigned()) {
        const std::uint64_t number = value.get<std::uint64_t>();
        return number >= (zeroAllowed ? 0 : 1) && number <= 0xffffffffull;
    }
    if (value.is_number_integer()) {
        const std::int64_t number = value.get<std::int64_t>();
        return number >= (zeroAllowed ? 0 : 1) && number <= 0xffffffffll;
    }
    if (!value.is_number_float()) return false;
    const double number = value.get<double>();
    return std::isfinite(number) && std::floor(number) == number
        && number >= (zeroAllowed ? 0 : 1) && number <= 0xffffffffull;
}
} // namespace

HelperResult runHelper(const std::filesystem::path &program,
                       const std::vector<std::wstring> &arguments,
                       std::string_view input, const Cancellation &cancel,
                       std::chrono::milliseconds timeout, std::size_t outputLimit,
                       const UnicodeEnvironment &environment, bool activationSubmission)
{
    ProcessRequest request;
    request.program = program;
    request.arguments = arguments;
    request.stdinBytes = std::string(input);
    request.environment = environment;
    request.timeout = timeout;
    request.stdoutLimit = outputLimit;
    request.stderrLimit = outputLimit;
    request.createNoWindow = true;
    request.activationSubmission = activationSubmission;
    const ProcessResult result = runOwnedProcess(request, cancel);
    return {result.out, result.err, result.exitCode, result.submitted};
}

Json Desktop::publicJson() const
{
    return Json{{"state", "found"}, {"product", product}, {"display_name", displayName},
                {"package_version", publicText(packageVersion)}, {"architecture", publicText(architecture)},
                {"application_id", registered ? Json(publicText(applicationId)) : Json(nullptr)},
                {"manifest_executable", registered ? Json(publicText(manifestExecutable)) : Json(nullptr)},
                {"runtime_kind", registered ? Json(runtimeKind) : Json(nullptr)}};
}

std::string elevation() { return tokenElevation(GetCurrentProcess()); }

void requireNonElevated()
{
    const std::string state = elevation();
    if (state == "elevated") fail("GUARD_ELEVATED", "Launch requires a non-elevated Guard process");
    if (state != "not_elevated") fail("GUARD_ELEVATION_QUERY_FAILED", "Cannot verify that Guard is not elevated");
}

Desktop parseDiscovery(const std::string &json, const std::filesystem::path &overrideExecutable)
{
    const Json envelope = parseStrictJsonObject(json, 64 * 1024, discoveryError);
    const Json &schema = field(envelope, "schema_version");
    if (!schema.is_number() || schema.get<double>() != 1)
        fail("APPX_DISCOVERY_PROTOCOL_UNSUPPORTED", "Unsupported APPX discovery schema");
    const Json &recordsValue = field(envelope, "records");
    if (!recordsValue.is_array()) fail(discoveryError, "Discovery records must be an array");
    const Json &records = recordsValue;
    if (records.size() > 16) fail(discoveryError, "Too many package records");
    for (const auto &value : records) {
        if (!value.is_object()) fail(discoveryError, "Package record must be an object");
        validateRecord(value);
    }
    if (!overrideExecutable.empty()) {
        const std::filesystem::path executable = canonicalFile(overrideExecutable);
        for (const auto &value : records) {
            const std::string name = value.value("package_name", "");
            if (!supported(name)) continue;
            const std::filesystem::path root(utf8ToWide(value.value("install_location", "")));
            const auto canonicalRoot = root.empty() ? std::nullopt : canonicalPathOf(root, true);
            if (!canonicalRoot || !pathWithin(*canonicalRoot, executable)) continue;
            Desktop result = registeredDesktop(value);
            if (normalizedWindowsPath(executable.wstring())
                != normalizedWindowsPath(result.executable.wstring()))
                fail("APPX_OVERRIDE_INVALID", "Override is inside a package but is not its registered Desktop entry");
            result.discoverySource = "executable_override";
            return result;
        }
        // A package path can never become a bare executable by omitting discovery metadata.
        if (normalizedWindowsPath(executable.wstring()).find(L"/windowsapps/") != std::wstring::npos)
            fail("APPX_OVERRIDE_INVALID", "Package executable requires registered discovery metadata");
        Desktop result;
        result.product = "executable_override";
        result.displayName = "Desktop executable override";
        result.packageName = "executable_override";
        result.packageVersion = "manual";
        result.architecture = "manual";
        result.discoverySource = "executable_override";
        result.executable = executable;
        result.installLocation = executable.parent_path();
        return result;
    }
    for (const char *name : {"OpenAI.Codex", "OpenAI.ChatGPT-Desktop"}) {
        for (const auto &value : records) {
            if (value.value("package_name", "") == name) return registeredDesktop(value);
        }
    }
    fail("CODEX_NOT_INSTALLED", "No supported Desktop package is installed");
}

Desktop discoverDesktop(const Config &config, const Cancellation &cancel)
{
    cancel.check();
    // Native AppModel discovery only: no PowerShell child, no fallback to it
    // (failures surface as APPX_DISCOVERY_FAILED / CODEX_NOT_INSTALLED).
    const Json envelope = discoverPackagesNative(cancel);
    return parseDiscovery(jsonDump(envelope), config.executableOverride);
}

ProcessState desktopProcessState(const Desktop &desktop)
{
    const UniqueHandle snapshot(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0));
    if (!snapshot.valid()) return {};
    PROCESSENTRY32W entry{};
    entry.dwSize = sizeof(entry);
    if (!Process32FirstW(snapshot.get(), &entry)) return {};
    struct Candidate { DWORD pid; DWORD parent; std::wstring path; };
    std::vector<Candidate> candidates;
    bool unknown = false;
    const std::wstring targetName = desktop.executable.filename().wstring();
    do {
        const std::wstring name(entry.szExeFile);
        if (name.size() != targetName.size()) continue;
        bool same = true;
        for (std::size_t index = 0; index < name.size(); ++index)
            if (towlower(name[index]) != towlower(targetName[index])) { same = false; break; }
        if (!same) continue;
        const UniqueHandle process(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, FALSE,
                                               entry.th32ProcessID));
        if (!process.valid()) { unknown = true; continue; }
        if (WaitForSingleObject(process.get(), 0) == WAIT_OBJECT_0) continue;
        const std::wstring path = imagePath(process.get());
        if (path.empty()) { unknown = true; continue; }
        if (normalizedWindowsPath(path) == normalizedWindowsPath(desktop.executable.wstring()))
            candidates.push_back({entry.th32ProcessID, entry.th32ParentProcessID, path});
    } while (Process32NextW(snapshot.get(), &entry));
    if (GetLastError() != ERROR_NO_MORE_FILES) return {};
    for (const auto &candidate : candidates) {
        // Electron helper processes descend from a matching Desktop root. Do not report their PID.
        const bool child = std::any_of(candidates.begin(), candidates.end(), [&](const Candidate &other) {
            return other.pid == candidate.parent;
        });
        if (!child) return {"running", candidate.pid};
    }
    return {(unknown || !candidates.empty()) ? "unknown" : "stopped", 0};
}

namespace detail {
Json activationRequest(const std::string &bytes)
{
    Json request = parseStrictJsonObject(bytes, requestLimit, activationError);
    exactKeys(request, {"version", "aumid", "expected_package_full_name", "expected_executable", "arguments"},
              activationError);
    if (!field(request, "version").is_number() || field(request, "version").get<double>() != 1)
        fail(activationError, "Unsupported activation protocol version");
    const std::string aumid = stringField(request, "aumid", activationError);
    const std::string package = stringField(request, "expected_package_full_name", activationError);
    const std::string executable = stringField(request, "expected_executable", activationError);
    const std::string arguments = stringField(request, "arguments", activationError);
    // Identity strings (AUMID, package full name) are restricted to the
    // printable non-space ASCII range of the previous validation regex.
    const auto identityPrintable = [](const std::string &value) {
        for (const char character : value)
            if (static_cast<unsigned char>(character) < 0x21
                || static_cast<unsigned char>(character) > 0x7e) return false;
        return true;
    };
    // Activation arguments allow a space but no other control or non-ASCII.
    const auto argumentPrintable = [](const std::string &value) {
        for (const char character : value)
            if (static_cast<unsigned char>(character) < 0x20
                || static_cast<unsigned char>(character) > 0x7e) return false;
        return true;
    };
    const auto exclamations = [](const std::string &value) {
        std::size_t count = 0;
        for (const char character : value) if (character == '!') ++count;
        return count;
    };
    if (aumid.size() > 2048 || !identityPrintable(aumid) || exclamations(aumid) != 1
        || aumid.front() == '!' || aumid.back() == '!')
        fail(activationError, "Invalid application identity");
    if (package.size() > 1024 || !identityPrintable(package)) fail(activationError, "Invalid package identity");
    if (executable.empty() || !std::filesystem::path(utf8ToWide(executable)).is_absolute())
        fail(activationError, "Expected executable must be absolute");
    if (arguments.size() > 4096) fail(activationError, "Activation arguments exceed 4 KiB");
    if (!argumentPrintable(arguments)) fail(activationError, "Activation arguments must be printable ASCII");
    return request;
}

Json activationReceipt(const std::string &bytes)
{
    const Json receipt = parseStrictJsonObject(bytes, receiptLimit, activationError);
    const Json shape = emptyReceipt();
    exactKeys(receipt, {"version", "failure", "activation_returned", "activation_hresult", "pid",
                        "instance", "package_identity", "observed_package_full_name", "aumid",
                        "observed_aumid", "elevation", "early_exit_code", "observation_failure"},
              activationError);
    for (const char *key : {"version", "failure", "activation_returned", "activation_hresult", "pid",
                            "instance", "package_identity", "observed_package_full_name", "aumid",
                            "observed_aumid", "elevation", "early_exit_code", "observation_failure"})
        if (receipt.find(key) == receipt.end()) fail(activationError, "Invalid activation receipt shape");
    if (field(receipt, "version") != field(shape, "version")
        || !field(receipt, "activation_returned").is_boolean())
        fail(activationError, "Invalid activation receipt shape");
    for (const char *key : {"failure", "activation_hresult", "observed_package_full_name",
                            "observed_aumid", "observation_failure"})
        (void)stringField(receipt, key, activationError, false, true);
    for (const char *key : {"pid", "early_exit_code"}) {
        const Json &value = field(receipt, key);
        if (!value.is_null() && !unsignedNumber(value, std::string(key) == "early_exit_code"))
            fail(activationError, "Invalid process identifier or exit code");
    }
    const Json &elevationField = field(receipt, "elevation");
    if (!elevationField.is_null() && !elevationField.is_boolean())
        fail(activationError, "Invalid target elevation");
    const std::string instance = stringValue(field(receipt, "instance"));
    if (instance != "unknown" && instance != "created" && instance != "reused")
        fail(activationError, "Invalid instance observation");
    for (const char *key : {"package_identity", "aumid"}) {
        const std::string observation = stringValue(field(receipt, key));
        if (observation != "not_queried" && observation != "matched" && observation != "missing"
            && observation != "mismatch" && observation != "query_failed")
            fail(activationError, "Invalid identity observation");
    }
    const bool returned = field(receipt, "activation_returned").get<bool>();
    if (returned != !field(receipt, "pid").is_null()
        || (returned && (!field(receipt, "failure").is_null()
                         || !field(receipt, "activation_hresult").is_null())))
        fail(activationError, "Contradictory activation facts");
    return receipt;
}
} // namespace detail

Json activateDesktop(const Desktop &desktop, const std::string &arguments, const Cancellation &cancel)
{
    if (!desktop.registered) fail(activationError, "Application activation requires a registered target");
    requireNonElevated();
    const Json request{{"version", 1}, {"aumid", desktop.aumid},
        {"expected_package_full_name", desktop.packageFullName},
        {"expected_executable", wideToUtf8(desktop.executable.wstring())},
        {"arguments", arguments}};
    const std::string bytes = jsonDump(request);
    (void)detail::activationRequest(bytes);
    const HelperResult result = runHelper(currentExecutablePath(), {L"internal-activate-package"},
                                          bytes, cancel, std::chrono::milliseconds(30000),
                                          receiptLimit, UnicodeEnvironment::current(), true);
    Json receipt;
    try {
        if (result.exitCode != 0) fail(activationError, "Activation worker exited without a successful receipt");
        receipt = detail::activationReceipt(result.out);
    } catch (const Error &) {
        fail("APPX_ACTIVATION_OUTCOME_UNKNOWN", "Activation request was submitted but the receipt is unavailable or invalid; refresh Desktop state");
    }
    if (!field(receipt, "failure").is_null()) {
        const std::string message = receipt.value("failure", "");
        fail("APPX_ACTIVATION_WORKER_FAILED", publicText(message));
    }
    return receipt;
}

int activationWorker()
{
    try {
        const HANDLE input = GetStdHandle(STD_INPUT_HANDLE);
        const auto type = GetFileType(input);
        if (type != FILE_TYPE_PIPE && type != FILE_TYPE_DISK)
            fail(activationError, "Worker requires bounded redirected input");
        const auto started = std::chrono::steady_clock::now();
        std::string bytes;
        std::array<char, 4096> buffer{};
        for (;;) {
            if (std::chrono::steady_clock::now() - started >= std::chrono::seconds(30))
                fail(activationError, "Worker request read timed out");
            DWORD available = static_cast<DWORD>(buffer.size());
            if (type == FILE_TYPE_PIPE) {
                if (!PeekNamedPipe(input, nullptr, 0, nullptr, &available, nullptr)) {
                    if (GetLastError() == ERROR_BROKEN_PIPE) break;
                    fail(activationError, "Cannot read worker input");
                }
                if (!available) { Sleep(10); continue; }
            }
            DWORD count = 0;
            if (!ReadFile(input, buffer.data(),
                          static_cast<DWORD>(std::min<DWORD>(available, static_cast<DWORD>(buffer.size()))),
                          &count, nullptr)) {
                if (GetLastError() == ERROR_BROKEN_PIPE) break;
                fail(activationError, "Cannot read worker input");
            }
            if (!count) break;
            bytes.append(buffer.data(), count);
            if (bytes.size() > requestLimit) fail(activationError, "Worker request exceeds 16 KiB");
        }
        const Json request = detail::activationRequest(bytes);
        requireNonElevated();
        const Json receipt = activateAndObserve(request);
        const std::string output = jsonDump(receipt) + "\n";
        DWORD written = 0;
        if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), output.data(),
                       static_cast<DWORD>(output.size()), &written, nullptr)
            || written != static_cast<DWORD>(output.size())) return 1;
        return 0;
    } catch (const Error &error) {
        const std::string output = error.code + ": " + publicText(error.message) + "\n";
        DWORD written = 0;
        (void)WriteFile(GetStdHandle(STD_ERROR_HANDLE), output.data(),
                        static_cast<DWORD>(output.size()), &written, nullptr);
        return 1;
    }
}

} // namespace cpg

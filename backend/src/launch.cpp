#include "launch_internal.h"
#include "cpg/path.h"
#include "cpg/win_handle.h"

#include <algorithm>
#include <array>
#include <cctype>
#include <chrono>
#include <cstdint>
#include <limits>
#include <vector>

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>

namespace cpg {
namespace {
constexpr std::size_t daemonOutputLimit = 64 * 1024;

std::uint64_t millisecondsSinceEpoch()
{
    return std::chrono::duration_cast<std::chrono::milliseconds>(
        std::chrono::system_clock::now().time_since_epoch()).count();
}

bool asciiEqualsFolded(std::string_view left, std::string_view right)
{
    if (left.size() != right.size()) return false;
    for (std::size_t index = 0; index < left.size(); ++index)
        if (std::tolower(static_cast<unsigned char>(left[index]))
            != std::tolower(static_cast<unsigned char>(right[index])))
            return false;
    return true;
}

std::string asciiTrimmed(std::string_view value)
{
    std::size_t begin = 0, end = value.size();
    while (begin < end && std::isspace(static_cast<unsigned char>(value[begin]))) ++begin;
    while (end > begin && std::isspace(static_cast<unsigned char>(value[end - 1]))) --end;
    return std::string(value.substr(begin, end - begin));
}

std::string toLower(std::string value)
{
    for (char &character : value)
        character = static_cast<char>(std::tolower(static_cast<unsigned char>(character)));
    return value;
}

class StartupLock final {
public:
    StartupLock() : handle_(open()) {
        char previous[64]{};
        DWORD count = 0;
        if (!ReadFile(handle_.get(), previous, sizeof(previous), &count, nullptr))
            throw Error("LAUNCH_BUSY", "Cannot read the cross-process startup lock.");
        bool valid = false;
        const std::int64_t last = parseTimestamp(std::string(previous, count), valid);
        const std::int64_t now = static_cast<std::int64_t>(millisecondsSinceEpoch());
        if (valid && last <= now && now - last < 5000)
            throw Error("LAUNCH_BUSY", "Another Guard instance just launched Desktop; refresh before retrying.");
    }
    void markSubmitted() {
        LARGE_INTEGER zero{};
        const std::string timestamp = std::to_string(millisecondsSinceEpoch());
        DWORD written = 0;
        if (!SetFilePointerEx(handle_.get(), zero, nullptr, FILE_BEGIN)
            || !SetEndOfFile(handle_.get())
            || !WriteFile(handle_.get(), timestamp.data(), static_cast<DWORD>(timestamp.size()), &written, nullptr)
            || written != static_cast<DWORD>(timestamp.size()) || !FlushFileBuffers(handle_.get()))
            throw Error("LAUNCH_BUSY", "Cannot record the startup guard; Desktop was not submitted.");
    }
private:
    static std::int64_t parseTimestamp(const std::string &text, bool &valid)
    {
        valid = false;
        const std::string trimmed = asciiTrimmed(text);
        if (trimmed.empty() || trimmed.size() > 20) return 0;
        std::int64_t value = 0;
        for (const char character : trimmed) {
            if (character < '0' || character > '9') return 0;
            const signed char digit = static_cast<signed char>(character - '0');
            if (value > (std::numeric_limits<std::int64_t>::max() - digit) / 10) return 0;
            value = value * 10 + digit;
        }
        valid = true;
        return value;
    }
    static HANDLE open() {
        wchar_t base[MAX_PATH + 1] = {};
        const DWORD size = GetTempPathW(MAX_PATH + 1, base);
        if (!size || size >= MAX_PATH + 1)
            throw Error("LAUNCH_BUSY", "Another Guard instance is launching, or the startup lock is unavailable.");
        const std::filesystem::path path = std::filesystem::path(std::wstring(base, size))
            / L"codex-proxy-guard-startup.lock";
        HANDLE result = CreateFileW(path.c_str(), GENERIC_READ | GENERIC_WRITE,
                                    0, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
        if (result == INVALID_HANDLE_VALUE)
            throw Error("LAUNCH_BUSY", "Another Guard instance is launching, or the startup lock is unavailable.");
        return result;
    }
    UniqueHandle handle_;
};

bool absolutePath(const std::filesystem::path &path) {
    // Native CLI selection requires a drive-absolute or UNC path; root-relative
    // and drive-relative forms are refused.
    const std::wstring wide = path.wstring();
    if (wide.empty() || wide.find(L'\0') != std::wstring::npos) return false;
    const bool drive = wide.size() >= 3
        && ((wide[0] >= L'A' && wide[0] <= L'Z') || (wide[0] >= L'a' && wide[0] <= L'z'))
        && wide[1] == L':' && (wide[2] == L'/' || wide[2] == L'\\');
    return drive || wide.rfind(L"\\\\", 0) == 0 || wide.rfind(L"//", 0) == 0;
}

std::filesystem::path canonicalExe(const std::filesystem::path &path) {
    if (!absolutePath(path) || !std::filesystem::is_regular_file(path)) return {};
    const std::wstring extension = path.extension().wstring();
    if (extension.size() != 4 || !asciiEqualsFolded(wideToUtf8(extension), ".exe")) return {};
    const auto canonical = canonicalPathOf(path, false);
    return canonical.value_or(std::filesystem::path());
}

std::string bypassEntry(const std::string &entry) {
    if (asciiEqualsFolded(entry, "localhost")) return "localhost";
    const std::wstring wide = utf8ToWide(entry);
    IN_ADDR ipv4{};
    if (InetPtonW(AF_INET, wide.c_str(), &ipv4) == 1
        && (ntohl(ipv4.S_un.S_addr) >> 24) == 127) return entry;
    IN6_ADDR ipv6{};
    if (InetPtonW(AF_INET6, wide.c_str(), &ipv6) == 1
        && IN6_IS_ADDR_LOOPBACK(&ipv6)) return "[" + entry + "]";
    throw Error("PROXY_BYPASS_UNSUPPORTED", "A no_proxy entry has no exact Chromium bypass equivalent; use only localhost or loopback IP literals.");
}

void requireStopped(const ProcessState &state) {
    if (state.state == "stopped") return;
    if (state.state == "running")
        throw Error("CODEX_ALREADY_RUNNING", "Fully exit Desktop before launching it through Guard.");
    throw Error("CODEX_RUNNING_UNKNOWN", "Cannot confirm whether Desktop is running; nothing was launched.");
}

bool sameTarget(const Desktop &first, const Desktop &second) {
    return first.registered == second.registered
        && normalizedWindowsPath(first.executable.wstring()) == normalizedWindowsPath(second.executable.wstring())
        && first.packageFullName == second.packageFullName
        && first.packageFamilyName == second.packageFamilyName
        && first.aumid == second.aumid && first.applicationId == second.applicationId
        && first.runtimeKind == second.runtimeKind && first.manifestExecutable == second.manifestExecutable;
}

Json processElevation(HANDLE process) {
    HANDLE raw = nullptr;
    if (!OpenProcessToken(process, TOKEN_QUERY, &raw)) return nullptr;
    UniqueHandle token(raw);
    TOKEN_ELEVATION result{};
    DWORD count = 0;
    if (!GetTokenInformation(token.get(), TokenElevation, &result, sizeof(result), &count))
        return nullptr;
    return result.TokenIsElevated != 0;
}

Json nativeLaunch(const Desktop &desktop, const UnicodeEnvironment &environment,
                  const Cancellation &cancel) {
    if (desktop.registered)
        throw Error("APPX_METADATA_INCOMPLETE", "Registered applications require application-model activation.");
    const std::wstring executable = desktop.executable.wstring();
    if (!absolutePath(desktop.executable) || executable.find(L'"') != std::wstring::npos
        || !std::filesystem::is_regular_file(desktop.executable))
        throw Error("CODEX_EXECUTABLE_MISSING", "Desktop executable is missing or invalid.");
    const std::vector<wchar_t> &block = environment.block();
    std::wstring command = L"\"" + executable + L"\"";
    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION process{};
    cancel.check();
    if (!CreateProcessW(executable.c_str(), command.data(), nullptr, nullptr, FALSE,
                        CREATE_UNICODE_ENVIRONMENT | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
                        const_cast<wchar_t *>(block.data()), nullptr, &startup, &process))
        throw Error("CODEX_LAUNCH_FAILED", "Windows could not create the Desktop process.");
    // These handles are observations only. In particular, cancellation and
    // Guard shutdown must never terminate the newly created Desktop.
    UniqueHandle processHandle(process.hProcess);
    UniqueHandle threadHandle(process.hThread);
    return Json{{"pid", static_cast<std::uint64_t>(process.dwProcessId)},
                {"launch_method", "native_process"},
                {"activation_state", "not_submitted"}, {"instance", "created"},
                {"package_identity", "not_applicable"}, {"aumid", "not_applicable"},
                {"target_elevation", processElevation(processHandle.get())}};
}

const Json &at(const Json &object, const char *key)
{
    static const Json nullValue = Json();
    const auto entry = object.find(key);
    return entry == object.end() ? nullValue : *entry;
}

std::string stringAt(const Json &object, const char *key)
{
    const Json &value = at(object, key);
    return value.is_string() ? value.get<std::string>() : std::string();
}

} // namespace

ProxyPlan proxyPlan(const Config &config) {
    config.validate();
    std::string bypass;
    for (const std::string &entry : config.noProxy) {
        if (!bypass.empty()) bypass += ';';
        bypass += bypassEntry(entry);
    }
    const std::string endpoint = config.proxyUrl();
    if (endpoint.size() > 2048 || bypass.size() > 4096)
        throw Error("PROXY_LAUNCH_PLAN_INVALID", "Proxy launch arguments exceed the size limit.");
    return {endpoint, "--proxy-server=\"" + endpoint + "\" --proxy-bypass-list=\"" + bypass + "\""};
}

UnicodeEnvironment proxyEnvironment(const Config &config) {
    config.validate();
    UnicodeEnvironment environment = UnicodeEnvironment::current();
    for (const wchar_t *key : {L"HTTP_PROXY", L"HTTPS_PROXY", L"http_proxy", L"https_proxy"})
        environment.set(key, utf8ToWide(config.proxyUrl()));
    environment.set(L"NO_PROXY", utf8ToWide(config.noProxyValue()));
    environment.set(L"no_proxy", utf8ToWide(config.noProxyValue()));
    environment.remove(L"ALL_PROXY");
    environment.remove(L"all_proxy");
    return environment;
}

CodexCli detail::resolveCliFrom(const std::filesystem::path &explicitHome,
                                const std::filesystem::path &userHome,
                                const std::string &path, const std::filesystem::path &overridePath,
                                const std::filesystem::path &cwd) {
    std::filesystem::path home;
    if (!explicitHome.empty()) {
        const std::filesystem::path resolved = absolutePath(explicitHome)
            ? explicitHome : cwd / explicitHome;
        if (!std::filesystem::is_directory(resolved))
            throw Error("CODEX_HOME_INVALID", "Explicit CODEX_HOME must resolve to an existing directory.");
        const auto canonical = canonicalPathOf(resolved, true);
        if (!canonical) throw Error("CODEX_HOME_INVALID", "Explicit CODEX_HOME must resolve to an existing directory.");
        home = *canonical;
    } else {
        if (userHome.empty())
            throw Error("CODEX_HOME_UNRESOLVED", "Cannot determine the user Home for Codex CLI resolution.");
        const std::filesystem::path candidate = userHome / ".codex";
        if (std::filesystem::is_directory(candidate)) {
            const auto canonical = canonicalPathOf(candidate, true);
            if (canonical) home = *canonical;
        }
    }
    // Pin every resolved scope, including absolute homes, so subsequent cwd or
    // environment changes cannot redirect this operation to a different Home.
    if (!overridePath.empty()) {
        const std::filesystem::path executable = canonicalExe(overridePath);
        if (executable.empty())
            throw Error("CODEX_CLI_OVERRIDE_INVALID", "Codex CLI override must be an existing absolute native .exe, not a shell shim.");
        return {executable, home};
    }
    if (!home.empty()) {
        for (const wchar_t *relative : {L"packages/app-server-daemon/current/bin/codex.exe",
                                        L"packages/standalone/current/bin/codex.exe",
                                        L"packages/standalone/current/codex.exe"}) {
            const std::filesystem::path executable = canonicalExe(home / relative);
            if (!executable.empty()) return {executable, home};
        }
    }
    const auto cwdCanonical = canonicalPathOf(cwd, true);
    const std::wstring current = cwdCanonical
        ? normalizedWindowsPath(cwdCanonical->wstring()) : std::wstring();
    std::size_t offset = 0;
    while (offset <= path.size()) {
        const std::size_t separator = path.find(';', offset);
        const std::size_t stop = separator == std::string::npos ? path.size() : separator;
        std::string entry = path.substr(offset, stop - offset);
        offset = separator == std::string::npos ? path.size() + 1 : separator + 1;
        if (entry.size() > 1 && entry.front() == '"' && entry.back() == '"')
            entry = entry.substr(1, entry.size() - 2);
        const std::filesystem::path directoryPath = std::filesystem::path(utf8ToWide(entry));
        if (!absolutePath(directoryPath)) continue;
        const std::wstring tail = directoryPath.wstring();
        if (tail.size() >= 2 && tail.compare(tail.size() - 2, 2, L"\\.") == 0) continue;
        if (tail.size() >= 2 && tail.compare(tail.size() - 2, 2, L"/.") == 0) continue;
        if (!std::filesystem::is_directory(directoryPath)) continue;
        const auto canonical = canonicalPathOf(directoryPath, true);
        if (!canonical) continue;
        if (!current.empty() && normalizedWindowsPath(canonical->wstring()) == current) continue;
        const std::filesystem::path executable = canonicalExe(*canonical / L"codex.exe");
        if (!executable.empty()) return {executable, home};
    }
    throw Error("CODEX_CLI_UNAVAILABLE", "No native Codex CLI was found in the controlled locations; use normal launch or configure an absolute CLI override.");
}

CodexCli resolveCodexCli(const Config &config) {
    // For registered repair the CLI must operate on the same explicitly
    // authorized Home whose backend block was prepared, never Guard's Home.
    const wchar_t *explicitEnvironment = _wgetenv(L"CODEX_HOME");
    const std::filesystem::path explicitHome = config.manageBackend
        ? config.home
        : (explicitEnvironment ? std::filesystem::path(explicitEnvironment) : std::filesystem::path());
    const wchar_t *profileEnvironment = _wgetenv(L"USERPROFILE");
    std::filesystem::path userHome = profileEnvironment ? std::filesystem::path(profileEnvironment)
                                                        : std::filesystem::path();
    if (userHome.empty()) {
        const wchar_t *homeEnvironment = _wgetenv(L"HOME");
        if (homeEnvironment) userHome = std::filesystem::path(homeEnvironment);
    }
    const wchar_t *pathEnvironment = _wgetenv(L"PATH");
    return detail::resolveCliFrom(explicitHome, userHome,
                                  pathEnvironment ? wideToUtf8(pathEnvironment) : std::string(),
                                  config.cliOverride,
                                  std::filesystem::current_path());
}

std::string detail::daemonStopStatus(const std::string &output) {
    try {
        const Json object = parseStrictJsonObject(output, daemonOutputLimit, "CODEX_DAEMON_STOP_FAILED");
        const Json &status = at(object, "status");
        if (!status.is_string()) return {};
        const std::string value = status.get<std::string>();
        if (value == "stopped") return "stopped";
        if (value == "notRunning") return "not_needed";
    } catch (const Error &) {}
    return {};
}

Json detail::activationReceipt(const Json &worker) {
    const auto failed = [&](const char *name, const char *code, const char *message) {
        if (!at(worker, name).is_null()) throw Error(code, message);
    };
    failed("failure", "APPX_ACTIVATION_FAILED", "The activation worker reported failure; Desktop may already have been created.");
    failed("activation_hresult", "APPX_ACTIVATION_FAILED", "Windows application activation failed; no fallback was attempted.");
    const Json &pid = at(worker, "pid");
    const bool pidValid = pid.is_number_integer()
        && pid.get<std::int64_t>() > 0;
    const Json &returned = at(worker, "activation_returned");
    const bool returnedValue = returned.is_boolean() && returned.get<bool>();
    if (!returnedValue || !pidValid)
        throw Error("APPX_ACTIVATION_OUTCOME_UNKNOWN", "Activation produced no confirmed target; refresh Desktop state and do not retry automatically.");
    for (const char *name : {"package_identity", "aumid"}) {
        const std::string observation = stringAt(worker, name);
        if (observation != "matched") {
            const std::string prefix = std::string(name) == "aumid" ? "APPX_AUMID_" : "APPX_IDENTITY_";
            const std::string suffix = observation == "missing" ? "MISSING"
                : observation == "mismatch" ? "MISMATCH" : "QUERY_FAILED";
            throw Error(prefix + suffix, "Activated Desktop identity could not be confirmed; the application may already exist.");
        }
    }
    failed("early_exit_code", "APPX_TARGET_EXITED_EARLY", "The activated Desktop exited during observation.");
    failed("observation_failure", "APPX_ACTIVATION_OUTCOME_UNKNOWN", "Activated Desktop observations were incomplete; do not retry automatically.");
    const Json &targetElevation = at(worker, "elevation");
    if (!targetElevation.is_boolean())
        throw Error("APPX_TARGET_ELEVATION_QUERY_FAILED", "Activated Desktop elevation could not be verified; do not retry automatically.");
    if (targetElevation.get<bool>())
        throw Error("APPX_TARGET_ELEVATED", "Activated Desktop is elevated; normal-user launch could not be verified.");
    return Json{{"pid", at(worker, "pid")}, {"launch_method", "appmodel_activation"},
                {"activation_state", "returned"}, {"instance", at(worker, "instance")},
                {"package_identity", "matched"}, {"aumid", "matched"},
                {"target_elevation", at(worker, "elevation")}};
}

std::string stopCodexDaemon(const CodexCli &cli, const Cancellation &cancel,
                            std::chrono::milliseconds budget) {
    cancel.check();
    if (budget <= std::chrono::milliseconds::zero() || canonicalExe(cli.executable).empty())
        throw Error("CODEX_DAEMON_STOP_FAILED", "Resolved Codex CLI or operation budget is invalid.");
    UnicodeEnvironment environment = UnicodeEnvironment::current();
    if (!cli.home.empty()) environment.set(L"CODEX_HOME", cli.home.wstring());
    HelperResult result;
    try {
        result = runHelper(cli.executable, {L"app-server", L"daemon", L"stop"}, {}, cancel,
                           budget, daemonOutputLimit, environment);
    } catch (const Error &error) {
        if (cancel.cancelled())
            throw Error("LAUNCH_CANCELLED", "Repair cancelled; the shared background server state is unconfirmed.");
        if (error.code.find("TIMEOUT") != std::string::npos)
            throw Error("CODEX_DAEMON_STOP_TIMEOUT", "Repair exceeded its budget; Desktop was not started and shared service state is unconfirmed.");
        throw Error("CODEX_DAEMON_STOP_FAILED", "The public daemon-stop helper failed; shared service state is unconfirmed.");
    }
    if (result.out.size() > daemonOutputLimit || result.err.size() > daemonOutputLimit
        || !strictUtf8(result.out) || !strictUtf8(result.err))
        throw Error("CODEX_DAEMON_STOP_FAILED", "Codex CLI output exceeded its limit or was not valid UTF-8.");
    if (result.exitCode != 0) {
        const std::string combined = toLower(result.out + '\n' + result.err);
        if (combined.find("unrecognized subcommand") != std::string::npos
            || combined.find("unknown command") != std::string::npos)
            throw Error("CODEX_DAEMON_UNSUPPORTED", "This Codex CLI does not support the public daemon lifecycle command; use normal launch.");
        throw Error("CODEX_DAEMON_STOP_FAILED", "The public Codex daemon-stop command failed; Desktop was not started.");
    }
    const std::string status = detail::daemonStopStatus(result.out);
    if (status.empty())
        throw Error("CODEX_DAEMON_STOP_FAILED", "Unexpected daemon lifecycle JSON; shared service state is unconfirmed.");
    return status;
}

Json detail::launchWith(const Config &config, LaunchOptions options,
                        const Cancellation &cancel, const LaunchServices &services, LaunchFacts *facts) {
    LaunchFacts localFacts;
    if (!facts) facts = &localFacts;
    *facts = LaunchFacts{};
    if (options.repair && options.activationOnly)
        throw Error("INVALID_LAUNCH_OPTIONS", "Activation-only cannot be combined with daemon repair.");
    config.validate();
    cancel.check();
    services.requireElevation();
    const Desktop desktop = services.discover();
    cancel.check();
    requireStopped(services.process(desktop));
    if (options.activationOnly && !desktop.registered)
        throw Error("ACTIVATION_ONLY_UNSUPPORTED", "Activation-only requires a registered Desktop application.");
    if (desktop.registered && (desktop.packageFullName.empty() || desktop.packageFamilyName.empty()
        || desktop.applicationId.empty() || desktop.manifestExecutable.empty()
        || desktop.aumid != desktop.packageFamilyName + '!' + desktop.applicationId
        || desktop.runtimeKind != "full_trust_desktop"))
        throw Error("APPX_METADATA_INCOMPLETE", "Desktop has no verified FullTrust application identity.");

    ProxyPlan plan;
    std::string backend = "not_applicable";
    if (!options.activationOnly) {
        plan = desktop.registered ? proxyPlan(config) : ProxyPlan{config.proxyUrl(), {}};
        if (desktop.registered) {
            if (!config.manageBackend) {
                if (options.repair)
                    throw Error("BACKEND_PROXY_REQUIRED_FOR_REPAIR", "Authorize the Codex backend proxy block before a registered repair launch; the daemon was not touched.");
                backend = "not_authorized";
            } else {
                cancel.check();
                services.prepare();
                backend = "prepared";
            }
        }
    }
    std::string daemon = "skipped";
    facts->backendProxyConfig = backend;
    CodexCli cli;
    try {
        if (options.repair) {
            cancel.check();
            if (!sameTarget(desktop, services.discover()))
                throw Error("APPX_PACKAGE_CHANGED", "Desktop changed before repair; refresh and retry.");
            requireStopped(services.process(desktop));
            cli = services.resolve();
            cancel.check();
            facts->daemonPreparation = "unconfirmed";
            daemon = services.stop(cli); // exactly once, and only after preparation
            facts->daemonPreparation = daemon;
            if (daemon != "stopped" && daemon != "not_needed")
                throw Error("CODEX_DAEMON_STOP_FAILED", "Daemon stop did not confirm its final state.");
            cancel.check();
        }
        // Fresh registration plus a final process check cover a package update
        // or another client starting Desktop during the cancellable stop wait.
        if (!sameTarget(desktop, services.discover()))
            throw Error("APPX_PACKAGE_CHANGED", "Desktop changed during preparation; refresh and retry.");
        requireStopped(services.process(desktop));
        services.requireElevation();
        cancel.check();
        Json receipt;
        services.markSubmitted();
        if (desktop.registered) {
            facts->activationState = "unknown";
            const Json worker = services.activate(desktop, plan.arguments);
            if (at(worker, "activation_returned").is_boolean()
                && at(worker, "activation_returned").get<bool>()) facts->activationState = "returned";
            receipt = detail::activationReceipt(worker);
            receipt["proxy_delivery"] = options.activationOnly ? "not_established"
                : backend == "prepared" ? "activation_arguments_and_home_config" : "activation_arguments";
        } else {
            UnicodeEnvironment environment = proxyEnvironment(config);
            if (!cli.home.empty()) environment.set(L"CODEX_HOME", cli.home.wstring());
            facts->activationState = "unknown";
            receipt = services.native(desktop, environment);
            facts->activationState = "returned";
            receipt["proxy_delivery"] = "process_environment";
        }
        receipt["proxy_endpoint"] = options.activationOnly ? Json(nullptr) : Json(plan.endpoint);
        receipt["backend_proxy_config"] = backend;
        receipt["daemon_preparation"] = daemon;
        receipt["desktop"] = desktop.publicJson();
        return receipt;
    } catch (const Error &error) {
        if (error.code == "LAUNCH_CANCELLED" && facts->activationState == "unknown")
            facts->activationState = "not_submitted";
        if (daemon == "stopped")
            throw Error(error.code, error.message + "; the shared Codex background server was stopped before this failure.");
        throw;
    }
}

Json launchPipeline(const Config &config, const std::filesystem::path &configPath,
                    LaunchOptions options, const Cancellation &cancel, LaunchFacts *facts) {
    cancel.check();
    ConfigLease lease(configPath, config);
    requireNonElevated();
    StartupLock startup;
    detail::LaunchServices services;
    services.requireElevation = requireNonElevated;
    services.discover = [&] { return discoverDesktop(config, cancel); };
    services.process = desktopProcessState;
    services.prepare = [&] { prepareProxyEnv(config); };
    services.resolve = [&] { return resolveCodexCli(config); };
    services.stop = [&](const CodexCli &cli) { return stopCodexDaemon(cli, cancel); };
    services.activate = [&](const Desktop &desktop, const std::string &arguments) { return activateDesktop(desktop, arguments, cancel); };
    services.native = [&](const Desktop &desktop, const UnicodeEnvironment &environment) { return nativeLaunch(desktop, environment, cancel); };
    services.markSubmitted = [&] { startup.markSubmitted(); };
    return detail::launchWith(config, options, cancel, services, facts);
}

} // namespace cpg

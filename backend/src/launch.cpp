#include "launch_internal.h"
#include <QDateTime>
#include <QDir>
#include <QFileInfo>
#include <QJsonDocument>
#include <QRegularExpression>
#include <algorithm>
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
constexpr qsizetype daemonOutputLimit = 64 * 1024;

class Handle final {
public:
    explicit Handle(HANDLE value = INVALID_HANDLE_VALUE) : value_(value) {}
    ~Handle() { if (value_ && value_ != INVALID_HANDLE_VALUE) CloseHandle(value_); }
    Handle(const Handle &) = delete;
    Handle &operator=(const Handle &) = delete;
    HANDLE get() const { return value_; }
private:
    HANDLE value_;
};

class StartupLock final {
public:
    StartupLock() : handle_(open()) {
        char previous[64]{};
        DWORD count = 0;
        if (!ReadFile(handle_.get(), previous, sizeof(previous), &count, nullptr))
            throw Error("LAUNCH_BUSY", "Cannot read the cross-process startup lock.");
        bool valid = false;
        const qint64 last = QByteArray(previous, static_cast<qsizetype>(count)).trimmed().toLongLong(&valid);
        const qint64 now = QDateTime::currentMSecsSinceEpoch();
        if (valid && last <= now && now - last < 5000)
            throw Error("LAUNCH_BUSY", "Another Guard instance just launched Desktop; refresh before retrying.");
    }
    void markSubmitted() {
        LARGE_INTEGER zero{};
        const QByteArray timestamp = QByteArray::number(QDateTime::currentMSecsSinceEpoch());
        DWORD written = 0;
        if (!SetFilePointerEx(handle_.get(), zero, nullptr, FILE_BEGIN)
            || !SetEndOfFile(handle_.get())
            || !WriteFile(handle_.get(), timestamp.constData(), static_cast<DWORD>(timestamp.size()), &written, nullptr)
            || written != static_cast<DWORD>(timestamp.size()) || !FlushFileBuffers(handle_.get()))
            throw Error("LAUNCH_BUSY", "Cannot record the startup guard; Desktop was not submitted.");
    }
private:
    static HANDLE open() {
        const QString path = QDir::toNativeSeparators(QDir::temp().filePath("codex-proxy-guard-startup.lock"));
        HANDLE result = CreateFileW(reinterpret_cast<LPCWSTR>(path.utf16()), GENERIC_READ | GENERIC_WRITE,
                                    0, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
        if (result == INVALID_HANDLE_VALUE)
            throw Error("LAUNCH_BUSY", "Another Guard instance is launching, or the startup lock is unavailable.");
        return result;
    }
    Handle handle_;
};

bool absolutePath(const QString &path) {
    // QDir's Windows rooted-path handling also accepts forms like /foo; native
    // CLI selection requires a drive-absolute or UNC path instead.
    static const QRegularExpression drive(QStringLiteral("^[A-Za-z]:[/\\\\]"));
    return !path.contains(QChar::Null) && (drive.match(path).hasMatch()
           || path.startsWith("\\\\") || path.startsWith("//"));
}

QString canonicalExe(const QString &path) {
    const QFileInfo file(path);
    if (!absolutePath(path) || !file.isFile() || file.suffix().compare("exe", Qt::CaseInsensitive) != 0)
        return {};
    return file.canonicalFilePath();
}

QString bypassEntry(const QString &entry) {
    if (entry.compare("localhost", Qt::CaseInsensitive) == 0) return "localhost";
    IN_ADDR ipv4{};
    if (InetPtonW(AF_INET, reinterpret_cast<LPCWSTR>(entry.utf16()), &ipv4) == 1
        && (ntohl(ipv4.S_un.S_addr) >> 24) == 127) return entry;
    IN6_ADDR ipv6{};
    if (InetPtonW(AF_INET6, reinterpret_cast<LPCWSTR>(entry.utf16()), &ipv6) == 1
        && IN6_IS_ADDR_LOOPBACK(&ipv6)) return '[' + entry + ']';
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
        && normalizedWindowsPath(first.executable) == normalizedWindowsPath(second.executable)
        && first.packageFullName == second.packageFullName
        && first.packageFamilyName == second.packageFamilyName
        && first.aumid == second.aumid && first.applicationId == second.applicationId
        && first.runtimeKind == second.runtimeKind && first.manifestExecutable == second.manifestExecutable;
}

QJsonValue processElevation(HANDLE process) {
    HANDLE raw = nullptr;
    if (!OpenProcessToken(process, TOKEN_QUERY, &raw)) return QJsonValue::Null;
    Handle token(raw);
    TOKEN_ELEVATION result{};
    DWORD count = 0;
    if (!GetTokenInformation(token.get(), TokenElevation, &result, sizeof(result), &count))
        return QJsonValue::Null;
    return result.TokenIsElevated != 0;
}

QJsonObject nativeLaunch(const Desktop &desktop, const QProcessEnvironment &environment,
                         const Cancellation &cancel) {
    if (desktop.registered)
        throw Error("APPX_METADATA_INCOMPLETE", "Registered applications require application-model activation.");
    const QString executable = QDir::toNativeSeparators(desktop.executable);
    if (!absolutePath(executable) || executable.contains('"') || !QFileInfo(executable).isFile())
        throw Error("CODEX_EXECUTABLE_MISSING", "Desktop executable is missing or invalid.");
    QStringList entries = environment.toStringList();
    std::sort(entries.begin(), entries.end(), [](const QString &left, const QString &right) {
        return left.compare(right, Qt::CaseInsensitive) < 0;
    });
    std::vector<wchar_t> block;
    for (const QString &entry : entries) {
        const std::wstring value = entry.toStdWString();
        block.insert(block.end(), value.begin(), value.end());
        block.push_back(L'\0');
    }
    block.push_back(L'\0');
    if (entries.isEmpty()) block.push_back(L'\0');
    std::wstring command = (QStringLiteral("\"") + executable + '"').toStdWString();
    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION process{};
    cancel.check();
    if (!CreateProcessW(reinterpret_cast<LPCWSTR>(executable.utf16()), command.data(), nullptr, nullptr, FALSE,
                        CREATE_UNICODE_ENVIRONMENT | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
                        block.data(), nullptr, &startup, &process))
        throw Error("CODEX_LAUNCH_FAILED", "Windows could not create the Desktop process.");
    // These handles are observations only. In particular, cancellation and
    // Guard shutdown must never terminate the newly created Desktop.
    Handle processHandle(process.hProcess);
    Handle threadHandle(process.hThread);
    return {{"pid", static_cast<qint64>(process.dwProcessId)}, {"launch_method", "native_process"},
            {"activation_state", "not_submitted"}, {"instance", "created"},
            {"package_identity", "not_applicable"}, {"aumid", "not_applicable"},
            {"target_elevation", processElevation(processHandle.get())}};
}
}

ProxyPlan proxyPlan(const Config &config) {
    config.validate();
    QStringList bypass;
    for (const QString &entry : config.noProxy) bypass.append(bypassEntry(entry));
    const QString endpoint = config.proxyUrl();
    const QString list = bypass.join(';');
    if (endpoint.size() > 2048 || list.size() > 4096)
        throw Error("PROXY_LAUNCH_PLAN_INVALID", "Proxy launch arguments exceed the size limit.");
    return {endpoint, QStringLiteral("--proxy-server=\"%1\" --proxy-bypass-list=\"%2\"").arg(endpoint, list)};
}

QProcessEnvironment proxyEnvironment(const Config &config) {
    config.validate();
    auto environment = QProcessEnvironment::systemEnvironment();
    for (const QString &key : {QStringLiteral("HTTP_PROXY"), QStringLiteral("HTTPS_PROXY"),
                               QStringLiteral("http_proxy"), QStringLiteral("https_proxy")})
        environment.insert(key, config.proxyUrl());
    environment.insert("NO_PROXY", config.noProxyValue());
    environment.insert("no_proxy", config.noProxyValue());
    environment.remove("ALL_PROXY");
    environment.remove("all_proxy");
    return environment;
}

CodexCli detail::resolveCliFrom(const QString &explicitHome, const QString &userHome,
                              const QString &path, const QString &overridePath, const QString &cwd) {
    QString home;
    if (!explicitHome.isEmpty()) {
        const QFileInfo directory(absolutePath(explicitHome) ? explicitHome : QDir(cwd).filePath(explicitHome));
        if (!directory.isDir() || directory.canonicalFilePath().isEmpty())
            throw Error("CODEX_HOME_INVALID", "Explicit CODEX_HOME must resolve to an existing directory.");
        home = directory.canonicalFilePath();
    } else {
        if (userHome.isEmpty())
            throw Error("CODEX_HOME_UNRESOLVED", "Cannot determine the user Home for Codex CLI resolution.");
        const QFileInfo directory(QDir(userHome).filePath(".codex"));
        if (directory.isDir()) home = directory.canonicalFilePath();
    }
    // Pin every resolved scope, including absolute homes, so subsequent cwd or
    // environment changes cannot redirect this operation to a different Home.
    if (!overridePath.isEmpty()) {
        const QString executable = canonicalExe(overridePath);
        if (executable.isEmpty())
            throw Error("CODEX_CLI_OVERRIDE_INVALID", "Codex CLI override must be an existing absolute native .exe, not a shell shim.");
        return {executable, home};
    }
    if (!home.isEmpty()) {
        for (const QString &relative : {QStringLiteral("packages/app-server-daemon/current/bin/codex.exe"),
                                       QStringLiteral("packages/standalone/current/bin/codex.exe"),
                                       QStringLiteral("packages/standalone/current/codex.exe")}) {
            const QString executable = canonicalExe(QDir(home).filePath(relative));
            if (!executable.isEmpty()) return {executable, home};
        }
    }
    const QString current = normalizedWindowsPath(QFileInfo(cwd).canonicalFilePath());
    for (QString entry : path.split(';', Qt::SkipEmptyParts)) {
        if (entry.startsWith('"') && entry.endsWith('"') && entry.size() > 1)
            entry = entry.mid(1, entry.size() - 2);
        if (!absolutePath(entry) || entry.endsWith("/.") || entry.endsWith("\\.")) continue;
        const QFileInfo directory(entry);
        if (!directory.isDir()) continue;
        const QString canonical = directory.canonicalFilePath();
        if (canonical.isEmpty() || normalizedWindowsPath(canonical) == current) continue;
        const QString executable = canonicalExe(QDir(canonical).filePath("codex.exe"));
        if (!executable.isEmpty()) return {executable, home};
    }
    throw Error("CODEX_CLI_UNAVAILABLE", "No native Codex CLI was found in the controlled locations; use normal launch or configure an absolute CLI override.");
}

CodexCli resolveCodexCli(const Config &config) {
    // For registered repair the CLI must operate on the same explicitly
    // authorized Home whose backend block was prepared, never Guard's Home.
    const QString explicitHome = config.manageBackend ? config.home : qEnvironmentVariable("CODEX_HOME");
    QString userHome = qEnvironmentVariable("USERPROFILE");
    if (userHome.isEmpty()) userHome = qEnvironmentVariable("HOME");
    return detail::resolveCliFrom(explicitHome, userHome, qEnvironmentVariable("PATH"), config.cliOverride, QDir::currentPath());
}

QString detail::daemonStopStatus(const QByteArray &output) {
    try {
        const auto object = parseJsonObject(output, daemonOutputLimit, "CODEX_DAEMON_STOP_FAILED");
        const auto status = object.value("status");
        if (!status.isString()) return {};
        if (status.toString() == "stopped") return "stopped";
        if (status.toString() == "notRunning") return "not_needed";
    } catch (const Error &) {}
    return {};
}

QJsonObject detail::activationReceipt(const QJsonObject &worker) {
    const auto failed = [&](const QString &field, const QString &code, const QString &message) {
        if (!worker.value(field).isNull() && !worker.value(field).isUndefined())
            throw Error(code, message);
    };
    failed("failure", "APPX_ACTIVATION_FAILED", "The activation worker reported failure; Desktop may already have been created.");
    failed("activation_hresult", "APPX_ACTIVATION_FAILED", "Windows application activation failed; no fallback was attempted.");
    if (!worker.value("activation_returned").toBool() || worker.value("pid").toInteger() <= 0)
        throw Error("APPX_ACTIVATION_OUTCOME_UNKNOWN", "Activation produced no confirmed target; refresh Desktop state and do not retry automatically.");
    for (const QString &field : {QStringLiteral("package_identity"), QStringLiteral("aumid")}) {
        const QString observation = worker.value(field).toString();
        if (observation != "matched") {
            const QString prefix = field == "aumid" ? "APPX_AUMID_" : "APPX_IDENTITY_";
            const QString suffix = observation == "missing" ? "MISSING"
                : observation == "mismatch" ? "MISMATCH" : "QUERY_FAILED";
            throw Error(prefix + suffix, "Activated Desktop identity could not be confirmed; the application may already exist.");
        }
    }
    failed("early_exit_code", "APPX_TARGET_EXITED_EARLY", "The activated Desktop exited during observation.");
    failed("observation_failure", "APPX_ACTIVATION_OUTCOME_UNKNOWN", "Activated Desktop observations were incomplete; do not retry automatically.");
    return {{"pid", worker.value("pid")}, {"launch_method", "appmodel_activation"},
            {"activation_state", "returned"}, {"instance", worker.value("instance")},
            {"package_identity", "matched"}, {"aumid", "matched"},
            {"target_elevation", worker.value("elevation")}};
}

QString stopCodexDaemon(const CodexCli &cli, const Cancellation &cancel, int budgetMs) {
    cancel.check();
    if (budgetMs <= 0 || canonicalExe(cli.executable).isEmpty())
        throw Error("CODEX_DAEMON_STOP_FAILED", "Resolved Codex CLI or operation budget is invalid.");
    auto environment = QProcessEnvironment::systemEnvironment();
    if (!cli.home.isEmpty()) environment.insert("CODEX_HOME", cli.home);
    HelperResult result;
    try {
        result = runHelper(cli.executable, {"app-server", "daemon", "stop"}, {}, cancel,
                           budgetMs, daemonOutputLimit, environment);
    } catch (const Error &error) {
        if (cancel.cancelled())
            throw Error("LAUNCH_CANCELLED", "Repair cancelled; the shared background server state is unconfirmed.");
        if (error.code.contains("TIMEOUT"))
            throw Error("CODEX_DAEMON_STOP_TIMEOUT", "Repair exceeded its budget; Desktop was not started and shared service state is unconfirmed.");
        throw Error("CODEX_DAEMON_STOP_FAILED", "The public daemon-stop helper failed; shared service state is unconfirmed.");
    }
    if (result.out.size() > daemonOutputLimit || result.err.size() > daemonOutputLimit
        || !strictUtf8(result.out) || !strictUtf8(result.err))
        throw Error("CODEX_DAEMON_STOP_FAILED", "Codex CLI output exceeded its limit or was not valid UTF-8.");
    if (result.exitCode != 0) {
        const QByteArray combined = (result.out + '\n' + result.err).toLower();
        if (combined.contains("unrecognized subcommand") || combined.contains("unknown command"))
            throw Error("CODEX_DAEMON_UNSUPPORTED", "This Codex CLI does not support the public daemon lifecycle command; use normal launch.");
        throw Error("CODEX_DAEMON_STOP_FAILED", "The public Codex daemon-stop command failed; Desktop was not started.");
    }
    const QString status = detail::daemonStopStatus(result.out);
    if (status.isEmpty())
        throw Error("CODEX_DAEMON_STOP_FAILED", "Unexpected daemon lifecycle JSON; shared service state is unconfirmed.");
    return status;
}

QJsonObject detail::launchWith(const Config &config, LaunchOptions options,
                               const Cancellation &cancel, const LaunchServices &services) {
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
    if (desktop.registered && (desktop.packageFullName.isEmpty() || desktop.packageFamilyName.isEmpty()
        || desktop.applicationId.isEmpty() || desktop.manifestExecutable.isEmpty()
        || desktop.aumid != desktop.packageFamilyName + '!' + desktop.applicationId
        || desktop.runtimeKind != "full_trust_desktop"))
        throw Error("APPX_METADATA_INCOMPLETE", "Desktop has no verified FullTrust application identity.");

    ProxyPlan plan;
    QString backend = "not_applicable";
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
    QString daemon = "skipped";
    CodexCli cli;
    try {
        if (options.repair) {
            cancel.check();
            if (!sameTarget(desktop, services.discover()))
                throw Error("APPX_PACKAGE_CHANGED", "Desktop changed before repair; refresh and retry.");
            requireStopped(services.process(desktop));
            cli = services.resolve();
            cancel.check();
            daemon = services.stop(cli); // exactly once, and only after preparation
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
        QJsonObject receipt;
        services.markSubmitted();
        if (desktop.registered) {
            receipt = activationReceipt(services.activate(desktop, plan.arguments));
            receipt.insert("proxy_delivery", options.activationOnly ? "not_established"
                           : backend == "prepared" ? "activation_arguments_and_home_config" : "activation_arguments");
        } else {
            auto environment = proxyEnvironment(config);
            if (!cli.home.isEmpty()) environment.insert("CODEX_HOME", cli.home);
            receipt = services.native(desktop, environment);
            receipt.insert("proxy_delivery", "process_environment");
        }
        receipt.insert("proxy_endpoint", options.activationOnly ? QJsonValue(QJsonValue::Null) : QJsonValue(plan.endpoint));
        receipt.insert("backend_proxy_config", backend);
        receipt.insert("daemon_preparation", daemon);
        receipt.insert("desktop", desktop.publicJson());
        return receipt;
    } catch (const Error &error) {
        if (daemon == "stopped")
            throw Error(error.code, error.message + "; the shared Codex background server was stopped before this failure.");
        throw;
    }
}

QJsonObject launchPipeline(const Config &config, const QString &configPath,
                           LaunchOptions options, const Cancellation &cancel) {
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
    services.activate = [&](const Desktop &desktop, const QString &arguments) { return activateDesktop(desktop, arguments, cancel); };
    services.native = [&](const Desktop &desktop, const QProcessEnvironment &environment) { return nativeLaunch(desktop, environment, cancel); };
    services.markSubmitted = [&] { startup.markSubmitted(); };
    return detail::launchWith(config, options, cancel, services);
}
}

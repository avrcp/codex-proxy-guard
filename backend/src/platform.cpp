#include "platform.h"
#include "platform_protocol_p.h"
#include <QCoreApplication>
#include <QDir>
#include <QElapsedTimer>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QProcess>
#include <QRegularExpression>
#include <QThread>
#include <algorithm>
#include <array>
#include <cmath>
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
constexpr qsizetype requestLimit = 16 * 1024;
constexpr qsizetype receiptLimit = 64 * 1024;
const QString discoveryError = QStringLiteral("APPX_DISCOVERY_PROTOCOL_INVALID");
const QString activationError = QStringLiteral("APPX_ACTIVATION_PROTOCOL_INVALID");

[[noreturn]] void fail(const QString &code, const QString &message)
{
    throw Error(code, message);
}
class Handle {
public:
    explicit Handle(HANDLE value = nullptr) : value_(value) {}
    ~Handle() { if (value_ && value_ != INVALID_HANDLE_VALUE) CloseHandle(value_); }
    Handle(const Handle &) = delete;
    Handle &operator=(const Handle &) = delete;
    HANDLE get() const { return value_; }
    bool valid() const { return value_ && value_ != INVALID_HANDLE_VALUE; }
private:
    HANDLE value_;
};
const wchar_t *wide(const QString &text) { return reinterpret_cast<const wchar_t *>(text.utf16()); }
QString field(const QJsonObject &object, const QString &key, const QString &code,
              bool optional = false, bool nullable = false)
{
    const auto value = object.value(key);
    if ((optional && value.isUndefined()) || (nullable && value.isNull())) return {};
    if (!value.isString()) fail(code, "Invalid string field: " + key);
    const auto result = value.toString();
    if (result.contains(QChar::Null)) fail(code, "Embedded NUL in field: " + key);
    return result;
}
void exactKeys(const QJsonObject &object, const QStringList &allowed, const QString &code)
{
    for (auto it = object.begin(); it != object.end(); ++it)
        if (!allowed.contains(it.key())) fail(code, "Unexpected protocol field");
}
QString canonicalFile(const QString &path)
{
    const QFileInfo info(path);
    if (!info.isAbsolute() || !info.isFile()) fail("CODEX_EXECUTABLE_MISSING", "Desktop executable is not an existing absolute file");
    const auto canonical = info.canonicalFilePath();
    if (canonical.isEmpty()) fail("CODEX_EXECUTABLE_INVALID", "Cannot resolve Desktop executable");
    return canonical;
}
bool within(const QString &root, const QString &path)
{
    const auto prefix = normalizedWindowsPath(root) + '/';
    return normalizedWindowsPath(path).startsWith(prefix);
}
bool supported(const QString &name) { return name == "OpenAI.Codex" || name == "OpenAI.ChatGPT-Desktop"; }
QString runtime(const QJsonObject &app)
{
    const auto entry = field(app, "entry_point", discoveryError, true);
    if (entry.compare("Windows.FullTrustApplication", Qt::CaseInsensitive) == 0) return "full_trust_desktop";
    if (entry.trimmed().isEmpty()
        && field(app, "runtime_behavior", discoveryError, true, true).trimmed().isEmpty()
        && field(app, "trust_level", discoveryError, true, true).trimmed().isEmpty()) return "unknown";
    return "app_container";
}
void validateRecord(const QJsonObject &record)
{
    for (const auto &key : {"package_name", "package_version", "install_location"})
        (void)field(record, key, discoveryError);
    for (const auto &key : {"package_full_name", "package_family_name", "architecture"})
        (void)field(record, key, discoveryError, true);
    const auto apps = record.value("applications");
    if (!apps.isUndefined() && !apps.isArray()) fail(discoveryError, "Applications must be an array");
    if (apps.toArray().size() > 16) fail(discoveryError, "Too many application records");
    for (const auto &value : apps.toArray()) {
        if (!value.isObject()) fail(discoveryError, "Application must be an object");
        const auto app = value.toObject();
        for (const auto &key : {"application_id", "manifest_executable", "entry_point"})
            (void)field(app, key, discoveryError, true);
        for (const auto &key : {"runtime_behavior", "trust_level"})
            (void)field(app, key, discoveryError, true, true);
    }
}
Desktop registeredDesktop(const QJsonObject &record)
{
    Desktop desktop;
    desktop.packageName = field(record, "package_name", discoveryError);
    desktop.product = desktop.packageName == "OpenAI.Codex" ? "chat_gpt" : "chat_gpt_classic";
    desktop.displayName = desktop.packageName == "OpenAI.Codex" ? "ChatGPT Desktop" : "ChatGPT Desktop (Classic)";
    desktop.packageVersion = field(record, "package_version", discoveryError);
    desktop.architecture = field(record, "architecture", discoveryError, true).trimmed();
    if (desktop.architecture.isEmpty()) desktop.architecture = "unknown";
    desktop.packageFullName = field(record, "package_full_name", discoveryError, true).trimmed();
    desktop.packageFamilyName = field(record, "package_family_name", discoveryError, true).trimmed();
    if (desktop.packageFullName.isEmpty() || desktop.packageFamilyName.isEmpty())
        fail("APPX_METADATA_INCOMPLETE", "Package identity metadata is missing");
    const QFileInfo root(field(record, "install_location", discoveryError));
    if (!root.isAbsolute() || !root.isDir() || root.canonicalFilePath().isEmpty())
        fail("APPX_INSTALL_LOCATION_INVALID", "Package installation directory is unavailable");
    desktop.installLocation = root.canonicalFilePath();
    QJsonArray candidates;
    for (const auto &value : record.value("applications").toArray()) {
        const auto app = value.toObject();
        const auto manifest = field(app, "manifest_executable", discoveryError, true);
        QString relative = manifest;
        relative.replace('\\', '/');
        const bool trusted = desktop.packageName == "OpenAI.Codex"
            ? field(app, "application_id", discoveryError, true) == "App"
                && relative.compare("app/ChatGPT.exe", Qt::CaseInsensitive) == 0
            : runtime(app) == "full_trust_desktop"
                && relative.section('/', -1).compare("ChatGPT.exe", Qt::CaseInsensitive) == 0;
        if (trusted) candidates.append(app);
    }
    if (candidates.isEmpty()) fail("APPX_APPLICATION_MISSING", "No trusted Desktop application was found");
    if (candidates.size() != 1) fail("APPX_APPLICATION_AMBIGUOUS", "Multiple trusted Desktop applications were found");
    const auto app = candidates.first().toObject();
    desktop.applicationId = field(app, "application_id", discoveryError, true).trimmed();
    desktop.manifestExecutable = field(app, "manifest_executable", discoveryError, true).trimmed();
    if (desktop.applicationId.isEmpty() || desktop.manifestExecutable.isEmpty())
        fail("APPX_METADATA_INCOMPLETE", "Application identity metadata is missing");
    QString relative = desktop.manifestExecutable;
    relative.replace('\\', '/');
    if (relative.startsWith('/') || relative.contains(':') || relative.split('/').contains(".."))
        fail("APPX_EXECUTABLE_INVALID", "Manifest executable must remain within the package directory");
    desktop.executable = canonicalFile(QDir(desktop.installLocation).filePath(relative));
    if (!within(desktop.installLocation, desktop.executable))
        fail("APPX_EXECUTABLE_INVALID", "Manifest executable resolves outside the package directory");
    desktop.aumid = desktop.packageFamilyName + '!' + desktop.applicationId;
    desktop.runtimeKind = runtime(app);
    desktop.registered = true;
    desktop.discoverySource = "appx_manifest";
    return desktop;
}
QString tokenElevation(HANDLE process)
{
    HANDLE raw = nullptr;
    if (!OpenProcessToken(process, TOKEN_QUERY, &raw)) return "unknown";
    const Handle token(raw);
    TOKEN_ELEVATION result{};
    DWORD size = 0;
    if (!GetTokenInformation(token.get(), TokenElevation, &result, sizeof(result), &size)
        || size != sizeof(result)) return "unknown";
    return result.TokenIsElevated ? "elevated" : "not_elevated";
}
QString imagePath(HANDLE process)
{
    std::array<wchar_t, 32768> buffer{};
    DWORD size = static_cast<DWORD>(buffer.size());
    if (!QueryFullProcessImageNameW(process, 0, buffer.data(), &size)) return {};
    return QString::fromWCharArray(buffer.data(), static_cast<int>(size));
}
quint64 ticks(const FILETIME &time)
{
    return (static_cast<quint64>(time.dwHighDateTime) << 32) | time.dwLowDateTime;
}
void observeIdentity(HANDLE process, QJsonObject &receipt, const QString &expected,
                     const QString &stateField, const QString &observedField, bool appId)
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
    const auto text = QString::fromWCharArray(buffer.data(), static_cast<int>(size - 1));
    if (text.contains(QChar::Null) || !text.isValidUtf16()) {
        receipt[stateField] = "query_failed";
        return;
    }
    receipt[observedField] = text;
    receipt[stateField] = text == expected ? "matched" : "mismatch";
}
QJsonObject emptyReceipt()
{
    return {{"version", 1}, {"failure", QJsonValue::Null}, {"activation_returned", false},
            {"activation_hresult", QJsonValue::Null}, {"pid", QJsonValue::Null},
            {"instance", "unknown"}, {"package_identity", "not_queried"},
            {"observed_package_full_name", QJsonValue::Null}, {"aumid", "not_queried"},
            {"observed_aumid", QJsonValue::Null}, {"elevation", QJsonValue::Null},
            {"early_exit_code", QJsonValue::Null}, {"observation_failure", QJsonValue::Null}};
}
QJsonObject activateAndObserve(const QJsonObject &request)
{
    auto receipt = emptyReceipt();
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
    const auto aumid = request.value("aumid").toString();
    const auto arguments = request.value("arguments").toString();
    DWORD pid = 0;
    const HRESULT activated = manager->ActivateApplication(wide(aumid), wide(arguments), AO_NONE, &pid);
    if (FAILED(activated)) {
        receipt["activation_hresult"] = QString("0x%1").arg(static_cast<quint32>(activated), 8, 16, QChar('0')).toUpper();
        return receipt;
    }
    receipt["activation_returned"] = true;
    receipt["pid"] = static_cast<qint64>(pid);
    const Handle process(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, FALSE, pid));
    if (!process.valid()) {
        receipt["package_identity"] = "query_failed";
        receipt["aumid"] = "query_failed";
        receipt["observation_failure"] = "APPX_TARGET_VERIFY_FAILED: Cannot open activated process";
        return receipt;
    }
    observeIdentity(process.get(), receipt, request.value("expected_package_full_name").toString(),
                    "package_identity", "observed_package_full_name", false);
    observeIdentity(process.get(), receipt, aumid, "aumid", "observed_aumid", true);
    const auto elevated = tokenElevation(process.get());
    if (elevated != "unknown") receipt["elevation"] = elevated == "elevated";
    FILETIME creation{}, exited{}, kernel{}, user{};
    if (GetProcessTimes(process.get(), &creation, &exited, &kernel, &user))
        receipt["instance"] = ticks(creation) + 250ULL * 10000ULL < ticks(submitted) ? "reused" : "created";
    else receipt["observation_failure"] = "APPX_TARGET_TIME_QUERY_FAILED: Cannot query process creation time";
    const auto actualPath = imagePath(process.get());
    if (!actualPath.isEmpty() && normalizedWindowsPath(actualPath)
        != normalizedWindowsPath(request.value("expected_executable").toString()))
        receipt["observation_failure"] = "APPX_TARGET_IMAGE_MISMATCH: Activated executable differs from selected Desktop";
    const DWORD wait = WaitForSingleObject(process.get(), 150);
    if (wait == WAIT_OBJECT_0) {
        DWORD code = 0;
        if (GetExitCodeProcess(process.get(), &code)) receipt["early_exit_code"] = static_cast<qint64>(code);
        else receipt["observation_failure"] = "APPX_TARGET_EXIT_QUERY_FAILED: Cannot query early exit";
    } else if (wait != WAIT_TIMEOUT) {
        receipt["observation_failure"] = "APPX_TARGET_WAIT_FAILED: Cannot observe target lifetime";
    }
    return receipt;
}
bool unsignedNumber(const QJsonValue &value, bool zeroAllowed)
{
    if (!value.isDouble()) return false;
    const auto number = value.toDouble();
    return std::isfinite(number) && std::floor(number) == number
        && number >= (zeroAllowed ? 0 : 1) && number <= std::numeric_limits<quint32>::max();
}
}

QString normalizedWindowsPath(const QString &path)
{
    QString result = path;
    result.replace('\\', '/');
    if (result.startsWith("//?/UNC/", Qt::CaseInsensitive)) result = "//" + result.mid(8);
    else if (result.startsWith("//?/")) result = result.mid(4);
    return QDir::cleanPath(result).toCaseFolded();
}
QJsonObject Desktop::publicJson() const
{
    return {{"state", "found"}, {"product", product}, {"display_name", displayName},
            {"package_version", publicText(packageVersion)}, {"architecture", publicText(architecture)},
            {"application_id", registered ? QJsonValue(publicText(applicationId)) : QJsonValue(QJsonValue::Null)},
            {"manifest_executable", registered ? QJsonValue(publicText(manifestExecutable)) : QJsonValue(QJsonValue::Null)},
            {"runtime_kind", registered ? QJsonValue(runtimeKind) : QJsonValue(QJsonValue::Null)}};
}
QString elevation() { return tokenElevation(GetCurrentProcess()); }
void requireNonElevated()
{
    const auto state = elevation();
    if (state == "elevated") fail("GUARD_ELEVATED", "Launch requires a non-elevated Guard process");
    if (state != "not_elevated") fail("GUARD_ELEVATION_QUERY_FAILED", "Cannot verify that Guard is not elevated");
}
HelperResult runHelper(const QString &program, const QStringList &arguments, const QByteArray &input,
                       const Cancellation &cancel, int timeoutMs, qsizetype outputLimit,
                       const QProcessEnvironment &environment, bool activationSubmission)
{
    cancel.check();
    if (timeoutMs <= 0 || outputLimit <= 0 || input.size() > receiptLimit)
        fail("HELPER_BUDGET_INVALID", "Invalid helper input or budget");
    QProcess child;
    child.setProgram(program);
    child.setArguments(arguments);
    child.setProcessEnvironment(environment);
    child.setProcessChannelMode(QProcess::SeparateChannels);
    child.setCreateProcessArgumentsModifier([](QProcess::CreateProcessArguments *args) {
        args->flags |= CREATE_NO_WINDOW;
    });
    HelperResult result;
    QElapsedTimer timer;
    timer.start();
    const auto stop = [&child] {
        if (child.state() != QProcess::NotRunning) {
            child.kill(); // TerminateProcess on this owned direct child only.
            (void)child.waitForFinished(3000);
        }
    };
    const auto abort = [&](const QString &code, const QString &message) -> void {
        stop();
        if (activationSubmission && result.submitted)
            fail("APPX_ACTIVATION_OUTCOME_UNKNOWN", "Activation request was submitted; refresh Desktop state before retrying. " + message);
        fail(code, message);
    };
    const auto check = [&] {
        if (cancel.cancelled()) abort("LAUNCH_CANCELLED", "Operation cancelled");
        if (timer.elapsed() >= timeoutMs) abort("HELPER_TIMEOUT", "Owned helper exceeded its time budget");
    };
    const auto drain = [&] {
        const auto out = child.readAllStandardOutput();
        const auto err = child.readAllStandardError();
        if (out.size() > outputLimit - result.out.size() || err.size() > outputLimit - result.err.size())
            abort("HELPER_OUTPUT_LIMIT", "Owned helper exceeded its output budget");
        result.out += out;
        result.err += err;
    };
    child.start();
    while (child.state() == QProcess::Starting) {
        check();
        (void)child.waitForStarted(20);
    }
    if (child.state() == QProcess::NotRunning) abort("HELPER_START_FAILED", "Cannot start owned helper");
    check();
    if (!input.isEmpty()) {
        const qint64 accepted = child.write(input);
        if (accepted == input.size()) result.submitted = true;
        if (accepted != input.size()) abort("HELPER_IO_FAILED", "Cannot deliver complete helper request");
        while (child.bytesToWrite() > 0 && child.state() != QProcess::NotRunning) {
            check();
            (void)child.waitForBytesWritten(20);
            drain();
        }
    }
    child.closeWriteChannel();
    while (child.state() != QProcess::NotRunning) {
        check();
        (void)child.waitForReadyRead(20);
        drain();
    }
    drain();
    check();
    result.exitCode = child.exitStatus() == QProcess::NormalExit ? child.exitCode() : -1;
    return result;
}

Desktop parseDiscovery(const QByteArray &json, const QString &overrideExecutable)
{
    const auto envelope = parseJsonObject(json, 64 * 1024, discoveryError);
    if (!envelope.value("schema_version").isDouble() || envelope.value("schema_version").toDouble() != 1)
        fail("APPX_DISCOVERY_PROTOCOL_UNSUPPORTED", "Unsupported APPX discovery schema");
    const auto recordsValue = envelope.value("records");
    if (!recordsValue.isArray()) fail(discoveryError, "Discovery records must be an array");
    const auto records = recordsValue.toArray();
    if (records.size() > 16) fail(discoveryError, "Too many package records");
    for (const auto &value : records) {
        if (!value.isObject()) fail(discoveryError, "Package record must be an object");
        validateRecord(value.toObject());
    }
    if (!overrideExecutable.isEmpty()) {
        const auto executable = canonicalFile(overrideExecutable);
        for (const auto &value : records) {
            const auto record = value.toObject();
            if (!supported(record.value("package_name").toString())) continue;
            const auto root = QFileInfo(record.value("install_location").toString()).canonicalFilePath();
            if (root.isEmpty() || !within(root, executable)) continue;
            auto result = registeredDesktop(record);
            if (normalizedWindowsPath(executable) != normalizedWindowsPath(result.executable))
                fail("APPX_OVERRIDE_INVALID", "Override is inside a package but is not its registered Desktop entry");
            result.discoverySource = "executable_override";
            return result;
        }
        // A package path can never become a bare executable by omitting discovery metadata.
        if (normalizedWindowsPath(executable).contains("/windowsapps/"))
            fail("APPX_OVERRIDE_INVALID", "Package executable requires registered discovery metadata");
        Desktop result;
        result.product = "executable_override";
        result.displayName = "Desktop executable override";
        result.packageName = "executable_override";
        result.packageVersion = "manual";
        result.architecture = "manual";
        result.discoverySource = "executable_override";
        result.executable = executable;
        result.installLocation = QFileInfo(executable).absolutePath();
        return result;
    }
    for (const auto &name : {"OpenAI.Codex", "OpenAI.ChatGPT-Desktop"}) {
        for (const auto &value : records) {
            const auto record = value.toObject();
            if (record.value("package_name").toString() == name) return registeredDesktop(record);
        }
    }
    fail("CODEX_NOT_INSTALLED", "No supported Desktop package is installed");
}
Desktop discoverDesktop(const Config &config, const Cancellation &cancel)
{
    cancel.check();
    QFile resource(":/resources/appx-discovery.ps1");
    if (!resource.open(QIODevice::ReadOnly)) fail("APPX_DISCOVERY_FAILED", "Discovery resource is unavailable");
    const auto script = resource.read(64 * 1024 + 1);
    if (script.size() > 64 * 1024 || !strictUtf8(script)) fail("APPX_DISCOVERY_FAILED", "Invalid discovery resource");
    std::array<wchar_t, MAX_PATH + 1> buffer{};
    const UINT size = GetSystemDirectoryW(buffer.data(), static_cast<UINT>(buffer.size()));
    if (!size || size >= buffer.size()) fail("APPX_DISCOVERY_FAILED", "Cannot resolve system PowerShell");
    const QString powershell = QString::fromWCharArray(buffer.data(), static_cast<int>(size))
        + "/WindowsPowerShell/v1.0/powershell.exe";
    const auto result = runHelper(powershell, {"-NoLogo", "-NoProfile", "-NonInteractive", "-Command", QString::fromUtf8(script)},
                                  {}, cancel, 15000, 128 * 1024);
    if (result.out.size() > 64 * 1024) fail("APPX_DISCOVERY_OUTPUT_LIMIT", "Discovery output exceeded 64 KiB");
    if (result.exitCode != 0) fail("APPX_DISCOVERY_FAILED", "System PowerShell discovery failed");
    return parseDiscovery(result.out, config.executableOverride);
}
ProcessState desktopProcessState(const Desktop &desktop)
{
    const Handle snapshot(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0));
    if (!snapshot.valid()) return {};
    PROCESSENTRY32W entry{};
    entry.dwSize = sizeof(entry);
    if (!Process32FirstW(snapshot.get(), &entry)) return {};
    struct Candidate { DWORD pid; DWORD parent; QString path; };
    std::vector<Candidate> candidates;
    bool unknown = false;
    const auto targetName = QFileInfo(desktop.executable).fileName();
    do {
        if (QString::fromWCharArray(entry.szExeFile).compare(targetName, Qt::CaseInsensitive) != 0) continue;
        const Handle process(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, FALSE, entry.th32ProcessID));
        if (!process.valid()) { unknown = true; continue; }
        if (WaitForSingleObject(process.get(), 0) == WAIT_OBJECT_0) continue;
        const auto path = imagePath(process.get());
        if (path.isEmpty()) { unknown = true; continue; }
        if (normalizedWindowsPath(path) == normalizedWindowsPath(desktop.executable))
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
    return {unknown || !candidates.empty() ? "unknown" : "stopped", 0};
}

namespace detail {
QJsonObject activationRequest(const QByteArray &bytes)
{
    auto request = parseJsonObject(bytes, requestLimit, activationError);
    exactKeys(request, {"version", "aumid", "expected_package_full_name", "expected_executable", "arguments"}, activationError);
    if (request.value("version").toDouble(-1) != 1) fail(activationError, "Unsupported activation protocol version");
    const auto aumid = field(request, "aumid", activationError);
    const auto package = field(request, "expected_package_full_name", activationError);
    const auto executable = field(request, "expected_executable", activationError);
    const auto arguments = field(request, "arguments", activationError);
    static const QRegularExpression identity("^[!-~]+$");
    if (aumid.size() > 2048 || !identity.match(aumid).hasMatch() || aumid.count('!') != 1
        || aumid.startsWith('!') || aumid.endsWith('!')) fail(activationError, "Invalid application identity");
    if (package.size() > 1024 || !identity.match(package).hasMatch()) fail(activationError, "Invalid package identity");
    if (executable.isEmpty() || !QFileInfo(executable).isAbsolute()) fail(activationError, "Expected executable must be absolute");
    if (arguments.size() > 4096) fail(activationError, "Activation arguments exceed 4 KiB");
    for (const auto c : arguments)
        if (c.unicode() < 0x20 || c.unicode() > 0x7e) fail(activationError, "Activation arguments must be printable ASCII");
    return request;
}
QJsonObject activationReceipt(const QByteArray &bytes)
{
    const auto receipt = parseJsonObject(bytes, receiptLimit, activationError);
    const auto shape = emptyReceipt();
    exactKeys(receipt, shape.keys(), activationError);
    if (receipt.keys() != shape.keys() || receipt.value("version").toDouble(-1) != 1
        || !receipt.value("activation_returned").isBool()) fail(activationError, "Invalid activation receipt shape");
    for (const auto &key : {"failure", "activation_hresult", "observed_package_full_name", "observed_aumid", "observation_failure"})
        (void)field(receipt, key, activationError, false, true);
    for (const auto &key : {"pid", "early_exit_code"}) {
        const auto value = receipt.value(key);
        if (!value.isNull() && !unsignedNumber(value, QString(key) == "early_exit_code"))
            fail(activationError, "Invalid process identifier or exit code");
    }
    if (!receipt.value("elevation").isNull() && !receipt.value("elevation").isBool())
        fail(activationError, "Invalid target elevation");
    if (!QStringList{"unknown", "created", "reused"}.contains(receipt.value("instance").toString()))
        fail(activationError, "Invalid instance observation");
    for (const auto &key : {"package_identity", "aumid"})
        if (!QStringList{"not_queried", "matched", "missing", "mismatch", "query_failed"}.contains(receipt.value(key).toString()))
            fail(activationError, "Invalid identity observation");
    const bool returned = receipt.value("activation_returned").toBool();
    if (returned != !receipt.value("pid").isNull()
        || (returned && (!receipt.value("failure").isNull() || !receipt.value("activation_hresult").isNull())))
        fail(activationError, "Contradictory activation facts");
    return receipt;
}
}
QJsonObject activateDesktop(const Desktop &desktop, const QString &arguments, const Cancellation &cancel)
{
    if (!desktop.registered) fail(activationError, "Application activation requires a registered target");
    requireNonElevated();
    const QJsonObject request{{"version", 1}, {"aumid", desktop.aumid},
        {"expected_package_full_name", desktop.packageFullName}, {"expected_executable", desktop.executable},
        {"arguments", arguments}};
    const auto bytes = QJsonDocument(request).toJson(QJsonDocument::Compact);
    (void)detail::activationRequest(bytes);
    const auto result = runHelper(QCoreApplication::applicationFilePath(), {"internal-activate-package"},
                                  bytes, cancel, 30000, receiptLimit, QProcessEnvironment::systemEnvironment(), true);
    QJsonObject receipt;
    try {
        if (result.exitCode != 0) fail(activationError, "Activation worker exited without a successful receipt");
        receipt = detail::activationReceipt(result.out);
    } catch (const Error &) {
        fail("APPX_ACTIVATION_OUTCOME_UNKNOWN", "Activation request was submitted but the receipt is unavailable or invalid; refresh Desktop state");
    }
    if (!receipt.value("failure").isNull()) {
        const auto message = receipt.value("failure").toString();
        fail("APPX_ACTIVATION_WORKER_FAILED", publicText(message));
    }
    return receipt;
}
int activationWorker()
{
    try {
        const HANDLE input = GetStdHandle(STD_INPUT_HANDLE);
        const auto type = GetFileType(input);
        if (type != FILE_TYPE_PIPE && type != FILE_TYPE_DISK) fail(activationError, "Worker requires bounded redirected input");
        QElapsedTimer timer;
        timer.start();
        QByteArray bytes;
        std::array<char, 4096> buffer{};
        for (;;) {
            if (timer.elapsed() >= 30000) fail(activationError, "Worker request read timed out");
            DWORD available = static_cast<DWORD>(buffer.size());
            if (type == FILE_TYPE_PIPE) {
                if (!PeekNamedPipe(input, nullptr, 0, nullptr, &available, nullptr)) {
                    if (GetLastError() == ERROR_BROKEN_PIPE) break;
                    fail(activationError, "Cannot read worker input");
                }
                if (!available) { QThread::msleep(10); continue; }
            }
            DWORD count = 0;
            if (!ReadFile(input, buffer.data(), std::min(available, static_cast<DWORD>(buffer.size())), &count, nullptr)) {
                if (GetLastError() == ERROR_BROKEN_PIPE) break;
                fail(activationError, "Cannot read worker input");
            }
            if (!count) break;
            bytes.append(buffer.data(), static_cast<qsizetype>(count));
            if (bytes.size() > requestLimit) fail(activationError, "Worker request exceeds 16 KiB");
        }
        const auto request = detail::activationRequest(bytes);
        requireNonElevated();
        const auto receipt = activateAndObserve(request);
        const auto output = QJsonDocument(receipt).toJson(QJsonDocument::Compact) + '\n';
        DWORD written = 0;
        if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), output.constData(), static_cast<DWORD>(output.size()), &written, nullptr)
            || written != static_cast<DWORD>(output.size())) return 1;
        return 0;
    } catch (const Error &error) {
        const auto output = (error.code + ": " + publicText(error.message) + '\n').toUtf8();
        DWORD written = 0;
        (void)WriteFile(GetStdHandle(STD_ERROR_HANDLE), output.constData(), static_cast<DWORD>(output.size()), &written, nullptr);
        return 1;
    }
}
}

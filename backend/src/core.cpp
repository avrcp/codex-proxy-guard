#include "core.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <shlobj.h>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonParseError>
#include <QRegularExpression>
#include <QSet>
#include <QStringDecoder>
#include <QUuid>
#include <algorithm>
#include <cstring>
#include <limits>
#include <optional>
#include <sstream>
#include <utility>
#include <vector>

#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "../third_party/toml++/toml.hpp"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

namespace cpg {
namespace {
constexpr qsizetype configLimit = 64 * 1024;
constexpr qsizetype envLimit = 1024 * 1024;
const QByteArray blockBegin = "# BEGIN CODEX PROXY GUARD: proxy-v1";
const QByteArray blockEnd = "# END CODEX PROXY GUARD: proxy-v1";

[[noreturn]] void fail(const QString &code, const QString &message) { throw Error(code, message); }
const wchar_t *wide(const QString &s) { return reinterpret_cast<const wchar_t *>(s.utf16()); }
struct Handle {
    HANDLE value = INVALID_HANDLE_VALUE;
    explicit Handle(HANDLE handle = INVALID_HANDLE_VALUE) : value(handle) {}
    ~Handle() { if (value != INVALID_HANDLE_VALUE) CloseHandle(value); }
    Handle(const Handle &) = delete;
    Handle &operator=(const Handle &) = delete;
};

QByteArray readHandle(HANDLE handle, qsizetype limit, const QString &code)
{
    LARGE_INTEGER size{};
    if (!GetFileSizeEx(handle, &size) || size.QuadPart < 0 || size.QuadPart > limit)
        fail(code, "File is unreadable or exceeds its size limit.");
    LARGE_INTEGER zero{};
    if (!SetFilePointerEx(handle, zero, nullptr, FILE_BEGIN)) fail(code, "Cannot seek the file.");
    QByteArray bytes(static_cast<qsizetype>(size.QuadPart), Qt::Uninitialized);
    DWORD got = 0;
    if (!bytes.isEmpty() && (!ReadFile(handle, bytes.data(), static_cast<DWORD>(bytes.size()), &got, nullptr)
                            || got != static_cast<DWORD>(bytes.size())))
        fail(code, "Cannot read the file completely.");
    return bytes;
}

bool writeHandle(HANDLE handle, const QByteArray &bytes)
{
    LARGE_INTEGER zero{};
    DWORD wrote = 0;
    return SetFilePointerEx(handle, zero, nullptr, FILE_BEGIN)
        && (bytes.isEmpty() || (WriteFile(handle, bytes.constData(), static_cast<DWORD>(bytes.size()), &wrote, nullptr)
                               && wrote == static_cast<DWORD>(bytes.size())))
        && SetEndOfFile(handle) && FlushFileBuffers(handle);
}

void ensureParent(const QString &path, const QString &code)
{
    if (!QDir().mkpath(QFileInfo(path).absolutePath())) fail(code, "Cannot create the destination directory.");
}

std::optional<Config> tryConfig(const QByteArray &bytes)
{
    try { return Config::parse(bytes); } catch (const Error &) { return std::nullopt; }
}

class Transaction {
public:
    Handle handle;
    QByteArray original;
    Transaction(const QString &path, const Config *expected, bool invalid, bool create = false)
    {
        if (create || invalid) ensureParent(path, "CONFIG_LOCK_FAILED");
        handle.value = CreateFileW(wide(path), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ,
                                   nullptr, create ? CREATE_NEW : OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
        bool created = create;
        if (handle.value == INVALID_HANDLE_VALUE && !create && invalid
            && (GetLastError() == ERROR_FILE_NOT_FOUND || GetLastError() == ERROR_PATH_NOT_FOUND)) {
            handle.value = CreateFileW(wide(path), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ,
                                       nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
            created = handle.value != INVALID_HANDLE_VALUE;
        }
        if (handle.value == INVALID_HANDLE_VALUE)
            fail("CONFIG_LOCK_FAILED", "Configuration is busy, missing, or not writable.");
        original = readHandle(handle.value, configLimit, "CONFIG_READ_FAILED");
        const auto current = tryConfig(original);
        if (!created && ((invalid && current) || (expected && (!current || *current != *expected))))
            fail("CONFIG_CHANGED", "Refresh and confirm the configuration again.");
    }
    void commit(const Config &config)
    {
        const QByteArray bytes = config.toml();
        if (bytes.size() > configLimit) fail("CONFIG_TOO_LARGE", "Configuration exceeds 64 KiB.");
        if (!writeHandle(handle.value, bytes)) {
            if (!writeHandle(handle.value, original))
                fail("CONFIG_ROLLBACK_FAILED", "The device rejected both the write and restoration; inspect the Guard configuration.");
            fail("CONFIG_WRITE_FAILED", "The original configuration was restored.");
        }
    }
};

QString knownFolder(REFKNOWNFOLDERID id)
{
    PWSTR result = nullptr;
    if (FAILED(SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY, nullptr, &result)))
        fail("CONFIG_PATH_UNAVAILABLE", "Cannot resolve the user directory.");
    const QString path = QString::fromWCharArray(result);
    CoTaskMemFree(result);
    return path;
}

bool absoluteHome(const QString &home)
{
    // Qt treats drive-relative and root-relative Windows paths differently from
    // fully qualified paths. Require a drive root or a server/share explicitly.
    if (home.isEmpty() || home.size() >= 4096 || home.contains(QChar::Null)
        || home.contains('\r') || home.contains('\n')) return false;
    QString normalized = QDir::fromNativeSeparators(home);
    // Rust canonicalize() persisted extended drive/UNC paths. Accept those
    // existing bindings, while refusing arbitrary Windows device namespaces.
    if (normalized.startsWith("//?/UNC/", Qt::CaseInsensitive)) normalized = "//" + normalized.mid(8);
    else if (normalized.startsWith("//?/")) normalized = normalized.mid(4);
    if (normalized.startsWith("//./")) return false;
    static const QRegularExpression drive("^[A-Za-z]:/");
    static const QRegularExpression unc("^//[^/]+/[^/]+(?:/|$)");
    return drive.match(normalized).hasMatch() || unc.match(normalized).hasMatch();
}

QString boundHome(const Config &config)
{
    if (!config.manageBackend || !absoluteHome(config.home))
        fail("BACKEND_PROXY_SCOPE_UNCONFIRMED", "An absolute Codex Home must be explicitly authorized first.");
    return config.home;
}

void checkKeys(const toml::table &table, std::initializer_list<std::string_view> keys)
{
    for (const auto &[key, value] : table) {
        Q_UNUSED(value);
        if (std::find(keys.begin(), keys.end(), key.str()) == keys.end())
            fail("CONFIG_INVALID", "Configuration contains an unknown field.");
    }
}
const toml::table *subtable(const toml::table &table, std::string_view key)
{
    const auto *node = table.get(key);
    if (!node) return nullptr;
    if (!node->is_table()) fail("CONFIG_INVALID", "Configuration section has the wrong type.");
    return node->as_table();
}
void stringField(const toml::table &table, std::string_view key, QString &value)
{
    if (const auto *node = table.get(key)) {
        if (!node->is_string()) fail("CONFIG_INVALID", "Configuration string has the wrong type.");
        value = QString::fromStdString(node->as_string()->get());
    }
}
void boolField(const toml::table &table, std::string_view key, bool &value)
{
    if (const auto *node = table.get(key)) {
        if (!node->is_boolean()) fail("CONFIG_INVALID", "Configuration boolean has the wrong type.");
        value = node->as_boolean()->get();
    }
}
void intField(const toml::table &table, std::string_view key, int &value)
{
    if (const auto *node = table.get(key)) {
        if (!node->is_integer()) fail("CONFIG_INVALID", "Configuration integer has the wrong type.");
        const auto number = node->as_integer()->get();
        if (number < std::numeric_limits<int>::min() || number > std::numeric_limits<int>::max())
            fail("CONFIG_INVALID", "Configuration integer is out of range.");
        value = static_cast<int>(number);
    }
}

struct EnvFile {
    bool exists = false;
    QByteArray bytes;
    qsizetype begin = -1;
    qsizetype end = -1;
    QMap<QByteArray, QByteArray> values;
    bool conflict = false;
};

EnvFile readEnv(const QString &path)
{
    EnvFile env;
    Handle file(CreateFileW(wide(path), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                            nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (file.value == INVALID_HANDLE_VALUE) {
        if (GetLastError() == ERROR_FILE_NOT_FOUND || GetLastError() == ERROR_PATH_NOT_FOUND) return env;
        fail("BACKEND_PROXY_ENV_IO", "Cannot read the authorized Home proxy file.");
    }
    env.exists = true;
    env.bytes = readHandle(file.value, envLimit, "BACKEND_PROXY_ENV_IO");
    if (!strictUtf8(env.bytes)) fail("BACKEND_PROXY_ENV_ENCODING", "The proxy file is not valid UTF-8.");
    bool inside = false;
    qsizetype offset = 0;
    while (offset < env.bytes.size()) {
        const auto newline = env.bytes.indexOf('\n', offset);
        const auto stop = newline < 0 ? env.bytes.size() : newline;
        QByteArray line = env.bytes.mid(offset, stop - offset);
        if (line.endsWith('\r')) line.chop(1);
        const bool bom = offset == 0 && line.startsWith("\xef\xbb\xbf");
        if (bom) line.remove(0, 3);
        const auto next = newline < 0 ? env.bytes.size() : newline + 1;
        if (line == blockBegin) {
            if (env.begin >= 0 || env.end >= 0) fail("BACKEND_PROXY_BLOCK_INVALID", "Duplicate or misordered Guard block.");
            env.begin = offset + (bom ? 3 : 0);
            inside = true;
        } else if (line == blockEnd) {
            if (!inside || env.end >= 0) fail("BACKEND_PROXY_BLOCK_INVALID", "Duplicate or misordered Guard block end.");
            env.end = next;
            inside = false;
        } else if (line.startsWith("# BEGIN CODEX PROXY GUARD:") || line.startsWith("# END CODEX PROXY GUARD:")) {
            fail("BACKEND_PROXY_BLOCK_INVALID", "Unknown Guard block version.");
        } else if (inside) {
            const auto equal = line.indexOf('=');
            const auto key = line.left(equal);
            if (equal < 0 || (key != "HTTP_PROXY" && key != "HTTPS_PROXY" && key != "NO_PROXY")
                || env.values.contains(key))
                fail("BACKEND_PROXY_BLOCK_INVALID", "The Guard block has been edited or contains duplicate keys.");
            env.values.insert(key, line.mid(equal + 1));
        } else {
            QString text = QString::fromUtf8(line).trimmed();
            if (!text.startsWith('#')) {
                const auto equal = text.indexOf('=');
                if (equal >= 0) {
                    QString key = text.left(equal).trimmed();
                    if (key.startsWith("export") && key.size() > 6 && key.at(6).isSpace()) key = key.mid(6).trimmed();
                    key = key.toUpper();
                    if (key == "HTTP_PROXY" || key == "HTTPS_PROXY" || key == "NO_PROXY" || key == "ALL_PROXY") env.conflict = true;
                }
            }
        }
        offset = next;
    }
    if (inside || ((env.begin >= 0) != (env.end >= 0)) || (env.begin >= 0 && env.values.size() != 3))
        fail("BACKEND_PROXY_BLOCK_INVALID", "The Guard block is truncated or incomplete.");
    return env;
}

bool currentValues(const EnvFile &env, const Config &config)
{
    return env.begin >= 0 && env.values.value("HTTP_PROXY") == config.proxyUrl().toUtf8()
        && env.values.value("HTTPS_PROXY") == config.proxyUrl().toUtf8()
        && env.values.value("NO_PROXY") == config.noProxyValue().toUtf8();
}

void atomicEnv(const QString &path, const EnvFile &expected, const QByteArray &updated)
{
    if (updated.size() > envLimit) fail("BACKEND_PROXY_ENV_IO", "The updated proxy file exceeds 1 MiB.");
    ensureParent(path, "BACKEND_PROXY_ENV_IO");
    const QString temp = QFileInfo(path).absolutePath() + "/.cpg-proxy-env-" + QUuid::createUuid().toString(QUuid::WithoutBraces) + ".tmp";
    struct TempCleanup { QString path; ~TempCleanup() { DeleteFileW(wide(path)); } } cleanup{temp};
    Handle staged(CreateFileW(wide(temp), GENERIC_WRITE | DELETE, 0, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (staged.value == INVALID_HANDLE_VALUE || !writeHandle(staged.value, updated))
        fail("BACKEND_PROXY_ENV_IO", "Cannot write the temporary proxy file.");
    // Hold write exclusion during verification and the atomic replacement. A
    // concurrently opened writer makes verification fail instead of being lost.
    Handle original(CreateFileW(wide(path), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_DELETE,
                                nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    const bool exists = original.value != INVALID_HANDLE_VALUE;
    if (!exists && GetLastError() != ERROR_FILE_NOT_FOUND && GetLastError() != ERROR_PATH_NOT_FOUND)
        fail("BACKEND_PROXY_ENV_IO", "Cannot re-verify the original proxy file.");
    if (exists != expected.exists || (exists && readHandle(original.value, envLimit, "BACKEND_PROXY_ENV_IO") != expected.bytes))
        fail("BACKEND_PROXY_CONFIG_CONFLICT", "The proxy file changed; no changes were applied.");
    // The extended POSIX rename permits replacing our still-open verified
    // reader. Keep write exclusion through the one atomic filesystem operation;
    // unsupported filesystems fail closed. Do not use ReplaceFileW: documented
    // failure modes can remove the original name before returning an error.
    const QString target = QDir::toNativeSeparators(QFileInfo(path).absoluteFilePath());
    const auto nameBytes = static_cast<size_t>(target.size()) * sizeof(wchar_t);
    const auto bufferBytes = sizeof(FILE_RENAME_INFO) + nameBytes;
    std::vector<quint64> storage((bufferBytes + sizeof(quint64) - 1) / sizeof(quint64));
    auto *rename = reinterpret_cast<FILE_RENAME_INFO *>(storage.data());
    rename->Flags = exists ? FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS : 0;
    rename->FileNameLength = static_cast<DWORD>(nameBytes);
    std::memcpy(rename->FileName, target.utf16(), nameBytes);
    if (!SetFileInformationByHandle(staged.value, FileRenameInfoEx, rename, static_cast<DWORD>(bufferBytes)))
        fail("BACKEND_PROXY_ENV_IO", "Cannot atomically replace the proxy file (Windows error " + QString::number(GetLastError()) + ").");
}

// QJsonDocument validates syntax but deliberately coalesces duplicate keys.
// Walk its already-validated source to reject duplicates at every nesting level,
// comparing decoded keys (including escaped spellings) before they are lost.
class JsonKeys {
    const QByteArray &bytes;
    qsizetype position = 0;
    QString code;
    void white() { while (position < bytes.size() && QByteArray(" \t\r\n").contains(bytes.at(position))) ++position; }
    QByteArray stringToken()
    {
        const auto start = position++;
        while (position < bytes.size()) {
            const char ch = bytes.at(position++);
            if (ch == '\\') ++position;
            else if (ch == '"') return bytes.mid(start, position - start);
        }
        fail(code, "Invalid JSON string.");
    }
    void value(int depth)
    {
        if (depth > 64) fail(code, "JSON nesting exceeds its limit.");
        white();
        if (position >= bytes.size()) fail(code, "Incomplete JSON value.");
        const char ch = bytes.at(position);
        if (ch == '{') {
            ++position; white(); QSet<QString> keys;
            while (position < bytes.size() && bytes.at(position) != '}') {
                const QByteArray token = stringToken();
                const auto key = QJsonDocument::fromJson("[" + token + "]").array().at(0).toString();
                if (keys.contains(key)) fail(code, "Duplicate JSON object key.");
                keys.insert(key); white(); ++position; value(depth + 1); white();
                if (bytes.at(position) != ',') break;
                ++position; white();
            }
            ++position;
        } else if (ch == '[') {
            ++position; white();
            while (position < bytes.size() && bytes.at(position) != ']') {
                value(depth + 1); white(); if (bytes.at(position) != ',') break; ++position; white();
            }
            ++position;
        } else if (ch == '"') { stringToken(); }
        else { while (position < bytes.size() && !QByteArray(" \t\r\n,]}").contains(bytes.at(position))) ++position; }
    }
public:
    JsonKeys(const QByteArray &input, QString errorCode) : bytes(input), code(std::move(errorCode)) {}
    void check() { if (bytes.startsWith("\xef\xbb\xbf")) position = 3; value(0); }
};
} // namespace

Error::Error(QString errorCode, QString text)
    : std::runtime_error(errorCode.toStdString()), code(std::move(errorCode)), message(publicText(text, 1024)) {}
QJsonObject Error::json() const { return {{"code", code}, {"message", message}}; }
void Cancellation::check() const { if (cancelled()) fail("LAUNCH_CANCELLED", "Operation cancelled."); }

bool strictUtf8(const QByteArray &bytes)
{
    QStringDecoder decoder(QStringDecoder::Utf8, QStringConverter::Flag::Stateless);
    const QString decoded = decoder.decode(bytes);
    Q_UNUSED(decoded);
    return !decoder.hasError();
}

QJsonObject parseJsonObject(const QByteArray &bytes, qsizetype limit, const QString &errorCode)
{
    if (bytes.size() > limit || !strictUtf8(bytes)) fail(errorCode, "JSON input is oversized or not valid UTF-8.");
    QJsonParseError error{};
    const auto document = QJsonDocument::fromJson(bytes, &error);
    if (error.error != QJsonParseError::NoError || !document.isObject()) fail(errorCode, "Expected a valid JSON object.");
    JsonKeys(bytes, errorCode).check();
    return document.object();
}

QString publicText(const QString &input, int limit)
{
    QString text = input;
    const QString profile = qEnvironmentVariable("USERPROFILE");
    if (!profile.isEmpty()) {
        text.replace(profile, "%USERPROFILE%", Qt::CaseInsensitive);
        text.replace(QDir::fromNativeSeparators(profile), "%USERPROFILE%", Qt::CaseInsensitive);
    }
    static const QRegularExpression ansi("\\x1b(?:\\[[0-?]*[ -/]*[@-~]|\\][^\\x07]*(?:\\x07|\\x1b\\\\))");
    text.remove(ansi);
    static const QRegularExpression credentials("(://)[^\\s/@]+@", QRegularExpression::CaseInsensitiveOption);
    text.replace(credentials, "\\1[REDACTED]@");
    static const QRegularExpression secret("(?i)(proxy-authorization:|authorization:|set-cookie:|cookie:|token\\s*=|api[_-]?key\\s*=|authorization\\s*=|cookie\\s*=|password\\s*=|secret\\s*=)[^\\r\\n]*");
    text.replace(secret, "\\1[REDACTED]");
    QString safe;
    for (const auto ch : text) {
        if (ch.isSpace()) safe += ' ';
        else if (ch.category() != QChar::Other_Control && ch.category() != QChar::Other_Format
                 && ch.category() != QChar::Other_Surrogate) safe += ch;
    }
    safe = safe.simplified();
    return safe.left(std::max(0, limit));
}

void Config::validate() const
{
    if (version != 2) fail("CONFIG_INVALID", "Unsupported configuration version; expected version 2.");
    if (scheme.compare("http", Qt::CaseInsensitive) != 0) fail("CONFIG_INVALID", "Proxy scheme must be http.");
    if (port < 1 || port > 65535) fail("CONFIG_INVALID", "Proxy port must be between 1 and 65535.");
    if (host.isEmpty() || host != host.trimmed() || host.contains(QChar::Null)) fail("CONFIG_INVALID", "Invalid proxy host.");
    IN_ADDR ipv4{};
    IN6_ADDR ipv6{};
    bool loopback = host.compare("localhost", Qt::CaseInsensitive) == 0;
    if (InetPtonW(AF_INET, wide(host), &ipv4) == 1) loopback = reinterpret_cast<const unsigned char *>(&ipv4)[0] == 127;
    if (InetPtonW(AF_INET6, wide(host), &ipv6) == 1) {
        const auto *bytes = reinterpret_cast<const unsigned char *>(&ipv6);
        loopback = std::all_of(bytes, bytes + 15, [](unsigned char c) { return c == 0; }) && bytes[15] == 1;
    }
    if (!loopback) fail("CONFIG_INVALID", "Proxy host must be localhost or a loopback IP address.");
    if (noProxy.isEmpty() || noProxy.size() > 32 || noProxyValue().toUtf8().size() > 4096)
        fail("CONFIG_INVALID", "Proxy exclusions must contain 1 to 32 bounded entries.");
    for (const auto &entry : noProxy) {
        if (entry.isEmpty() || entry != entry.trimmed() || entry.toUtf8().size() > 255
            || entry.contains('\r') || entry.contains('\n') || entry.contains(QChar::Null))
            fail("CONFIG_INVALID", "Proxy exclusions contain an invalid entry.");
    }
    if (alternateScreen != "auto" && alternateScreen != "always" && alternateScreen != "never")
        fail("CONFIG_INVALID", "Invalid alternate-screen setting.");
}
QString Config::proxyUrl() const { return "http://" + (host.contains(':') ? "[" + host + "]" : host) + ":" + QString::number(port); }
QString Config::noProxyValue() const { return noProxy.join(','); }

Config Config::parse(const QByteArray &bytes)
{
    if (bytes.size() > configLimit || !strictUtf8(bytes)) fail("CONFIG_INVALID", "Configuration exceeds 64 KiB or is not valid UTF-8.");
    try {
        const auto table = toml::parse(std::string_view(bytes.constData(), static_cast<size_t>(bytes.size())));
        checkKeys(table, {"version", "proxy", "codex", "tui"});
        Config config;
        intField(table, "version", config.version);
        if (const auto *proxy = subtable(table, "proxy")) {
            checkKeys(*proxy, {"scheme", "host", "port", "no_proxy"});
            stringField(*proxy, "scheme", config.scheme); stringField(*proxy, "host", config.host); intField(*proxy, "port", config.port);
            if (const auto *node = proxy->get("no_proxy")) {
                if (!node->is_array()) fail("CONFIG_INVALID", "Proxy exclusions must be an array.");
                config.noProxy.clear();
                for (const auto &entry : *node->as_array()) {
                    if (!entry.is_string()) fail("CONFIG_INVALID", "Proxy exclusions must be strings.");
                    config.noProxy.append(QString::fromStdString(entry.as_string()->get()));
                }
            }
        }
        if (const auto *codex = subtable(table, "codex")) {
            checkKeys(*codex, {"executable_override", "cli_executable_override", "refuse_if_running", "manage_codex_proxy_env", "proxy_env_home"});
            stringField(*codex, "executable_override", config.executableOverride);
            stringField(*codex, "cli_executable_override", config.cliOverride);
            stringField(*codex, "proxy_env_home", config.home);
            boolField(*codex, "refuse_if_running", config.refuseIfRunning);
            boolField(*codex, "manage_codex_proxy_env", config.manageBackend);
        }
        if (const auto *tui = subtable(table, "tui")) {
            checkKeys(*tui, {"alternate_screen"}); stringField(*tui, "alternate_screen", config.alternateScreen);
        }
        config.validate(); return config;
    } catch (const toml::parse_error &) { fail("CONFIG_INVALID", "Configuration is not valid TOML."); }
}

QByteArray Config::toml() const
{
    validate();
    toml::array exclusions;
    for (const auto &entry : noProxy) exclusions.push_back(entry.toStdString());
    toml::table table{{"version", version},
        {"proxy", toml::table{{"scheme", scheme.toStdString()}, {"host", host.toStdString()}, {"port", port}, {"no_proxy", std::move(exclusions)}}},
        {"codex", toml::table{{"executable_override", executableOverride.toStdString()}, {"cli_executable_override", cliOverride.toStdString()},
             {"refuse_if_running", refuseIfRunning}, {"manage_codex_proxy_env", manageBackend}, {"proxy_env_home", home.toStdString()}}},
        {"tui", toml::table{{"alternate_screen", alternateScreen.toStdString()}}}};
    std::ostringstream output; output << table;
    return QByteArray::fromStdString(output.str()) + '\n';
}
QString Config::defaultPath() { return QDir(knownFolder(FOLDERID_RoamingAppData)).filePath("codex-proxy-guard/config.toml"); }
QString Config::defaultHome() { return QDir(knownFolder(FOLDERID_Profile)).filePath(".codex"); }
Config Config::load(const QString &path)
{
    Handle file(CreateFileW(wide(path), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                            nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (file.value == INVALID_HANDLE_VALUE) fail("CONFIG_READ_FAILED", "Cannot read the Guard configuration.");
    return parse(readHandle(file.value, configLimit, "CONFIG_READ_FAILED"));
}
Config Config::loadOrCreate(const QString &path)
{
    if (QFileInfo::exists(path)) return load(path);
    try { initializeConfig(path, Config{}, false); }
    catch (const Error &) { if (QFileInfo::exists(path)) return load(path); throw; }
    return load(path);
}

struct ConfigLease::Impl { Handle handle; };
ConfigLease::ConfigLease(const QString &path, const Config &expected) : impl_(std::make_unique<Impl>())
{
    impl_->handle.value = CreateFileW(wide(path), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (impl_->handle.value == INVALID_HANDLE_VALUE) fail("CONFIG_LOCK_FAILED", "Configuration is changing or unavailable.");
    const auto current = tryConfig(readHandle(impl_->handle.value, configLimit, "CONFIG_READ_FAILED"));
    if (!current || *current != expected) fail("CONFIG_CHANGED", "Refresh before launching with the new configuration.");
}
ConfigLease::~ConfigLease() = default;

void initializeConfig(const QString &path, const Config &config, bool force)
{
    config.validate();
    if (!force || !QFileInfo::exists(path)) { Transaction(path, nullptr, false, true).commit(config); return; }
    // Force initialization still holds the same write/delete exclusion as every
    // editor and must never silently discard an existing authorization binding.
    Transaction transaction(path, nullptr, false);
    const auto current = tryConfig(transaction.original);
    if (current && current->manageBackend && (current->home != config.home || !config.manageBackend))
        fail("BACKEND_PROXY_REVOKE_REQUIRED", "Revoke the existing proxy authorization before resetting this configuration.");
    transaction.commit(config);
}

Config updateProxy(const QString &path, const Config &expected, bool invalid, const QString &host, int port)
{
    Transaction transaction(path, invalid ? nullptr : &expected, invalid);
    Config updated = expected; updated.host = host; updated.port = port;
    transaction.commit(updated); return updated;
}
QString consentHome(const Config &config)
{
    const QString home = config.home.isEmpty() ? Config::defaultHome() : config.home;
    if (!absoluteHome(home)) fail("BACKEND_PROXY_SCOPE_UNCONFIRMED", "The Codex Home must be an absolute path.");
    return home;
}
Config updateConsent(const QString &path, const Config &expected, bool enable, const QString &confirmedHome)
{
    Transaction transaction(path, &expected, false);
    if (!absoluteHome(confirmedHome) || consentHome(expected) != confirmedHome)
        fail("BACKEND_PROXY_SCOPE_UNCONFIRMED", "The confirmed Home no longer matches the configuration.");
    Config updated = expected;
    if (enable) {
        if (expected.manageBackend) fail("CONFIG_CHANGED", "Proxy management is already authorized; refresh before confirming.");
        const auto canonical = QFileInfo(confirmedHome).canonicalFilePath();
        updated.home = canonical.isEmpty() ? confirmedHome : canonical;
        updated.manageBackend = true;
    } else {
        if (!expected.manageBackend || expected.home != confirmedHome)
            fail("CONFIG_CHANGED", "Proxy management is no longer authorized for the confirmed Home.");
        try { revokeProxyEnv(confirmedHome); }
        catch (const Error &) { fail("BACKEND_PROXY_REVOKE_FAILED", "The managed block was not safely removed; consent and its bound Home remain unchanged."); }
        updated.manageBackend = false; updated.home.clear();
    }
    try { transaction.commit(updated); }
    catch (const Error &error) {
        if (!enable && error.code != "CONFIG_ROLLBACK_FAILED")
            fail("BACKEND_PROXY_CONSENT_SAVE_FAILED", "The block was removed but consent could not be saved; refresh and retry.");
        throw;
    }
    return updated;
}

QString inspectProxyEnv(const Config &config)
{
    if (!config.manageBackend) return "not_authorized";
    try {
        config.validate();
        const EnvFile env = readEnv(QDir(boundHome(config)).filePath(".env"));
        if (env.conflict) return "conflict";
        if (env.begin < 0) return "pending";
        return currentValues(env, config) ? "current" : "stale";
    } catch (const Error &error) {
        if (error.code == "BACKEND_PROXY_ENV_IO") return "unavailable";
        return "invalid";
    }
}
void prepareProxyEnv(const Config &config)
{
    config.validate();
    const QString path = QDir(boundHome(config)).filePath(".env");
    const EnvFile env = readEnv(path);
    if (env.conflict) fail("BACKEND_PROXY_CONFIG_CONFLICT", "Proxy keys already exist outside Guard's block; resolve them manually.");
    if (currentValues(env, config)) return;
    const auto url = config.proxyUrl().toUtf8();
    const QByteArray block = blockBegin + "\nHTTP_PROXY=" + url + "\nHTTPS_PROXY=" + url + "\nNO_PROXY=" + config.noProxyValue().toUtf8() + "\n" + blockEnd + "\n";
    QByteArray updated;
    if (env.begin >= 0) updated = env.bytes.left(env.begin) + block + env.bytes.mid(env.end);
    else updated = env.bytes + ((!env.bytes.isEmpty() && !env.bytes.endsWith('\n')) ? "\n" : "") + block;
    atomicEnv(path, env, updated);
}
void revokeProxyEnv(const QString &home)
{
    if (!absoluteHome(home)) fail("BACKEND_PROXY_SCOPE_UNCONFIRMED", "The Codex Home must be an absolute path.");
    const QString path = QDir(home).filePath(".env");
    const EnvFile env = readEnv(path);
    if (env.begin < 0) return;
    const QByteArray updated = env.bytes.left(env.begin) + env.bytes.mid(env.end);
    if (updated.isEmpty()) {
        // Delete through a held handle, excluding both edits and pathname
        // replacement, after re-verifying the exact bytes we intend to remove.
        Handle file(CreateFileW(wide(path), GENERIC_READ | DELETE, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
        if (file.value == INVALID_HANDLE_VALUE) fail("BACKEND_PROXY_ENV_IO", "Cannot lock the proxy file for removal.");
        if (readHandle(file.value, envLimit, "BACKEND_PROXY_ENV_IO") != env.bytes)
            fail("BACKEND_PROXY_CONFIG_CONFLICT", "The proxy file changed; no changes were applied.");
        FILE_DISPOSITION_INFO info{TRUE};
        if (!SetFileInformationByHandle(file.value, FileDispositionInfo, &info, sizeof(info)))
            fail("BACKEND_PROXY_ENV_IO", "Cannot remove the managed proxy file.");
    } else atomicEnv(path, env, updated);
}
} // namespace cpg

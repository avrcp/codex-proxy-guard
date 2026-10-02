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
#include <ktmw32.h>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonParseError>
#include <QRegularExpression>
#include <QSet>
#include <QStringDecoder>
#include <algorithm>
#include <functional>
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
// Private linked-test seam, never configured from requests or the environment.
thread_local std::function<void(bool)> envTransactionHook;

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

bool mayContainConsent(const QByteArray &bytes)
{
    // A broken configuration may be the only durable record of a managed Home.
    // Never erase that evidence during default-based editor recovery or reset.
    // Decode key escapes as well; malformed TOML cannot be trusted to have only
    // bare keys, and ambiguous/comment-only evidence deliberately fails closed.
    if (!strictUtf8(bytes)) return true;
    QString text = QString::fromUtf8(bytes);
    static const QRegularExpression escape("\\\\(?:u([0-9a-fA-F]{4})|U([0-9a-fA-F]{8}))");
    QString decoded;
    qsizetype copied = 0;
    auto matches = escape.globalMatch(text);
    while (matches.hasNext()) {
        const auto match = matches.next();
        decoded += text.mid(copied, match.capturedStart() - copied);
        const auto hex = match.captured(1).isEmpty() ? match.captured(2) : match.captured(1);
        const char32_t point = hex.toUInt(nullptr, 16);
        if (point <= 0x10ffff && !(point >= 0xd800 && point <= 0xdfff)) decoded += QString::fromUcs4(&point, 1);
        else decoded += match.captured();
        copied = match.capturedEnd();
    }
    decoded += text.mid(copied);
    return decoded.contains("manage_codex_proxy_env", Qt::CaseInsensitive)
        || decoded.contains("proxy_env_home", Qt::CaseInsensitive);
}

class Transaction {
public:
    Handle handle;
    QByteArray original;
    Transaction(const QString &path, const Config *expected, bool invalid, bool create = false)
    {
        if (create || invalid) ensureParent(path, "CONFIG_LOCK_FAILED");
        handle.value = CreateFileW(wide(QDir::toNativeSeparators(path)), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ,
                                   nullptr, create ? CREATE_NEW : OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
        bool created = create;
        if (handle.value == INVALID_HANDLE_VALUE && !create && invalid
            && (GetLastError() == ERROR_FILE_NOT_FOUND || GetLastError() == ERROR_PATH_NOT_FOUND)) {
            handle.value = CreateFileW(wide(QDir::toNativeSeparators(path)), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ,
                                       nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
            created = handle.value != INVALID_HANDLE_VALUE;
        }
        if (handle.value == INVALID_HANDLE_VALUE)
            fail("CONFIG_LOCK_FAILED", "Configuration is busy, missing, or not writable.");
        original = readHandle(handle.value, configLimit, "CONFIG_READ_FAILED");
        const auto current = tryConfig(original);
        if (!created && ((invalid && current) || (expected && (!current || *current != *expected))))
            fail("CONFIG_CHANGED", "Refresh and confirm the configuration again.");
        if (!created && !current && mayContainConsent(original))
            fail("CONFIG_CONSENT_REPAIR_REQUIRED", "The invalid configuration contains proxy authorization fields; repair it manually while preserving the bound Home before resetting or saving.");
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

bool generatedNoProxy(const QByteArray &bytes)
{
    if (bytes.isEmpty() || bytes.size() > 4096 || bytes.contains('\r') || bytes.contains('\n') || bytes.contains('\0')) return false;
    // Configuration entries can themselves contain commas. Find a partition
    // into at most 32 valid entries instead of falsely rejecting a block that
    // Guard itself generated from such entries.
    QList<qsizetype> boundaries{-1};
    for (qsizetype i = 0; i < bytes.size(); ++i) if (bytes.at(i) == ',') boundaries.append(i);
    boundaries.append(bytes.size());
    std::vector<int> fewest(static_cast<size_t>(boundaries.size()), 33);
    fewest[0] = 0;
    for (qsizetype i = 0; i < boundaries.size() - 1; ++i) {
        if (fewest[static_cast<size_t>(i)] >= 32) continue;
        for (qsizetype j = i + 1; j < boundaries.size(); ++j) {
            const auto length = boundaries[j] - boundaries[i] - 1;
            if (length > 255) break;
            if (length == 0) continue;
            const auto entry = QString::fromUtf8(bytes.mid(boundaries[i] + 1, length));
            if (entry != entry.trimmed()) continue;
            auto &next = fewest[static_cast<size_t>(j)];
            next = std::min(next, fewest[static_cast<size_t>(i)] + 1);
        }
    }
    return fewest.back() <= 32;
}

void validateManagedValues(const EnvFile &env)
{
    const auto http = env.values.value("HTTP_PROXY");
    if (http != env.values.value("HTTPS_PROXY") || !generatedNoProxy(env.values.value("NO_PROXY")))
        fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block contains values Guard could not have generated.");
    static const QRegularExpression endpoint("^http://(\\[[^\\]]+\\]|[^:/?#@\\s]+):([0-9]{1,5})$");
    const auto match = endpoint.match(QString::fromUtf8(http));
    if (!match.hasMatch()) fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint was edited.");
    Config generated;
    generated.host = match.captured(1);
    if (generated.host.startsWith('[')) generated.host = generated.host.mid(1, generated.host.size() - 2);
    generated.port = match.captured(2).toInt();
    try { generated.validate(); }
    catch (const Error &) { fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint is not a valid loopback proxy."); }
    if (generated.proxyUrl().toUtf8() != http)
        fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint was edited.");
}

EnvFile readEnv(const QString &path)
{
    EnvFile env;
    Handle file(CreateFileW(wide(QDir::toNativeSeparators(path)), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
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
    if (env.begin >= 0) validateManagedValues(env);
    return env;
}

bool currentValues(const EnvFile &env, const Config &config)
{
    return env.begin >= 0 && env.values.value("HTTP_PROXY") == config.proxyUrl().toUtf8()
        && env.values.value("HTTPS_PROXY") == config.proxyUrl().toUtf8()
        && env.values.value("NO_PROXY") == config.noProxyValue().toUtf8();
}

class EnvTransaction {
    Handle handle_;
    bool committed_ = false;
public:
    EnvTransaction() : handle_(CreateTransaction(nullptr, nullptr, 0, 0, 0, 5000, nullptr))
    {
        if (handle_.value == INVALID_HANDLE_VALUE)
            fail("BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE", "Windows file transactions are unavailable; the proxy file was not changed.");
    }
    ~EnvTransaction() { if (!committed_) RollbackTransaction(handle_.value); }
    HANDLE handle() const { return handle_.value; }
    void commit()
    {
        if (!CommitTransaction(handle_.value))
            fail("BACKEND_PROXY_ENV_IO", "The proxy file transaction could not commit; no partial edit was published.");
        committed_ = true;
    }
};

void atomicEnv(const QString &path, const EnvFile &expected, const QByteArray &updated, bool remove = false)
{
    if (updated.size() > envLimit) fail("BACKEND_PROXY_ENV_IO", "The updated proxy file exceeds 1 MiB.");
    ensureParent(path, "BACKEND_PROXY_ENV_IO");
    const QString target = QDir::toNativeSeparators(path);
    EnvTransaction transaction;
    {
        // A transacted writer locks both data and namespace identity, including
        // against non-transacted rename-and-save editors. The lock survives the
        // file-handle close until commit/rollback. A normal shared-delete handle
        // plus pathname rename cannot provide this compare-and-write guarantee.
        Handle file(CreateFileTransactedW(wide(target), GENERIC_READ | GENERIC_WRITE | DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
            expected.exists ? OPEN_EXISTING : CREATE_NEW, FILE_ATTRIBUTE_NORMAL,
            nullptr, transaction.handle(), nullptr, nullptr));
        if (file.value == INVALID_HANDLE_VALUE) {
            const DWORD error = GetLastError();
            if (error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND || error == ERROR_FILE_EXISTS || error == ERROR_ALREADY_EXISTS)
                fail("BACKEND_PROXY_CONFIG_CONFLICT", "The proxy file changed; no changes were applied.");
            if (error == ERROR_TRANSACTIONS_UNSUPPORTED_REMOTE || error == ERROR_NOT_SUPPORTED
                || error == ERROR_INVALID_FUNCTION || error == ERROR_EFS_NOT_ALLOWED_IN_TRANSACTION)
                fail("BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE", "This location does not support the required Windows file transactions; the proxy file was not changed.");
            fail("BACKEND_PROXY_ENV_IO", "Cannot lock the proxy file for an atomic transaction.");
        }
        if (readHandle(file.value, envLimit, "BACKEND_PROXY_ENV_IO") != expected.bytes)
            fail("BACKEND_PROXY_CONFIG_CONFLICT", "The proxy file changed; no changes were applied.");
        if (envTransactionHook) envTransactionHook(false);
        if (remove) {
            if (!DeleteFileTransactedW(wide(target), transaction.handle()))
                fail("BACKEND_PROXY_ENV_IO", "Cannot remove the managed proxy file transactionally.");
        } else if (!writeHandle(file.value, updated)) {
            fail("BACKEND_PROXY_ENV_IO", "Cannot stage the proxy file transaction.");
        }
        if (envTransactionHook) envTransactionHook(true);
    }
    transaction.commit();
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

namespace testing {
void setEnvTransactionHook(std::function<void(bool)> hook) { envTransactionHook = std::move(hook); }
}

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
    Handle file(CreateFileW(wide(QDir::toNativeSeparators(path)), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
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
    impl_->handle.value = CreateFileW(wide(QDir::toNativeSeparators(path)), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
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
        catch (const Error &error) {
            static const QSet<QString> reasons{"BACKEND_PROXY_ENV_IO", "BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE",
                "BACKEND_PROXY_CONFIG_CONFLICT", "BACKEND_PROXY_BLOCK_INVALID", "BACKEND_PROXY_ENV_ENCODING",
                "BACKEND_PROXY_SCOPE_UNCONFIRMED"};
            const QString reason = reasons.contains(error.code) ? error.code : "BACKEND_PROXY_ENV_FAILURE";
            fail("BACKEND_PROXY_REVOKE_FAILED", "The managed block was not safely removed; consent and its bound Home remain unchanged. Cause: " + reason + ".");
        }
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
    atomicEnv(path, env, updated, updated.isEmpty());
}
} // namespace cpg

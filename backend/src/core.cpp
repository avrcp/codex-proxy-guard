#include "core.h"
#include "cpg/path.h"
#include "cpg/port.h"
#include "cpg/win_handle.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <ktmw32.h>

#include <algorithm>
#include <cctype>
#include <cstdint>
#include <functional>
#include <limits>
#include <map>
#include <optional>
#include <set>
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
constexpr std::size_t configLimit = 64 * 1024;
constexpr std::size_t envLimit = 1024 * 1024;
const char *blockBegin = "# BEGIN CODEX PROXY GUARD: proxy-v1";
const char *blockEnd = "# END CODEX PROXY GUARD: proxy-v1";
// Private linked-test seam, never configured from requests or the environment.
thread_local std::function<void(bool)> envTransactionHook;

[[noreturn]] void fail(const std::string &code, const std::string &message)
{
    throw Error(code, message);
}

std::string asciiTrimmed(std::string_view value)
{
    std::size_t begin = 0, end = value.size();
    while (begin < end && std::isspace(static_cast<unsigned char>(value[begin]))) ++begin;
    while (end > begin && std::isspace(static_cast<unsigned char>(value[end - 1]))) --end;
    return std::string(value.substr(begin, end - begin));
}

std::string asciiFolded(std::string_view value)
{
    std::string folded(value);
    for (char &character : folded)
        character = static_cast<char>(std::tolower(static_cast<unsigned char>(character)));
    return folded;
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

bool startsWith(std::string_view text, std::string_view prefix)
{
    return text.size() >= prefix.size() && text.substr(0, prefix.size()) == prefix;
}

void appendUtf8CodePoint(std::string &out, char32_t point)
{
    if (point < 0x80) {
        out.push_back(static_cast<char>(point));
    } else if (point < 0x800) {
        out.push_back(static_cast<char>(0xc0 | (point >> 6)));
        out.push_back(static_cast<char>(0x80 | (point & 0x3f)));
    } else if (point < 0x10000) {
        out.push_back(static_cast<char>(0xe0 | (point >> 12)));
        out.push_back(static_cast<char>(0x80 | ((point >> 6) & 0x3f)));
        out.push_back(static_cast<char>(0x80 | (point & 0x3f)));
    } else {
        out.push_back(static_cast<char>(0xf0 | (point >> 18)));
        out.push_back(static_cast<char>(0x80 | ((point >> 12) & 0x3f)));
        out.push_back(static_cast<char>(0x80 | ((point >> 6) & 0x3f)));
        out.push_back(static_cast<char>(0x80 | (point & 0x3f)));
    }
}

bool containsInsensitive(std::string_view text, std::string_view needle)
{
    if (needle.empty() || text.size() < needle.size()) return false;
    const std::string haystack = asciiFolded(text);
    const std::string token = asciiFolded(needle);
    return haystack.find(token) != std::string::npos;
}

bool startsWithInsensitive(std::string_view text, std::string_view prefix)
{
    if (text.size() < prefix.size()) return false;
    for (std::size_t index = 0; index < prefix.size(); ++index)
        if (std::tolower(static_cast<unsigned char>(text[index]))
            != std::tolower(static_cast<unsigned char>(prefix[index])))
            return false;
    return true;
}

std::string readHandle(HANDLE handle, std::size_t limit, const std::string &code)
{
    LARGE_INTEGER size{};
    if (!GetFileSizeEx(handle, &size) || size.QuadPart < 0
        || static_cast<unsigned long long>(size.QuadPart) > limit)
        fail(code, "File is unreadable or exceeds its size limit.");
    LARGE_INTEGER zero{};
    if (!SetFilePointerEx(handle, zero, nullptr, FILE_BEGIN)) fail(code, "Cannot seek the file.");
    std::string bytes(static_cast<std::size_t>(size.QuadPart), '\0');
    DWORD got = 0;
    if (!bytes.empty() && (!ReadFile(handle, bytes.data(), static_cast<DWORD>(bytes.size()), &got, nullptr)
                           || got != static_cast<DWORD>(bytes.size())))
        fail(code, "Cannot read the file completely.");
    return bytes;
}

bool writeHandle(HANDLE handle, const std::string &bytes)
{
    LARGE_INTEGER zero{};
    DWORD wrote = 0;
    return SetFilePointerEx(handle, zero, nullptr, FILE_BEGIN)
        && (bytes.empty() || (WriteFile(handle, bytes.data(), static_cast<DWORD>(bytes.size()), &wrote, nullptr)
                              && wrote == static_cast<DWORD>(bytes.size())))
        && SetEndOfFile(handle) && FlushFileBuffers(handle);
}

void ensureParent(const std::filesystem::path &path, const std::string &code)
{
    std::error_code error;
    const std::filesystem::path parent = path.parent_path();
    if (!parent.empty() && !std::filesystem::create_directories(parent, error) && error)
        fail(code, "Cannot create the destination directory.");
}

std::optional<Config> tryConfig(const std::string &bytes)
{
    try { return Config::parse(bytes); } catch (const Error &) { return std::nullopt; }
}

std::string decodeTomlEscapes(std::string_view text)
{
    // Decode \uXXXX / \UXXXXXXXX escapes so key spellings are compared after
    // decoding, exactly like the previous implementation.
    std::string decoded;
    decoded.reserve(text.size());
    for (std::size_t index = 0; index < text.size(); ++index) {
        if (text[index] != '\\' || index + 1 >= text.size()) {
            decoded.push_back(text[index]);
            continue;
        }
        const char kind = text[++index];
        const std::size_t width = kind == 'u' ? 4 : kind == 'U' ? 8 : 0;
        if (width == 0 || index + width > text.size()) {
            decoded.push_back('\\');
            decoded.push_back(kind);
            continue;
        }
        std::uint32_t point = 0;
        bool valid = true;
        for (std::size_t step = 1; step <= width; ++step) {
            const char digit = text[index + step];
            int value = 0;
            if (digit >= '0' && digit <= '9') value = digit - '0';
            else if (digit >= 'a' && digit <= 'f') value = digit - 'a' + 10;
            else if (digit >= 'A' && digit <= 'F') value = digit - 'A' + 10;
            else { valid = false; break; }
            point = (point << 4) | static_cast<std::uint32_t>(value);
        }
        if (!valid || point > 0x10ffff || (point >= 0xd800 && point <= 0xdfff)) {
            decoded.push_back('\\');
            decoded.push_back(kind);
            continue;
        }
        appendUtf8CodePoint(decoded, point);
        index += width;
    }
    return decoded;
}

bool mayContainConsent(const std::string &bytes)
{
    // A broken configuration may be the only durable record of a managed Home.
    // Never erase that evidence during default-based editor recovery or reset.
    // Decode key escapes as well; malformed TOML cannot be trusted to have only
    // bare keys, and ambiguous/comment-only evidence deliberately fails closed.
    if (!strictUtf8(bytes)) return true;
    const std::string decoded = decodeTomlEscapes(bytes);
    return containsInsensitive(decoded, "manage_codex_proxy_env")
        || containsInsensitive(decoded, "proxy_env_home");
}

class Transaction {
public:
    UniqueHandle handle;
    std::string original;
    Transaction(const std::filesystem::path &path, const Config *expected, bool invalid, bool create = false)
    {
        if (create || invalid) ensureParent(path, "CONFIG_LOCK_FAILED");
        handle.reset(CreateFileW(path.c_str(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ,
                                 nullptr, create ? CREATE_NEW : OPEN_EXISTING,
                                 FILE_ATTRIBUTE_NORMAL, nullptr));
        bool created = create;
        if (!handle.valid() && !create && invalid
            && (GetLastError() == ERROR_FILE_NOT_FOUND || GetLastError() == ERROR_PATH_NOT_FOUND)) {
            handle.reset(CreateFileW(path.c_str(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ,
                                     nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
            created = handle.valid();
        }
        if (!handle.valid())
            fail("CONFIG_LOCK_FAILED", "Configuration is busy, missing, or not writable.");
        original = readHandle(handle.get(), configLimit, "CONFIG_READ_FAILED");
        const auto current = tryConfig(original);
        if (!created && ((invalid && current) || (expected && (!current || *current != *expected))))
            fail("CONFIG_CHANGED", "Refresh and confirm the configuration again.");
        if (!created && !current && mayContainConsent(original))
            fail("CONFIG_CONSENT_REPAIR_REQUIRED", "The invalid configuration contains proxy authorization fields; repair it manually while preserving the bound Home before resetting or saving.");
    }
    void commit(const Config &config)
    {
        const std::string bytes = config.toml();
        if (bytes.size() > configLimit) fail("CONFIG_TOO_LARGE", "Configuration exceeds 64 KiB.");
        if (!writeHandle(handle.get(), bytes)) {
            if (!writeHandle(handle.get(), original))
                fail("CONFIG_ROLLBACK_FAILED", "The device rejected both the write and restoration; inspect the Guard configuration.");
            fail("CONFIG_WRITE_FAILED", "The original configuration was restored.");
        }
    }
};

bool absoluteHome(const std::filesystem::path &home)
{
    // Require a drive root or a server/share explicitly. Drive-relative and
    // root-relative Windows paths are not accepted.
    const std::wstring wide = home.wstring();
    if (wide.empty() || wide.size() >= 4096) return false;
    for (const wchar_t character : wide)
        if (character == L'\0' || character == L'\r' || character == L'\n') return false;
    std::wstring normalized = wide;
    for (wchar_t &character : normalized) if (character == L'\\') character = L'/';
    // Rust canonicalize() persisted extended drive/UNC paths. Accept those
    // existing bindings, while refusing arbitrary Windows device namespaces.
    const auto starts = [&normalized](const wchar_t *prefix, std::size_t width) {
        if (normalized.size() < width) return false;
        for (std::size_t index = 0; index < width; ++index)
            if (towlower(normalized[index]) != towlower(prefix[index])) return false;
        return true;
    };
    if (starts(L"//?/unc/", 8)) normalized = L"//" + normalized.substr(8);
    else if (starts(L"//?/", 4)) normalized = normalized.substr(4);
    if (starts(L"//./", 4)) return false;
    const bool drive = normalized.size() >= 3
        && ((normalized[0] >= L'A' && normalized[0] <= L'Z')
            || (normalized[0] >= L'a' && normalized[0] <= L'z'))
        && normalized[1] == L':' && normalized[2] == L'/';
    if (drive) return true;
    if (!starts(L"//", 2)) return false;
    // //server/share... : two non-empty segments before any further content.
    std::size_t index = 2;
    std::size_t segments = 0;
    std::size_t segmentLength = 0;
    for (; index <= normalized.size(); ++index) {
        if (index == normalized.size() || normalized[index] == L'/') {
            if (segmentLength == 0) return false; // empty component
            ++segments;
            segmentLength = 0;
            if (segments == 2) return index == normalized.size() || normalized[index] == L'/';
            continue;
        }
        ++segmentLength;
    }
    return false;
}

std::filesystem::path boundHome(const Config &config)
{
    if (!config.manageBackend || !absoluteHome(config.home))
        fail("BACKEND_PROXY_SCOPE_UNCONFIRMED", "An absolute Codex Home must be explicitly authorized first.");
    return config.home;
}

void checkKeys(const toml::table &table, std::initializer_list<std::string_view> keys)
{
    for (const auto &[key, value] : table) {
        (void)value;
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
void stringField(const toml::table &table, std::string_view key, std::string &value)
{
    if (const auto *node = table.get(key)) {
        if (!node->is_string()) fail("CONFIG_INVALID", "Configuration string has the wrong type.");
        value = node->as_string()->get();
    }
}
void pathField(const toml::table &table, std::string_view key, std::filesystem::path &value)
{
    std::string text;
    stringField(table, key, text);
    if (table.get(key)) value = std::filesystem::path(utf8ToWide(text));
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
    std::string bytes;
    std::ptrdiff_t begin = -1;
    std::ptrdiff_t end = -1;
    std::map<std::string, std::string> values;
    bool conflict = false;
};

// Read-only lexical gate for the authorized Home .env. Codex reads this file
// with dotenv-style logical lines: while a quote stays open, or after a value
// ending in a backslash, physical lines are joined into one value. A marker
// inside such a span is value content, not an independent managed block, so
// marker recognition and byte-splice editing are only safe when every physical
// line is an independent single-line construct. Returns the 1-based number of
// the first line Guard refuses to edit, or 0 when the whole file is safe.
std::ptrdiff_t unsafeEnvLine(const std::string &bytes)
{
    enum class Ctx { Start, Key, Value, Comment };
    Ctx ctx = Ctx::Start;
    bool inQuote = false;
    char quoteChar = 0;
    bool escaped = false;
    bool spaced = false;
    std::ptrdiff_t line = 1;
    std::size_t offset = bytes.rfind("\xef\xbb\xbf", 0) == 0 ? 3 : 0;
    const std::size_t size = bytes.size();
    for (; offset < size; ++offset) {
        const char ch = bytes[offset];
        if (inQuote) {
            if (ch == '\n') return line; // The quoted value spans a physical line.
            if (escaped) escaped = false;
            else if (ch == '\\') escaped = true;
            else if (ch == quoteChar) { inQuote = false; quoteChar = 0; }
            continue;
        }
        if (ch == '\n') {
            ctx = Ctx::Start;
            escaped = false;
            spaced = false;
            ++line;
            continue;
        }
        if (ctx == Ctx::Comment) continue;
        if (escaped) { escaped = false; continue; }
        if (ch == '\\') {
            // Outside quotes dotenv joins a trailing backslash with the next
            // physical line; refuse to guess what that continuation means.
            if (offset + 1 >= size || bytes[offset + 1] == '\n') return line;
            escaped = true;
            continue;
        }
        if (ctx == Ctx::Start) {
            if (ch == ' ' || ch == '\t') continue;
            if (ch == '#') { ctx = Ctx::Comment; continue; } // Quotes inside comments never pair.
            ctx = Ctx::Key;
        }
        if (ctx == Ctx::Key) {
            if (ch == '=') { ctx = Ctx::Value; spaced = false; continue; }
            if (ch == '\'' || ch == '"') return line; // Quoted key: assignment boundary is not decidable.
            if (ch == '#') { ctx = Ctx::Comment; continue; } // No assignment on this line.
            continue;
        }
        // Unquoted value context. A hash only starts a trailing comment after
        // whitespace; before it, it is literal value content.
        if (ch == '\'' || ch == '"') { inQuote = true; quoteChar = ch; continue; }
        if ((ch == ' ' || ch == '\t') && !spaced) { spaced = true; continue; }
        if (ch == '#' && spaced) ctx = Ctx::Comment;
    }
    if (inQuote || escaped) return line; // Unterminated quote or trailing backslash at end of file.
    return 0;
}

bool generatedNoProxy(const std::string &bytes)
{
    if (bytes.empty() || bytes.size() > 4096 || bytes.find('\r') != std::string::npos
        || bytes.find('\n') != std::string::npos || bytes.find('\0') != std::string::npos)
        return false;
    // Configuration entries can themselves contain commas. Find a partition
    // into at most 32 valid entries instead of falsely rejecting a block that
    // Guard itself generated from such entries.
    std::vector<std::ptrdiff_t> boundaries{-1};
    for (std::size_t i = 0; i < bytes.size(); ++i) if (bytes[i] == ',') boundaries.push_back(static_cast<std::ptrdiff_t>(i));
    boundaries.push_back(static_cast<std::ptrdiff_t>(bytes.size()));
    std::vector<int> fewest(boundaries.size(), 33);
    fewest[0] = 0;
    for (std::size_t i = 0; i + 1 < boundaries.size(); ++i) {
        if (fewest[i] >= 32) continue;
        for (std::size_t j = i + 1; j < boundaries.size(); ++j) {
            const auto length = boundaries[j] - boundaries[i] - 1;
            if (length > 255) break;
            if (length == 0) continue;
            const std::string entry = asciiTrimmed(std::string_view(bytes).substr(
                static_cast<std::size_t>(boundaries[i] + 1), static_cast<std::size_t>(length)));
            if (entry.size() != static_cast<std::size_t>(length)) continue;
            auto &next = fewest[j];
            next = std::min(next, fewest[i] + 1);
        }
    }
    return fewest.back() <= 32;
}

void validateManagedValues(const EnvFile &env)
{
    const std::string &http = env.values.at("HTTP_PROXY");
    if (http != env.values.at("HTTPS_PROXY") || !generatedNoProxy(env.values.at("NO_PROXY")))
        fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block contains values Guard could not have generated.");
    // ^http://(\[[^\]]+\]|[^:/?#\s]+):([0-9]{1,5})$
    std::size_t offset = 0;
    const std::string prefix = "http://";
    if (http.size() < prefix.size() || http.compare(0, prefix.size(), prefix) != 0)
        fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint was edited.");
    offset = prefix.size();
    std::string host;
    if (offset < http.size() && http[offset] == '[') {
        const std::size_t close = http.find(']', offset);
        if (close == std::string::npos) fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint was edited.");
        host = http.substr(offset + 1, close - offset - 1);
        if (host.empty()) fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint was edited.");
        offset = close + 1;
    } else {
        const std::size_t start = offset;
        while (offset < http.size()) {
            const char ch = http[offset];
            if (ch == ':' || ch == '/' || ch == '?' || ch == '#' || std::isspace(static_cast<unsigned char>(ch))) break;
            ++offset;
        }
        host = http.substr(start, offset - start);
        if (host.empty()) fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint was edited.");
    }
    if (offset >= http.size() || http[offset] != ':')
        fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint was edited.");
    ++offset;
    const std::size_t digitsStart = offset;
    while (offset < http.size() && http[offset] >= '0' && http[offset] <= '9') ++offset;
    if (offset - digitsStart < 1 || offset - digitsStart > 5 || offset != http.size())
        fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint was edited.");
    Config generated;
    generated.host = host;
    try {
        generated.port = parsePortStrict(http.substr(digitsStart, offset - digitsStart));
        generated.validate();
    }
    catch (const Error &) { fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint is not a valid loopback proxy."); }
    if (generated.proxyUrl() != http)
        fail("BACKEND_PROXY_BLOCK_INVALID", "The managed block proxy endpoint was edited.");
}

EnvFile readEnv(const std::filesystem::path &path)
{
    EnvFile env;
    UniqueHandle file(CreateFileW(path.c_str(), GENERIC_READ,
                                  FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                                  nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (!file.valid()) {
        if (GetLastError() == ERROR_FILE_NOT_FOUND || GetLastError() == ERROR_PATH_NOT_FOUND) return env;
        fail("BACKEND_PROXY_ENV_IO", "Cannot read the authorized Home proxy file.");
    }
    env.exists = true;
    env.bytes = readHandle(file.get(), envLimit, "BACKEND_PROXY_ENV_IO");
    if (!strictUtf8(env.bytes)) fail("BACKEND_PROXY_ENV_ENCODING", "The proxy file is not valid UTF-8.");
    const std::ptrdiff_t unsafe = unsafeEnvLine(env.bytes);
    if (unsafe > 0)
        fail("BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED",
             "The proxy file uses multi-line or unterminated quoting at line " + std::to_string(unsafe)
                 + "; Guard cannot safely edit it automatically. Fix the file manually and retry.");
    bool inside = false;
    std::size_t offset = 0;
    while (offset < env.bytes.size()) {
        const auto newline = env.bytes.find('\n', offset);
        const std::size_t stop = newline == std::string::npos ? env.bytes.size() : newline;
        std::string line = env.bytes.substr(offset, stop - offset);
        if (!line.empty() && line.back() == '\r') line.pop_back();
        const bool bom = offset == 0 && line.rfind("\xef\xbb\xbf", 0) == 0;
        if (bom) line.erase(0, 3);
        const std::size_t next = newline == std::string::npos ? env.bytes.size() : newline + 1;
        if (line == blockBegin) {
            if (env.begin >= 0 || env.end >= 0) fail("BACKEND_PROXY_BLOCK_INVALID", "Duplicate or misordered Guard block.");
            env.begin = static_cast<std::ptrdiff_t>(offset + (bom ? 3 : 0));
            inside = true;
        } else if (line == blockEnd) {
            if (!inside || env.end >= 0) fail("BACKEND_PROXY_BLOCK_INVALID", "Duplicate or misordered Guard block end.");
            env.end = static_cast<std::ptrdiff_t>(next);
            inside = false;
        } else if (startsWith(line, "# BEGIN CODEX PROXY GUARD:")
                   || startsWith(line, "# END CODEX PROXY GUARD:")) {
            fail("BACKEND_PROXY_BLOCK_INVALID", "Unknown Guard block version.");
        } else if (inside) {
            const auto equal = line.find('=');
            const std::string key = equal == std::string::npos ? "" : line.substr(0, equal);
            if (equal == std::string::npos
                || (key != "HTTP_PROXY" && key != "HTTPS_PROXY" && key != "NO_PROXY")
                || env.values.count(key) > 0)
                fail("BACKEND_PROXY_BLOCK_INVALID", "The Guard block has been edited or contains duplicate keys.");
            env.values.emplace(key, line.substr(equal + 1));
        } else {
            const std::string text = asciiTrimmed(line);
            if (!text.empty() && text[0] != '#') {
                const auto equal = text.find('=');
                if (equal != std::string::npos) {
                    std::string key = asciiTrimmed(text.substr(0, equal));
                    if (key.starts_with("export") && key.size() > 6 && std::isspace(static_cast<unsigned char>(key[6])))
                        key = asciiTrimmed(key.substr(6));
                    key = asciiFolded(key);
                    if (key == "http_proxy" || key == "https_proxy" || key == "no_proxy" || key == "all_proxy")
                        env.conflict = true;
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
    return env.begin >= 0 && env.values.at("HTTP_PROXY") == config.proxyUrl()
        && env.values.at("HTTPS_PROXY") == config.proxyUrl()
        && env.values.at("NO_PROXY") == config.noProxyValue();
}

class EnvTransaction {
    UniqueHandle handle_;
    bool committed_ = false;
public:
    EnvTransaction() : handle_(CreateTransaction(nullptr, nullptr, 0, 0, 0, 5000, nullptr))
    {
        if (!handle_.valid())
            fail("BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE", "Windows file transactions are unavailable; the proxy file was not changed.");
    }
    ~EnvTransaction() { if (!committed_) RollbackTransaction(handle_.get()); }
    HANDLE handle() const { return handle_.get(); }
    void commit()
    {
        if (!CommitTransaction(handle_.get()))
            fail("BACKEND_PROXY_ENV_IO", "The proxy file transaction could not commit; no partial edit was published.");
        committed_ = true;
    }
};

void atomicEnv(const std::filesystem::path &path, const EnvFile &expected, const std::string &updated, bool remove = false)
{
    if (updated.size() > envLimit) fail("BACKEND_PROXY_ENV_IO", "The updated proxy file exceeds 1 MiB.");
    ensureParent(path, "BACKEND_PROXY_ENV_IO");
    EnvTransaction transaction;
    {
        // A transacted writer locks both data and namespace identity, including
        // against non-transacted rename-and-save editors. The lock survives the
        // file-handle close until commit/rollback. A normal shared-delete handle
        // plus pathname rename cannot provide this compare-and-write guarantee.
        UniqueHandle file(CreateFileTransactedW(path.c_str(), GENERIC_READ | GENERIC_WRITE | DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
            expected.exists ? OPEN_EXISTING : CREATE_NEW, FILE_ATTRIBUTE_NORMAL,
            nullptr, transaction.handle(), nullptr, nullptr));
        if (!file.valid()) {
            const DWORD error = GetLastError();
            if (error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND || error == ERROR_FILE_EXISTS || error == ERROR_ALREADY_EXISTS)
                fail("BACKEND_PROXY_CONFIG_CONFLICT", "The proxy file changed; no changes were applied.");
            if (error == ERROR_TRANSACTIONS_UNSUPPORTED_REMOTE || error == ERROR_NOT_SUPPORTED
                || error == ERROR_INVALID_FUNCTION || error == ERROR_EFS_NOT_ALLOWED_IN_TRANSACTION)
                fail("BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE", "This location does not support the required Windows file transactions; the proxy file was not changed.");
            fail("BACKEND_PROXY_ENV_IO", "Cannot lock the proxy file for an atomic transaction.");
        }
        if (readHandle(file.get(), envLimit, "BACKEND_PROXY_ENV_IO") != expected.bytes)
            fail("BACKEND_PROXY_CONFIG_CONFLICT", "The proxy file changed; no changes were applied.");
        if (envTransactionHook) envTransactionHook(false);
        if (remove) {
            if (!DeleteFileTransactedW(path.c_str(), transaction.handle()))
                fail("BACKEND_PROXY_ENV_IO", "Cannot remove the managed proxy file transactionally.");
        } else if (!writeHandle(file.get(), updated)) {
            fail("BACKEND_PROXY_ENV_IO", "Cannot stage the proxy file transaction.");
        }
        if (envTransactionHook) envTransactionHook(true);
    }
    transaction.commit();
}
} // namespace

namespace testing {
void setEnvTransactionHook(std::function<void(bool)> hook) { envTransactionHook = std::move(hook); }
}

void Config::validate() const
{
    if (version != 2) fail("CONFIG_INVALID", "Unsupported configuration version; expected version 2.");
    if (!asciiEqualsFolded(scheme, "http")) fail("CONFIG_INVALID", "Proxy scheme must be http.");
    if (port < 1 || port > 65535) throw Error("CONFIG_INVALID", "Proxy port must be between 1 and 65535.", "proxy.port");
    if (host.empty() || asciiTrimmed(host) != host || host.find('\0') != std::string::npos)
        throw Error("CONFIG_INVALID", "Invalid proxy host.", "proxy.host");
    IN_ADDR ipv4{};
    IN6_ADDR ipv6{};
    bool loopback = asciiEqualsFolded(host, "localhost");
    const std::wstring wideHost = utf8ToWide(host);
    if (InetPtonW(AF_INET, wideHost.c_str(), &ipv4) == 1)
        loopback = reinterpret_cast<const unsigned char *>(&ipv4)[0] == 127;
    if (InetPtonW(AF_INET6, wideHost.c_str(), &ipv6) == 1) {
        const auto *bytes = reinterpret_cast<const unsigned char *>(&ipv6);
        loopback = std::all_of(bytes, bytes + 15, [](unsigned char c) { return c == 0; }) && bytes[15] == 1;
    }
    if (!loopback) throw Error("CONFIG_INVALID", "Proxy host must be localhost or a loopback IP address.", "proxy.host");
    if (noProxy.empty() || noProxy.size() > 32 || noProxyValue().size() > 4096)
        fail("CONFIG_INVALID", "Proxy exclusions must contain 1 to 32 bounded entries.");
    for (const auto &entry : noProxy) {
        if (entry.empty() || asciiTrimmed(entry) != entry || entry.size() > 255
            || entry.find('\r') != std::string::npos || entry.find('\n') != std::string::npos
            || entry.find('\0') != std::string::npos)
            fail("CONFIG_INVALID", "Proxy exclusions contain an invalid entry.");
    }
    if (alternateScreen != "auto" && alternateScreen != "always" && alternateScreen != "never")
        fail("CONFIG_INVALID", "Invalid alternate-screen setting.");
}

std::string Config::proxyUrl() const
{
    const std::string bracketed = host.find(':') != std::string::npos ? "[" + host + "]" : host;
    return "http://" + bracketed + ":" + std::to_string(port);
}
std::string Config::noProxyValue() const
{
    std::string joined;
    for (std::size_t index = 0; index < noProxy.size(); ++index) {
        if (index > 0) joined += ',';
        joined += noProxy[index];
    }
    return joined;
}

Config Config::parse(const std::string &bytes)
{
    if (bytes.size() > configLimit || !strictUtf8(bytes))
        fail("CONFIG_INVALID", "Configuration exceeds 64 KiB or is not valid UTF-8.");
    try {
        const auto table = toml::parse(std::string_view(bytes));
        checkKeys(table, {"version", "proxy", "codex", "tui"});
        Config config;
        intField(table, "version", config.version);
        if (const auto *proxy = subtable(table, "proxy")) {
            checkKeys(*proxy, {"scheme", "host", "port", "no_proxy"});
            stringField(*proxy, "scheme", config.scheme);
            stringField(*proxy, "host", config.host);
            intField(*proxy, "port", config.port);
            if (const auto *node = proxy->get("no_proxy")) {
                if (!node->is_array()) fail("CONFIG_INVALID", "Proxy exclusions must be an array.");
                config.noProxy.clear();
                for (const auto &entry : *node->as_array()) {
                    if (!entry.is_string()) fail("CONFIG_INVALID", "Proxy exclusions must be strings.");
                    config.noProxy.push_back(entry.as_string()->get());
                }
            }
        }
        if (const auto *codex = subtable(table, "codex")) {
            checkKeys(*codex, {"executable_override", "cli_executable_override", "refuse_if_running", "manage_codex_proxy_env", "proxy_env_home"});
            pathField(*codex, "executable_override", config.executableOverride);
            pathField(*codex, "cli_executable_override", config.cliOverride);
            pathField(*codex, "proxy_env_home", config.home);
            boolField(*codex, "refuse_if_running", config.refuseIfRunning);
            boolField(*codex, "manage_codex_proxy_env", config.manageBackend);
        }
        if (const auto *tui = subtable(table, "tui")) {
            checkKeys(*tui, {"alternate_screen"});
            stringField(*tui, "alternate_screen", config.alternateScreen);
        }
        config.validate();
        return config;
    } catch (const toml::parse_error &) { fail("CONFIG_INVALID", "Configuration is not valid TOML."); }
}

std::string Config::toml() const
{
    validate();
    toml::array exclusions;
    for (const auto &entry : noProxy) exclusions.push_back(entry);
    toml::table table{{"version", version},
        {"proxy", toml::table{{"scheme", scheme}, {"host", host}, {"port", port}, {"no_proxy", std::move(exclusions)}}},
        {"codex", toml::table{{"executable_override", wideToUtf8(executableOverride.wstring())},
             {"cli_executable_override", wideToUtf8(cliOverride.wstring())},
             {"refuse_if_running", refuseIfRunning}, {"manage_codex_proxy_env", manageBackend},
             {"proxy_env_home", wideToUtf8(home.wstring())}}},
        {"tui", toml::table{{"alternate_screen", alternateScreen}}}};
    std::ostringstream output;
    output << table;
    return output.str() + '\n';
}

std::filesystem::path Config::defaultPath()
{
    return knownFolder(FOLDERID_RoamingAppData) / "codex-proxy-guard" / "config.toml";
}
std::filesystem::path Config::defaultHome()
{
    return knownFolder(FOLDERID_Profile) / ".codex";
}

Config Config::load(const std::filesystem::path &path)
{
    UniqueHandle file(CreateFileW(path.c_str(), GENERIC_READ,
                                  FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                                  nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (!file.valid()) fail("CONFIG_READ_FAILED", "Cannot read the Guard configuration.");
    return parse(readHandle(file.get(), configLimit, "CONFIG_READ_FAILED"));
}

Config Config::loadOrCreate(const std::filesystem::path &path)
{
    if (std::filesystem::exists(path)) return load(path);
    try { initializeConfig(path, Config{}, false); }
    catch (const Error &) { if (std::filesystem::exists(path)) return load(path); throw; }
    return load(path);
}

struct ConfigLease::Impl { UniqueHandle handle; };
ConfigLease::ConfigLease(const std::filesystem::path &path, const Config &expected)
    : impl_(std::make_unique<Impl>())
{
    impl_->handle.reset(CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                    OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (!impl_->handle.valid()) fail("CONFIG_LOCK_FAILED", "Configuration is changing or unavailable.");
    const auto current = tryConfig(readHandle(impl_->handle.get(), configLimit, "CONFIG_READ_FAILED"));
    if (!current || *current != expected) fail("CONFIG_CHANGED", "Refresh before launching with the new configuration.");
}
ConfigLease::~ConfigLease() = default;

void initializeConfig(const std::filesystem::path &path, const Config &config, bool force)
{
    config.validate();
    if (!force || !std::filesystem::exists(path)) { Transaction(path, nullptr, false, true).commit(config); return; }
    // Force initialization still holds the same write/delete exclusion as every
    // editor and must never silently discard an existing authorization binding.
    Transaction transaction(path, nullptr, false);
    const auto current = tryConfig(transaction.original);
    if (current && current->manageBackend && (current->home != config.home || !config.manageBackend))
        fail("BACKEND_PROXY_REVOKE_REQUIRED", "Revoke the existing proxy authorization before resetting this configuration.");
    transaction.commit(config);
}

Config updateProxy(const std::filesystem::path &path, const Config &expected, bool invalid,
                   const std::string &host, int port)
{
    Transaction transaction(path, invalid ? nullptr : &expected, invalid);
    Config updated = expected;
    updated.host = host;
    updated.port = port;
    transaction.commit(updated);
    return updated;
}

std::filesystem::path consentHome(const Config &config)
{
    const std::filesystem::path home = config.home.empty() ? Config::defaultHome() : config.home;
    if (!absoluteHome(home)) fail("BACKEND_PROXY_SCOPE_UNCONFIRMED", "The Codex Home must be an absolute path.");
    return home;
}

Config updateConsent(const std::filesystem::path &path, const Config &expected, bool enable,
                     const std::filesystem::path &confirmedHome)
{
    Transaction transaction(path, &expected, false);
    if (!absoluteHome(confirmedHome) || consentHome(expected) != confirmedHome)
        fail("BACKEND_PROXY_SCOPE_UNCONFIRMED", "The confirmed Home no longer matches the configuration.");
    Config updated = expected;
    if (enable) {
        if (expected.manageBackend) fail("CONFIG_CHANGED", "Proxy management is already authorized; refresh before confirming.");
        const auto canonical = canonicalPathOf(confirmedHome, true);
        updated.home = canonical.value_or(confirmedHome);
        updated.manageBackend = true;
    } else {
        if (!expected.manageBackend || expected.home != confirmedHome)
            fail("CONFIG_CHANGED", "Proxy management is no longer authorized for the confirmed Home.");
        try { revokeProxyEnv(confirmedHome); }
        catch (const Error &error) {
            static const std::set<std::string> reasons{"BACKEND_PROXY_ENV_IO", "BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE",
                "BACKEND_PROXY_CONFIG_CONFLICT", "BACKEND_PROXY_BLOCK_INVALID", "BACKEND_PROXY_ENV_ENCODING",
                "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED", "BACKEND_PROXY_SCOPE_UNCONFIRMED"};
            const std::string reason = reasons.count(error.code) ? error.code : "BACKEND_PROXY_ENV_FAILURE";
            fail("BACKEND_PROXY_REVOKE_FAILED", "The managed block was not safely removed; consent and its bound Home remain unchanged. Cause: " + reason + ".");
        }
        updated.manageBackend = false;
        updated.home.clear();
    }
    try { transaction.commit(updated); }
    catch (const Error &error) {
        if (!enable && error.code != "CONFIG_ROLLBACK_FAILED")
            fail("BACKEND_PROXY_CONSENT_SAVE_FAILED", "The block was removed but consent could not be saved; refresh and retry.");
        throw;
    }
    return updated;
}

std::string inspectProxyEnv(const Config &config)
{
    if (!config.manageBackend) return "not_authorized";
    try {
        config.validate();
        const EnvFile env = readEnv(boundHome(config) / ".env");
        if (env.conflict) return "conflict";
        if (env.begin < 0) return "pending";
        return currentValues(env, config) ? "current" : "stale";
    } catch (const Error &error) {
        if (error.code == "BACKEND_PROXY_ENV_IO") return "unavailable";
        if (error.code == "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED") return "unsupported";
        return "invalid";
    }
}

void prepareProxyEnv(const Config &config)
{
    config.validate();
    const std::filesystem::path path = boundHome(config) / ".env";
    const EnvFile env = readEnv(path);
    if (env.conflict) fail("BACKEND_PROXY_CONFIG_CONFLICT", "Proxy keys already exist outside Guard's block; resolve them manually.");
    if (currentValues(env, config)) return;
    const std::string url = config.proxyUrl();
    const std::string block = std::string(blockBegin) + "\nHTTP_PROXY=" + url + "\nHTTPS_PROXY=" + url
        + "\nNO_PROXY=" + config.noProxyValue() + "\n" + blockEnd + "\n";
    std::string updated;
    if (env.begin >= 0) {
        updated = env.bytes.substr(0, static_cast<std::size_t>(env.begin)) + block
            + env.bytes.substr(static_cast<std::size_t>(env.end));
    } else {
        updated = env.bytes + ((!env.bytes.empty() && env.bytes.back() != '\n') ? "\n" : "") + block;
    }
    atomicEnv(path, env, updated);
}

void revokeProxyEnv(const std::filesystem::path &home)
{
    if (!absoluteHome(home)) fail("BACKEND_PROXY_SCOPE_UNCONFIRMED", "The Codex Home must be an absolute path.");
    const std::filesystem::path path = home / ".env";
    const EnvFile env = readEnv(path);
    if (env.begin < 0) return;
    const std::string updated = env.bytes.substr(0, static_cast<std::size_t>(env.begin))
        + env.bytes.substr(static_cast<std::size_t>(env.end));
    atomicEnv(path, env, updated, updated.empty());
}

} // namespace cpg

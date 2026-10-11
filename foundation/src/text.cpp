#include "cpg/text.h"
#include "cpg/error.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

namespace cpg {
namespace {

char asciiLower(char value)
{
    return value >= 'A' && value <= 'Z' ? static_cast<char>(value - 'A' + 'a') : value;
}

bool iequals(std::string_view left, std::string_view right)
{
    if (left.size() != right.size()) return false;
    for (std::size_t index = 0; index < left.size(); ++index)
        if (asciiLower(left[index]) != asciiLower(right[index])) return false;
    return true;
}

// Case-insensitive replace over a UTF-8 byte string. Only ASCII letters fold,
// so a match can never start inside a multi-byte sequence.
void replaceAllInsensitive(std::string &text, std::string_view needle, std::string_view replacement)
{
    if (needle.empty() || text.size() < needle.size()) return;
    std::string result;
    result.reserve(text.size());
    std::size_t copied = 0;
    const std::size_t limit = text.size() - needle.size();
    while (copied <= limit) {
        if (iequals(std::string_view(text).substr(copied, needle.size()), needle)) {
            result.append(replacement);
            copied += needle.size();
        } else {
            result.push_back(text[copied]);
            ++copied;
        }
    }
    result.append(text, copied, std::string::npos);
    text = std::move(result);
}

// Remove CSI ("ESC [ <0x30-0x3F>* <0x20-0x2F>* <0x40-0x7E>") and OSC
// ("ESC ] ... BEL" or "ESC ] ... ESC \") sequences. Anything else after ESC
// stays verbatim.
void removeAnsiOscSequences(std::string &text)
{
    std::string result;
    result.reserve(text.size());
    std::size_t index = 0;
    while (index < text.size()) {
        if (text[index] != '\x1b') { result.push_back(text[index++]); continue; }
        if (index + 1 >= text.size()) { result.push_back(text[index++]); continue; }
        const char introducer = text[index + 1];
        if (introducer == '[') {
            std::size_t scan = index + 2;
            while (scan < text.size() && static_cast<unsigned char>(text[scan]) >= 0x30
                   && static_cast<unsigned char>(text[scan]) <= 0x3f) ++scan;
            while (scan < text.size() && static_cast<unsigned char>(text[scan]) >= 0x20
                   && static_cast<unsigned char>(text[scan]) <= 0x2f) ++scan;
            if (scan < text.size() && static_cast<unsigned char>(text[scan]) >= 0x40
                && static_cast<unsigned char>(text[scan]) <= 0x7e) {
                index = scan + 1;
                continue;
            }
        } else if (introducer == ']') {
            std::size_t scan = index + 2;
            bool terminated = false;
            while (scan < text.size()) {
                if (text[scan] == '\x07') { index = scan + 1; terminated = true; break; }
                if (text[scan] == '\x1b' && scan + 1 < text.size() && text[scan + 1] == '\\') {
                    index = scan + 2;
                    terminated = true;
                    break;
                }
                ++scan;
            }
            if (terminated) continue;
        }
        result.push_back(text[index]);
        ++index;
    }
    text = std::move(result);
}

bool asciiWhitespace(char value)
{
    return value == ' ' || value == '\t' || value == '\r' || value == '\n'
        || value == '\v' || value == '\f';
}

// "://user:pass@host" -> "://[REDACTED]@host". The credentials span runs from
// after "://" to the first '@' and must be non-empty and free of '/', '@' and
// whitespace. A span that hits '/', whitespace or the end first is copied
// verbatim.
void redactUriCredentials(std::string &text)
{
    std::string result;
    result.reserve(text.size());
    std::size_t index = 0;
    while (index < text.size()) {
        if (text.compare(index, 3, "://") != 0) {
            result.push_back(text[index++]);
            continue;
        }
        std::size_t scan = index + 3;
        while (scan < text.size()) {
            const char value = text[scan];
            if (asciiWhitespace(value) || value == '/' || value == '@') break;
            ++scan;
        }
        if (scan < text.size() && text[scan] == '@' && scan > index + 3) {
            result.append("://[REDACTED]@");
            index = scan + 1;
        } else {
            result.append("://");
            index += 3;
        }
    }
    text = std::move(result);
}

bool matchesWord(std::string_view text, std::size_t position, std::string_view word)
{
    if (position + word.size() > text.size()) return false;
    for (std::size_t index = 0; index < word.size(); ++index)
        if (asciiLower(text[position + index]) != asciiLower(word[index])) return false;
    return true;
}

// Secret-bearing line prefixes, mirroring the previous implementation's
// vocabulary: header forms include the colon; assignment forms expect an
// optional whitespace run and '=' after the word ("api" additionally takes an
// optional "_" or "-" followed by "key"). Returns the matched prefix length
// starting at `position`, or 0 when nothing matches there.
std::size_t secretPrefixLength(std::string_view line, std::size_t position)
{
    static const struct { std::string_view word; bool assignment; } prefixes[] = {
        {"proxy-authorization:", false}, {"authorization:", false},
        {"set-cookie:", false}, {"cookie:", false}, {"token", true},
        {"api", true}, {"authorization", true}, {"cookie", true},
        {"password", true}, {"secret", true},
    };
    for (const auto &prefix : prefixes) {
        if (!matchesWord(line, position, prefix.word)) continue;
        if (!prefix.assignment) return prefix.word.size();
        std::size_t scan = position + prefix.word.size();
        if (prefix.word == "api") {
            if (scan < line.size() && (line[scan] == '_' || line[scan] == '-')) ++scan;
            if (!matchesWord(line, scan, "key")) continue;
            scan += 3;
        }
        while (scan < line.size() && (line[scan] == ' ' || line[scan] == '\t')) ++scan;
        if (scan < line.size() && line[scan] == '=') return scan - position + 1;
    }
    return 0;
}

// Keep the matched prefix, replace everything after it on the same line with
// "[REDACTED]"; the earliest match on the line wins.
void redactSecretLines(std::string &text)
{
    std::string result;
    result.reserve(text.size());
    std::size_t lineStart = 0;
    while (lineStart < text.size()) {
        std::size_t lineEnd = text.find('\n', lineStart);
        const bool hasNewline = lineEnd != std::string::npos;
        if (!hasNewline) lineEnd = text.size();
        const std::string_view line(text.data() + lineStart, lineEnd - lineStart);
        std::size_t match = std::string::npos;
        std::size_t matchLength = 0;
        for (std::size_t index = 0; index < line.size(); ++index) {
            matchLength = secretPrefixLength(line, index);
            if (matchLength > 0) { match = index; break; }
        }
        if (match == std::string::npos) {
            result.append(line);
        } else {
            result.append(line.substr(0, match + matchLength));
            result.append("[REDACTED]");
        }
        if (hasNewline) result.push_back('\n');
        lineStart = lineEnd + 1;
    }
    text = std::move(result);
}

bool isWhitespaceCodePoint(char32_t point)
{
    switch (point) {
    case 0x09: case 0x0a: case 0x0b: case 0x0c: case 0x0d: case 0x20:
    case 0x85: case 0xa0: case 0x1680: case 0x2028: case 0x2029:
    case 0x202f: case 0x205f: case 0x3000:
        return true;
    default:
        return point >= 0x2000 && point <= 0x200a;
    }
}

// Practical Cf coverage: soft hyphen, Arabic/Hebrew marks, zero-width and
// bidi controls, joiners, invisible operators, byte-order marks and unicode
// tag characters.
bool isFormatCodePoint(char32_t point)
{
    return point == 0x00ad || (point >= 0x0600 && point <= 0x0605) || point == 0x061c
        || point == 0x06dd || point == 0x070f || point == 0x08e2 || point == 0x180e
        || (point >= 0x200b && point <= 0x200f) || (point >= 0x202a && point <= 0x202e)
        || (point >= 0x2060 && point <= 0x2064) || (point >= 0x2066 && point <= 0x206f)
        || point == 0xfeff || (point >= 0xfff9 && point <= 0xfffb)
        || (point >= 0x110bd && point <= 0x110cd) || (point >= 0x13430 && point <= 0x1343f)
        || (point >= 0x1bca0 && point <= 0x1bca3) || (point >= 0x1d173 && point <= 0x1d17a)
        || point == 0xe0001 || (point >= 0xe0020 && point <= 0xe007f);
}

// Decode UTF-8; map whitespace to ' ', drop control/format/surrogate code
// points, re-encode everything else. Invalid bytes are dropped defensively —
// output is always valid UTF-8 even for hostile input.
void appendFiltered(std::string &out, const std::string &text)
{
    std::size_t index = 0;
    while (index < text.size()) {
        const auto byte = static_cast<unsigned char>(text[index]);
        std::size_t length = 0;
        char32_t point = 0;
        if (byte < 0x80) { length = 1; point = byte; }
        else if ((byte & 0xe0) == 0xc0 && index + 1 < text.size()) {
            length = 2; point = byte & 0x1f;
        } else if ((byte & 0xf0) == 0xe0 && index + 2 < text.size()) {
            length = 3; point = byte & 0x0f;
        } else if ((byte & 0xf8) == 0xf0 && index + 3 < text.size()) {
            length = 4; point = byte & 0x07;
        }
        bool valid = length > 0;
        for (std::size_t step = 1; valid && step < length; ++step) {
            const auto continuation = static_cast<unsigned char>(text[index + step]);
            if ((continuation & 0xc0) != 0x80) valid = false;
            else point = (point << 6) | (continuation & 0x3f);
        }
        if (!valid) { ++index; continue; }
        index += length;
        if (isWhitespaceCodePoint(point)) { out.push_back(' '); continue; }
        const bool control = point < 0x20 || (point >= 0x7f && point <= 0x9f);
        const bool surrogate = point >= 0xd800 && point <= 0xdfff;
        if (control || surrogate || isFormatCodePoint(point)) continue;
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
}

void collapseWhitespace(std::string &text)
{
    std::size_t begin = 0, end = text.size();
    while (begin < end && text[begin] == ' ') ++begin;
    while (end > begin && text[end - 1] == ' ') --end;
    std::string result;
    result.reserve(end - begin);
    bool pending = false;
    for (std::size_t index = begin; index < end; ++index) {
        if (text[index] == ' ') { pending = true; continue; }
        if (pending && !result.empty()) result.push_back(' ');
        pending = false;
        result.push_back(text[index]);
    }
    text = std::move(result);
}

} // namespace

bool strictUtf8(std::string_view bytes)
{
    if (bytes.empty()) return true;
    return MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, bytes.data(),
                               static_cast<int>(bytes.size()), nullptr, 0) != 0;
}

std::wstring utf8ToWide(std::string_view utf8)
{
    if (utf8.empty()) return {};
    const int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.data(),
                                         static_cast<int>(utf8.size()), nullptr, 0);
    if (size <= 0) throw Error("TEXT_ENCODING_INVALID", "Text is not valid UTF-8.");
    std::wstring wide(static_cast<std::size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.data(),
                        static_cast<int>(utf8.size()), wide.data(), size);
    return wide;
}

std::string wideToUtf8(std::wstring_view wide)
{
    if (wide.empty()) return {};
    const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, wide.data(),
                                         static_cast<int>(wide.size()), nullptr, 0,
                                         nullptr, nullptr);
    if (size <= 0) throw Error("TEXT_ENCODING_INVALID", "Text is not valid UTF-16.");
    std::string utf8(static_cast<std::size_t>(size), '\0');
    WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, wide.data(),
                        static_cast<int>(wide.size()), utf8.data(), size, nullptr, nullptr);
    return utf8;
}

std::string publicText(std::string_view input, std::size_t limit)
{
    std::string text(input);
    wchar_t profileBuffer[MAX_PATH + 1] = {};
    const DWORD profileLength = GetEnvironmentVariableW(L"USERPROFILE", profileBuffer,
                                                        static_cast<DWORD>(std::size(profileBuffer)));
    if (profileLength > 0 && profileLength < std::size(profileBuffer)) {
        const std::string profile = wideToUtf8(profileBuffer);
        std::string forward = profile;
        for (char &value : forward) if (value == '\\') value = '/';
        replaceAllInsensitive(text, profile, "%USERPROFILE%");
        replaceAllInsensitive(text, forward, "%USERPROFILE%");
    }
    removeAnsiOscSequences(text);
    redactUriCredentials(text);
    redactSecretLines(text);
    std::string filtered;
    appendFiltered(filtered, text);
    collapseWhitespace(filtered);
    // Truncate by code points, never inside a multi-byte sequence.
    std::size_t points = 0, bytes = 0;
    while (bytes < filtered.size() && points < limit) {
        const auto byte = static_cast<unsigned char>(filtered[bytes]);
        std::size_t length = 1;
        if ((byte & 0xe0) == 0xc0) length = 2;
        else if ((byte & 0xf0) == 0xe0) length = 3;
        else if ((byte & 0xf8) == 0xf0) length = 4;
        bytes += length;
        ++points;
    }
    filtered.resize(std::min(bytes, filtered.size()));
    return filtered;
}

} // namespace cpg

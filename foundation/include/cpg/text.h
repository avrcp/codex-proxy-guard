#pragma once

#include <cstddef>
#include <string>
#include <string_view>

namespace cpg {

// Strict UTF-8 validation (surrogates, overlongs and 5/6-byte forms refused)
// via MultiByteToWideChar with MB_ERR_INVALID_CHARS.
bool strictUtf8(std::string_view bytes);

// Lossless UTF-8 <-> UTF-16 conversions; invalid input throws
// Error("TEXT_ENCODING_INVALID", ...). Never use raw MultiByteToWideChar
// conversions with error flags silently disabled.
std::wstring utf8ToWide(std::string_view utf8);
std::string wideToUtf8(std::wstring_view wide);

// Redact and flatten untrusted text for display or protocol emission:
// USERPROFILE paths, ANSI CSI/OSC escape sequences, URI userinfo, secret
// bearing lines (authorization/cookie/token/api key/password/secret),
// control/format/surrogate code points; whitespace collapsed; truncated to
// `limit` code points. Deterministic scanner — no regular expressions over
// untrusted input.
std::string publicText(std::string_view text, std::size_t limit = 160);

} // namespace cpg

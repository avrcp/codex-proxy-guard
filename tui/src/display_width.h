#pragma once

#include <cwchar>
#include <string>
#include <string_view>

namespace cpg::tui {

// Terminal cell width of one UTF-16 code unit in context: 0 for the low half
// of a surrogate pair and combining marks, 2 for fullwidth/ideographic forms
// and (conservatively) astral-plane high halves, 1 otherwise.
int codeUnitWidth(wchar_t current, bool surrogateHighBefore);

// Display width of a UTF-16 string, surrogate-aware.
int displayWidth(std::wstring_view text);

// Append `text` to `out` truncated to `availableCells` display columns; when
// truncation happens an ellipsis ("...") replaces the tail. Returns the
// number of columns consumed.
int appendTruncated(std::wstring &out, std::wstring_view text, int availableCells);

} // namespace cpg::tui

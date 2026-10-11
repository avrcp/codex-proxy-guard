#pragma once

#include <cwchar>
#include <string>
#include <string_view>

namespace cpg::tui {

// Terminal cell width of one UTF-16 code unit in context: 0 for the low half
// of a surrogate pair and combining marks, 2 for fullwidth/ideographic forms
// and (conservatively) astral-plane high halves, 1 otherwise.
int codeUnitWidth(wchar_t current, bool surrogateHighBefore);

// One atomic display element: a base glyph (possibly a surrogate pair), its
// terminal cell width, and whether it is a zero-width mark that must attach
// to the preceding leading cell instead of occupying a cell of its own.
// Isolated surrogate halves are normalized to U+FFFD so no invalid UTF-16
// code unit is ever emitted to the console.
struct DisplayCluster {
    std::wstring text;
    int width = 0;
    bool combiningOnly = false;
    std::size_t consumed = 0;
};

// Parses the next display cluster starting at `offset`; `consumed` is 0 only
// when the offset is at or past the end of `text`.
DisplayCluster nextDisplayCluster(std::wstring_view text, std::size_t offset);

// Display width of a UTF-16 string, cluster-aware: combining marks add 0 and
// surrogate pairs are measured as one (wide) cluster.
int displayWidth(std::wstring_view text);

// Append `text` to `out` truncated to `availableCells` display columns; when
// truncation happens an ellipsis ("...") replaces the tail. Surrogate pairs
// and combining marks are never split from their base cluster. Returns the
// number of columns consumed.
int appendTruncated(std::wstring &out, std::wstring_view text, int availableCells);

} // namespace cpg::tui

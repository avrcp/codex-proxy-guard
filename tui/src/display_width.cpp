#include "display_width.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

namespace cpg::tui {
namespace {

bool ctype3Flag(wchar_t value, WORD flag)
{
    WORD type = 0;
    if (GetStringTypeW(CT_CTYPE3, &value, 1, &type) == 0) return false;
    return (type & flag) != 0;
}

bool isSurrogateHigh(wchar_t value)
{
    return value >= 0xd800 && value <= 0xdbff;
}

bool isSurrogateLow(wchar_t value)
{
    return value >= 0xdc00 && value <= 0xdfff;
}

} // namespace

int codeUnitWidth(wchar_t current, bool surrogateHighBefore)
{
    if (surrogateHighBefore) return 0; // the pair was already measured as 2
    if (isSurrogateLow(current)) return 0;
    if (isSurrogateHigh(current)) return 2; // astral plane measured at the high half
    if (ctype3Flag(current, C3_NONSPACING | C3_DIACRITIC)) return 0;
    if (ctype3Flag(current, C3_FULLWIDTH | C3_IDEOGRAPH)) return 2;
    return 1;
}

int displayWidth(std::wstring_view text)
{
    int width = 0;
    bool previousHigh = false;
    for (const wchar_t unit : text) {
        width += codeUnitWidth(unit, previousHigh);
        previousHigh = isSurrogateHigh(unit) && !previousHigh;
    }
    return width;
}

int appendTruncated(std::wstring &out, std::wstring_view text, int availableCells)
{
    if (availableCells <= 0) return 0;
    int used = 0;
    bool previousHigh = false;
    for (std::size_t index = 0; index < text.size(); ++index) {
        const wchar_t unit = text[index];
        const int unitWidth = codeUnitWidth(unit, previousHigh);
        previousHigh = isSurrogateHigh(unit) && !previousHigh;
        const int remaining = availableCells - used;
        if (unitWidth > remaining) break;
        out.push_back(unit);
        used += unitWidth;
    }
    if (used < displayWidth(text) && availableCells >= 3) {
        while (used > availableCells - 3 && !out.empty()) {
            used -= codeUnitWidth(out.back(), false);
            out.pop_back();
        }
        out.append(L"...");
        used = availableCells;
    }
    return used;
}

} // namespace cpg::tui

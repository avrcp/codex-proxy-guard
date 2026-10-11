#include "display_width.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <vector>

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

DisplayCluster replacementCluster()
{
    DisplayCluster cluster;
    cluster.text.assign(1, L'\xfffd');
    cluster.width = 1;
    cluster.consumed = 1;
    return cluster;
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

DisplayCluster nextDisplayCluster(std::wstring_view text, std::size_t offset)
{
    DisplayCluster cluster;
    if (offset >= text.size()) return cluster;
    const wchar_t first = text[offset];
    if (isSurrogateHigh(first)) {
        if (offset + 1 < text.size() && isSurrogateLow(text[offset + 1])) {
            cluster.text.assign(text.substr(offset, 2));
            cluster.width = 2;
            cluster.consumed = 2;
            return cluster;
        }
        return replacementCluster(); // isolated high half
    }
    if (isSurrogateLow(first)) return replacementCluster(); // isolated low half
    cluster.text.assign(1, first);
    cluster.consumed = 1;
    if (ctype3Flag(first, C3_NONSPACING | C3_DIACRITIC)) {
        cluster.combiningOnly = true; // zero width: attaches to the previous cell
        return cluster;
    }
    if (ctype3Flag(first, C3_FULLWIDTH | C3_IDEOGRAPH)) cluster.width = 2;
    else cluster.width = 1;
    return cluster;
}

int displayWidth(std::wstring_view text)
{
    int width = 0;
    std::size_t offset = 0;
    for (;;) {
        const DisplayCluster cluster = nextDisplayCluster(text, offset);
        if (cluster.consumed == 0) break;
        width += cluster.width;
        offset += cluster.consumed;
    }
    return width;
}

int appendTruncated(std::wstring &out, std::wstring_view text, int availableCells)
{
    if (availableCells <= 0) return 0;
    int used = 0;
    bool truncated = false;
    std::vector<std::pair<std::size_t, int>> appended; // (units in out, columns)
    std::size_t offset = 0;
    while (offset < text.size()) {
        const DisplayCluster cluster = nextDisplayCluster(text, offset);
        offset += cluster.consumed;
        if (cluster.combiningOnly) {
            // A mark never separates from the base it follows, even when the
            // base itself did not fit (it is then simply not appended).
            out.append(cluster.text);
            appended.emplace_back(cluster.text.size(), 0);
            continue;
        }
        if (used + cluster.width > availableCells) {
            truncated = true;
            break;
        }
        out.append(cluster.text);
        appended.emplace_back(cluster.text.size(), cluster.width);
        used += cluster.width;
    }
    if (truncated && availableCells >= 3) {
        while (used > availableCells - 3 && !appended.empty()) {
            const auto [units, width] = appended.back();
            appended.pop_back();
            out.resize(out.size() - units);
            used -= width;
        }
        out.append(L"...");
        used = availableCells;
    }
    return used;
}

} // namespace cpg::tui

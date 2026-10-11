#include "screen.h"
#include "display_width.h"

#include <algorithm>

namespace cpg::tui {

void Screen::resize(int width, int height)
{
    width_ = std::max(0, width);
    height_ = std::max(0, height);
    cells_.assign(static_cast<std::size_t>(width_) * static_cast<std::size_t>(height_), Cell{});
}

void Screen::clear()
{
    std::fill(cells_.begin(), cells_.end(), Cell{});
}

Cell &Screen::cell(int x, int y)
{
    return cells_[static_cast<std::size_t>(y) * static_cast<std::size_t>(width_)
                  + static_cast<std::size_t>(x)];
}

const Cell &Screen::cell(int x, int y) const
{
    return cells_[static_cast<std::size_t>(y) * static_cast<std::size_t>(width_)
                  + static_cast<std::size_t>(x)];
}

int Screen::text(int x, int y, std::wstring_view value, std::uint8_t style)
{
    if (y < 0 || y >= height_) return 0;
    int column = std::max(0, x);
    int used = 0;
    std::size_t offset = 0;
    while (offset < value.size()) {
        const DisplayCluster cluster = nextDisplayCluster(value, offset);
        offset += cluster.consumed;
        if (cluster.combiningOnly) {
            // Zero-width marks attach to the leading cell that ends at the
            // previous column; they never address cell(column, ...) — that is
            // what made the old unit-based writer go out of bounds at the
            // right edge.
            if (column > 0 && column <= width_) {
                int lead = column - 1;
                while (lead > 0 && cell(lead, y).continuation) --lead;
                if (!cell(lead, y).continuation) cell(lead, y).glyph.append(cluster.text);
            }
            continue;
        }
        if (column + cluster.width > width_) break;
        Cell &lead = cell(column, y);
        lead.glyph = cluster.text;
        lead.style = style;
        lead.continuation = false;
        for (int fill = 1; fill < cluster.width; ++fill) {
            Cell &tail = cell(column + fill, y);
            tail.glyph.clear();
            tail.style = style;
            tail.continuation = true;
        }
        column += cluster.width;
        used += cluster.width;
    }
    return used;
}

void Screen::rule(int y, std::uint8_t style)
{
    if (y < 0 || y >= height_) return;
    for (int x = 0; x < width_; ++x) cell(x, y) = Cell{std::wstring(1, L'\x2500'), style, false};
}

std::wstring renderFrame(const Screen &screen)
{
    std::wstring frame;
    frame.reserve(static_cast<std::size_t>(screen.width() * screen.height() * 3 / 2 + 64));
    frame.append(L"\x1b[H"); // home
    std::uint8_t activeStyle = StyleDefault;
    bool styleInitialized = false; // force an initial reset SGR
    const auto applyStyle = [&](std::uint8_t style) {
        if (styleInitialized && style == activeStyle) return;
        frame.append(L"\x1b[0");
        if (style & StyleBold) frame.append(L";1");
        if (style & StyleDim) frame.append(L";2");
        if (style & StyleGreen) frame.append(L";32");
        if (style & StyleYellow) frame.append(L";33");
        if (style & StyleRed) frame.append(L";31");
        frame.append(L"m");
        activeStyle = style;
        styleInitialized = true;
    };
    for (int y = 0; y < screen.height(); ++y) {
        if (y > 0) frame.push_back(L'\n');
        for (int x = 0; x < screen.width(); ++x) {
            const Cell &current = screen.cell(x, y);
            if (current.continuation) continue; // the wide glyph already covers it
            applyStyle(current.style);
            if (current.glyph.empty()) frame.push_back(L' ');
            else frame.append(current.glyph);
        }
        applyStyle(StyleDefault);
        frame.append(L"\x1b[K"); // clear to end of line
    }
    return frame;
}

} // namespace cpg::tui

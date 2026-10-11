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
    bool previousHigh = false;
    for (std::size_t index = 0; index < value.size(); ++index) {
        const wchar_t unit = value[index];
        const int unitWidth = codeUnitWidth(unit, previousHigh);
        previousHigh = (unit >= 0xd800 && unit <= 0xdbff) && !previousHigh;
        if (column + unitWidth > width_) break;
        if (column >= 0) {
            cell(column, y) = Cell{unit, style};
            for (int fill = 1; fill < unitWidth; ++fill)
                cell(column + fill, y) = Cell{L' ', style};
        }
        column += unitWidth;
        used += unitWidth;
    }
    return used;
}

void Screen::rule(int y, std::uint8_t style)
{
    if (y < 0 || y >= height_) return;
    for (int x = 0; x < width_; ++x) cell(x, y) = Cell{L'\x2500', style};
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
        int x = 0;
        while (x < screen.width()) {
            const Cell &current = screen.cell(x, y);
            applyStyle(current.style);
            frame.push_back(current.glyph);
            int run = 1;
            while (x + run < screen.width()
                   && screen.cell(x + run, y).style == current.style) {
                frame.push_back(screen.cell(x + run, y).glyph);
                ++run;
            }
            x += run;
        }
        applyStyle(StyleDefault);
        frame.append(L"\x1b[K"); // clear to end of line
    }
    return frame;
}

} // namespace cpg::tui

#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace cpg::tui {

// Style bits mapped onto standard 16-color SGR sequences; colors never carry
// meaning on their own — every state is also spelled out in text.
enum Style : std::uint8_t {
    StyleDefault = 0,
    StyleBold = 1u << 0,
    StyleDim = 1u << 1,
    StyleGreen = 1u << 2,
    StyleYellow = 1u << 3,
    StyleRed = 1u << 4,
};

struct Cell {
    // Full glyph of the leading cell (an ASCII char, one wide CJK char or a
    // two-unit surrogate pair, optionally followed by combining marks); empty
    // for the continuation cells of a wide glyph.
    std::wstring glyph = L" ";
    std::uint8_t style = StyleDefault;
    bool continuation = false;
};

// Off-screen frame buffer. All composition happens here; the renderer turns a
// finished frame into one console write. Wide glyphs occupy their lead cell
// and mark the following cell(s) as continuation: the renderer emits the wide
// glyph once and skips continuation cells, so a row's display width always
// equals Screen::width().
class Screen {
public:
    Screen() = default;
    void resize(int width, int height);
    int width() const { return width_; }
    int height() const { return height_; }
    bool valid() const { return width_ > 0 && height_ > 0; }

    void clear();
    // Writes a single line of text starting at (x, y), clipped to the frame;
    // returns the display columns consumed.
    int text(int x, int y, std::wstring_view value, std::uint8_t style = StyleDefault);
    // Draws a horizontal rule of box-drawing characters.
    void rule(int y, std::uint8_t style = StyleDim);
    Cell &cell(int x, int y);
    const Cell &cell(int x, int y) const;

private:
    int width_ = 0;
    int height_ = 0;
    std::vector<Cell> cells_;
};

// Serializes a frame as one VT byte stream: home cursor, per-line runs with
// SGR changes and clear-to-end-of-line, hidden cursor assumed.
std::wstring renderFrame(const Screen &screen);

} // namespace cpg::tui

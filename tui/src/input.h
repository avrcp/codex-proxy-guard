#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <vector>

namespace cpg::tui {

// Normalized key event, independent of the console record spelling.
struct Key {
    wchar_t unicode = 0; // 0 for non-character keys
    WORD virtualKey = 0;
    bool control = false;
    bool alt = false;
    bool shift = false;
};

struct InputBatch {
    std::vector<Key> keys;
    bool resized = false;
};

// Filters a raw console record batch into normalized events (pure; exposed
// for unit tests over synthetic INPUT_RECORD arrays).
InputBatch normalizeRecords(const INPUT_RECORD *records, DWORD count);

// Reads one batch from the console input buffer (blocking with the given
// timeout in milliseconds; 0 polls).
bool readInputBatch(HANDLE input, InputBatch &out, DWORD waitMilliseconds);

} // namespace cpg::tui

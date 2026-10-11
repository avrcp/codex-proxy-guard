#include "input.h"

namespace cpg::tui {

InputBatch normalizeRecords(const INPUT_RECORD *records, DWORD count)
{
    InputBatch batch;
    for (DWORD index = 0; index < count; ++index) {
        const INPUT_RECORD &record = records[index];
        if (record.EventType == WINDOW_BUFFER_SIZE_EVENT) {
            batch.resized = true;
            continue;
        }
        if (record.EventType != KEY_EVENT) continue;
        const KEY_EVENT_RECORD &key = record.Event.KeyEvent;
        if (!key.bKeyDown) continue;
        const DWORD modifiers = key.dwControlKeyState;
        batch.keys.push_back(Key{
            key.uChar.UnicodeChar,
            key.wVirtualKeyCode,
            (modifiers & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED)) != 0,
            (modifiers & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED)) != 0,
            (modifiers & SHIFT_PRESSED) != 0});
    }
    return batch;
}

bool readInputBatch(HANDLE input, InputBatch &out, DWORD waitMilliseconds)
{
    if (WaitForSingleObject(input, waitMilliseconds) != WAIT_OBJECT_0) return false;
    INPUT_RECORD records[64];
    DWORD count = 0;
    if (!ReadConsoleInputW(input, records, static_cast<DWORD>(std::size(records)), &count))
        return false;
    out = normalizeRecords(records, count);
    return true;
}

} // namespace cpg::tui

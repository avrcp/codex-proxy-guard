#include "clipboard.h"

#include <cstring>

namespace cpg::tui {

bool copyDiagnosticText(const std::wstring &text, ClipboardApi &api, void *owner)
{
    if (!api.open(owner)) return false;
    struct CloseScope {
        ClipboardApi &api;
        ~CloseScope() { api.close(); }
    } closer{api};
    if (!api.empty()) return false;
    const std::size_t bytes = (text.size() + 1) * sizeof(wchar_t);
    void *memory = api.allocate(bytes);
    if (!memory) return false;
    void *locked = api.lock(memory);
    if (!locked) {
        api.freeMemory(memory);
        return false;
    }
    std::memcpy(locked, text.c_str(), bytes);
    if (!api.unlock(locked)) {
        api.freeMemory(memory);
        return false;
    }
    if (!api.setText(memory)) {
        // The system refused the handle, so ownership never transferred.
        api.freeMemory(memory);
        return false;
    }
    return true; // the clipboard owns the memory now; never free it
}

bool copyDiagnosticToClipboard(const std::wstring &text)
{
    struct WindowScope {
        HWND window = nullptr;
        bool owned = false;
        ~WindowScope()
        {
            if (owned && window) DestroyWindow(window);
        }
    } owner;
    owner.window = GetConsoleWindow();
    if (!owner.window) {
        // No console window (detached pipes, service-hosted sessions): a
        // message-only window still gives the clipboard a real owner.
        owner.window = CreateWindowExW(0, L"STATIC", L"", WS_POPUP, 0, 0, 0, 0,
                                       HWND_MESSAGE, nullptr, GetModuleHandleW(nullptr),
                                       nullptr);
        owner.owned = true;
        if (!owner.window) return false;
    }
    Win32ClipboardApi api;
    return copyDiagnosticText(text, api, owner.window);
}

} // namespace cpg::tui

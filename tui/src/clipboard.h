#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <cstddef>
#include <string>

namespace cpg::tui {

// Thin, injectable Win32 clipboard adapter so the ownership rules are
// unit-testable without a real clipboard: a non-null owner window is
// required (EmptyClipboard sets the owner to the OpenClipboard window, and
// SetClipboardData fails with a NULL owner), every return value is checked,
// and the HGLOBAL is freed only when SetClipboardData refuses it.
class ClipboardApi {
public:
    virtual ~ClipboardApi() = default;
    virtual bool open(void *owner) = 0;              // OpenClipboard
    virtual bool empty() = 0;                        // EmptyClipboard
    virtual void *allocate(std::size_t bytes) = 0;   // GlobalAlloc(GMEM_MOVEABLE)
    virtual void *lock(void *memory) = 0;            // GlobalLock
    virtual bool unlock(void *memory) = 0;           // GlobalUnlock (NO_ERROR is success)
    virtual bool setText(void *memory) = 0;          // SetClipboardData(CF_UNICODETEXT)
    virtual void freeMemory(void *memory) = 0;       // GlobalFree
    virtual void close() = 0;                        // CloseClipboard
};

// Win32-backed production adapter.
class Win32ClipboardApi : public ClipboardApi {
public:
    bool open(void *owner) override { return OpenClipboard(static_cast<HWND>(owner)) != 0; }
    bool empty() override { return EmptyClipboard() != 0; }
    void *allocate(std::size_t bytes) override { return GlobalAlloc(GMEM_MOVEABLE, bytes); }
    void *lock(void *memory) override { return GlobalLock(memory); }
    bool unlock(void *memory) override
    {
        if (GlobalUnlock(memory)) return true;
        return GetLastError() == NO_ERROR;
    }
    bool setText(void *memory) override
    {
        return SetClipboardData(CF_UNICODETEXT, memory) != nullptr;
    }
    void freeMemory(void *memory) override { GlobalFree(memory); }
    void close() override { CloseClipboard(); }
};

// Copies UTF-16 text through the adapter. Returns false on any failure; the
// HGLOBAL is freed unless SetClipboardData accepted it (the system owns it
// from then on and it must not be freed here).
bool copyDiagnosticText(const std::wstring &text, ClipboardApi &api, void *owner);

// Production entry: owns the clipboard through the console window, or a
// short-lived message-only window when no console window exists.
bool copyDiagnosticToClipboard(const std::wstring &text);

} // namespace cpg::tui

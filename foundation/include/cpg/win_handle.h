#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

namespace cpg {

// Owning RAII wrapper for kernel HANDLE values. A handle is "owned" when it is
// neither nullptr nor INVALID_HANDLE_VALUE; both sentinels are treated as
// empty and are never closed.
class UniqueHandle {
public:
    UniqueHandle() = default;
    explicit UniqueHandle(HANDLE value) : value_(value) {}
    ~UniqueHandle() { reset(); }
    UniqueHandle(UniqueHandle &&other) noexcept : value_(other.release()) {}
    UniqueHandle &operator=(UniqueHandle &&other) noexcept {
        if (this != &other) { reset(other.release()); }
        return *this;
    }
    UniqueHandle(const UniqueHandle &) = delete;
    UniqueHandle &operator=(const UniqueHandle &) = delete;

    HANDLE get() const noexcept { return value_; }
    bool valid() const noexcept { return value_ != nullptr && value_ != INVALID_HANDLE_VALUE; }
    explicit operator bool() const noexcept { return valid(); }
    HANDLE release() noexcept {
        const HANDLE value = value_;
        value_ = nullptr;
        return value;
    }
    void reset(HANDLE value = nullptr) noexcept {
        if (valid()) CloseHandle(value_);
        value_ = value;
    }

private:
    HANDLE value_ = nullptr;
};

} // namespace cpg

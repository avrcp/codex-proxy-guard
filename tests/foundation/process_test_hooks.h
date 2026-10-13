#pragma once

#include <windows.h>
#include <functional>

// Compiled only into foundation_tests' private copy of process.cpp. No
// production configuration, command, or environment variable enables these.
namespace cpg::testing {
struct ProcessHooks {
    std::function<void()> beforeCreate;
    std::function<void(HANDLE)> afterCreate;
    std::function<void(unsigned)> beforeThread;
    std::function<void(bool, HANDLE, DWORD &)> afterRead;
    std::function<BOOL(HANDLE, const void *, DWORD, DWORD *)> write;
    std::function<void()> afterWrite;
    std::function<void()> beforeJoin;
    std::function<void()> beforeVerdict;
};
extern thread_local ProcessHooks *processHooks;
struct ProcessHookScope {
    ProcessHooks *previous;
    explicit ProcessHookScope(ProcessHooks &hooks) : previous(processHooks) { processHooks = &hooks; }
    ~ProcessHookScope() { processHooks = previous; }
};
}

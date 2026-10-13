#pragma once
#include "tui_state.h"
#include <functional>
namespace cpg::tui {
// Production and tests use this same action executor. Each key synchronizes
// busy before another key from the same input batch can be dispatched.
class TuiController {
public:
    TuiController(EngineSession &session, TuiState &state,
        std::function<bool(const std::wstring &)> copyDiagnostic = {})
        : session_(session), state_(state), copyDiagnostic_(std::move(copyDiagnostic)) { synchronize(); }
    void handleKey(const Key &key);
    void refresh();
    void poll();
    void cancel();
    void requestClose();
    bool shouldExit() const { return exitRequested_ && !session_.busy(); }
    bool exitRequested() const { return exitRequested_; }
    bool busy() const { return session_.busy(); }
private:
    void synchronize();
    EngineSession &session_;
    TuiState &state_;
    std::function<bool(const std::wstring &)> copyDiagnostic_;
    bool exitRequested_ = false;
    std::int64_t refreshedLaunch_ = -1;
};
} // namespace cpg::tui

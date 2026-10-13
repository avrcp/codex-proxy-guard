#pragma once
#include "terminal_session.h"
#include "cpg/console_control.h"

namespace cpg::tui::detail {
// Member declaration order is the teardown contract: restore the terminal
// before acknowledging console close. Engine/session owners live inside this
// scope and settle before either member is destroyed.
struct InteractiveTerminalScope {
    ConsoleControlScope controls;
    TerminalSession terminal;
    InteractiveTerminalScope() = default;
    explicit InteractiveTerminalScope(TerminalApi &api) : terminal(api) {}
};
} // namespace cpg::tui::detail

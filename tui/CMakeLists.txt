# Win32 VT TUI plus the plain-console fallback over the shared engine session.
add_library(guard_tui STATIC
    src/cli_console.cpp
    src/clipboard.cpp
    src/display_width.cpp
    src/input.cpp
    src/plain_input.cpp
    src/screen.cpp
    src/terminal_session.cpp
    src/tui_app.cpp
    src/tui_controller.cpp
    src/tui_state.cpp)
target_include_directories(guard_tui PUBLIC src)
target_link_libraries(guard_tui PUBLIC guard_engine PRIVATE cpg_build_options)
target_compile_definitions(guard_tui PUBLIC
    NOMINMAX WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0A00)

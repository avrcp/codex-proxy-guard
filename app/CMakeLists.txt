# The single production executable: one Console-subsystem file, several
# process roles selected by structured argument classification before any
# role is entered (TUI, plain console, CLI, activation worker).
add_executable(CodexProxyGuard main.cpp version.rc "${CMAKE_BINARY_DIR}/backend/generated/licenses.rc")
target_link_libraries(CodexProxyGuard PRIVATE guard_engine guard_tui guard_foundation shell32 cpg_build_options)
if(MSVC)
    target_link_options(CodexProxyGuard PRIVATE
        "/MANIFESTUAC:level='asInvoker' uiAccess='false'"
        /guard:cf
        /DYNAMICBASE /NXCOMPAT /HIGHENTROPYVA /CETCOMPAT /INCREMENTAL:NO)
endif()

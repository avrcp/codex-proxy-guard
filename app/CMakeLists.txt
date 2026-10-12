add_library(guard_startup STATIC startup_mode.cpp)
target_include_directories(guard_startup PUBLIC ${CMAKE_CURRENT_SOURCE_DIR} ${CMAKE_SOURCE_DIR}/tui/src)
target_link_libraries(guard_startup PUBLIC guard_foundation PRIVATE cpg_build_options)

# The single production executable: one Console-subsystem file, several
# process roles selected by structured argument classification before any
# role is entered (TUI, plain console, CLI, activation worker).
add_executable(CodexProxyGuard main.cpp version.rc "${CMAKE_BINARY_DIR}/backend/generated/licenses.rc")
target_link_libraries(CodexProxyGuard PRIVATE guard_startup guard_engine guard_tui guard_foundation shell32 cpg_build_options)
if(MSVC)
    target_link_options(CodexProxyGuard PRIVATE
        "/MANIFESTUAC:level='asInvoker' uiAccess='false'"
        /guard:cf
        /DYNAMICBASE /NXCOMPAT /HIGHENTROPYVA /CETCOMPAT /INCREMENTAL:NO)
endif()

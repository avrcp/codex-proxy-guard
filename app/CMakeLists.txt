add_library(guard_startup STATIC startup_mode.cpp)
target_include_directories(guard_startup PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(guard_startup PUBLIC guard_foundation)
if(MSVC)
    target_compile_options(guard_startup PRIVATE /W4 /WX /utf-8 /permissive-)
endif()

# The single production executable: one file, several process roles selected
# by structured argument classification before any role is entered. The legacy
# Qt GUI adapter is linked only while the regression build is enabled; the
# no-Qt build uses the console subsystem for the interactive console role.
if(CPG_QT_GUI)
    add_executable(CodexProxyGuard WIN32 main.cpp "${CMAKE_BINARY_DIR}/backend/generated/licenses.rc")
    target_link_libraries(CodexProxyGuard PRIVATE guard_startup guard_gui guard_engine guard_foundation shell32)
    target_compile_definitions(CodexProxyGuard PRIVATE CPG_ENABLE_QT_GUI=1)
else()
    add_executable(CodexProxyGuard main.cpp "${CMAKE_BINARY_DIR}/backend/generated/licenses.rc")
    target_link_libraries(CodexProxyGuard PRIVATE guard_startup guard_engine guard_foundation shell32)
endif()
if(MSVC)
    target_compile_options(CodexProxyGuard PRIVATE /W4 /WX /utf-8 /permissive-)
    target_link_options(CodexProxyGuard PRIVATE "/MANIFESTUAC:level='asInvoker' uiAccess='false'")
endif()

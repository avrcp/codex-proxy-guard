add_library(guard_startup STATIC startup_mode.cpp)
target_include_directories(guard_startup PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(guard_startup PUBLIC Qt6::Core)
if(MSVC)
    target_compile_options(guard_startup PRIVATE /W4 /WX /utf-8 /permissive-)
endif()

# The single production executable: one file, several process roles selected
# by structured argument classification before any Application object exists.
qt_add_executable(CodexProxyGuard WIN32 main.cpp)
target_link_libraries(CodexProxyGuard PRIVATE guard_startup guard_gui guard_engine shell32)
qt_add_resources(CodexProxyGuard "notices"
    PREFIX "/licenses"
    BASE "${CMAKE_SOURCE_DIR}"
    FILES "${CMAKE_SOURCE_DIR}/LICENSE" "${CMAKE_SOURCE_DIR}/THIRD_PARTY_NOTICES.md")
if(MSVC)
    target_compile_options(CodexProxyGuard PRIVATE /W4 /WX /utf-8 /permissive-)
    target_link_options(CodexProxyGuard PRIVATE "/MANIFESTUAC:level='asInvoker' uiAccess='false'")
endif()

if(BUILD_TESTING)
    find_package(Qt6 6.8 REQUIRED COMPONENTS Test)
    qt_add_executable(startup_mode_tests tests/test_startup_mode.cpp)
    target_link_libraries(startup_mode_tests PRIVATE guard_startup Qt6::Test)
    if(MSVC)
        target_compile_options(startup_mode_tests PRIVATE /W4 /WX /utf-8 /permissive-)
    endif()
    add_test(NAME app_startup_mode COMMAND startup_mode_tests)
    set_tests_properties(app_startup_mode PROPERTIES TIMEOUT 60)
endif()

add_executable(tui_tests tui_tests.cpp)
target_include_directories(tui_tests PRIVATE
    ${CMAKE_SOURCE_DIR}/tests/third_party/doctest
    ${CMAKE_SOURCE_DIR}/tests/support
    ${CMAKE_SOURCE_DIR}/tui/src
    ${CMAKE_SOURCE_DIR}/backend/src)
target_link_libraries(tui_tests PRIVATE guard_tui guard_engine cpg_build_options)
target_compile_definitions(tui_tests PRIVATE
    NOMINMAX WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0A00 _CRT_SECURE_NO_WARNINGS)
add_test(NAME tui_tests COMMAND tui_tests)
set_tests_properties(tui_tests PROPERTIES TIMEOUT 120)

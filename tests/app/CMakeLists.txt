add_executable(command_line_tests command_line_tests.cpp)
target_include_directories(command_line_tests PRIVATE
    ${CMAKE_SOURCE_DIR}/tests/third_party/doctest
    ${CMAKE_SOURCE_DIR}/tests/support
    ${CMAKE_SOURCE_DIR}/app)
target_link_libraries(command_line_tests PRIVATE guard_engine cpg_build_options)
add_test(NAME app_command_line COMMAND command_line_tests)
set_tests_properties(app_command_line PROPERTIES TIMEOUT 60)

add_executable(startup_mode_tests startup_mode_tests.cpp)
target_include_directories(startup_mode_tests PRIVATE
    ${CMAKE_SOURCE_DIR}/tests/third_party/doctest
    ${CMAKE_SOURCE_DIR}/tests/support
    ${CMAKE_SOURCE_DIR}/app)
target_link_libraries(startup_mode_tests PRIVATE guard_startup cpg_build_options)
add_test(NAME app_startup_mode COMMAND startup_mode_tests)
set_tests_properties(app_startup_mode PROPERTIES TIMEOUT 60)

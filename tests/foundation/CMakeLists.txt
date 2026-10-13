add_executable(foundation_tests foundation_tests.cpp
    ${CMAKE_SOURCE_DIR}/foundation/src/process.cpp)
# The directly linked test copy replaces the archive's process object. The
# production library and executable never contain the fault-injection seams.
target_compile_definitions(foundation_tests PRIVATE CPG_PROCESS_TESTING)
target_link_libraries(foundation_tests PRIVATE guard_foundation cpg_build_options)
target_include_directories(foundation_tests PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
    ${CMAKE_SOURCE_DIR}/tests/third_party/doctest)
add_test(NAME foundation_tests COMMAND foundation_tests)
set_tests_properties(foundation_tests PROPERTIES TIMEOUT 120)

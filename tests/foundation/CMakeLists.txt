add_executable(foundation_tests foundation_tests.cpp)
target_link_libraries(foundation_tests PRIVATE guard_foundation)
target_include_directories(foundation_tests PRIVATE
    ${CMAKE_SOURCE_DIR}/tests/third_party/doctest)
if(MSVC)
    target_compile_options(foundation_tests PRIVATE /W4 /WX /utf-8 /permissive-)
endif()
add_test(NAME foundation_tests COMMAND foundation_tests)
set_tests_properties(foundation_tests PROPERTIES TIMEOUT 120)

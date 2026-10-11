# doctest engine suites; they spawn the unified production executable, so the
# app target must already exist (registered from the top-level file).
foreach(name engine_core engine_platform engine_launch engine_bridge)
    add_executable(${name}_suite ${name}_tests.cpp)
    target_include_directories(${name}_suite PRIVATE
        ${CMAKE_SOURCE_DIR}/tests/third_party/doctest
        ${CMAKE_SOURCE_DIR}/tests/support)
    target_link_libraries(${name}_suite PRIVATE guard_engine)
    target_compile_options(${name}_suite PRIVATE /W4 /WX /utf-8 /permissive-)
    target_compile_definitions(${name}_suite PRIVATE
        TARGET_ENGINE_PATH="$<TARGET_FILE:CodexProxyGuard>"
        NOMINMAX WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0A00
        _CRT_SECURE_NO_WARNINGS)
    add_test(NAME backend_${name} COMMAND ${name}_suite)
    set_tests_properties(backend_${name} PROPERTIES TIMEOUT 120)
endforeach()

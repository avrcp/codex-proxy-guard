add_library(guard_engine STATIC src/core.cpp src/platform.cpp src/launch.cpp src/bridge.cpp)
target_include_directories(guard_engine PUBLIC src)
target_link_libraries(guard_engine PUBLIC Qt6::Core PRIVATE ws2_32 shell32 ole32 advapi32 ktmw32)
target_compile_definitions(guard_engine PUBLIC
    NOMINMAX WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0A00
    CPG_PRODUCT_VERSION="${CPG_PRODUCT_VERSION}"
    CPG_BUILD_COMMIT="${CPG_BUILD_COMMIT}"
    CPG_BUILD_DIRTY="${CPG_BUILD_DIRTY}")
target_compile_options(guard_engine PRIVATE /W4 /WX /utf-8 /permissive-)
qt_add_resources(guard_engine "discovery" PREFIX "/" BASE "${CMAKE_SOURCE_DIR}"
    FILES "${CMAKE_SOURCE_DIR}/resources/appx-discovery.ps1")
add_executable(codex-proxy-guard src/main.cpp)
target_link_libraries(codex-proxy-guard PRIVATE guard_engine)
target_compile_options(codex-proxy-guard PRIVATE /W4 /WX /utf-8 /permissive-)
target_link_options(codex-proxy-guard PRIVATE "/MANIFESTUAC:level='asInvoker' uiAccess='false'")
if(BUILD_TESTING)
    foreach(name core platform launch bridge)
        qt_add_executable(test_${name} tests/test_${name}.cpp)
        target_link_libraries(test_${name} PRIVATE guard_engine Qt6::Test)
        target_compile_options(test_${name} PRIVATE /W4 /WX /utf-8 /permissive-)
        target_compile_definitions(test_${name} PRIVATE TARGET_ENGINE_PATH="$<TARGET_FILE:codex-proxy-guard>")
        add_dependencies(test_${name} codex-proxy-guard)
        add_test(NAME backend_${name} COMMAND test_${name})
        set_tests_properties(backend_${name} PROPERTIES TIMEOUT 90)
    endforeach()
endif()

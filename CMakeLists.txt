cmake_minimum_required(VERSION 3.25)
# CMP0091 NEW (implied by >= 3.15) makes this the authoritative CRT selection
# for every target; it must be set before the first project()/enable_language.
# The single-EXE product always links the static CRT.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
project(CodexProxyGuard VERSION 0.7.0 LANGUAGES CXX RC)
if(NOT WIN32)
    message(FATAL_ERROR "Codex Proxy Guard requires Windows.")
endif()
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CPG_PRODUCT_VERSION "0.7.0-rc.1" CACHE STRING "Product version")
set(CPG_BUILD_COMMIT "unknown" CACHE STRING "Source commit")
set(CPG_BUILD_DIRTY "true" CACHE STRING "Source dirty flag")

# Shared MSVC hardening options for EVERY compiled object, production and
# tests alike: /sdl and /guard:cf must instrument the static libraries too,
# not just the final executable translation unit - CFG holes in library code
# defeat the point of the mitigation. Third-party headers (doctest, nlohmann,
# toml++) are already isolated with warning push/pop pragmas.
add_library(cpg_build_options INTERFACE)
if(MSVC)
    target_compile_options(cpg_build_options INTERFACE
        /W4 /WX /utf-8 /permissive- /sdl /guard:cf)
endif()

include(CTest)
add_subdirectory(foundation)
add_subdirectory(backend)
add_subdirectory(tui)
add_subdirectory(app)
if(BUILD_TESTING)
    find_package(Python3 REQUIRED COMPONENTS Interpreter)
    add_subdirectory(tests/foundation)
    add_subdirectory(tests/app)
    add_subdirectory(tests/tui)
    # Engine integration tests spawn the unified production executable, so they
    # are registered only after the app target exists (no CMake cycles).
    add_subdirectory(backend/tests)
    add_test(NAME verify_package_fixtures
        COMMAND Python3::Interpreter "${CMAKE_SOURCE_DIR}/scripts/tests/test_verify_package.py")
    set_tests_properties(verify_package_fixtures PROPERTIES TIMEOUT 120)
endif()

cmake_minimum_required(VERSION 3.25)
option(CPG_STATIC_QT "Require a statically built Qt SDK and static CRT (/MT)" OFF)
# CMP0091 NEW (implied by >= 3.15) makes this the authoritative CRT selection
# for every target; it must be set before the first project()/enable_language.
if(CPG_STATIC_QT)
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()
project(CodexProxyGuard VERSION 0.6.1 LANGUAGES CXX)
if(NOT WIN32)
    message(FATAL_ERROR "Codex Proxy Guard requires Windows.")
endif()
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
set(CMAKE_AUTORCC ON)
set(CPG_PRODUCT_VERSION "0.6.1-rc.1" CACHE STRING "Product version")
set(CPG_BUILD_COMMIT "unknown" CACHE STRING "Source commit")
set(CPG_BUILD_DIRTY "true" CACHE STRING "Source dirty flag")
find_package(Qt6 6.8 REQUIRED COMPONENTS Core Gui Widgets)
if(CPG_STATIC_QT)
    # The imported Qt targets must genuinely be static libraries from the
    # configured prefix; a dynamic SDK mixed with /MT is a configuration error.
    # QT_STATIC itself is defined by Qt's static package files and is never
    # set manually here.
    foreach(qt_component Core Gui Widgets)
        get_target_property(qt_type Qt6::${qt_component} TYPE)
        if(NOT qt_type STREQUAL "STATIC_LIBRARY")
            message(FATAL_ERROR "CPG_STATIC_QT requires static Qt libraries; Qt6::${qt_component} is ${qt_type}. Point CMAKE_PREFIX_PATH at the static SDK built by scripts/build-qt-static.ps1.")
        endif()
    endforeach()
endif()
include(CTest)
if(BUILD_TESTING)
    find_package(Qt6 6.8 REQUIRED COMPONENTS Test)
    find_package(Python3 REQUIRED COMPONENTS Interpreter)
    add_test(NAME verify_package_fixtures
        COMMAND Python3::Interpreter "${CMAKE_SOURCE_DIR}/scripts/tests/test_verify_package.py")
    set_tests_properties(verify_package_fixtures PROPERTIES TIMEOUT 120)
endif()
add_subdirectory(backend)
add_subdirectory(gui)
add_subdirectory(app)
# Backend integration tests spawn the unified production executable, so they
# are registered only after the app target exists (no CMake cycles).
if(BUILD_TESTING)
    add_subdirectory(backend/tests)
endif()

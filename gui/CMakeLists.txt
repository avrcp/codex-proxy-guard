cmake_minimum_required(VERSION 3.25)
project(CodexProxyGuardGui VERSION 0.6.1 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
find_package(Qt6 6.8 REQUIRED COMPONENTS Core Gui Widgets)
qt_standard_project_setup()

# The product version/cache provenance definitions live in the top-level
# CMakeLists only; this subdirectory inherits them (single source of truth).
add_library(guard_gui STATIC
    src/engine/engine_bridge.cpp src/engine/engine_bridge.h
    src/engine/protocol.cpp src/engine/protocol.h
    src/engine/snapshot.cpp src/engine/snapshot.h
    src/controller/launcher_controller.cpp src/controller/launcher_controller.h
    src/gui_entry.cpp src/gui_entry.h
    src/ui/main_window.cpp src/ui/main_window.h
    src/ui/proxy_settings_dialog.cpp src/ui/proxy_settings_dialog.h
    src/ui/theme.cpp src/ui/theme.h
    src/ui/widgets/info_row.cpp src/ui/widgets/info_row.h
)
target_include_directories(guard_gui PUBLIC src)
target_link_libraries(guard_gui PUBLIC Qt6::Core Qt6::Gui Qt6::Widgets)
target_compile_definitions(guard_gui PUBLIC
    CPG_PRODUCT_VERSION="${CPG_PRODUCT_VERSION}"
    CPG_BUILD_COMMIT="${CPG_BUILD_COMMIT}"
    CPG_BUILD_DIRTY="${CPG_BUILD_DIRTY}")
if(MSVC)
    target_compile_options(guard_gui PRIVATE /W4 /WX /utf-8 /permissive-)
endif()

include(CTest)
if(BUILD_TESTING)
    find_package(Qt6 6.8 REQUIRED COMPONENTS Test)
    add_executable(fake_engine tests/fake_engine.cpp)
    target_link_libraries(fake_engine PRIVATE Qt6::Core)
    foreach(test_name protocol_tests controller_tests engine_bridge_tests)
        qt_add_executable(${test_name} tests/${test_name}.cpp)
        target_link_libraries(${test_name} PRIVATE guard_gui Qt6::Test)
        if(MSVC)
            target_compile_options(${test_name} PRIVATE /W4 /WX /utf-8 /permissive-)
        endif()
        if(CPG_STATIC_QT)
            # The CTest environment uses QT_QPA_PLATFORM=offscreen. Static
            # executables only contain the plugins they explicitly import, so
            # test targets get the offscreen integration instead of the
            # production Windows set; production never links offscreen.
            if(NOT TARGET Qt6::QOffscreenIntegrationPlugin)
                message(FATAL_ERROR "Static Qt test plugin missing: Qt6::QOffscreenIntegrationPlugin")
            endif()
            qt_import_plugins(${test_name}
                NO_DEFAULT
                INCLUDE Qt6::QOffscreenIntegrationPlugin)
        endif()
        add_test(NAME ${test_name} COMMAND ${test_name})
        set_tests_properties(${test_name} PROPERTIES TIMEOUT 60 ENVIRONMENT "QT_QPA_PLATFORM=offscreen")
    endforeach()
    if(MSVC)
        target_compile_options(fake_engine PRIVATE /W4 /WX /utf-8 /permissive-)
    endif()
    add_dependencies(engine_bridge_tests fake_engine)
    add_dependencies(controller_tests fake_engine)
endif()

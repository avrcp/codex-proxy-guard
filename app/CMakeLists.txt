add_library(guard_startup STATIC startup_mode.cpp)
target_include_directories(guard_startup PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(guard_startup PUBLIC Qt6::Core)
if(MSVC)
    target_compile_options(guard_startup PRIVATE /W4 /WX /utf-8 /permissive-)
endif()

# The single production executable: one file, several process roles selected
# by structured argument classification before any Application object exists.
qt_add_executable(CodexProxyGuard WIN32 main.cpp)
target_link_libraries(CodexProxyGuard PRIVATE guard_startup guard_gui guard_engine shell32)
# Embed the complete offline notice set: project license, third-party notices
# and every Qt license text from the pinned SDK manifest directory. The
# headless `licenses` command streams these without GUI, config or network.
file(GLOB CPG_LICENSE_FILES CONFIGURE_DEPENDS
    "${CMAKE_SOURCE_DIR}/licenses/Qt/*.txt"
    "${CMAKE_SOURCE_DIR}/licenses/Qt/*.json")
file(GLOB_RECURSE CPG_THIRD_PARTY_NOTICE_FILES CONFIGURE_DEPENDS
    "${CMAKE_SOURCE_DIR}/licenses/Qt/third-party/*")
list(APPEND CPG_LICENSE_FILES ${CPG_THIRD_PARTY_NOTICE_FILES})
list(FILTER CPG_LICENSE_FILES EXCLUDE REGEX "sdk-6\.8\.3-msvc2022-x64\.json$")
if(NOT CPG_LICENSE_FILES)
    message(FATAL_ERROR "No license files found to embed")
endif()
qt_add_resources(CodexProxyGuard "notices"
    PREFIX "/licenses"
    BASE "${CMAKE_SOURCE_DIR}"
    FILES "${CMAKE_SOURCE_DIR}/LICENSE" "${CMAKE_SOURCE_DIR}/THIRD_PARTY_NOTICES.md")
qt_add_resources(CodexProxyGuard "qt_notices"
    PREFIX "/licenses"
    BASE "${CMAKE_SOURCE_DIR}/licenses"
    FILES ${CPG_LICENSE_FILES})
if(MSVC)
    target_compile_options(CodexProxyGuard PRIVATE /W4 /WX /utf-8 /permissive-)
    target_link_options(CodexProxyGuard PRIVATE "/MANIFESTUAC:level='asInvoker' uiAccess='false'")
endif()

if(CPG_STATIC_QT)
    # Explicit, verified static plugin set (manual section S4): the Windows QPA
    # integration, the image codecs the dynamic package deployed, and the
    # modern Windows style. NO_DEFAULT prevents silently linking anything the
    # SDK happens to build. Test executables keep their own offscreen set in
    # gui/CMakeLists.txt; production never links offscreen.
    foreach(required_plugin Qt6::QWindowsIntegrationPlugin Qt6::QGifPlugin
            Qt6::QICOPlugin Qt6::QJpegPlugin Qt6::QModernWindowsStylePlugin)
        if(NOT TARGET ${required_plugin})
            message(FATAL_ERROR "Static Qt plugin missing from the pinned SDK: ${required_plugin}")
        endif()
    endforeach()
    qt_import_plugins(CodexProxyGuard
        NO_DEFAULT
        INCLUDE Qt6::QWindowsIntegrationPlugin
                Qt6::QGifPlugin
                Qt6::QICOPlugin
                Qt6::QJpegPlugin
                Qt6::QModernWindowsStylePlugin)
endif()

if(BUILD_TESTING)
    find_package(Qt6 6.8 REQUIRED COMPONENTS Test)
    qt_add_executable(startup_mode_tests tests/test_startup_mode.cpp)
    target_link_libraries(startup_mode_tests PRIVATE guard_startup Qt6::Test)
    if(MSVC)
        target_compile_options(startup_mode_tests PRIVATE /W4 /WX /utf-8 /permissive-)
    endif()
    add_test(NAME app_startup_mode COMMAND startup_mode_tests)
    set_tests_properties(app_startup_mode PROPERTIES TIMEOUT 60)
endif()

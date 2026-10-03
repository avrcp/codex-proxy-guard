add_library(guard_engine STATIC src/core.cpp src/platform.cpp src/launch.cpp src/bridge.cpp src/cli.cpp)
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

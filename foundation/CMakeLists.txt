# STL/Win32 foundation primitives shared by the engine, TUI and app roles.
add_library(guard_foundation STATIC
    src/build_info.cpp
    src/cancellation.cpp
    src/error.cpp
    src/json.cpp
    src/path.cpp
    src/process.cpp
    src/text.cpp)
target_include_directories(guard_foundation
    PUBLIC
        ${CMAKE_CURRENT_SOURCE_DIR}/include
        ${CMAKE_SOURCE_DIR}/backend/third_party/json)
target_compile_definitions(guard_foundation PUBLIC
    NOMINMAX WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0A00
    CPG_PRODUCT_VERSION="${CPG_PRODUCT_VERSION}"
    CPG_BUILD_COMMIT="${CPG_BUILD_COMMIT}"
    CPG_BUILD_DIRTY="${CPG_BUILD_DIRTY}")
target_link_libraries(guard_foundation PUBLIC shell32 ole32)
if(MSVC)
    target_compile_options(guard_foundation PRIVATE /W4 /WX /utf-8 /permissive-)
endif()

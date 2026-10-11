# Qt-free engine: core, platform (native AppModel discovery), launch,
# bridge (interim regression role) and the CLI/console frontends over the
# foundation primitives.
file(MAKE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/generated")

# The embedded notice set: project license, third-party notices and (until the
# Qt assets are removed) the complete Qt license tree, sorted exactly like the
# previous resource iterator. Rendered as one RCDATA entry per file.
set(CPG_LICENSE_ROOTS LICENSE THIRD_PARTY_NOTICES.md)
file(GLOB CPG_QT_LICENSE_FILES CONFIGURE_DEPENDS
    "${CMAKE_SOURCE_DIR}/licenses/Qt/*.txt"
    "${CMAKE_SOURCE_DIR}/licenses/Qt/*.json")
file(GLOB_RECURSE CPG_QT_NOTICE_FILES CONFIGURE_DEPENDS
    "${CMAKE_SOURCE_DIR}/licenses/Qt/third-party/*")
list(FILTER CPG_QT_LICENSE_FILES EXCLUDE REGEX "sdk-6\.8\.3-msvc2022-x64\.json$")
set(CPG_LICENSE_ENTRIES "")
foreach(entry IN LISTS CPG_LICENSE_ROOTS)
    list(APPEND CPG_LICENSE_ENTRIES "${entry}|${CMAKE_SOURCE_DIR}/${entry}")
endforeach()
foreach(license_file IN LISTS CPG_QT_LICENSE_FILES CPG_QT_NOTICE_FILES)
    file(RELATIVE_PATH relative "${CMAKE_SOURCE_DIR}/licenses" "${license_file}")
    list(APPEND CPG_LICENSE_ENTRIES "Qt/${relative}|${license_file}")
endforeach()
list(SORT CPG_LICENSE_ENTRIES)
set(CPG_LICENSES_MANIFEST "// Generated notice manifest; do not edit.\n")
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}#include <string>\n#include <vector>\n")
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}namespace cpg { struct LicenseEntry { std::string name; int id; };\n")
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}inline const std::vector<LicenseEntry> &cpgLicensesManifest() {\n")
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}    static const std::vector<LicenseEntry> entries{\n")
set(CPG_LICENSES_RC "// Generated embedded notices; do not edit.\n")
set(CPG_LICENSE_ID 1)
foreach(entry IN LISTS CPG_LICENSE_ENTRIES)
    string(FIND "${entry}" "|" _separator)
    string(SUBSTRING "${entry}" 0 ${_separator} _name)
    math(EXPR _after "${_separator} + 1")
    string(SUBSTRING "${entry}" ${_after} -1 _path)
    string(REPLACE "\\" "/" _rc_path "${_path}")
    set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}        LicenseEntry{\"${_name}\", ${CPG_LICENSE_ID}},\n")
    set(CPG_LICENSES_RC "${CPG_LICENSES_RC}${CPG_LICENSE_ID} NOTICES \"${_rc_path}\"\n")
    math(EXPR CPG_LICENSE_ID "${CPG_LICENSE_ID} + 1")
endforeach()
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}    };\n    return entries;\n}\n}\n")
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/generated/licenses_manifest.h" "${CPG_LICENSES_MANIFEST}")
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/generated/licenses.rc" "${CPG_LICENSES_RC}")

add_library(guard_engine STATIC src/core.cpp src/platform.cpp src/package_discovery.cpp src/launch.cpp src/session.cpp src/bridge.cpp src/cli.cpp)
target_include_directories(guard_engine PUBLIC src "${CMAKE_CURRENT_BINARY_DIR}/generated")
target_link_libraries(guard_engine PUBLIC guard_foundation PRIVATE ws2_32 ktmw32 shlwapi)
target_compile_definitions(guard_engine PUBLIC
    NOMINMAX WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0A00
    _CRT_SECURE_NO_WARNINGS)
target_compile_options(guard_engine PRIVATE /W4 /WX /utf-8 /permissive-)

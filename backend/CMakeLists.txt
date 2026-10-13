# Qt-free engine: core, platform (native AppModel discovery), launch,
# typed session and the CLI frontends over the
# foundation primitives.
file(MAKE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/generated")

# The embedded notice set: project license, third-party notices and the
# vendored third-party licenses (toml++, nlohmann/json), sorted; one RCDATA
# entry per file.
set(CPG_LICENSE_ENTRIES
    "LICENSE|${CMAKE_SOURCE_DIR}/LICENSE"
    "THIRD_PARTY_NOTICES.md|${CMAKE_SOURCE_DIR}/THIRD_PARTY_NOTICES.md"
    "toml++/LICENSE|${CMAKE_SOURCE_DIR}/backend/third_party/toml++/LICENSE"
    "json/LICENSE.MIT|${CMAKE_SOURCE_DIR}/backend/third_party/json/LICENSE.MIT")
list(SORT CPG_LICENSE_ENTRIES)
set(CPG_LICENSES_MANIFEST "// Generated notice manifest; do not edit.
")
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}#include <string>
#include <vector>
")
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}namespace cpg { struct LicenseEntry { std::string name; int id; };
")
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}inline const std::vector<LicenseEntry> &cpgLicensesManifest() {
")
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}    static const std::vector<LicenseEntry> entries{
")
set(CPG_LICENSES_RC "// Generated embedded notices; do not edit.
")
set(CPG_LICENSE_ID 1)
foreach(entry IN LISTS CPG_LICENSE_ENTRIES)
    string(FIND "${entry}" "|" _separator)
    string(SUBSTRING "${entry}" 0 ${_separator} _name)
    math(EXPR _after "${_separator} + 1")
    string(SUBSTRING "${entry}" ${_after} -1 _path)
    string(REPLACE "\\" "/" _rc_path "${_path}")
    set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}        LicenseEntry{\"${_name}\", ${CPG_LICENSE_ID}},
")
    set(CPG_LICENSES_RC "${CPG_LICENSES_RC}${CPG_LICENSE_ID} NOTICES \"${_rc_path}\"
")
    math(EXPR CPG_LICENSE_ID "${CPG_LICENSE_ID} + 1")
endforeach()
set(CPG_LICENSES_MANIFEST "${CPG_LICENSES_MANIFEST}    };
    return entries;
}
}
")
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/generated/licenses_manifest.h" "${CPG_LICENSES_MANIFEST}")
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/generated/licenses.rc" "${CPG_LICENSES_RC}")

add_library(guard_engine STATIC src/core.cpp src/platform.cpp src/package_discovery.cpp src/launch.cpp src/session.cpp src/command_line.cpp src/cli.cpp)
target_include_directories(guard_engine PUBLIC src "${CMAKE_CURRENT_BINARY_DIR}/generated")
target_link_libraries(guard_engine PUBLIC guard_foundation PRIVATE ws2_32 ktmw32 shlwapi cpg_build_options)
target_compile_definitions(guard_engine PUBLIC
    NOMINMAX WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0A00
    _CRT_SECURE_NO_WARNINGS)

# Preserve the compiler's actual /showIncludes bytes. CMake's AUTO decoding
# can double-decode UTF-8 output on a host with a different ANSI code page,
# leaving Ninja unable to recognize header dependencies. The VS 18 toolchain
# ignores VSLANG and always emits the localized (e.g. GBK Chinese) prefix, so
# the captured value is intentionally raw non-ASCII bytes.
#
# The prefix MUST be stored as a cache variable: CMake 4.3's Ninja generator
# silently skips writing CMakeFiles/rules.ninja entirely when the same
# bytes are assigned as a plain directory-scope variable, leaving build.ninja
# pointing at a file that does not exist. The cache path also survives the
# re-runs of this probe with an unchanged value.
if(MSVC AND CMAKE_GENERATOR MATCHES "Ninja")
    set(_cpg_probe "${CMAKE_BINARY_DIR}/CMakeFiles/CpgIncludes")
    file(MAKE_DIRECTORY "${_cpg_probe}")
    file(WRITE "${_cpg_probe}/cpg_deps_header.h" "#pragma once\n")
    file(WRITE "${_cpg_probe}/main.cpp" "#include \"cpg_deps_header.h\"\n")
    execute_process(COMMAND "${CMAKE_CXX_COMPILER}" /nologo /showIncludes /c main.cpp
        WORKING_DIRECTORY "${_cpg_probe}" TIMEOUT 15
        OUTPUT_VARIABLE _cpg_output ERROR_VARIABLE _cpg_error
        RESULT_VARIABLE _cpg_result ENCODING NONE)
    if(NOT _cpg_result EQUAL 0 OR NOT "${_cpg_output}" MATCHES "(^|\n)([^\r\n]+:[ \t]+)([A-Za-z]:[/\\])")
        message(FATAL_ERROR "Cannot determine the MSVC header dependency prefix: ${_cpg_error}")
    endif()
    set(CMAKE_CL_SHOWINCLUDES_PREFIX "${CMAKE_MATCH_2}"
        CACHE STRING "Exact /showIncludes prefix bytes emitted by this cl.exe" FORCE)
    set(CMAKE_CXX_CL_SHOWINCLUDES_PREFIX "${CMAKE_MATCH_2}"
        CACHE STRING "Exact /showIncludes prefix bytes emitted by this cl.exe" FORCE)
endif()

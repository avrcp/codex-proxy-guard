# Preserve the compiler's actual /showIncludes bytes. CMake's AUTO decoding
# can double-decode UTF-8 output on a host with a different ANSI code page,
# leaving Ninja unable to recognize header dependencies.
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
    set(CMAKE_CL_SHOWINCLUDES_PREFIX "${CMAKE_MATCH_2}")
    set(CMAKE_CXX_CL_SHOWINCLUDES_PREFIX "${CMAKE_MATCH_2}")
endif()

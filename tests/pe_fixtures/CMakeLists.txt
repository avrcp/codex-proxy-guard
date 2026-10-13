# Toolchain-grounded PE fixtures for the verifier's CET gate: the same MSVC
# linker either marks the image CET-compatible (/CETCOMPAT) or explicitly
# does not (/CETCOMPAT:NO), so where the marker lands is decided by the
# toolchain, not by this repository's python models. Only the linked files
# are analyzed; nothing is executed.
add_executable(cpg_pe_fixture_cet cet_fixture.cpp)
target_link_libraries(cpg_pe_fixture_cet PRIVATE cpg_build_options)
target_link_options(cpg_pe_fixture_cet PRIVATE /guard:cf /CETCOMPAT /INCREMENTAL:NO)

add_executable(cpg_pe_fixture_nocet cet_fixture.cpp)
target_link_libraries(cpg_pe_fixture_nocet PRIVATE cpg_build_options)
target_link_options(cpg_pe_fixture_nocet PRIVATE /guard:cf /CETCOMPAT:NO /INCREMENTAL:NO)

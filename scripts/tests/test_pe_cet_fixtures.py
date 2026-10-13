"""Cross-check the verifier's CET evidence against the real MSVC toolchain.

The two fixture executables are linked from one source with only
/CETCOMPAT vs /CETCOMPAT:NO differing, so the linker - not this test -
decides where the extended-characteristics record lands. The production
executable (linked with /CETCOMPAT by app/CMakeLists.txt) is the positive
control that the release gate's expectations match the shipped image.
Nothing here is executed; only the files are parsed.
"""
import argparse
import pathlib
import runpy
import sys

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
VERIFIER = REPOSITORY / 'scripts' / 'verify-package.py'


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--cet', required=True,
                        help='fixture executable linked with /CETCOMPAT')
    parser.add_argument('--no-cet', required=True,
                        help='fixture executable linked with /CETCOMPAT:NO')
    parser.add_argument('--production', required=True,
                        help='the production CodexProxyGuard.exe')
    arguments = parser.parse_args()
    verifier = runpy.run_path(str(VERIFIER))

    cet = verifier['pe_mitigations'](pathlib.Path(arguments.cet).read_bytes())
    assert cet['cet_compat'] is True, \
        f'/CETCOMPAT fixture lost its extended record: {cet}'
    no_cet = verifier['pe_mitigations'](pathlib.Path(arguments.no_cet).read_bytes())
    assert no_cet['cet_compat'] is False, \
        f'/CETCOMPAT:NO fixture was mistaken for CET-compatible: {no_cet}'

    production = verifier['pe_mitigations'](
        pathlib.Path(arguments.production).read_bytes())
    missing = [name for name in verifier['REQUIRED_MITIGATIONS']
               if not production.get(name)]
    assert not missing, f'production image failed the mitigation gate: {missing}'
    print('CET toolchain fixtures consistent with the verifier '
          f'(cet={cet["cet_compat"]}, no-cet={no_cet["cet_compat"]}, '
          'production image fully mitigated)')


if __name__ == '__main__':
    try:
        main()
    except AssertionError as failure:
        print(failure, file=sys.stderr)
        sys.exit(1)

"""Write release-manifest.json + SHA256SUMS for the Rust single-EXE release.

Called by scripts/build-portable-rust.ps1 AFTER signing (Authenticode
changes the EXE bytes, so every recorded digest must describe the final
artifact). Digests are recomputed here from the packaged files; the PE
mitigation facts are derived with the same parsing core verify-package.py
uses, and the independent verifier re-derives everything again before
promotion - nothing this script writes is trusted on faith.
"""
import argparse
import datetime
import hashlib
import importlib.util
import json
import pathlib
import sys

HERE = pathlib.Path(__file__).parent
_spec = importlib.util.spec_from_file_location('verify_package', HERE / 'verify-package.py')
verify_package = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(verify_package)

SCHEMA = 'rust-static-single'
ARTIFACTS = (
    'CodexProxyGuard.exe', 'SHA256SUMS', 'build-info.json',
    'source.zip', 'release-manifest.json',
)
SUMMED_ARTIFACTS = ('CodexProxyGuard.exe', 'build-info.json', 'source.zip')


def file_digest(path: pathlib.Path) -> str:
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--release', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--commit', required=True)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--signed', action='store_true')
    mode.add_argument('--unsigned-dev', action='store_true')
    parser.add_argument('--thumbprint', default='')
    parser.add_argument('--expected-thumbprint', default='')
    parser.add_argument('--publisher', default='')
    parser.add_argument('--timestamp-present', choices=['true', 'false'], default='false')
    args = parser.parse_args()

    release = pathlib.Path(args.release)
    executable = release / 'CodexProxyGuard.exe'
    source = release / 'source.zip'
    info = json.loads((release / 'build-info.json').read_text(encoding='utf-8-sig'))
    if info.get('version') != args.version or info.get('commit') != args.commit:
        raise SystemExit('packaged build-info disagrees with the requested release identity')
    if info.get('dirty') is not False:
        raise SystemExit('release must be built from a clean commit')
    if info.get('language') != 'Rust':
        raise SystemExit('release build-info language must be Rust')

    exe_bytes = executable.read_bytes()
    mitigations = verify_package.pe_mitigations(exe_bytes)
    manifest = {
        'schema': SCHEMA,
        'schema_version': 1,
        'version': args.version,
        'commit': args.commit,
        'dirty': False,
        'created_utc': datetime.datetime.now(datetime.timezone.utc)
                        .replace(microsecond=0).isoformat().replace('+00:00', 'Z'),
        'language': 'Rust',
        'subsystem': 'console',
        'crt': 'static',
        'qt': False,
        'powershell_appx_discovery': False,
        'artifacts': list(ARTIFACTS),
        'executable': {
            'sha256': file_digest(executable),
            'size': len(exe_bytes),
            'signed': bool(args.signed),
        },
        'source_zip': {
            'path': source.name,
            'sha256': file_digest(source),
        },
        'mitigations': mitigations,
    }
    if args.signed:
        manifest.update({
            'signed': True,
            'publisher': args.publisher,
            'signer_thumbprint': args.thumbprint,
            'expected_certificate_sha1': args.expected_thumbprint or args.thumbprint,
            'timestamp_present': args.timestamp_present == 'true',
            'signature_verification_policy':
                'independent WinVerifyTrust (WINTRUST_ACTION_GENERIC_VERIFY_V2)',
        })
    else:
        manifest.update({
            'signed': False,
            'unsigned_dev': True,
        })

    (release / 'release-manifest.json').write_text(
        json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    sums = ''.join(
        f'{file_digest(release / name)}  {name}\n' for name in SUMMED_ARTIFACTS)
    (release / 'SHA256SUMS').write_text(sums, encoding='ascii')
    print(f'manifest written: {manifest["executable"]["sha256"][:16]}... '
          f'signed={manifest["signed"]}')


if __name__ == '__main__':
    sys.exit(main())

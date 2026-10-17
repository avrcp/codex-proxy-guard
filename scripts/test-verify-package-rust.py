"""Fixture suite for the `rust-static-single` verifier schema.

Builds a synthetic-but-structurally-valid PE32+ image (AMD64, console,
CFG flags with FID table, CET type-20 debug record, KERNEL32 import) and
packages it into the Rust release layout via scripts/write-rust-manifest.py,
then runs scripts/verify-package.py with --skip-execution against:

- the untouched fixture (must pass), and
- ten tampered variants (must each be rejected for the right reason).

Run: python scripts/test-verify-package-rust.py
"""
import importlib.util
import json
import pathlib
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile

HERE = pathlib.Path(__file__).parent
VERSION = '9.9.9-test'
COMMIT = '0123456789abcdef0123456789abcdef01234567'

_spec = importlib.util.spec_from_file_location('verify_package', HERE / 'verify-package.py')
verify_package = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(verify_package)


def build_synthetic_exe(*, cet: bool = True, fid_table: bool = True,
                        system_import: bool = True) -> bytes:
    """One-section PE32+ exercising every gate the verifier parses."""
    section = bytearray()

    def put(data: bytes):
        rva = 0x1000 + len(section)
        section.extend(data)
        return rva, 0x200 + len(section) - len(data)

    name = b'KERNEL32.DLL\x00' if system_import else b'MALWARE.DLL\x00\x00'
    kernel_name_rva, _ = put(name)
    while len(section) % 4:
        section.append(0)
    import_rva, _ = put(
        struct.pack('<IIIII', 0, 0, 0, kernel_name_rva, 0) + b'\x00' * 20)
    load_config = bytearray(200)
    struct.pack_into('<I', load_config, 0, 200)
    struct.pack_into('<Q', load_config, 128, 0x1000 if fid_table else 0)
    struct.pack_into('<Q', load_config, 136, 1 if fid_table else 0)
    struct.pack_into('<I', load_config, 144, 0x100 | (0x400 if fid_table else 0))
    config_rva, _ = put(bytes(load_config))
    debug_rva, debug_off = put(b'\x00' * 28)
    cet_rva, cet_off = put(struct.pack('<I', 1 if cet else 0))

    image = bytearray(0x200)
    image[0:2] = b'MZ'
    struct.pack_into('<I', image, 0x3c, 0x80)
    pe = 0x80
    image[pe:pe + 4] = b'PE\x00\x00'
    struct.pack_into('<HH', image, pe + 4, 0x8664, 1)
    optional = pe + 24
    struct.pack_into('<H', image, pe + 20, 240)
    struct.pack_into('<H', image, optional, 0x20b)
    struct.pack_into('<H', image, optional + 68, 3)          # console subsystem
    struct.pack_into('<H', image, optional + 70, 0x4160)     # ASLR/DEP/CFG/HE
    struct.pack_into('<I', image, optional + 108, 16)
    dirs = optional + 112
    struct.pack_into('<II', image, dirs + 8, import_rva, 40)
    struct.pack_into('<II', image, dirs + 6 * 8, debug_rva, 28)
    struct.pack_into('<II', image, dirs + 10 * 8, config_rva, 200)
    table = optional + 240
    image[table:table + 8] = b'.rdata\x00\x00'
    struct.pack_into('<IIII', image, table + 8, len(section), 0x1000,
                     len(section), 0x200)
    struct.pack_into('<I', image, table + 36, 0x40000040)
    image.extend(section)
    struct.pack_into('<I', image, debug_off + 12, 20)        # EX_DLLCHARACTERISTICS
    struct.pack_into('<I', image, debug_off + 16, 4)
    struct.pack_into('<I', image, debug_off + 20, cet_rva)
    struct.pack_into('<I', image, debug_off + 24, cet_off)
    return bytes(image)


def build_release(root: pathlib.Path, exe_bytes: bytes) -> pathlib.Path:
    release = root / 'release'
    release.mkdir()
    (release / 'CodexProxyGuard.exe').write_bytes(exe_bytes)
    (release / 'build-info.json').write_text(json.dumps({
        'commit': COMMIT, 'dirty': False, 'language': 'Rust', 'version': VERSION,
    }), encoding='utf-8')
    source = root / 'source.zip'
    with zipfile.ZipFile(source, 'w') as bundle:
        bundle.comment = COMMIT.encode('ascii')
        bundle.writestr('README.md', 'fixture source\n')
    shutil.copy(source, release / 'source.zip')
    subprocess.run([
        sys.executable, str(HERE / 'write-rust-manifest.py'),
        '--release', str(release), '--version', VERSION, '--commit', COMMIT,
        '--unsigned-dev',
    ], check=True, capture_output=True)
    return release


def verify(release: pathlib.Path) -> subprocess.CompletedProcess:
    return subprocess.run([
        sys.executable, str(HERE / 'verify-package.py'),
        '--release', str(release), '--schema', 'rust-static-single',
        '--skip-execution',
    ], capture_output=True, text=True)


def rewrite_manifest(release: pathlib.Path, mutate) -> None:
    path = release / 'release-manifest.json'
    manifest = json.loads(path.read_text(encoding='utf-8-sig'))
    mutate(manifest)
    path.write_text(json.dumps(manifest, indent=2), encoding='utf-8')


def case(root: pathlib.Path, label: str, needle: str, *, exe_kwargs=None,
         after_build=None) -> bool:
    exe = build_synthetic_exe(**(exe_kwargs or {}))
    release = build_release(root, exe)
    if after_build:
        after_build(release)
    result = verify(release)
    rejected = result.returncode != 0 and needle in result.stderr
    print(f"{'PASS' if rejected else 'FAIL'}  {label}"
          + ('' if rejected else f'  rc={result.returncode} err={result.stderr.strip()[:160]}'))
    return rejected


def main() -> None:
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        exe = build_synthetic_exe()
        release = build_release(root, exe)
        result = verify(release)
        ok = result.returncode == 0 and 'verified' in result.stdout
        print(f"{'PASS' if ok else 'FAIL'}  intact fixture verifies "
              f"({result.stdout.strip() or result.stderr.strip()[:160]})")
        failures += 0 if ok else 1

    def bump(release):
        path = release / 'CodexProxyGuard.exe'
        data = bytearray(path.read_bytes())
        data[0x210] ^= 0xFF  # inside the load-config slack: structure intact
        path.write_bytes(bytes(data))

    def wrong_commit(release):
        rewrite_manifest(release, lambda manifest: (
            manifest.__setitem__('commit', 'f' * 40)))

    def forged_cet(release):
        # Manifest keeps claiming cet_compat=true; the PE record is gone.
        rewrite_manifest(release, lambda manifest: (
            manifest['mitigations'].__setitem__('cet_compat', True)))

    def corrupt_source(release):
        with (release / 'source.zip').open('ab') as stream:
            stream.write(b'\x00')

    def extra_dll(release):
        (release / 'payload.dll').write_bytes(b'MZ')

    def corrupt_sums(release):
        path = release / 'SHA256SUMS'
        lines = path.read_text(encoding='ascii').splitlines()
        path.write_text('\n'.join(
            '0' * 64 + '  ' + line.split('  ')[1]
            if 'CodexProxyGuard.exe' in line else line
            for line in lines) + '\n', encoding='ascii')

    def forged_signature(release):
        def mutate(manifest):
            manifest['executable']['signed'] = True
            manifest.update({
                'signed': True, 'publisher': 'Fixture Signer',
                'signer_thumbprint': 'aa' * 20,
                'expected_certificate_sha1': 'aa' * 20,
                'timestamp_present': True, 'authenticode_status': 'Valid',
                'signature_verification_policy': 'fixture',
            })
            manifest.pop('unsigned_dev', None)
        rewrite_manifest(release, mutate)

    def dirty_manifest(release):
        rewrite_manifest(release, lambda manifest: (
            manifest.__setitem__('dirty', True)))

    cases = [
        ('tampered EXE byte', 'digest does not match the manifest',
         {'after_build': bump}),
        ('wrong commit provenance', 'commit mismatch',
         {'after_build': wrong_commit}),
        ('missing CET record, manifest claims it', 'cet_compat',
         {'exe_kwargs': {'cet': False}, 'after_build': forged_cet}),
        ('missing CFG function table', 'cf_function_table_present',
         {'exe_kwargs': {'fid_table': False}}),
        ('non-system import', 'Non-system import',
         {'exe_kwargs': {'system_import': False}}),
        ('modified source.zip', 'Source ZIP digest does not match',
         {'after_build': corrupt_source}),
        ('extra DLL in release', 'artifact set mismatch',
         {'after_build': extra_dll}),
        ('SHA256SUMS EXE digest wrong', 'SHA256SUMS digest mismatch',
         {'after_build': corrupt_sums}),
        ('signed claim on unsigned EXE', 'Authenticode',
         {'after_build': forged_signature}),
        ('dirty provenance', 'clean commit', {'after_build': dirty_manifest}),
    ]
    for label, needle, kwargs in cases:
        with tempfile.TemporaryDirectory() as tmp:
            failures += 0 if case(pathlib.Path(tmp), label, needle, **kwargs) else 1
    print(f'\n{len(cases) + 1 - failures}/{len(cases) + 1} passed')
    sys.exit(1 if failures else 0)


if __name__ == '__main__':
    main()

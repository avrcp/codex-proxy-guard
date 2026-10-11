"""Verify the no-Qt single-EXE portable release (schema: static-single).

Expected package layout under dist/releases/<version>-<commit>/:

- CodexProxyGuard.exe        the single console-subsystem executable
- release-manifest.json      binds version/commit/artifacts together
- SHA256SUMS.txt             digests of every file in the release directory
- CodexProxyGuard-<version>-windows-x86_64.zip   transport ZIP with only the EXE
- CodexProxyGuard-<version>-windows-x86_64.zip.sha256
- CodexProxyGuard-<version>-source.zip            source snapshot
- size-report.json           size evidence

The verifier re-reads every artifact from disk; nothing the build script
wrote is trusted on faith. It executes the packaged EXE to read its embedded
build-info, checks the console subsystem byte, scans the import table for
non-system DLLs, and refuses manifests that still claim Qt or PowerShell
discovery.
"""
import argparse
import hashlib
import json
import pathlib
import struct
import subprocess
import tempfile
import zipfile

REQUIRED_MANIFEST_KEYS = (
    'schema', 'schema_version', 'version', 'commit', 'dirty',
    'created_utc', 'artifacts', 'executable',
)
ALLOWED_IMPORT_PREFIXES = (
    'ADVAPI32.DLL', 'API-MS-WIN-', 'BCRYPT.DLL', 'CFG.DLL', 'CLR.DLL',
    'COMBASE.DLL', 'COMCTL32.DLL', 'CRYPT32.DLL', 'DBGHELP.DLL',
    'DNSAPI.DLL', 'DPAPI.DLL', 'EXT-MS-WIN-', 'GDI32.DLL', 'GDIPLUS.DLL',
    'KTMW32.DLL',
    'IMM32.DLL', 'KERNEL32.DLL', 'MSVCRT.DLL', 'NCRYPT.DLL', 'NTDLL.DLL',
    'OLE32.DLL', 'OLEAUT32.DLL', 'POWRPROF.DLL', 'RPCRT4.DLL', 'SECUR32.DLL',
    'SHELL32.DLL', 'SHLWAPI.DLL', 'SHCORE.DLL', 'USER32.DLL', 'USERENV.DLL',
    'UCRTBASE.DLL', 'VERSION.DLL', 'WIN32U.DLL', 'WS2_32.DLL', 'WTSAPI32.DLL',
)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_digest(path: pathlib.Path) -> str:
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def check_sidecar(archive: pathlib.Path) -> str:
    actual = file_digest(archive)
    sidecar = pathlib.Path(str(archive) + '.sha256').read_text(encoding='utf-8-sig').split()
    if sidecar != [actual, archive.name]:
        raise SystemExit('ZIP sidecar digest mismatch')
    return actual


def pe_header(data: bytes):
    if data[:2] != b'MZ':
        raise SystemExit('EXE has no MZ signature')
    pe_offset = struct.unpack_from('<I', data, 0x3c)[0]
    if data[pe_offset:pe_offset + 4] != b'PE\x00\x00':
        raise SystemExit('EXE has no PE signature')
    return pe_offset


def pe_subsystem(data: bytes) -> int:
    pe_offset = pe_header(data)
    optional_header = pe_offset + 24
    magic = struct.unpack_from('<H', data, optional_header)[0]
    if magic not in (0x10b, 0x20b):
        raise SystemExit('Unknown PE optional header magic')
    return struct.unpack_from('<H', data, optional_header + 68)[0]


def pe_imports(data: bytes) -> list:
    """Import scan of the first import directory (dumpbin-free)."""
    pe_offset = pe_header(data)
    optional_header = pe_offset + 24
    magic = struct.unpack_from('<H', data, optional_header)[0]
    section_table = optional_header + struct.unpack_from('<H', data, pe_offset + 20)[0]
    number_sections = struct.unpack_from('<H', data, pe_offset + 6)[0]
    data_directories = optional_header + (112 if magic == 0x20b else 96)
    # The import table is the second data directory (index 1).
    import_rva = struct.unpack_from('<I', data, data_directories + 8)[0]

    def rva_to_offset(rva: int) -> int:
        for index in range(number_sections):
            entry = section_table + index * 40
            virtual_size = struct.unpack_from('<I', data, entry + 8)[0]
            virtual_address = struct.unpack_from('<I', data, entry + 12)[0]
            raw_size = struct.unpack_from('<I', data, entry + 16)[0]
            raw_pointer = struct.unpack_from('<I', data, entry + 20)[0]
            if virtual_address <= rva < virtual_address + max(virtual_size, raw_size):
                return raw_pointer + (rva - virtual_address)
        raise SystemExit(f'Import RVA outside sections: {rva:#x}')

    imports = []
    descriptor = rva_to_offset(import_rva) if import_rva else 0
    while descriptor and data[descriptor:descriptor + 20] != b'\x00' * 20:
        name_rva = struct.unpack_from('<I', data, descriptor + 12)[0]
        name_offset = rva_to_offset(name_rva)
        end = data.index(b'\x00', name_offset)
        imports.append(data[name_offset:end].decode('ascii', 'replace').upper())
        descriptor += 20
    return imports


def run(args: pathlib.Path, *arguments: str) -> str:
    completed = subprocess.run([str(args), *arguments], capture_output=True, text=True,
                               timeout=120)
    if completed.returncode != 0:
        raise SystemExit(f'{args.name} {" ".join(arguments)} failed: {completed.stderr.strip()}')
    return completed.stdout.strip()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--release', required=True, help='release directory to verify')
    parser.add_argument('--schema', default='static-single',
                        choices=['static-single'], help='expected package schema')
    parser.add_argument('--skip-execution', action='store_true',
                        help='test fixtures only: skip executing the packaged EXE')
    args = parser.parse_args()
    release = pathlib.Path(args.release)
    if not release.is_dir():
        raise SystemExit(f'Release directory missing: {release}')

    manifest = json.loads((release / 'release-manifest.json').read_text(encoding='utf-8-sig'))
    for key in REQUIRED_MANIFEST_KEYS:
        if key not in manifest:
            raise SystemExit(f'release-manifest.json missing key: {key}')
    if manifest.get('schema') != 'static-single' or manifest.get('schema_version') != 1:
        raise SystemExit('Unexpected manifest schema')
    if manifest.get('qt'):
        raise SystemExit('Manifest claims Qt linkage in a no-Qt release')
    if manifest.get('powershell_appx_discovery'):
        raise SystemExit('Manifest claims PowerShell discovery in a native-discovery release')
    if manifest.get('subsystem') != 'console':
        raise SystemExit('Manifest must record the console subsystem')
    if manifest.get('crt') != 'static':
        raise SystemExit('Manifest must record the static CRT')

    executable = release / 'CodexProxyGuard.exe'
    if not executable.is_file():
        raise SystemExit('CodexProxyGuard.exe missing from the release')
    exe_bytes = executable.read_bytes()
    exe_digest = file_digest(executable)
    if manifest['executable'].get('sha256') != exe_digest:
        raise SystemExit('EXE digest does not match the manifest')
    if manifest['executable'].get('size') != len(exe_bytes):
        raise SystemExit('EXE size does not match the manifest')
    if manifest['executable'].get('qt') is not False:
        raise SystemExit('Executable record must record qt=false')
    if pe_subsystem(exe_bytes) != 3:
        raise SystemExit('EXE is not a console-subsystem binary')
    imports = pe_imports(exe_bytes)
    if not imports:
        raise SystemExit('EXE import table is empty or unreadable')
    forbidden = [name for name in imports if not name.startswith(ALLOWED_IMPORT_PREFIXES)]
    if forbidden:
        raise SystemExit(f'Non-system import detected: {", ".join(forbidden)}')
    for name in imports:
        if name.startswith(('QT6', 'MSVCP140', 'VCRUNTIME140', 'CONCRT140', 'VCRUNTIME140_1')):
            raise SystemExit(f'Forbidden CRT/Qt import: {name}')

    # Execute the packaged EXE itself for provenance and licenses (skipped
    # only by the fixture suite, which cannot execute its synthetic payload).
    if not args.skip_execution:
        info = json.loads(run(executable, 'build-info'))
        if info.get('version') != manifest['version']:
            raise SystemExit('Packaged build-info version mismatch')
        if info.get('commit') != manifest['commit']:
            raise SystemExit('Packaged build-info commit mismatch')
        if info.get('language') != 'C++20':
            raise SystemExit('Packaged build-info language mismatch')
        licenses = run(executable, 'licenses')
        if '===== LICENSE =====' not in licenses or '===== json/LICENSE.MIT =====' not in licenses:
            raise SystemExit('Embedded license stream incomplete')

    # Transport ZIP: exactly the EXE, digest-bound by sidecar and manifest.
    zips = sorted(release.glob('*.zip'))
    transport = [path for path in zips if 'source' not in path.name]
    if len(transport) != 1:
        raise SystemExit('Expected exactly one transport ZIP')
    zip_digest = check_sidecar(transport[0])
    with zipfile.ZipFile(transport[0]) as bundle:
        if bundle.testzip() is not None:
            raise SystemExit('ZIP CRC failed')
        names = [entry.filename.replace('\\', '/') for entry in bundle.infolist()
                 if not entry.is_dir()]
        if sorted(names) != ['CodexProxyGuard.exe']:
            raise SystemExit(f'Transport ZIP must contain only the EXE, found: {names}')
        if digest(bundle.read('CodexProxyGuard.exe')) != exe_digest:
            raise SystemExit('Transport ZIP EXE digest mismatch')
    if manifest.get('transport_zip', {}).get('sha256') != zip_digest:
        raise SystemExit('Transport ZIP digest does not match the manifest')

    source = [path for path in zips if 'source' in path.name]
    if len(source) != 1:
        raise SystemExit('Expected exactly one source ZIP')
    check_sidecar(source[0])

    sums = release / 'SHA256SUMS.txt'
    if not sums.is_file():
        raise SystemExit('SHA256SUMS.txt missing')
    recorded = {}
    for line in sums.read_text(encoding='utf-8-sig').splitlines():
        parts = line.split()
        if len(parts) == 2:
            recorded[parts[1]] = parts[0]
    if recorded.get('CodexProxyGuard.exe') != exe_digest:
        raise SystemExit('SHA256SUMS.txt EXE digest mismatch')
    if recorded.get(transport[0].name) != zip_digest:
        raise SystemExit('SHA256SUMS.txt transport ZIP digest mismatch')

    if manifest['executable'].get('signed') and manifest.get('authenticode_status') != 'Valid':
        raise SystemExit('Signed release must record authenticode_status=Valid')
    if not manifest['executable'].get('signed') and not manifest.get('unsigned_dev'):
        raise SystemExit('Unsigned release must be explicitly marked unsigned_dev')
    print(f'verified {release.name}: {exe_digest[:16]}... '
          f'({len(exe_bytes)} bytes, imports: {", ".join(imports)})')


if __name__ == '__main__':
    main()

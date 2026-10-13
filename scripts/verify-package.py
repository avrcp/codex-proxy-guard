"""Verify the no-Qt single-EXE portable release (schema: static-single).

Expected package layout under dist/releases/<version>-<commit>/:

- CodexProxyGuard.exe        the single console-subsystem executable
- release-manifest.json      binds version/commit/artifacts together
- SHA256SUMS.txt             digests of every file in the release directory
- CodexProxyGuard-<version>-windows-x86_64.zip   transport ZIP with only the EXE
- CodexProxyGuard-<version>-windows-x86_64.zip.sha256
- CodexProxyGuard-<version>-source.zip            source snapshot
- CodexProxyGuard-<version>-source.zip.sha256
- size-report.json           size evidence

The verifier re-reads every artifact from disk; nothing the build script
wrote is trusted on faith. It executes the packaged EXE to read its embedded
build-info, checks the console subsystem byte, scans the import table for
non-system DLLs, refuses manifests that still claim Qt or PowerShell
discovery, verifies the source ZIP digest against the sidecar, the manifest
and SHA256SUMS and rejects build products inside it, requires the release
directory to contain exactly the manifest's artifact set, and independently
re-validates the Authenticode signature of a signed release through
WinVerifyTrust.
"""
import argparse
import ctypes
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
# Build products and secrets must never travel inside a source snapshot.
FORBIDDEN_SOURCE_SUFFIXES = (
    '.obj', '.exe', '.dll', '.pdb', '.lib', '.ilk', '.zip', '.7z', '.pfx', '.key',
)
FORBIDDEN_SOURCE_SEGMENTS = ('target', 'dist', '.git', '__pycache__')


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


def rva_to_offset(data: bytes, rva: int) -> int:
    pe_offset = pe_header(data)
    optional_header = pe_offset + 24
    section_table = optional_header + struct.unpack_from('<H', data, pe_offset + 20)[0]
    number_sections = struct.unpack_from('<H', data, pe_offset + 6)[0]
    for index in range(number_sections):
        entry = section_table + index * 40
        virtual_size = struct.unpack_from('<I', data, entry + 8)[0]
        virtual_address = struct.unpack_from('<I', data, entry + 12)[0]
        raw_size = struct.unpack_from('<I', data, entry + 16)[0]
        raw_pointer = struct.unpack_from('<I', data, entry + 20)[0]
        if virtual_address <= rva < virtual_address + max(virtual_size, raw_size):
            return raw_pointer + (rva - virtual_address)
    raise SystemExit(f'RVA outside sections: {rva:#x}')


def pe_imports(data: bytes) -> list:
    """Import scan of the first import directory (dumpbin-free)."""
    pe_offset = pe_header(data)
    optional_header = pe_offset + 24
    magic = struct.unpack_from('<H', data, optional_header)[0]
    data_directories = optional_header + (112 if magic == 0x20b else 96)
    # The import table is the second data directory (index 1).
    import_rva = struct.unpack_from('<I', data, data_directories + 8)[0]

    imports = []
    descriptor = rva_to_offset(data, import_rva) if import_rva else 0
    while descriptor and data[descriptor:descriptor + 20] != b'\x00' * 20:
        name_rva = struct.unpack_from('<I', data, descriptor + 12)[0]
        name_offset = rva_to_offset(data, name_rva)
        end = data.index(b'\x00', name_offset)
        imports.append(data[name_offset:end].decode('ascii', 'replace').upper())
        descriptor += 20
    return imports


# DllCharacteristics bits (PE optional header +70) and load-config GuardFlags
# bits (offset 144 in IMAGE_LOAD_CONFIG_DIRECTORY64).
DLLCHARACTERISTIC_MITIGATIONS = (
    ('dynamic_base', 0x0040),
    ('high_entropy_va', 0x0020),
    ('nx_compat', 0x0100),
    ('guard_cf', 0x4000),
    ('cet_compat_dllchar', 0x1000),
)
GUARDFLAG_MITIGATIONS = (
    ('cf_instrumented', 0x00000100),
    ('cf_function_table_present', 0x00000400),
    ('cet_compat_guard', 0x01000000),
)
# Newer MSVC linkers record CET in an extended-characteristics debug record
# (IMAGE_DEBUG_TYPE_EX_DLLCHARACTERISTICS, type 20) instead of the classic
# DllCharacteristics bit; any of the three spellings counts.
EX_DLLCHARACTERISTICS_DEBUG_TYPE = 20
EX_DLLCHARACTERISTICS_CET_COMPAT = 0x00000001
REQUIRED_MITIGATIONS = (
    'dynamic_base', 'high_entropy_va', 'nx_compat', 'guard_cf', 'cet_compat',
    'cf_instrumented', 'cf_function_table_present',
)


def pe_mitigations(data: bytes) -> dict:
    """Derive the exploit-mitigation facts of a PE32+ image from its headers:
    DllCharacteristics, the load-config GuardFlags (CFG instrumentation and
    the FID table) and the extended-characteristics debug record (CET)."""
    pe_offset = pe_header(data)
    optional_header = pe_offset + 24
    magic = struct.unpack_from('<H', data, optional_header)[0]
    if magic not in (0x10b, 0x20b):
        raise SystemExit('Unknown PE optional header magic')
    dll_characteristics = struct.unpack_from('<H', data, optional_header + 70)[0]
    mitigations = {name: bool(dll_characteristics & mask)
                   for name, mask in DLLCHARACTERISTIC_MITIGATIONS}
    data_directories = optional_header + (112 if magic == 0x20b else 96)
    load_config_rva = struct.unpack_from('<I', data, data_directories + 10 * 8)[0]
    guard_flags = 0
    if load_config_rva:
        config = rva_to_offset(data, load_config_rva)
        size = struct.unpack_from('<I', data, config)[0]
        if size >= 148:  # GuardFlags sits at offset 144
            guard_flags = struct.unpack_from('<I', data, config + 144)[0]
    for name, mask in GUARDFLAG_MITIGATIONS:
        mitigations[name] = bool(guard_flags & mask)
    cet_record = False
    debug_rva, debug_size = struct.unpack_from('<II', data, data_directories + 6 * 8)
    for index in range(debug_size // 28) if debug_rva else ():
        entry = rva_to_offset(data, debug_rva) + index * 28
        dtype = struct.unpack_from('<I', data, entry + 12)[0]
        dsize = struct.unpack_from('<I', data, entry + 16)[0]
        pointer = struct.unpack_from('<I', data, entry + 24)[0]
        if (dtype == EX_DLLCHARACTERISTICS_DEBUG_TYPE and dsize >= 4 and pointer
                and struct.unpack_from('<I', data, pointer)[0]
                & EX_DLLCHARACTERISTICS_CET_COMPAT):
            cet_record = True
    cet_dllchar = mitigations.pop('cet_compat_dllchar')
    cet_guard = mitigations.pop('cet_compat_guard')
    mitigations['cet_compat'] = cet_dllchar or cet_guard or cet_record
    return mitigations


def run(args: pathlib.Path, *arguments: str) -> str:
    completed = subprocess.run([str(args), *arguments], capture_output=True, text=True,
                               timeout=120)
    if completed.returncode != 0:
        raise SystemExit(f'{args.name} {" ".join(arguments)} failed: {completed.stderr.strip()}')
    return completed.stdout.strip()


def check_source_zip(archive: pathlib.Path, manifest: dict, recorded: dict) -> None:
    """The source snapshot must be digest-bound everywhere and may not carry
    build products or repository-local directories."""
    digest = check_sidecar(archive)
    if manifest.get('source_zip', {}).get('sha256') != digest:
        raise SystemExit('Source ZIP digest does not match the manifest')
    if manifest.get('source_zip', {}).get('path') != archive.name:
        raise SystemExit('Source ZIP path does not match the manifest')
    if recorded.get(archive.name) != digest:
        raise SystemExit('SHA256SUMS.txt source ZIP digest mismatch')
    with zipfile.ZipFile(archive) as bundle:
        if bundle.comment != manifest['commit'].encode('ascii'):
            raise SystemExit('Source ZIP Git commit does not match the manifest')
        if bundle.testzip() is not None:
            raise SystemExit('Source ZIP CRC failed')
        for entry in bundle.infolist():
            name = entry.filename.replace('\\', '/')
            if name.endswith('/'):
                continue
            if name.lower().endswith(FORBIDDEN_SOURCE_SUFFIXES):
                raise SystemExit(f'Source ZIP contains a build product: {name}')
            segments = [segment.lower() for segment in name.split('/')[:-1]]
            if any(segment in FORBIDDEN_SOURCE_SEGMENTS for segment in segments):
                raise SystemExit(f'Source ZIP contains a forbidden directory: {name}')


def check_artifact_set(release: pathlib.Path, manifest: dict) -> None:
    """The release directory must hold exactly the manifest's artifact set -
    no stale files from earlier runs and no missing sidecar."""
    expected = manifest.get('artifacts')
    if not isinstance(expected, list) or not expected:
        raise SystemExit('Manifest must list its artifacts')
    actual = sorted(entry.name for entry in release.iterdir() if entry.is_file())
    if sorted(expected) != actual:
        raise SystemExit(
            f'Release artifact set mismatch\n  manifest: {sorted(expected)}\n  actual:   {actual}')


def verify_authenticode(path: pathlib.Path) -> None:
    """Independent Authenticode validation through WinVerifyTrust - the
    manifest's own 'signed' claim is never trusted on faith."""
    import ctypes.wintypes as wt

    class GUID(ctypes.Structure):
        _fields_ = [('Data1', wt.DWORD), ('Data2', wt.WORD), ('Data3', wt.WORD),
                    ('Data4', ctypes.c_ubyte * 8)]

    class WINTRUST_FILE_INFO(ctypes.Structure):
        _fields_ = [('cbStruct', wt.DWORD), ('pcwszFilePath', wt.LPCWSTR),
                    ('hFile', wt.HANDLE), ('pgKnownSubject', ctypes.c_void_p)]

    class WINTRUST_DATA(ctypes.Structure):
        _fields_ = [('cbStruct', wt.DWORD), ('pPolicyCallbackData', ctypes.c_void_p),
                    ('pSIPClientData', ctypes.c_void_p), ('dwUIChoice', wt.DWORD),
                    ('fdwRevocationChecks', wt.DWORD), ('dwUnionChoice', wt.DWORD),
                    ('pFile', ctypes.POINTER(WINTRUST_FILE_INFO)),
                    ('dwStateAction', wt.DWORD), ('hWVTStateData', wt.HANDLE),
                    ('pwszURLReference', wt.LPCWSTR),
                    ('dwProvFlags', wt.DWORD), ('dwUIContext', wt.DWORD)]

    wintrust = ctypes.WinDLL('wintrust')
    action = GUID(0x00aac56b, 0xcd44, 0x11d0,
                  (ctypes.c_ubyte * 8)(0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee))
    info = WINTRUST_FILE_INFO(ctypes.sizeof(WINTRUST_FILE_INFO), str(path), None, None)
    data = WINTRUST_DATA(ctypes.sizeof(WINTRUST_DATA), None, None,
                         2,  # WTD_UI_NONE
                         0,  # WTD_REVOKE_NONE
                         1,  # WTD_CHOICE_FILE
                         ctypes.byref(info),
                         1,  # WTD_STATEACTION_VERIFY
                         None, None, 0, 0)
    result = wintrust.WinVerifyTrust(None, ctypes.byref(action), ctypes.byref(data))
    data.dwStateAction = 16  # WTD_STATEACTION_CLOSE
    wintrust.WinVerifyTrust(None, ctypes.byref(action), ctypes.byref(data))
    if result != 0:
        raise SystemExit(f'Authenticode validation failed (WinVerifyTrust 0x{result & 0xffffffff:08x})')


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--release', help='release directory to verify')
    parser.add_argument('--schema', default='static-single',
                        choices=['static-single'], help='expected package schema')
    parser.add_argument('--mitigations-of', metavar='EXE',
                        help='print the PE mitigation facts of an EXE as JSON and exit')
    parser.add_argument('--skip-execution', action='store_true',
                        help='test fixtures only: skip executing the packaged EXE')
    args = parser.parse_args()
    if args.mitigations_of:
        print(json.dumps(pe_mitigations(pathlib.Path(args.mitigations_of).read_bytes())))
        return
    if not args.release:
        raise SystemExit('--release is required unless --mitigations-of is given')
    release = pathlib.Path(args.release)
    if not release.is_dir():
        raise SystemExit(f'Release directory missing: {release}')

    manifest = json.loads((release / 'release-manifest.json').read_text(encoding='utf-8-sig'))
    for key in REQUIRED_MANIFEST_KEYS:
        if key not in manifest:
            raise SystemExit(f'release-manifest.json missing key: {key}')
    if manifest.get('schema') != 'static-single' or manifest.get('schema_version') != 1:
        raise SystemExit('Unexpected manifest schema')
    if manifest.get('dirty') is not False:
        raise SystemExit('Release provenance must describe a clean commit')
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

    # Mitigation gate: CFG must be instrumented across every production
    # object (FID table present), and ASLR/DEP/high-entropy/CET must be on.
    # Derived from the PE bytes independently of what the manifest claims.
    mitigations = pe_mitigations(exe_bytes)
    missing = [name for name in REQUIRED_MITIGATIONS if not mitigations.get(name)]
    if missing:
        raise SystemExit(f'PE mitigation gate failed, missing: {", ".join(missing)}')
    if 'mitigations' not in manifest:
        raise SystemExit('Manifest must record its PE mitigations')
    if manifest.get('mitigations') != mitigations:
        raise SystemExit('Manifest mitigations disagree with the PE headers')

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
        if info.get('dirty') is not False:
            raise SystemExit('Packaged build-info is not a clean commit')
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

    check_source_zip(source[0], manifest, recorded)
    check_artifact_set(release, manifest)

    if manifest['executable'].get('signed') and manifest.get('authenticode_status') != 'Valid':
        raise SystemExit('Signed release must record authenticode_status=Valid')
    if manifest['executable'].get('signed'):
        verify_authenticode(executable)
    if not manifest['executable'].get('signed') and not manifest.get('unsigned_dev'):
        raise SystemExit('Unsigned release must be explicitly marked unsigned_dev')
    print(f'verified {release.name}: {exe_digest[:16]}... '
          f'({len(exe_bytes)} bytes, imports: {", ".join(imports)})')


if __name__ == '__main__':
    main()

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
    'created_utc', 'artifacts', 'executable', 'signed',
)
# Rust single-EXE release layout (scripts/build-portable-rust.ps1):
# exactly these files, no transport ZIP.
RUST_SCHEMA = 'rust-static-single'
RUST_REQUIRED_MANIFEST_KEYS = (
    'schema', 'schema_version', 'version', 'commit', 'dirty', 'created_utc',
    'language', 'subsystem', 'crt', 'artifacts', 'executable', 'signed',
    'source_zip', 'mitigations',
)
RUST_ARTIFACTS = (
    'CodexProxyGuard.exe', 'SHA256SUMS', 'build-info.json',
    'source.zip', 'release-manifest.json',
)
RUST_SUMMED_ARTIFACTS = ('CodexProxyGuard.exe', 'build-info.json', 'source.zip')
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


# --- Strict PE parsing (x64 / PE32+ / console only) ----------------------------
# The product ships exactly one image shape; anything else is rejected
# instead of parsed with guesses. Every read is bounds-checked and fails
# closed instead of reaching past the file.
IMAGE_FILE_MACHINE_AMD64 = 0x8664
PE32_PLUS_MAGIC = 0x20b
SUBSYSTEM_CUI = 3


def read_struct(data: bytes, offset: int, fmt: str, what: str):
    """Bounds-checked struct.unpack_from: malformed headers are a parse
    failure, never a struct.error traceback or an out-of-file read."""
    end = offset + struct.calcsize(fmt)
    if offset < 0 or end > len(data):
        raise SystemExit(f'PE parse failed: {what} at {offset:#x} exceeds the image')
    return struct.unpack_from(fmt, data, offset)


def pe_layout(data: bytes):
    """Validated header layout of the only supported target: AMD64, PE32+,
    with the full 16-entry data directory inside SizeOfOptionalHeader and a
    section table that fits the file."""
    if data[:2] != b'MZ':
        raise SystemExit('EXE has no MZ signature')
    pe_offset = read_struct(data, 0x3c, '<I', 'e_lfanew')[0]
    signature = read_struct(data, pe_offset, '<4s', 'PE signature')[0]
    if signature != b'PE\x00\x00':
        raise SystemExit('EXE has no PE signature')
    machine, number_sections = read_struct(data, pe_offset + 4, '<HH', 'COFF header')
    if machine != IMAGE_FILE_MACHINE_AMD64:
        raise SystemExit(f'EXE is not an AMD64 x64 image (machine {machine:#06x})')
    size_of_optional = read_struct(data, pe_offset + 20, '<H', 'SizeOfOptionalHeader')[0]
    optional = pe_offset + 24
    magic = read_struct(data, optional, '<H', 'optional header magic')[0]
    if magic != PE32_PLUS_MAGIC:
        raise SystemExit(f'EXE is not a PE32+ image (magic {magic:#06x})')
    number_rva = read_struct(data, optional + 108, '<I', 'NumberOfRvaAndSizes')[0]
    data_directories = optional + 112
    if number_rva < 16:
        raise SystemExit(
            f'PE data directory count {number_rva} is below the required 16')
    if data_directories + number_rva * 8 > optional + size_of_optional:
        raise SystemExit('PE data directories exceed SizeOfOptionalHeader')
    section_table = optional + size_of_optional
    if section_table + number_sections * 40 > len(data):
        raise SystemExit('PE section table exceeds the image')
    return pe_offset, optional, data_directories, section_table, number_sections


def pe_subsystem(data: bytes) -> int:
    _, optional, _, _, _ = pe_layout(data)
    return read_struct(data, optional + 68, '<H', 'subsystem')[0]


def rva_to_offset(data: bytes, layout, rva: int, needed: int = 1) -> int:
    """Map an RVA into raw file bytes. An address that only exists in a
    section's zero-filled virtual tail - or past the end of the file - has no
    raw bytes and is refused instead of silently reading zeros."""
    _, _, _, section_table, number_sections = layout
    for index in range(number_sections):
        entry = section_table + index * 40
        virtual_size, virtual_address, raw_size, raw_pointer = read_struct(
            data, entry + 8, '<IIII', f'section {index} header')
        if virtual_address <= rva < virtual_address + max(virtual_size, raw_size):
            delta = rva - virtual_address
            if delta >= raw_size:
                raise SystemExit(
                    f'RVA {rva:#x} lies in a section zero-filled virtual tail '
                    'with no raw file bytes')
            offset = raw_pointer + delta
            if offset + needed > len(data):
                raise SystemExit(f'RVA {rva:#x} maps past the end of the image')
            return offset
    raise SystemExit(f'RVA outside sections: {rva:#x}')


def pe_imports(data: bytes) -> list:
    """Import scan of the first import directory (dumpbin-free)."""
    layout = pe_layout(data)
    import_rva, import_size = read_struct(
        data, layout[2] + 8, '<II', 'import directory')[0:2]
    imports = []
    if not import_rva:
        return imports
    descriptor = rva_to_offset(data, layout, import_rva, 20)
    for _ in range(import_size // 20):
        entry = read_struct(data, descriptor, '<20s', 'import descriptor')[0]
        if entry == b'\x00' * 20:
            break
        name_rva = struct.unpack_from('<I', entry, 12)[0]
        if not name_rva:
            raise SystemExit('PE parse failed: import descriptor has no name RVA')
        name_offset = rva_to_offset(data, layout, name_rva)
        end = data.find(b'\x00', name_offset)
        if end < 0:
            raise SystemExit('PE parse failed: unterminated import DLL name')
        imports.append(data[name_offset:end].decode('ascii', 'replace').upper())
        descriptor += 20
    return imports


# Optional-header DllCharacteristics bits (PE32+ optional header +70). The
# 0x1000 bit here is APPCONTAINER, not CET - it is never CET evidence.
DLLCHARACTERISTIC_MITIGATIONS = (
    ('dynamic_base', 0x0040),
    ('high_entropy_va', 0x0020),
    ('nx_compat', 0x0100),
    ('guard_cf', 0x4000),
)
# Load-config GuardFlags (IMAGE_LOAD_CONFIG_DIRECTORY64 +144) and the CFG
# function-table fields those flags must agree with (+128/+136).
GUARDFLAG_CF_INSTRUMENTED = 0x00000100
GUARDFLAG_CF_FUNCTION_TABLE_PRESENT = 0x00000400
LOAD_CONFIG_GUARD_FLAGS_OFFSET = 144
LOAD_CONFIG_CF_TABLE_OFFSET = 128
LOAD_CONFIG_CF_COUNT_OFFSET = 136
LOAD_CONFIG_MIN_SIZE = 148  # through GuardFlags
# CET shadow-stack compatibility is recorded ONLY in the extended DLL
# characteristics debug record (IMAGE_DEBUG_TYPE_EX_DLLCHARACTERISTICS,
# type 20, bit 0x0001). No record means no evidence; a truncated or
# out-of-bounds record is a parse failure, never a guessed value.
EX_DLLCHARACTERISTICS_DEBUG_TYPE = 20
EX_DLLCHARACTERISTICS_CET_COMPAT = 0x0001
DEBUG_DIRECTORY_ENTRY_SIZE = 28
REQUIRED_MITIGATIONS = (
    'dynamic_base', 'high_entropy_va', 'nx_compat', 'guard_cf', 'cet_compat',
    'cf_instrumented', 'cf_function_table_present',
)


def pe_cet_compat(data: bytes, layout) -> bool:
    """CET compatibility from the extended-characteristics debug record."""
    debug_rva, debug_size = read_struct(
        data, layout[2] + 6 * 8, '<II', 'debug directory')[0:2]
    if not debug_rva:
        return False
    directory = rva_to_offset(data, layout, debug_rva, debug_size)
    cet_compat = False
    for index in range(debug_size // DEBUG_DIRECTORY_ENTRY_SIZE):
        entry = directory + index * DEBUG_DIRECTORY_ENTRY_SIZE
        record = read_struct(data, entry, '<28s', 'debug directory entry')[0]
        if struct.unpack_from('<I', record, 12)[0] != EX_DLLCHARACTERISTICS_DEBUG_TYPE:
            continue
        payload_size = struct.unpack_from('<I', record, 16)[0]
        pointer = struct.unpack_from('<I', record, 24)[0]
        if payload_size < 4:
            raise SystemExit(
                'PE parse failed: extended-characteristics record is smaller '
                'than its DWORD payload')
        if not pointer:
            raise SystemExit(
                'PE parse failed: extended-characteristics record has no file pointer')
        if pointer + payload_size > len(data):
            raise SystemExit('PE parse failed: extended-characteristics record is truncated')
        if struct.unpack_from('<I', data, pointer)[0] & EX_DLLCHARACTERISTICS_CET_COMPAT:
            cet_compat = True
    return cet_compat


def pe_mitigations(data: bytes) -> dict:
    """Derive the exploit-mitigation facts the image can actually prove:
    DllCharacteristics, the load-config CFG block (flags agreeing with a
    non-empty function table) and the extended-characteristics debug record
    (CET). Object-level CFG coverage is proven by the build options, not by
    these image flags; cet_compat=true only marks the image compatible, not
    that hardware shadow stacks were enabled at run time."""
    layout = pe_layout(data)
    dll_characteristics = read_struct(
        data, layout[1] + 70, '<H', 'DllCharacteristics')[0]
    mitigations = {name: bool(dll_characteristics & mask)
                   for name, mask in DLLCHARACTERISTIC_MITIGATIONS}
    config_rva, _ = read_struct(
        data, layout[2] + 10 * 8, '<II', 'load config directory')[0:2]
    guard_flags = cf_table = cf_count = 0
    if config_rva:
        config = rva_to_offset(data, layout, config_rva, 4)
        declared = read_struct(data, config, '<I', 'load config size')[0]
        if declared >= LOAD_CONFIG_MIN_SIZE:
            guard_flags = read_struct(
                data, config + LOAD_CONFIG_GUARD_FLAGS_OFFSET, '<I', 'GuardFlags')[0]
            cf_table, cf_count = read_struct(
                data, config + LOAD_CONFIG_CF_TABLE_OFFSET, '<QQ',
                'GuardCFFunctionTable/Count')
    mitigations['cf_instrumented'] = bool(guard_flags & GUARDFLAG_CF_INSTRUMENTED)
    mitigations['cf_function_table_present'] = bool(
        (guard_flags & GUARDFLAG_CF_FUNCTION_TABLE_PRESENT) and cf_table and cf_count)
    mitigations['cet_compat'] = pe_cet_compat(data, layout)
    return mitigations


def run(args: pathlib.Path, *arguments: str) -> str:
    completed = subprocess.run([str(args), *arguments], capture_output=True, text=True,
                               encoding='utf-8', timeout=120)
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


# --- Independent Authenticode validation (WinVerifyTrust) ----------------------
# Structure and constant definitions follow the Windows SDK wintrust.h layout
# for a 64-bit host process. The trust provider is reached through an
# injectable adapter so the tests can drive the exact VERIFY -> CLOSE
# lifecycle; production loads the real entry point with a checked signature.
WTD_UI_NONE = 2
WTD_REVOKE_NONE = 0
WTD_CHOICE_FILE = 1
WTD_STATEACTION_VERIFY = 1
WTD_STATEACTION_CLOSE = 2


class WinTrustGuid(ctypes.Structure):
    _fields_ = [('Data1', ctypes.c_uint32), ('Data2', ctypes.c_uint16),
                ('Data3', ctypes.c_uint16), ('Data4', ctypes.c_ubyte * 8)]


WINTRUST_ACTION_GENERIC_VERIFY_V2 = WinTrustGuid(
    0x00aac56b, 0xcd44, 0x11d0,
    (ctypes.c_ubyte * 8)(0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee))


class WinTrustFileInfo(ctypes.Structure):
    _fields_ = [('cbStruct', ctypes.c_uint32), ('pcwszFilePath', ctypes.c_wchar_p),
                ('hFile', ctypes.c_void_p),
                ('pgKnownSubject', ctypes.POINTER(WinTrustGuid))]


class _WinTrustUnionChoice(ctypes.Union):
    # The dwUnionChoice-selected member; only the file spelling is typed.
    _fields_ = [('pFile', ctypes.POINTER(WinTrustFileInfo)),
                ('pCatalog', ctypes.c_void_p), ('pBlob', ctypes.c_void_p),
                ('pSgnr', ctypes.c_void_p)]


class WinTrustData(ctypes.Structure):
    # Full x64 SDK layout (through pSignatureSettings); trailing members stay
    # zero-initialized. cbStruct must equal sizeof(this) or WinVerifyTrust
    # rejects the request.
    _fields_ = [
        ('cbStruct', ctypes.c_uint32),
        ('pPolicyCallbackData', ctypes.c_void_p),
        ('pSIPClientData', ctypes.c_void_p),
        ('dwUIChoice', ctypes.c_uint32),
        ('fdwRevocationChecks', ctypes.c_uint32),
        ('dwUnionChoice', ctypes.c_uint32),
        ('pChoice', _WinTrustUnionChoice),
        ('dwStateAction', ctypes.c_uint32),
        ('hWVTStateData', ctypes.c_void_p),
        ('pwszURLReference', ctypes.c_wchar_p),
        ('dwProvFlags', ctypes.c_uint32),
        ('dwUIContext', ctypes.c_uint32),
        ('pExtendedErrorInfo', ctypes.c_void_p),
        ('enumiPolicy', ctypes.c_uint32),
        ('psResult', ctypes.c_void_p),
        ('pawszNetworkURL', ctypes.c_wchar_p),
        ('psWVTStateData', ctypes.c_void_p),
        ('pSignatureSettings', ctypes.c_void_p),
    ]
    _anonymous_ = ['pChoice']


def load_wintrust_api():
    """Load the real WinVerifyTrust entry point with a checked signature."""
    if not hasattr(ctypes, 'WinDLL'):
        raise SystemExit('Authenticode validation requires Windows')
    verify = ctypes.WinDLL('wintrust').WinVerifyTrust
    verify.argtypes = [ctypes.c_void_p, ctypes.POINTER(WinTrustGuid),
                       ctypes.POINTER(WinTrustData)]
    verify.restype = ctypes.c_long
    return verify


def build_wintrust_request(path: pathlib.Path):
    """One WINTRUST_DATA configured for a UI-less, file-choice verification.

    The file-info object, its typed pointer and the path string are returned
    together and must all outlive the calls: the trust provider reads
    pFile->pcwszFilePath while WinVerifyTrust runs."""
    file_info = WinTrustFileInfo()
    file_info.cbStruct = ctypes.sizeof(WinTrustFileInfo)
    file_info.pcwszFilePath = str(path)
    file_info_ptr = ctypes.pointer(file_info)  # typed LP_, not a byref argument
    data = WinTrustData()
    data.cbStruct = ctypes.sizeof(WinTrustData)
    data.dwUIChoice = WTD_UI_NONE
    data.fdwRevocationChecks = WTD_REVOKE_NONE
    data.dwUnionChoice = WTD_CHOICE_FILE
    data.pFile = file_info_ptr
    data.dwStateAction = WTD_STATEACTION_VERIFY
    return WINTRUST_ACTION_GENERIC_VERIFY_V2, file_info, file_info_ptr, data


def run_wintrust_lifecycle(api, action, data):
    """Run one VERIFY pass and release it with CLOSE afterwards.

    Every actually initiated verification - including one whose call raised -
    is followed by WTD_STATEACTION_CLOSE over the same state data. Cleanup
    errors are returned (as an exception object), never raised out of the
    finally block, so they cannot mask the primary verification result."""
    verify_status = None
    close_status = None
    try:
        verify_status = api(None, ctypes.byref(action), ctypes.byref(data))
    finally:
        data.dwStateAction = WTD_STATEACTION_CLOSE
        try:
            close_status = api(None, ctypes.byref(action), ctypes.byref(data))
        except Exception as error:
            close_status = error
    return verify_status, close_status


def verify_authenticode(path: pathlib.Path, *, api=None) -> None:
    """Independent Authenticode validation of a packaged artifact - the
    manifest's own 'signed' claim is never trusted on faith. Only a VERIFY
    status of exactly 0 counts as valid; everything else (including a failed
    state release afterwards) fails the release."""
    if api is None:
        api = load_wintrust_api()
    action, _file_info, _file_ptr, data = build_wintrust_request(path)
    try:
        verify_status, close_status = run_wintrust_lifecycle(api, action, data)
    except Exception as error:
        raise SystemExit(f'WinVerifyTrust invocation failed: {error}') from error
    if isinstance(close_status, Exception):
        raise SystemExit(
            f'WinVerifyTrust state release failed: {close_status} '
            f'(VERIFY 0x{verify_status & 0xffffffff:08x})')
    if verify_status != 0:
        raise SystemExit(
            f'Authenticode validation failed: WinVerifyTrust '
            f'0x{verify_status & 0xffffffff:08x} '
            f'(close 0x{close_status & 0xffffffff:08x})')
    if close_status != 0:
        # A file whose signature verified but whose state data cannot be
        # released is reported as a cleanup failure, never as Valid.
        raise SystemExit(
            f'WinVerifyTrust state release failed: 0x{close_status & 0xffffffff:08x}')


def normalize_thumbprint(value) -> str:
    """Compact lowercase hex form of a SHA-1 thumbprint, or '' if malformed."""
    if not isinstance(value, str):
        return ''
    compact = value.replace(':', '').replace(' ', '').lower()
    if len(compact) == 40 and all(character in '0123456789abcdef' for character in compact):
        return compact
    return ''


def check_signature_facts(manifest: dict) -> None:
    """Signature facts must be internally consistent: a signed release also
    records executable.signed, a non-empty publisher, the signer thumbprint,
    timestamp_present and its verification policy, and never the unsigned_dev
    marker; an unsigned release is explicitly unsigned_dev with no signed
    executable. The independent WinVerifyTrust pass that follows decides
    whether a 'signed' release is actually trusted."""
    signed = manifest.get('signed')
    executable_signed = manifest['executable'].get('signed')
    if signed is True:
        if executable_signed is not True:
            raise SystemExit('Signed manifest must record executable.signed=true')
        if manifest.get('unsigned_dev'):
            raise SystemExit('Signed manifest must not be marked unsigned_dev')
        if manifest.get('authenticode_status') != 'Valid':
            raise SystemExit('Signed release must record authenticode_status=Valid')
        publisher = manifest.get('publisher')
        if not isinstance(publisher, str) or not publisher.strip():
            raise SystemExit('Signed release must record a non-empty publisher')
        if not normalize_thumbprint(manifest.get('signer_thumbprint')):
            raise SystemExit('Signed release must record the signer certificate thumbprint')
        expected = manifest.get('expected_certificate_sha1')
        if expected is not None and normalize_thumbprint(expected) != normalize_thumbprint(
                manifest.get('signer_thumbprint')):
            raise SystemExit('Signer thumbprint does not match the expected certificate')
        if not isinstance(manifest.get('timestamp_present'), bool):
            raise SystemExit('Signed release must record timestamp_present as a fact')
        policy = manifest.get('signature_verification_policy')
        if not isinstance(policy, str) or not policy.strip():
            raise SystemExit('Signed release must record its signature verification policy')
    elif signed is False:
        if not manifest.get('unsigned_dev'):
            raise SystemExit('Unsigned release must be explicitly marked unsigned_dev')
        if executable_signed:
            raise SystemExit('Unsigned manifest must record executable.signed=false')
    else:
        raise SystemExit('Manifest must record signed=true/false')


def parse_sums(release: pathlib.Path) -> dict:
    sums = release / 'SHA256SUMS'
    if not sums.is_file():
        raise SystemExit('SHA256SUMS missing')
    recorded = {}
    for line in sums.read_text(encoding='utf-8-sig').splitlines():
        parts = line.split()
        if len(parts) == 2:
            recorded[parts[1]] = parts[0]
    return recorded


def verify_rust_release(release: pathlib.Path, manifest: dict, skip_execution: bool) -> None:
    """Verification of the Rust single-EXE release layout: EXE PE gates,
    build-info provenance, source ZIP binding, SHA256SUMS, exact artifact
    set and signature facts - the same parsing cores as static-single."""
    if manifest.get('schema') != RUST_SCHEMA or manifest.get('schema_version') != 1:
        raise SystemExit('Unexpected manifest schema')
    if manifest.get('dirty') is not False:
        raise SystemExit('Release provenance must describe a clean commit')
    if manifest.get('language') != 'Rust':
        raise SystemExit('Manifest must record the Rust language')
    if manifest.get('subsystem') != 'console':
        raise SystemExit('Manifest must record the console subsystem')
    if manifest.get('crt') != 'static':
        raise SystemExit('Manifest must record the static CRT')
    if manifest.get('qt'):
        raise SystemExit('Manifest claims Qt linkage in a no-Qt release')
    if manifest.get('powershell_appx_discovery'):
        raise SystemExit('Manifest claims PowerShell discovery in a native-discovery release')

    executable = release / 'CodexProxyGuard.exe'
    if not executable.is_file():
        raise SystemExit('CodexProxyGuard.exe missing from the release')
    exe_bytes = executable.read_bytes()
    exe_digest = file_digest(executable)
    if manifest['executable'].get('sha256') != exe_digest:
        raise SystemExit('EXE digest does not match the manifest')
    if manifest['executable'].get('size') != len(exe_bytes):
        raise SystemExit('EXE size does not match the manifest')
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

    mitigations = pe_mitigations(exe_bytes)
    missing = [name for name in REQUIRED_MITIGATIONS if not mitigations.get(name)]
    if missing:
        raise SystemExit(f'PE mitigation gate failed, missing: {", ".join(missing)}')
    if manifest.get('mitigations') != mitigations:
        raise SystemExit('Manifest mitigations disagree with the PE headers')

    # Provenance: the build-info.json fact must agree with the manifest, and
    # (outside the fixture suite) with what the packaged EXE itself prints.
    info = json.loads((release / 'build-info.json').read_text(encoding='utf-8-sig'))
    if info.get('version') != manifest['version']:
        raise SystemExit('Packaged build-info version mismatch')
    if info.get('commit') != manifest['commit']:
        raise SystemExit('Packaged build-info commit mismatch')
    if info.get('dirty') is not False:
        raise SystemExit('Packaged build-info is not a clean commit')
    if info.get('language') != 'Rust':
        raise SystemExit('Packaged build-info language mismatch')
    if not skip_execution:
        executed = json.loads(run(executable, 'build-info'))
        if executed != info:
            raise SystemExit('Executed build-info disagrees with build-info.json')
        run(executable, '--version')
        run(executable, '--help')
        licenses = run(executable, 'licenses')
        if '===== LICENSE =====' not in licenses:
            raise SystemExit('Embedded license stream incomplete')

    source = release / 'source.zip'
    if not source.is_file():
        raise SystemExit('source.zip missing from the release')
    source_digest = file_digest(source)
    if manifest.get('source_zip', {}).get('sha256') != source_digest:
        raise SystemExit('Source ZIP digest does not match the manifest')
    if manifest.get('source_zip', {}).get('path') != source.name:
        raise SystemExit('Source ZIP path does not match the manifest')
    with zipfile.ZipFile(source) as bundle:
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

    recorded = parse_sums(release)
    if sorted(recorded) != sorted(RUST_SUMMED_ARTIFACTS):
        raise SystemExit(
            f'SHA256SUMS must cover exactly {sorted(RUST_SUMMED_ARTIFACTS)}, '
            f'found {sorted(recorded)}')
    for name, recorded_digest in recorded.items():
        if file_digest(release / name) != recorded_digest:
            raise SystemExit(f'SHA256SUMS digest mismatch for {name}')
    if recorded['CodexProxyGuard.exe'] != exe_digest:
        raise SystemExit('SHA256SUMS EXE digest mismatch')

    check_artifact_set(release, manifest)

    check_signature_facts(manifest)
    if manifest.get('signed') is True:
        verify_authenticode(executable.resolve())
    print(f'verified {release.name}: {exe_digest[:16]}... '
          f'({len(exe_bytes)} bytes, imports: {", ".join(imports)})')


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--release', help='release directory to verify')
    parser.add_argument('--schema', default='static-single',
                        choices=['static-single', RUST_SCHEMA],
                        help='expected package schema')
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
    if args.schema == RUST_SCHEMA:
        for key in RUST_REQUIRED_MANIFEST_KEYS:
            if key not in manifest:
                raise SystemExit(f'release-manifest.json missing key: {key}')
        verify_rust_release(release, manifest, args.skip_execution)
        return
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

    # Mitigation gate over the PE-provable facts: the image carries CFG
    # instrumentation with a non-empty FID table, ASLR/DEP/high-entropy VA
    # and the CET compatibility record. Object-level CFG coverage comes from
    # the shared build options, not from these image flags. Everything is
    # re-derived from the PE bytes independently of the manifest's claims.
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

    check_signature_facts(manifest)
    if manifest.get('signed') is True:
        # Independent trust validation against the absolute path of the
        # signed final EXE, after every digest gate has already bound it to
        # the manifest and the transport ZIP.
        verify_authenticode(executable.resolve())
    print(f'verified {release.name}: {exe_digest[:16]}... '
          f'({len(exe_bytes)} bytes, imports: {", ".join(imports)})')


if __name__ == '__main__':
    main()

"""Failure-fixture tests for scripts/verify-package.py (static-single schema).

Builds small synthetic single-EXE releases (release-manifest.json +
SHA256SUMS.txt + transport ZIP + source ZIP + EXE stub) and asserts that the
verifier accepts a fully consistent release and rejects every tampered
variant with a nonzero exit. The fixture EXE is a stub payload: the
production execution gate (running build-info/licenses on the packaged EXE)
is exercised by the canonical build script; these fixtures pass
--skip-execution and instead verify every structural and digest gate.
"""
import ctypes
import hashlib
import json
import os
import pathlib
import runpy
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import zipfile

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
VERIFIER = REPOSITORY / 'scripts' / 'verify-package.py'
COMMIT = '0123456789abcdef0123456789abcdef01234567'
VERSION = '9.9.9-test'


class ExecutionEncodingTests(unittest.TestCase):
    def test_utf8_execution_output_with_non_utf8_host_locale(self):
        run = runpy.run_path(str(VERIFIER))['run']
        notice = '\u4e2d\u6587 \u2014 \u201cnotice\u201d'
        output = 'import sys; sys.stdout.buffer.write(' + repr(notice.encode('utf-8')) + ')'
        failure = 'import sys; sys.stderr.buffer.write(' + repr(notice.encode('utf-8')) + '); sys.exit(1)'
        # Exercise real subprocess bytes while forcing the default text decoder
        # to the host code page that broke the canonical package verifier.
        with mock.patch.object(subprocess, '_text_encoding', return_value='gbk'):
            self.assertEqual(run(pathlib.Path(sys.executable), '-c', output), notice)
            with self.assertRaises(SystemExit) as raised:
                run(pathlib.Path(sys.executable), '-c', failure)
            self.assertIn(notice, str(raised.exception))


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_sha256(path: pathlib.Path) -> str:
    return sha256_bytes(path.read_bytes())


def minimal_console_pe(mitigations: bool = True) -> bytes:
    """A tiny but structurally valid PE32+ console image importing
    KERNEL32.DLL – enough for the verifier's own PE parser. With
    mitigations=True it also carries the DllCharacteristics bits and a load
    config with CFG/CET GuardFlags the mitigation gate requires."""
    headers = bytearray(0x200)
    struct.pack_into('<H', headers, 0, 0x5A4D)              # MZ
    struct.pack_into('<I', headers, 0x3c, 0x40)             # e_lfanew
    offset = 0x40
    headers[offset:offset + 4] = b'PE' + bytes(2)
    offset += 4
    struct.pack_into('<HHIIIHH', headers, offset,
                     0x8664, 2, 0, 0, 0, 240, 0x22)        # COFF header
    offset += 20
    optional = offset
    struct.pack_into('<H', headers, optional, 0x20b)       # PE32+ magic
    struct.pack_into('<H', headers, optional + 68, 3)      # CUI subsystem
    struct.pack_into('<I', headers, optional + 108, 16)    # data directory count
    struct.pack_into('<I', headers, optional + 120, 0x1000)  # import directory RVA
    if mitigations:
        # DYNAMICBASE | HIGHENTROPYVA | NX_COMPAT | GUARD_CF | CET_COMPAT
        struct.pack_into('<H', headers, optional + 70, 0x5160)
        # Load config directory (index 10) points into .rdata below. PE32+
        # data directories start at optional+112, so index 10 is +192.
        struct.pack_into('<I', headers, optional + 192, 0x2000)
    offset += 240
    section = offset
    headers[section:section + 8] = b'.idata' + bytes(2)
    struct.pack_into('<I', headers, section + 8, 0x200)    # virtual size
    struct.pack_into('<I', headers, section + 12, 0x1000)  # virtual address
    struct.pack_into('<I', headers, section + 16, 0x200)   # raw size
    struct.pack_into('<I', headers, section + 20, 0x200)   # raw pointer
    rdata = section + 40
    headers[rdata:rdata + 8] = b'.rdata' + bytes(2)
    struct.pack_into('<I', headers, rdata + 8, 0x400)      # virtual size
    struct.pack_into('<I', headers, rdata + 12, 0x2000)    # virtual address
    struct.pack_into('<I', headers, rdata + 16, 0x400)     # raw size
    struct.pack_into('<I', headers, rdata + 20, 0x400)     # raw pointer
    idata = bytearray(0x200)
    struct.pack_into('<I', idata, 12, 0x1100)              # import name RVA
    idata[0x100:0x100 + 13] = b'KERNEL32.DLL' + bytes(1)
    rconfig = bytearray(0x400)
    struct.pack_into('<I', rconfig, 0, 0xC8)               # load config size
    struct.pack_into('<I', rconfig, 144,                   # GuardFlags
                     0x01000500 if mitigations else 0)     # CFG | FID | CET
    return bytes(headers) + bytes(idata) + bytes(rconfig)


def mitigation_facts(guard: bool = True) -> dict:
    return {
        'dynamic_base': guard, 'high_entropy_va': guard, 'nx_compat': guard,
        'guard_cf': guard, 'cet_compat': guard, 'cf_instrumented': guard,
        'cf_function_table_present': guard,
    }


class VerifierTests(unittest.TestCase):

    def setUp(self):
        self._temporary = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._temporary.name)
        self.release = self.root / f'{VERSION}-{COMMIT[:8]}'
        self.release.mkdir()
        self.exe = self.release / 'CodexProxyGuard.exe'
        self.exe.write_bytes(minimal_console_pe())
        self.transport = self.release / f'CodexProxyGuard-{VERSION}-windows-x86_64.zip'
        self.source = self.release / f'CodexProxyGuard-{VERSION}-source.zip'
        self.write_transport()
        self.write_source()
        self.manifest = {
            'schema': 'static-single', 'schema_version': 1,
            'version': VERSION, 'commit': COMMIT, 'dirty': False,
            'created_utc': '2026-10-11T00:00:00Z',
            'qt': False, 'powershell_appx_discovery': False,
            'subsystem': 'console', 'crt': 'static',
            'mitigations': mitigation_facts(),
            'signed': False, 'authenticode_status': 'Unsigned',
            'unsigned_dev': True,
            'executable': {'path': 'CodexProxyGuard.exe',
                           'sha256': file_sha256(self.exe),
                           'size': self.exe.stat().st_size,
                           'signed': False, 'qt': False},
            'transport_zip': {'path': self.transport.name,
                              'sha256': file_sha256(self.transport)},
            'source_zip': {'path': self.source.name,
                           'sha256': file_sha256(self.source)},
            'artifacts': ['CodexProxyGuard.exe', self.transport.name,
                          self.transport.name + '.sha256', self.source.name,
                          self.source.name + '.sha256', 'release-manifest.json',
                          'SHA256SUMS.txt', 'size-report.json'],
        }
        (self.release / 'size-report.json').write_text('{}\n', encoding='utf-8')
        self.write_manifest()
        self.write_sums()

    def tearDown(self):
        self._temporary.cleanup()

    def write_transport(self, extra_member=None):
        with zipfile.ZipFile(self.transport, 'w') as bundle:
            bundle.writestr('CodexProxyGuard.exe', self.exe.read_bytes())
            if extra_member:
                bundle.writestr(extra_member, 'forbidden')
        self.transport.with_name(self.transport.name + '.sha256').write_text(
            f'{file_sha256(self.transport)} {self.transport.name}\n', encoding='utf-8')

    def write_source(self):
        with zipfile.ZipFile(self.source, 'w') as bundle:
            bundle.comment = COMMIT.encode('ascii')
            bundle.writestr('source/main.cpp', 'int main() {}\n')
        self.source.with_name(self.source.name + '.sha256').write_text(
            f'{file_sha256(self.source)} {self.source.name}\n', encoding='utf-8')

    def write_manifest(self, **overrides):
        manifest = dict(self.manifest)
        manifest.update(overrides)
        (self.release / 'release-manifest.json').write_text(
            json.dumps(manifest, indent=2), encoding='utf-8')

    def write_sums(self):
        lines = [f'{file_sha256(self.exe)} CodexProxyGuard.exe',
                 f'{file_sha256(self.transport)} {self.transport.name}',
                 f'{file_sha256(self.source)} {self.source.name}']
        (self.release / 'SHA256SUMS.txt').write_text('\n'.join(lines) + '\n', encoding='utf-8')

    def verify(self) -> subprocess.CompletedProcess:
        return subprocess.run([sys.executable, str(VERIFIER),
                               '--release', str(self.release),
                               '--schema', 'static-single', '--skip-execution'],
                              capture_output=True, text=True, timeout=120)

    def test_accepts_consistent_release(self):
        completed = self.verify()
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_rejects_dirty_release_provenance(self):
        self.write_manifest(dirty=True)
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_source_archive_from_different_commit(self):
        with zipfile.ZipFile(self.source, 'a') as bundle:
            bundle.comment = b'f' * 40
        digest = file_sha256(self.source)
        self.source.with_name(self.source.name + '.sha256').write_text(
            f'{digest} {self.source.name}\n', encoding='utf-8')
        self.write_manifest(source_zip={'path': self.source.name, 'sha256': digest})
        self.write_sums()
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_manifest_digest_mismatch(self):
        self.write_manifest(executable=dict(self.manifest['executable'], sha256='0' * 64))
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_size_mismatch(self):
        self.write_manifest(executable=dict(self.manifest['executable'], size=1))
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_qt_claim(self):
        self.write_manifest(qt=True)
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_powershell_discovery_claim(self):
        self.write_manifest(powershell_appx_discovery=True)
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_wrong_subsystem_claim(self):
        self.write_manifest(subsystem='windows')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_unsigned_without_dev_marker(self):
        manifest = dict(self.manifest)
        manifest.pop('unsigned_dev')
        (self.release / 'release-manifest.json').write_text(
            json.dumps(manifest), encoding='utf-8')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_signed_claim_without_valid_authenticode(self):
        self.write_manifest(authenticode_status='Unavailable',
                            executable=dict(self.manifest['executable'], signed=True))
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_signed_release_without_publisher(self):
        self.write_signed_manifest(publisher='')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_signed_release_without_signer_thumbprint(self):
        manifest = self.write_signed_manifest()
        manifest.pop('signer_thumbprint')
        (self.release / 'release-manifest.json').write_text(
            json.dumps(manifest), encoding='utf-8')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_signed_release_marked_unsigned_dev(self):
        self.write_signed_manifest(unsigned_dev=True)
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_signed_manifest_with_unsigned_executable_record(self):
        self.write_signed_manifest()
        self.write_manifest(signed=True, unsigned_dev=False,
                            authenticode_status='Valid',
                            publisher='CN=Test Signer',
                            signer_thumbprint='AB' * 20,
                            timestamp_present=True,
                            signature_verification_policy='test policy')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_signer_thumbprint_mismatching_expected_certificate(self):
        self.write_signed_manifest(expected_certificate_sha1='CD' * 20)
        self.assertNotEqual(self.verify().returncode, 0)

    def test_consistent_signed_facts_still_require_actual_trust(self):
        # Fully consistent signature facts must not pass on their own: the
        # fixture EXE is unsigned, so the independent WinVerifyTrust pass (or
        # its absence on a non-Windows host) must fail the release.
        self.write_signed_manifest()
        self.assertNotEqual(self.verify().returncode, 0)

    def write_signed_manifest(self, **overrides):
        manifest = dict(self.manifest)
        manifest.update(signed=True, unsigned_dev=False,
                        authenticode_status='Valid',
                        publisher='CN=Test Signer, O=Guard',
                        signer_thumbprint='ab' * 20,
                        timestamp_present=True,
                        signature_verification_policy=(
                            'signtool verify /pa /all + signature object + '
                            'independent WinVerifyTrust'),
                        executable=dict(self.manifest['executable'], signed=True))
        manifest.update(overrides)
        (self.release / 'release-manifest.json').write_text(
            json.dumps(manifest, indent=2), encoding='utf-8')
        return manifest

    def test_rejects_transport_with_extra_member(self):
        self.write_transport(extra_member='extra.txt')
        self.write_manifest(transport_zip={'path': self.transport.name,
                                           'sha256': file_sha256(self.transport)})
        self.write_sums()
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_transport_sidecar_mismatch(self):
        self.transport.with_name(self.transport.name + '.sha256').write_text(
            f'{"0" * 64} {self.transport.name}\n', encoding='utf-8')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_tampered_sums(self):
        (self.release / 'SHA256SUMS.txt').write_text(
            f'{"0" * 64} CodexProxyGuard.exe\n', encoding='utf-8')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_missing_source_zip(self):
        self.write_sums()
        self.source.with_name(self.source.name + '.sha256').unlink()
        self.source.unlink()
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_source_zip_manifest_digest_mismatch(self):
        self.write_manifest(source_zip={'path': self.source.name, 'sha256': '0' * 64})
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_sums_missing_source_entry(self):
        lines = [f'{file_sha256(self.exe)} CodexProxyGuard.exe',
                 f'{file_sha256(self.transport)} {self.transport.name}']
        (self.release / 'SHA256SUMS.txt').write_text('\n'.join(lines) + '\n', encoding='utf-8')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_source_zip_with_build_product(self):
        with zipfile.ZipFile(self.source, 'w') as bundle:
            bundle.writestr('source/main.cpp', 'int main() {}\n')
            bundle.writestr('source/tui_probe.obj', 'binary garbage')
        self.source.with_name(self.source.name + '.sha256').write_text(
            f'{file_sha256(self.source)} {self.source.name}\n', encoding='utf-8')
        self.write_manifest(source_zip={'path': self.source.name,
                                        'sha256': file_sha256(self.source)})
        self.write_sums()
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_source_zip_with_target_directory(self):
        with zipfile.ZipFile(self.source, 'w') as bundle:
            bundle.writestr('source/main.cpp', 'int main() {}\n')
            bundle.writestr('target/build.ninja', 'rule cc')
        self.source.with_name(self.source.name + '.sha256').write_text(
            f'{file_sha256(self.source)} {self.source.name}\n', encoding='utf-8')
        self.write_manifest(source_zip={'path': self.source.name,
                                        'sha256': file_sha256(self.source)})
        self.write_sums()
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_extra_stale_file_in_release(self):
        (self.release / 'stale.exe').write_bytes(b'stale')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_artifact_list_missing_sidecar(self):
        manifest = dict(self.manifest)
        manifest['artifacts'] = [name for name in manifest['artifacts']
                                 if name != self.transport.name + '.sha256']
        (self.release / 'release-manifest.json').write_text(
            json.dumps(manifest), encoding='utf-8')
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_exe_without_cfg_instrumentation(self):
        self.exe.write_bytes(minimal_console_pe(mitigations=False))
        self.write_manifest(executable=dict(self.manifest['executable'],
                                             sha256=file_sha256(self.exe),
                                             size=self.exe.stat().st_size),
                            mitigations=mitigation_facts(False))
        self.write_transport()
        self.write_sums()
        self.assertNotEqual(self.verify().returncode, 0)

    def test_rejects_manifest_mitigation_lie(self):
        # The EXE carries the mitigations but the manifest claims they are
        # absent - the PE parse is the authority, not the manifest.
        self.write_manifest(mitigations=mitigation_facts(False))
        self.assertNotEqual(self.verify().returncode, 0)


class RC3ReviewReproductions(unittest.TestCase):
    """Regressions for the two RC3 release-validation defects (R01/R02).

    Committed red against the RC3 verifier on purpose: the WinTrust request
    could never be constructed (assigning byref() into the typed pFile field
    raises TypeError), the cleanup action was recorded as 16 while
    WTD_STATEACTION_CLOSE is 2, and CET compatibility was accepted from the
    APPCONTAINER DllCharacteristics bit and an unrelated load-config GuardFlags
    bit instead of the extended-characteristics debug record. The fix commits
    turn each reproduction green without changing what it asserts.
    """

    def test_typed_pointer_field_rejects_byref_construction(self):
        # ctypes semantics the verifier depends on: byref() produces an
        # external-call argument, not a field value; a typed POINTER field
        # must be assigned ctypes.pointer(instance).
        class FileInfo(ctypes.Structure):
            _fields_ = [('value', ctypes.c_uint32)]

        class TrustData(ctypes.Structure):
            _fields_ = [('pFile', ctypes.POINTER(FileInfo))]

        info = FileInfo(1)
        with self.assertRaises(TypeError):
            TrustData(ctypes.byref(info))
        self.assertEqual(TrustData(ctypes.pointer(info)).pFile.contents.value, 1)

    def test_wintrust_lifecycle_verifies_then_closes_with_action_2(self):
        module = runpy.run_path(str(VERIFIER))
        calls = []
        data_type = ctypes.POINTER(module['WinTrustData'])

        def fake_api(hwnd, action, data):
            calls.append(ctypes.cast(data, data_type).contents.dwStateAction)
            return 0

        module['verify_authenticode'](pathlib.Path('C:/definitely/not/signed.exe'),
                                      api=fake_api)
        self.assertEqual(calls, [1, 2])  # WTD_STATEACTION_VERIFY then CLOSE

    def test_appcontainer_bit_alone_is_not_cet_evidence(self):
        module = runpy.run_path(str(VERIFIER))
        image = bytearray(minimal_console_pe())
        pe = struct.unpack_from('<I', image, 0x3c)[0]
        optional = pe + 24
        # APPCONTAINER DllCharacteristics, CFG-only GuardFlags, no debug
        # record carrying the extended CET characteristics.
        struct.pack_into('<H', image, optional + 70, 0x5160)
        struct.pack_into('<I', image, 0x400 + 144, 0x500)
        struct.pack_into('<II', image, optional + 160, 0, 0)
        self.assertIs(module['pe_mitigations'](bytes(image))['cet_compat'], False)


class WinTrustLifecycleTests(unittest.TestCase):
    """The WinVerifyTrust request and its VERIFY -> CLOSE lifecycle, driven
    through an injected adapter: no trust provider, no certificate store and
    no network access are involved."""

    def setUp(self):
        self.module = runpy.run_path(str(VERIFIER))

    def fake_api(self, verify_result=0, close_result=0, verify_raises=None,
                 close_raises=None):
        calls = []
        data_type = ctypes.POINTER(self.module['WinTrustData'])

        def api(hwnd, action, data):
            state = ctypes.cast(data, data_type).contents.dwStateAction
            calls.append(state)
            if state == self.module['WTD_STATEACTION_VERIFY']:
                if verify_raises is not None:
                    raise verify_raises
                return verify_result
            if close_raises is not None:
                raise close_raises
            return close_result

        api.calls = calls
        return api

    def test_request_construction_assigns_the_typed_pointer(self):
        requested = pathlib.Path('X:/release/CodexProxyGuard.exe')
        action, file_info, file_ptr, data = self.module['build_wintrust_request'](
            requested)
        self.assertEqual(ctypes.addressof(data.pFile.contents),
                         ctypes.addressof(file_info))
        self.assertEqual(data.pFile.contents.pcwszFilePath, str(requested))
        self.assertEqual(data.cbStruct, ctypes.sizeof(self.module['WinTrustData']))
        self.assertEqual(file_info.cbStruct,
                         ctypes.sizeof(self.module['WinTrustFileInfo']))
        self.assertEqual(data.dwUIChoice, self.module['WTD_UI_NONE'])
        self.assertEqual(data.fdwRevocationChecks, self.module['WTD_REVOKE_NONE'])
        self.assertEqual(data.dwUnionChoice, self.module['WTD_CHOICE_FILE'])
        self.assertEqual(data.dwStateAction, self.module['WTD_STATEACTION_VERIFY'])

    def test_struct_layout_matches_the_x64_sdk(self):
        if ctypes.sizeof(ctypes.c_void_p) != 8:
            self.skipTest('x64 host required for the SDK layout constants')
        trust_data = self.module['WinTrustData']
        expected = {
            'cbStruct': 0, 'dwUIChoice': 24, 'fdwRevocationChecks': 28,
            'dwUnionChoice': 32, 'pFile': 40, 'dwStateAction': 48,
            'hWVTStateData': 56, 'pwszURLReference': 64, 'dwProvFlags': 72,
            'dwUIContext': 76, 'pExtendedErrorInfo': 80, 'enumiPolicy': 88,
            'psResult': 96, 'pawszNetworkURL': 104, 'psWVTStateData': 112,
            'pSignatureSettings': 120,
        }
        for field, offset in expected.items():
            self.assertEqual(getattr(trust_data, field).offset, offset, field)
        self.assertEqual(ctypes.sizeof(trust_data), 128)
        self.assertEqual(ctypes.sizeof(self.module['WinTrustFileInfo']), 32)

    def test_successful_verification_runs_verify_then_close(self):
        api = self.fake_api(verify_result=0, close_result=0)
        self.module['verify_authenticode'](
            pathlib.Path('X:/signed.exe'), api=api)  # must not raise
        self.assertEqual(api.calls, [self.module['WTD_STATEACTION_VERIFY'],
                                     self.module['WTD_STATEACTION_CLOSE']])

    def test_signature_error_still_releases_state_and_reports_the_long_code(self):
        # TRUST_E_NOSIGNATURE arrives as a negative LONG; only 0 is success.
        api = self.fake_api(verify_result=-2146762496, close_result=0)
        with self.assertRaises(SystemExit) as raised:
            self.module['verify_authenticode'](pathlib.Path('X:/unsigned.exe'), api=api)
        self.assertIn('800b0100', str(raised.exception))
        self.assertEqual(api.calls, [self.module['WTD_STATEACTION_VERIFY'],
                                     self.module['WTD_STATEACTION_CLOSE']])

    def test_invocation_failure_releases_state_and_never_succeeds(self):
        api = self.fake_api(verify_raises=RuntimeError('trust provider exploded'))
        with self.assertRaises(SystemExit) as raised:
            self.module['verify_authenticode'](pathlib.Path('X:/broken.exe'), api=api)
        self.assertIn('trust provider exploded', str(raised.exception))
        self.assertEqual(api.calls, [self.module['WTD_STATEACTION_VERIFY'],
                                     self.module['WTD_STATEACTION_CLOSE']])

    def test_close_failure_keeps_the_primary_result_and_is_not_valid(self):
        api = self.fake_api(verify_result=0, close_result=5)
        with self.assertRaises(SystemExit) as raised:
            self.module['verify_authenticode'](pathlib.Path('X:/signed.exe'), api=api)
        self.assertIn('state release failed', str(raised.exception))
        api = self.fake_api(verify_result=-2146762496, close_result=5)
        with self.assertRaises(SystemExit) as raised:
            self.module['verify_authenticode'](pathlib.Path('X:/bad.exe'), api=api)
        # The primary VERIFY status stays in the message, cleanup after it.
        self.assertIn('800b0100', str(raised.exception))
        self.assertIn('00000005', str(raised.exception))

    def test_thumbprint_normalization(self):
        normalize = self.module['normalize_thumbprint']
        self.assertEqual(normalize('AB:CD:EF:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00'),
                         'abcdef00112233445566778899aabbccddeeff00')
        self.assertEqual(normalize('abcdef00112233445566778899aabbccddeeff00'),
                         'abcdef00112233445566778899aabbccddeeff00')
        self.assertEqual(normalize(''), '')
        self.assertEqual(normalize(None), '')
        self.assertEqual(normalize('not-a-thumbprint'), '')
        self.assertEqual(normalize('ab' * 21), '')


@unittest.skipUnless(hasattr(ctypes, 'WinDLL'), 'Windows trust provider required')
class RealWinTrustTests(unittest.TestCase):
    """The same request/lifecycle against the real WinVerifyTrust entry point
    - no certificate store modification, no revocation network fetch."""

    def setUp(self):
        self.module = runpy.run_path(str(VERIFIER))
        self.api = self.module['load_wintrust_api']()

    def test_unsigned_fixture_is_rejected_by_the_real_provider(self):
        with tempfile.TemporaryDirectory() as directory:
            target = pathlib.Path(directory) / 'unsigned.exe'
            target.write_bytes(minimal_console_pe())
            with self.assertRaises(SystemExit) as raised:
                self.module['verify_authenticode'](target, api=self.api)
            self.assertNotEqual('0x00000000', str(raised.exception))

    def test_embedded_signed_system_binary_is_trusted(self):
        # vswhere.exe carries an embedded Microsoft signature and is required
        # by the canonical build script itself, so any build machine has it.
        program_files = os.environ.get('ProgramFiles(x86)', r'C:\Program Files (x86)')
        vswhere = pathlib.Path(program_files) / 'Microsoft Visual Studio' / \
            'Installer' / 'vswhere.exe'
        if not vswhere.is_file():
            self.skipTest('no embedded-signed vswhere.exe control binary')
        self.module['verify_authenticode'](vswhere, api=self.api)  # must not raise


if __name__ == '__main__':
    unittest.main()

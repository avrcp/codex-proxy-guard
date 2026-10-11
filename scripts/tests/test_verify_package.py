"""Failure-fixture tests for scripts/verify-package.py (static-single schema).

Builds small synthetic single-EXE releases (release-manifest.json +
SHA256SUMS.txt + transport ZIP + source ZIP + EXE stub) and asserts that the
verifier accepts a fully consistent release and rejects every tampered
variant with a nonzero exit. The fixture EXE is a stub payload: the
production execution gate (running build-info/licenses on the packaged EXE)
is exercised by the canonical build script; these fixtures pass
--skip-execution and instead verify every structural and digest gate.
"""
import hashlib
import json
import pathlib
import struct
import subprocess
import sys
import tempfile
import unittest
import zipfile

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
VERIFIER = REPOSITORY / 'scripts' / 'verify-package.py'
COMMIT = '0123456789abcdef0123456789abcdef01234567'
VERSION = '9.9.9-test'


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_sha256(path: pathlib.Path) -> str:
    return sha256_bytes(path.read_bytes())


def minimal_console_pe() -> bytes:
    """A tiny but structurally valid PE32+ console image importing
    KERNEL32.DLL — enough for the verifier's own PE parser."""
    headers = bytearray(0x200)
    struct.pack_into('<H', headers, 0, 0x5A4D)              # MZ
    struct.pack_into('<I', headers, 0x3c, 0x40)             # e_lfanew
    offset = 0x40
    headers[offset:offset + 4] = b'PE' + bytes(2)
    offset += 4
    struct.pack_into('<HHIIIHH', headers, offset,
                     0x8664, 1, 0, 0, 0, 240, 0x22)        # COFF header
    offset += 20
    optional = offset
    struct.pack_into('<H', headers, optional, 0x20b)       # PE32+ magic
    struct.pack_into('<H', headers, optional + 68, 3)      # CUI subsystem
    struct.pack_into('<I', headers, optional + 108, 16)    # data directory count
    struct.pack_into('<I', headers, optional + 120, 0x1000)  # import directory RVA
    offset += 240
    section = offset
    headers[section:section + 8] = b'.idata' + bytes(2)
    struct.pack_into('<I', headers, section + 8, 0x200)    # virtual size
    struct.pack_into('<I', headers, section + 12, 0x1000)  # virtual address
    struct.pack_into('<I', headers, section + 16, 0x200)   # raw size
    struct.pack_into('<I', headers, section + 20, 0x200)   # raw pointer
    idata = bytearray(0x200)
    struct.pack_into('<I', idata, 12, 0x1100)              # import name RVA
    idata[0x100:0x100 + 13] = b'KERNEL32.DLL' + bytes(1)
    return bytes(headers) + bytes(idata)


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
            'unsigned_dev': True,
            'executable': {'path': 'CodexProxyGuard.exe',
                           'sha256': file_sha256(self.exe),
                           'size': self.exe.stat().st_size,
                           'signed': False, 'qt': False},
            'transport_zip': {'path': self.transport.name,
                              'sha256': file_sha256(self.transport)},
            'artifacts': ['CodexProxyGuard.exe', self.transport.name,
                          self.source.name, 'release-manifest.json',
                          'SHA256SUMS.txt'],
        }
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


if __name__ == '__main__':
    unittest.main()

"""Failure-fixture tests for scripts/verify-package.py.

Builds small synthetic split releases (runtime ZIP + source-compliance ZIP +
release-manifest.json) and asserts that the verifier accepts a fully
consistent release and rejects every tampered variant with a nonzero exit.
The pinned Qt source digest is overridden so fixtures stay kilobytes small;
production invocations keep the official default.
"""
import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
import zipfile

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
VERIFIER = REPOSITORY / 'scripts' / 'verify-package.py'
COMMIT = '0123456789abcdef0123456789abcdef01234567'
VERSION = '9.9.9-test'
QT_URL = 'https://download.qt.io/archive/qt/6.8/6.8.3/submodules/qtbase-everywhere-src-6.8.3.tar.xz'
QT_NAME = 'qtbase-everywhere-src-6.8.3.tar.xz'


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_zip(path: pathlib.Path, members: dict) -> None:
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as bundle:
        for name, data in members.items():
            bundle.writestr(name, data)


def runtime_members(fake_qt_sha: str) -> dict:
    """Return (members, build-info dict) for a consistent runtime ZIP."""
    payload = {name: f'fixture {name}'.encode() for name in
               ('CodexProxyGuard.exe', 'engine/Qt6Core.dll', 'platforms/qwindows.dll',
                'licenses/tomlplusplus/LICENSE')}
    files_sha = {name: sha256_bytes(data) for name, data in payload.items()}
    info = {
        'product_version': VERSION, 'git_commit': COMMIT, 'git_dirty': False,
        'profile': 'dynamic-split',
        'engine': {'version': VERSION, 'language': 'C++20', 'protocol_version': 1,
                   'path': 'engine/codex-proxy-guard.exe'},
        'qt_source': {'url': QT_URL, 'sha256': fake_qt_sha,
                      'provided_by': 'CodexProxyGuard-9.9.9-test-source-compliance.zip'},
        'qt_sdk': {'deployed_files': {
            'engine/Qt6Core.dll': {'sdk_path': 'bin/Qt6Core.dll', 'sdk_sha256': files_sha['engine/Qt6Core.dll'],
                                   'deployed_sha256': files_sha['engine/Qt6Core.dll']},
            'platforms/qwindows.dll': {'sdk_path': 'plugins/platforms/qwindows.dll',
                                       'sdk_sha256': files_sha['platforms/qwindows.dll'],
                                       'deployed_sha256': files_sha['platforms/qwindows.dll']}}},
        'files_sha256': files_sha,
    }
    info_bytes = json.dumps(info).encode()
    sums = '\n'.join(f'{value}  {key}' for key, value in
                     {**files_sha, 'build-info.json': sha256_bytes(info_bytes)}.items()).encode()
    members = dict(payload)
    members['build-info.json'] = info_bytes
    members['SHA256SUMS.txt'] = sums
    return members, info


def compliance_members(fake_qt: bytes) -> dict:
    return {
        'app/CMakeLists.txt': b'# fixture application source\n',
        f'upstream/{QT_NAME}': fake_qt,
        'manifest.json': b'placeholder-replaced-by-caller',
        'REBUILD.md': b'fixture rebuild instructions\n',
        'SOURCE_REVISION.txt': f'application source commit: {COMMIT}\n'.encode(),
        'SOURCE_ACCESS.txt': b'fixture source access statement\n',
        'patches/README.txt': b'no patches\n',
        'recipes/toolchain.json': b'{"msvc": "fixture"}',
        'recipes/packaging.json': b'{"entry_point": "scripts/build-portable.ps1"}',
        'licenses/THIRD_PARTY_NOTICES.md': b'fixture notices\n',
        'licenses/Qt/sdk-6.8.3-msvc2022-x64.json': b'{"schema_version": 1}',
        'licenses/tomlplusplus/LICENSE': b'fixture toml++ license\n',
    }


class ReleaseFixture:
    def __init__(self, root: pathlib.Path):
        self.root = root
        self.release_dir = root / 'dist'
        self.release_dir.mkdir(parents=True)
        self.fake_qt = b'fixture qt source archive bytes'
        self.fake_qt_sha = sha256_bytes(self.fake_qt)
        self.runtime_zip = self.release_dir / 'CodexProxyGuard-9.9.9-test-windows-x86_64-dynamic.zip'
        self.compliance_zip = self.release_dir / 'CodexProxyGuard-9.9.9-test-source-compliance.zip'
        self.manifest_path = self.release_dir / 'release-manifest.json'
        self.rebuild()

    def rebuild(self, *, runtime_override=None, compliance_override=None,
                manifest_override=None, dirty=False) -> None:
        members, info = runtime_members(self.fake_qt_sha)
        if dirty:
            info['git_dirty'] = 'false'  # string, not boolean
            members['build-info.json'] = json.dumps(info).encode()
        if runtime_override:
            members.update(runtime_override)
        write_zip(self.runtime_zip, members)
        self.runtime_sha = sha256_bytes(self.runtime_zip.read_bytes())
        (self.release_dir / (self.runtime_zip.name + '.sha256')).write_text(
            f'{self.runtime_sha}  {self.runtime_zip.name}\n')

        compliance = compliance_members(self.fake_qt)
        if compliance_override:
            compliance.update(compliance_override)
        compliance.pop('manifest.json')
        listed = {name: sha256_bytes(data) for name, data in compliance.items()}
        compliance['manifest.json'] = json.dumps({
            'schema_version': 1, 'product_version': VERSION, 'source_commit': COMMIT,
            'qt_source': {'name': QT_NAME, 'sha256': self.fake_qt_sha, 'url': QT_URL},
            'files_sha256': listed}).encode()
        write_zip(self.compliance_zip, compliance)
        self.compliance_sha = sha256_bytes(self.compliance_zip.read_bytes())
        (self.release_dir / (self.compliance_zip.name + '.sha256')).write_text(
            f'{self.compliance_sha}  {self.compliance_zip.name}\n')

        manifest = {
            'schema_version': 2, 'profile': 'dynamic-split', 'product_version': VERSION,
            'source_commit': COMMIT, 'source_dirty': False,
            'runtime_archive': {'name': self.runtime_zip.name, 'sha256': self.runtime_sha},
            'source_compliance_archive': {'name': self.compliance_zip.name, 'sha256': self.compliance_sha},
            'qt_source': {'name': QT_NAME, 'url': QT_URL, 'sha256': self.fake_qt_sha},
        }
        if manifest_override:
            manifest.update(manifest_override)
        self.manifest_path.write_text(json.dumps(manifest))

    def run(self, *extra):
        return subprocess.run(
            [sys.executable, str(VERIFIER), str(self.runtime_zip),
             '--expected-commit', COMMIT, '--qt-source-sha256', self.fake_qt_sha, *extra],
            capture_output=True, text=True)


class VerifyPackageTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.fixture = ReleaseFixture(pathlib.Path(self._tmp.name))

    def tearDown(self):
        self._tmp.cleanup()

    def test_consistent_split_release_passes(self):
        result = self.fixture.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report['profile'], 'dynamic-split')
        self.assertTrue(report['split_linkage'] == 'verified')

    def test_wrong_commit_rejected(self):
        result = self.fixture.run('--expected-commit', 'fedcba9876543210' + '0' * 24)
        self.assertNotEqual(result.returncode, 0)

    def test_dirty_string_rejected(self):
        self.fixture.rebuild(dirty=True)
        result = self.fixture.run()
        self.assertNotEqual(result.returncode, 0)

    def test_extra_dll_rejected(self):
        self.fixture.rebuild(runtime_override={'engine/Qt6Gui.dll': b'smuggled'})
        result = self.fixture.run()
        self.assertNotEqual(result.returncode, 0)

    def test_missing_compliance_archive_rejected(self):
        self.fixture.compliance_zip.unlink()
        result = self.fixture.run()
        self.assertNotEqual(result.returncode, 0)

    def test_manifest_runtime_hash_mismatch_rejected(self):
        self.fixture.rebuild(manifest_override={
            'runtime_archive': {'name': self.fixture.runtime_zip.name,
                                'sha256': '0' * 64}})
        result = self.fixture.run()
        self.assertNotEqual(result.returncode, 0)

    def test_qt_source_hash_mismatch_rejected(self):
        self.fixture.rebuild(compliance_override={f'upstream/{QT_NAME}': b'tampered qt'})
        result = self.fixture.run()
        self.assertNotEqual(result.returncode, 0)

    def test_corrupt_zip_crc_rejected(self):
        raw = bytearray(self.fixture.runtime_zip.read_bytes())
        raw[len(raw) // 2] ^= 0xFF
        self.fixture.runtime_zip.write_bytes(raw)
        result = self.fixture.run()
        self.assertNotEqual(result.returncode, 0)

    def test_manifest_commit_mismatch_rejected(self):
        self.fixture.rebuild(manifest_override={'source_commit': 'f' * 40})
        result = self.fixture.run()
        self.assertNotEqual(result.returncode, 0)

    def test_dirty_manifest_string_rejected(self):
        self.fixture.rebuild(manifest_override={'source_dirty': 'false'})
        result = self.fixture.run()
        self.assertNotEqual(result.returncode, 0)

    def test_schema_pin_rejects_other_layout(self):
        # A legacy-shaped bundle (source embedded) must fail a dynamic-split pin.
        members, info = runtime_members(self.fixture.fake_qt_sha)
        members[f'sources/{QT_NAME}'] = self.fixture.fake_qt
        legacy = self.fixture.release_dir / 'legacy-shaped.zip'
        write_zip(legacy, members)
        sidecar = self.fixture.release_dir / (legacy.name + '.sha256')
        sidecar.write_text(f'{sha256_bytes(legacy.read_bytes())}  {legacy.name}\n')
        result = subprocess.run(
            [sys.executable, str(VERIFIER), str(legacy), '--expected-commit', COMMIT,
             '--qt-source-sha256', self.fixture.fake_qt_sha, '--schema', 'dynamic-split'],
            capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)


if __name__ == '__main__':
    unittest.main(verbosity=2)

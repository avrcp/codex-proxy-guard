"""Exercise the canonical release's early safety gates without compiling."""
import ctypes
from ctypes import wintypes
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / 'scripts' / 'build-portable.ps1'
SIGNATURE_METADATA = ROOT / 'scripts' / 'signature-metadata.ps1'
THUMBPRINT = 'aabbccddeeff00112233445566778899aabbccdd'


def invoke(*args, script=SCRIPT):
    return subprocess.run(
        ['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
         '-File', str(script), '-AllowUnsignedDev', *args],
        capture_output=True, text=True, errors='replace', timeout=15,
        creationflags=subprocess.CREATE_NO_WINDOW)


class ReleaseInputGuards(unittest.TestCase):
    def test_release_destination_junction_is_refused_before_build(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            root, outside = base / 'repo', base / 'outside'
            (root / 'scripts').mkdir(parents=True)
            outside.mkdir()
            sentinel = outside / 'keep.txt'
            sentinel.write_text('untouched', encoding='utf-8')
            script = root / 'scripts' / SCRIPT.name
            shutil.copyfile(SCRIPT, script)
            # build-portable.ps1 dot-sources its signature metadata helper.
            shutil.copyfile(SIGNATURE_METADATA,
                            root / 'scripts' / SIGNATURE_METADATA.name)
            (root / '.gitignore').write_text('/target/\n/dist/\n', encoding='utf-8')
            for args in (['init', '-q'], ['add', '.'],
                         ['-c', 'user.name=Release test', '-c', 'user.email=test@example.invalid',
                          'commit', '-qm', 'test fixture']):
                subprocess.run(['git', '-C', str(root), *args], check=True,
                               capture_output=True, timeout=15)
            for relative in ('dist', 'dist/releases'):
                junction = root / relative
                junction.parent.mkdir(exist_ok=True)
                result = subprocess.run(['powershell.exe', '-NoProfile', '-Command',
                    '& { param($Link, $Target) New-Item -ItemType Junction -Path $Link -Target $Target -ErrorAction Stop | Out-Null }',
                    str(junction), str(outside)], capture_output=True, timeout=15,
                    creationflags=subprocess.CREATE_NO_WINDOW)
                self.assertEqual(result.returncode, 0, result.stderr)
                try:
                    result = invoke(script=script)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('Release destination cannot traverse reparse points', result.stderr)
                    self.assertEqual(sentinel.read_text(encoding='utf-8'), 'untouched')
                finally:
                    # Remove only the junction itself; never recurse into its target.
                    junction.rmdir()

    def test_external_build_path_is_refused_without_deleting_contents(self):
        with tempfile.TemporaryDirectory() as directory:
            sentinel = pathlib.Path(directory) / 'keep.txt'
            sentinel.write_text('must survive', encoding='utf-8')
            result = invoke('-BuildDirectory', directory)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('strictly inside target', result.stderr)
            self.assertEqual(sentinel.read_text(encoding='utf-8'), 'must survive')

    def test_repository_root_and_target_root_are_not_build_workspaces(self):
        for path in (ROOT, ROOT / 'target'):
            with self.subTest(path=path):
                result = invoke('-BuildDirectory', str(path))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('strictly inside target', result.stderr)

    def test_version_cannot_escape_release_directory(self):
        result = invoke('-Version', '../../escape')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Invalid release version', result.stderr)

    def test_release_lock_refuses_a_parallel_invocation(self):
        target = ROOT / 'target'
        target.mkdir(exist_ok=True)
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD,
            wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        kernel.CreateFileW.restype = wintypes.HANDLE
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        handle = kernel.CreateFileW(str(target / 'release.lock'), 0xC0000000, 0, None, 4, 0x80, None)
        self.assertNotEqual(handle, ctypes.c_void_p(-1).value)
        try:
            result = invoke()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('release.lock', result.stderr)
        finally:
            self.assertTrue(kernel.CloseHandle(handle))


class SignatureMetadataTests(unittest.TestCase):
    """Get-SignatureMetadata derives its facts from the signature object -
    never from signtool's localized text output. The real cmdlet is shadowed
    by a function defined before dot-sourcing, so command resolution picks
    the fake without touching the certificate store."""

    def invoke_metadata(self, fake_signature, expected_thumbprint=THUMBPRINT):
        # Windows PowerShell 5.1 reads a UTF-8 BOM script as Unicode and
        # returns the facts through a UTF-8 file, so non-ASCII subjects
        # survive the round trip even when the hidden console's output code
        # page would mangle them.
        with tempfile.TemporaryDirectory() as directory:
            facts = pathlib.Path(directory) / 'metadata.json'
            command = (
                'function Get-AuthenticodeSignature { param($LiteralPath) '
                + fake_signature + ' }\n'
                f". '{SIGNATURE_METADATA}'\n"
                "$metadata = Get-SignatureMetadata -Path 'C:/fake/CodexProxyGuard.exe' "
                f"-ExpectedThumbprint '{expected_thumbprint}'\n"
                f"$metadata | ConvertTo-Json -Compress | Set-Content -Encoding UTF8 '{facts}'\n")
            driver = pathlib.Path(directory) / 'invoke-metadata.ps1'
            driver.write_text(command, encoding='utf-8-sig')
            process = subprocess.run(
                ['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
                 '-File', str(driver)],
                capture_output=True, text=True, errors='replace', timeout=60,
                creationflags=subprocess.CREATE_NO_WINDOW)
            recorded = facts.read_text(encoding='utf-8-sig') if facts.is_file() else ''
        return process, recorded

    def signer(self, subject='CN=Guard Signing, O=示例组织', thumbprint=None):
        thumbprint = thumbprint or ':'.join(
            THUMBPRINT[index:index + 2] for index in range(0, 40, 2)).upper()
        return (f"[pscustomobject]@{{ Subject = '{subject}'; Thumbprint = '{thumbprint}' }}")

    def valid_signature(self, **members):
        fields = {
            'Status': "'Valid'",
            'SignerCertificate': f'$({self.signer()})',
            'TimeStamperCertificate': "[pscustomobject]@{ Subject = 'CN=Timestamp CA' }",
        }
        fields.update(members)
        body = '; '.join(f'{name} = {value}' for name, value in fields.items())
        return f'[pscustomobject]@{{ {body} }}'

    def test_extracts_facts_from_the_signature_object(self):
        process, facts = self.invoke_metadata(self.valid_signature())
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertIn(THUMBPRINT, facts)
        self.assertIn('CN=Guard Signing', facts)
        self.assertIn('"timestamp_present":true', facts)

    def test_expected_thumbprint_may_use_colon_and_case_variants(self):
        spaced = ' '.join(THUMBPRINT[index:index + 4].upper()
                          for index in range(0, 40, 4))
        process, _ = self.invoke_metadata(self.valid_signature(),
                                          expected_thumbprint=spaced)
        self.assertEqual(process.returncode, 0, process.stderr)

    def test_thumbprint_mismatch_is_refused(self):
        process, _ = self.invoke_metadata(
            self.valid_signature(),
            expected_thumbprint='ff' * 20)
        self.assertNotEqual(process.returncode, 0)
        self.assertIn('does not match', process.stderr)

    def test_non_valid_status_is_refused(self):
        process, _ = self.invoke_metadata(
            self.valid_signature(Status="'HashMismatch'"))
        self.assertNotEqual(process.returncode, 0)
        self.assertIn('not Valid', process.stderr)

    def test_missing_signer_certificate_is_refused(self):
        process, _ = self.invoke_metadata(
            self.valid_signature(SignerCertificate='$null'))
        self.assertNotEqual(process.returncode, 0)
        self.assertIn('no signer certificate', process.stderr)

    def test_catalog_signature_is_not_embedded_evidence(self):
        process, _ = self.invoke_metadata(
            self.valid_signature(SignatureType="'Catalog'"))
        self.assertNotEqual(process.returncode, 0)
        self.assertIn('not an embedded Authenticode signature', process.stderr)

    def test_embedded_signature_type_is_accepted(self):
        process, _ = self.invoke_metadata(
            self.valid_signature(SignatureType="'Embedded'"))
        self.assertEqual(process.returncode, 0, process.stderr)

    def test_absent_timestamper_records_timestamp_present_false(self):
        process, facts = self.invoke_metadata(
            self.valid_signature(TimeStamperCertificate='$null'))
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertIn('"timestamp_present":false', facts)

    def test_multiline_localized_text_never_feeds_extraction(self):
        # The publisher identity survives subjects that span multiple lines
        # (real newline inside the single-quoted string) and non-ASCII text,
        # because signtool output is never parsed at all.
        subject = 'CN=多行\n签名主体, O=Org'
        process, facts = self.invoke_metadata(self.valid_signature(
            SignerCertificate='$(' + self.signer(subject=subject) + ')'))
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertIn('多行', facts)
        self.assertIn('签名主体', facts)


if __name__ == '__main__':
    unittest.main()

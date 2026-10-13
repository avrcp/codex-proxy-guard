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


if __name__ == '__main__':
    unittest.main()

"""Verify portable ZIP provenance, CRC and all SHA-256 manifests."""
import argparse
import hashlib
import json
import pathlib
import zipfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('archive', type=pathlib.Path)
parser.add_argument('--expected-commit', required=True)
args = parser.parse_args()
with args.archive.open('rb') as stream:
    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
sidecar = pathlib.Path(str(args.archive) + '.sha256').read_text(encoding='utf-8-sig').split()
if sidecar != [digest, args.archive.name]:
    raise SystemExit('ZIP sidecar digest mismatch')
with zipfile.ZipFile(args.archive) as archive:
    if archive.testzip() is not None:
        raise SystemExit('ZIP CRC failed')
    entries = [entry for entry in archive.infolist() if not entry.is_dir()]
    files = {entry.filename.replace('\\', '/'): entry for entry in entries}
    if len(files) != len(entries):
        raise SystemExit('Duplicate ZIP paths')
    for name in files:
        if name.startswith('/') or '..' in pathlib.PurePosixPath(name).parts or ':' in name:
            raise SystemExit('Unsafe ZIP path')
    def read(name):
        return archive.read(files[name])
    info = json.loads(read('build-info.json'))
    if info['git_commit'] != args.expected_commit or info['git_dirty'] is not False:
        raise SystemExit('Package is not the expected clean commit')
    if info['engine']['language'] != 'C++20' or info['engine']['protocol_version'] != 1:
        raise SystemExit('Unexpected engine provenance')
    expected = info['files_sha256']
    if set(files) != set(expected) | {'build-info.json', 'SHA256SUMS.txt'}:
        raise SystemExit('Unmanifested or missing package files')
    actual = {name: hashlib.sha256(read(name)).hexdigest() for name in files}
    for name, checksum in expected.items():
        if actual[name] != checksum:
            raise SystemExit(f'Member digest mismatch: {name}')
    sums = {}
    for line in read('SHA256SUMS.txt').decode('utf-8-sig').splitlines():
        checksum, name = line.split('  ', 1)
        if name in sums:
            raise SystemExit('Duplicate checksum entry')
        sums[name] = checksum
    if sums != {name: value for name, value in actual.items() if name != 'SHA256SUMS.txt'}:
        raise SystemExit('SHA256SUMS contents mismatch')
    for name in ('engine/Qt6Core.dll', 'engine/msvcp140.dll', 'engine/vcruntime140.dll',
                 'platforms/qwindows.dll', 'licenses/tomlplusplus/LICENSE'):
        if name not in files:
            raise SystemExit(f'Missing runtime/license file: {name}')
    for name, record in info['qt_sdk']['deployed_files'].items():
        if actual[name] != record['sdk_sha256'] or actual[name] != record['deployed_sha256']:
            raise SystemExit(f'Qt SDK digest mismatch: {name}')
    source = info['qt_source']
    if actual[source['archive']] != source['sha256']:
        raise SystemExit('Corresponding Qt source mismatch')
print(json.dumps({'version': info['product_version'], 'commit': args.expected_commit,
                  'dirty': False, 'manifest_files': len(expected), 'zip_crc': 'passed',
                  'sha256': digest}, indent=2))

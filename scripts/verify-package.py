"""Verify portable release provenance, CRC and all SHA-256 manifests.

Two package schemas are recognized:

- legacy (the 0.6.0-rc.1 full bundle): a single ZIP that embedded both the
  runtime and the corresponding Qt source archive under sources/.
- dynamic-split (current): a runtime ZIP without any source archive, plus a
  sibling source-compliance ZIP and release-manifest.json that bind both
  archives to the same clean commit, Qt source hash and SDK provenance.

Pass --schema to pin the expectation. The verifier re-reads every artifact
from disk; nothing the build scripts wrote is trusted on faith.
"""
import argparse
import hashlib
import json
import pathlib
import zipfile

QT_SOURCE_SHA256 = '56001b905601bb9023d399f3ba780d7fa940f3e4861e496a7c490331f49e0b80'
QT_SOURCE_NAME = 'qtbase-everywhere-src-6.8.3.tar.xz'
QT_SOURCE_URL = 'https://download.qt.io/archive/qt/6.8/6.8.3/submodules/' + QT_SOURCE_NAME
REQUIRED_RUNTIME_LEGACY = ('engine/Qt6Core.dll', 'engine/msvcp140.dll',
                            'engine/vcruntime140.dll', 'platforms/qwindows.dll',
                            'licenses/tomlplusplus/LICENSE')
REQUIRED_RUNTIME_UNIFIED = ('CodexProxyGuard.exe', 'platforms/qwindows.dll',
                            'licenses/tomlplusplus/LICENSE')


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


def member_map(bundle: zipfile.ZipFile) -> dict:
    if bundle.testzip() is not None:
        raise SystemExit('ZIP CRC failed')
    entries = [entry for entry in bundle.infolist() if not entry.is_dir()]
    files = {entry.filename.replace('\\', '/'): entry for entry in entries}
    if len(files) != len(entries):
        raise SystemExit('Duplicate ZIP paths')
    for name in files:
        parts = pathlib.PurePosixPath(name).parts
        if name.startswith('/') or '..' in parts or ':' in name or '\x00' in name:
            raise SystemExit(f'Unsafe ZIP path: {name}')
    return files


def verify_member_hashes(bundle: zipfile.ZipFile, files: dict, info: dict) -> dict:
    expected = info['files_sha256']
    if set(files) != set(expected) | {'build-info.json', 'SHA256SUMS.txt'}:
        raise SystemExit('Unmanifested or missing package files')
    actual = {name: digest(bundle.read(entry)) for name, entry in files.items()}
    for name, checksum in expected.items():
        if actual[name] != checksum:
            raise SystemExit(f'Member digest mismatch: {name}')
    sums = {}
    for line in bundle.read(files['SHA256SUMS.txt']).decode('utf-8-sig').splitlines():
        checksum, name = line.split('  ', 1)
        if name in sums:
            raise SystemExit('Duplicate checksum entry')
        sums[name] = checksum
    if sums != {name: value for name, value in actual.items() if name != 'SHA256SUMS.txt'}:
        raise SystemExit('SHA256SUMS contents mismatch')
    return actual


def verify_qt_deployment(actual: dict, info: dict) -> None:
    for name, record in info['qt_sdk']['deployed_files'].items():
        if actual[name] != record['sdk_sha256'] or actual[name] != record['deployed_sha256']:
            raise SystemExit(f'Qt SDK digest mismatch: {name}')


def verify_split(runtime_zip: pathlib.Path, bundle: zipfile.ZipFile, files: dict,
                 info: dict, expected_commit: str, qt_source_sha: str) -> None:
    for name in REQUIRED_RUNTIME_UNIFIED:
        if name not in files:
            raise SystemExit(f'Missing runtime/license file: {name}')
    production = [name for name in files if name.endswith('.exe')
                  and name != 'vc_redist.x64.exe']
    if production != ['CodexProxyGuard.exe']:
        raise SystemExit(f'Unified runtime must contain exactly one production executable: {production}')
    for name in files:
        if name.startswith('sources/'):
            raise SystemExit(f'Source archive must not ship inside the runtime ZIP: {name}')
    actual = verify_member_hashes(bundle, files, info)
    verify_qt_deployment(actual, info)
    qt_source = info['qt_source']
    if qt_source['sha256'] != qt_source_sha or qt_source['url'] != QT_SOURCE_URL:
        raise SystemExit('Runtime build-info Qt source provenance mismatch')

    manifest_path = runtime_zip.parent / 'release-manifest.json'
    if not manifest_path.is_file():
        raise SystemExit('dynamic-split runtime requires release-manifest.json beside it')
    manifest = json.loads(manifest_path.read_text(encoding='utf-8-sig'))
    if (manifest.get('schema_version') != 2 or manifest.get('profile') != 'dynamic-split'
            or isinstance(manifest.get('source_dirty'), str)):
        raise SystemExit('Release manifest schema mismatch')
    if manifest['source_commit'] != expected_commit or manifest['source_dirty'] is not False:
        raise SystemExit('Release manifest does not match the expected clean commit')
    runtime_record = manifest['runtime_archive']
    if runtime_record['name'] != runtime_zip.name:
        raise SystemExit('Release manifest runtime archive name mismatch')
    if runtime_record['sha256'] != check_sidecar(runtime_zip):
        raise SystemExit('Release manifest runtime archive digest mismatch')

    compliance_zip = runtime_zip.parent / manifest['source_compliance_archive']['name']
    if not compliance_zip.is_file():
        raise SystemExit('Source compliance archive listed in the manifest is missing')
    if file_digest(compliance_zip) != manifest['source_compliance_archive']['sha256']:
        raise SystemExit('Source compliance archive digest mismatch')
    compliance_sidecar = pathlib.Path(str(compliance_zip) + '.sha256').read_text(encoding='utf-8-sig').split()
    if compliance_sidecar != [manifest['source_compliance_archive']['sha256'], compliance_zip.name]:
        raise SystemExit('Source compliance sidecar digest mismatch')
    if manifest['qt_source']['sha256'] != qt_source_sha:
        raise SystemExit('Release manifest Qt source digest mismatch')

    with zipfile.ZipFile(compliance_zip) as compliance:
        compliance_files = member_map(compliance)
        required = {'app/CMakeLists.txt', f'upstream/{QT_SOURCE_NAME}', 'manifest.json',
                    'REBUILD.md', 'SOURCE_REVISION.txt', 'SOURCE_ACCESS.txt',
                    'patches/README.txt', 'recipes/toolchain.json', 'recipes/packaging.json',
                    'licenses/THIRD_PARTY_NOTICES.md', 'licenses/Qt/sdk-6.8.3-msvc2022-x64.json',
                    'licenses/tomlplusplus/LICENSE'}
        missing = required - set(compliance_files)
        if missing:
            raise SystemExit(f'Source compliance members missing: {sorted(missing)}')
        if digest(compliance.read(f'upstream/{QT_SOURCE_NAME}')) != qt_source_sha:
            raise SystemExit('Corresponding Qt source mismatch inside compliance archive')
        revision = compliance.read('SOURCE_REVISION.txt').decode('utf-8-sig')
        if expected_commit not in revision:
            raise SystemExit('Source revision does not name the expected commit')
        compliance_manifest = json.loads(compliance.read('manifest.json').decode('utf-8-sig'))
        if compliance_manifest['source_commit'] != expected_commit:
            raise SystemExit('Compliance manifest commit mismatch')
        if compliance_manifest['qt_source']['sha256'] != qt_source_sha:
            raise SystemExit('Compliance manifest Qt source digest mismatch')
        listed = compliance_manifest['files_sha256']
        if 'manifest.json' in listed:
            raise SystemExit('Compliance manifest must not hash itself')
        if set(listed) | {'manifest.json'} != set(compliance_files):
            raise SystemExit('Unmanifested or missing compliance members')
        for name, checksum in listed.items():
            if digest(compliance.read(name)) != checksum:
                raise SystemExit(f'Compliance member digest mismatch: {name}')
        print(json.dumps({'version': info['product_version'], 'commit': expected_commit,
                          'dirty': False, 'profile': 'dynamic-split',
                          'runtime_members': len(info['files_sha256']),
                          'compliance_members': len(compliance_files),
                          'zip_crc': 'passed', 'split_linkage': 'verified'}, indent=2))


def verify_legacy(bundle: zipfile.ZipFile, files: dict, info: dict, expected_commit: str) -> None:
    for name in REQUIRED_RUNTIME_LEGACY:
        if name not in files:
            raise SystemExit(f'Missing runtime/license file: {name}')
    actual = verify_member_hashes(bundle, files, info)
    verify_qt_deployment(actual, info)
    source = info['qt_source']
    if actual[source['archive']] != source['sha256']:
        raise SystemExit('Corresponding Qt source mismatch')
    print(json.dumps({'version': info['product_version'], 'commit': expected_commit,
                      'dirty': False, 'manifest_files': len(info['files_sha256']),
                      'zip_crc': 'passed'}, indent=2))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=pathlib.Path)
    parser.add_argument('--expected-commit', required=True)
    parser.add_argument('--schema', choices=('auto', 'legacy', 'dynamic-split'),
                        default='auto')
    parser.add_argument('--qt-source-sha256', default=QT_SOURCE_SHA256,
                        help='Expected Qt source archive digest (default: the pinned '
                             'official 6.8.3 release); custom Qt rebuilds pass their own')
    args = parser.parse_args()
    check_sidecar(args.archive)
    with zipfile.ZipFile(args.archive) as bundle:
        files = member_map(bundle)
        info = json.loads(bundle.read(files['build-info.json']).decode('utf-8-sig'))
        if info.get('git_commit') != args.expected_commit or info.get('git_dirty') is not False:
            raise SystemExit('Package is not the expected clean commit')
        if info['engine']['language'] != 'C++20' or info['engine']['protocol_version'] != 1:
            raise SystemExit('Unexpected engine provenance')
        detected = 'legacy' if any(name.startswith('sources/') for name in files) \
            else info.get('profile', 'legacy')
        schema = detected if args.schema == 'auto' else args.schema
        if schema != detected:
            raise SystemExit(f'Package does not match the requested schema: {detected}')
        if schema == 'legacy':
            verify_legacy(bundle, files, info, args.expected_commit)
        else:
            verify_split(args.archive, bundle, files, info, args.expected_commit,
                         args.qt_source_sha256)


if __name__ == '__main__':
    main()

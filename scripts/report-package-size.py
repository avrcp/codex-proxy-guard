"""Measure a portable package directory and ZIP without modifying either.

Read-only size evidence: category totals, duplicate content groups, largest
files and ZIP members, per-EXE digests, and provenance when the artifact's
build-info can be trusted. Reports are written outside the measured tree so
they never become part of their own input. All byte counts are logical file
sizes (MiB = bytes / 1,048,576); disk allocation is deliberately not reported.
"""
import argparse
import hashlib
import json
import os
import pathlib
import stat
import zipfile

MIB = 1024 * 1024
TOP = 20

# Package paths whose purpose is source distribution rather than running the
# product. Everything in sources/ ships only for license compliance.
def is_source(relative: str) -> bool:
    return relative.startswith('sources/')


# License text and notices: required distribution, not runtime code.
def is_notice(relative: str) -> bool:
    return (relative in ('LICENSE', 'THIRD_PARTY_NOTICES.md')
            or relative.startswith('licenses/'))


# Manifests, checksums and Qt deployment configuration.
def is_metadata(relative: str) -> bool:
    return (relative in ('build-info.json', 'SHA256SUMS.txt', 'qt.conf')
            or relative.endswith('.sha256')
            or relative.startswith('engine/qt.conf'))


def is_exe(relative: str) -> bool:
    return relative.endswith('.exe')


def classify(relative: str) -> str:
    for name in ('source', 'notice', 'metadata'):
        if {'source': is_source, 'notice': is_notice, 'metadata': is_metadata}[name](relative):
            return name
    return 'runtime'


def digest(path: pathlib.Path) -> str:
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def mib(amount: int) -> float:
    return round(amount / MIB, 3)


def scan_directory(root: pathlib.Path) -> dict:
    files = []
    for current, directories, names in os.walk(root):
        for name in names:
            path = pathlib.Path(current) / name
            info = path.lstat()
            if stat.S_ISLNK(info.st_mode):
                raise SystemExit(f'Reparse/symlink member is out of scope: {path}')
            if path.is_dir():
                continue
            relative = path.relative_to(root).as_posix()
            files.append({'relative': relative, 'bytes': info.st_size, 'sha256': digest(path)})
    if not files:
        raise SystemExit(f'No files under {root}')
    categories = {}
    for entry in files:
        categories.setdefault(classify(entry['relative']), 0)
        categories[classify(entry['relative'])] += entry['bytes']
    by_hash = {}
    for entry in files:
        by_hash.setdefault(entry['sha256'], []).append(entry)
    duplicates = [
        {'sha256': key, 'bytes': entries[0]['bytes'],
         'paths': sorted(entry['relative'] for entry in entries)}
        for key, entries in by_hash.items() if len(entries) > 1
    ]
    duplicates.sort(key=lambda item: item['bytes'] * len(item['paths']), reverse=True)
    return {
        'root': str(root),
        'directory_file_count': len(files),
        'directory_bytes': sum(f['bytes'] for f in files),
        'category_bytes': categories,
        'largest_files': sorted(
            [{'path': f['relative'], 'bytes': f['bytes']} for f in files],
            key=lambda item: item['bytes'], reverse=True)[:TOP],
        'duplicate_content_groups': duplicates,
        'exe': [{'path': f['relative'], 'bytes': f['bytes'], 'sha256': f['sha256']}
                for f in files if is_exe(f['relative'])],
    }


def scan_archive(archive: pathlib.Path) -> dict:
    with zipfile.ZipFile(archive) as bundle:
        if bundle.testzip() is not None:
            raise SystemExit('ZIP CRC check failed')
        members = [entry for entry in bundle.infolist() if not entry.is_dir()]
        names = [entry.filename.replace('\\', '/') for entry in members]
        if len(set(names)) != len(names):
            raise SystemExit('Duplicate ZIP member paths')
        for name in names:
            parts = pathlib.PurePosixPath(name).parts
            if name.startswith('/') or '..' in parts or ':' in name or '\x00' in name:
                raise SystemExit(f'Unsafe ZIP member path: {name}')
        categories = {}
        uncompressed = 0
        compressed = 0
        largest = []
        source_compressed = 0
        for entry, name in zip(members, names):
            category = classify(name)
            categories.setdefault(category, 0)
            categories[category] += entry.file_size
            uncompressed += entry.file_size
            compressed += entry.compress_size
            if category == 'source':
                source_compressed += entry.compress_size
            largest.append({'path': name, 'uncompressed_bytes': entry.file_size,
                            'compressed_bytes': entry.compress_size})
        info = {}
        if 'build-info.json' in names:
            info = json.loads(bundle.read('build-info.json').decode('utf-8-sig'))
    return {
        'archive': str(archive),
        'zip_bytes': archive.stat().st_size,
        'zip_member_count': len(members),
        'zip_members_uncompressed_bytes': uncompressed,
        'zip_members_compressed_bytes': compressed,
        'zip_container_overhead_bytes': archive.stat().st_size - compressed,
        'category_uncompressed_bytes': categories,
        'qt_source_compressed_bytes': source_compressed,
        'largest_zip_members': sorted(
            largest, key=lambda item: item['compressed_bytes'], reverse=True)[:TOP],
        'artifact_commit': info.get('git_commit'),
        'artifact_dirty': info.get('git_dirty'),
        'artifact_product_version': info.get('product_version'),
    }


def render_markdown(directory: dict, archive: dict, source_commit: str) -> str:
    lines = ['# Package size report', '']
    lines.append(f'- Measured directory: `{directory["root"]}`')
    lines.append(f'- Measured archive: `{archive["archive"]}`')
    lines.append(f'- Reporter source commit: `{source_commit}`')
    lines.append(f'- Artifact commit: `{archive.get("artifact_commit")}`'
                 f' (dirty: {archive.get("artifact_dirty")},'
                 f' version: {archive.get("artifact_product_version")})')
    lines.append('')
    lines.append('| Metric | Bytes | MiB |')
    lines.append('|---|---:|---:|')
    rows = [
        ('directory_bytes (all files)', directory['directory_bytes']),
        ('zip_bytes (archive on disk)', archive['zip_bytes']),
        ('zip_members_uncompressed_bytes', archive['zip_members_uncompressed_bytes']),
        ('zip_members_compressed_bytes', archive['zip_members_compressed_bytes']),
        ('zip_container_overhead_bytes', archive['zip_container_overhead_bytes']),
        ('qt_source_compressed_bytes (inside ZIP)', archive['qt_source_compressed_bytes']),
    ]
    for category in ('runtime', 'source', 'notice', 'metadata'):
        rows.append((f'category_bytes[{category}] (directory)',
                     directory['category_bytes'].get(category, 0)))
        rows.append((f'category_uncompressed_bytes[{category}] (ZIP members)',
                     archive['category_uncompressed_bytes'].get(category, 0)))
    for label, amount in rows:
        lines.append(f'| {label} | {amount} | {mib(amount)} |')
    lines.append('')
    lines.append(f'- Directory file count: {directory["directory_file_count"]}')
    lines.append(f'- ZIP member count: {archive["zip_member_count"]}')
    lines.append(f'- Runtime-only download (directory, without source/notice/metadata): '
                 f'{directory["category_bytes"].get("runtime", 0)} bytes')
    lines.append(f'- Full bundle download (directory total): {directory["directory_bytes"]} bytes')
    lines.append('')
    lines.append('## Executables')
    lines.append('')
    lines.append('| Path | Bytes | MiB | SHA-256 |')
    lines.append('|---|---:|---:|---|')
    for entry in directory['exe']:
        lines.append(f'| {entry["path"]} | {entry["bytes"]} | {mib(entry["bytes"])} | `{entry["sha256"]}` |')
    lines.append('')
    lines.append('## Largest files (directory)')
    lines.append('')
    lines.append('| Path | Bytes | MiB |')
    lines.append('|---|---:|---:|')
    for entry in directory['largest_files']:
        lines.append(f'| {entry["path"]} | {entry["bytes"]} | {mib(entry["bytes"])} |')
    lines.append('')
    lines.append('## Largest ZIP members (compressed)')
    lines.append('')
    lines.append('| Path | Compressed bytes | MiB | Uncompressed bytes |')
    lines.append('|---|---:|---:|---:|')
    for entry in archive['largest_zip_members']:
        lines.append(f'| {entry["path"]} | {entry["compressed_bytes"]} | '
                     f'{mib(entry["compressed_bytes"])} | {entry["uncompressed_bytes"]} |')
    if directory['duplicate_content_groups']:
        lines.append('')
        lines.append('## Duplicate content groups (SHA-256)')
        lines.append('')
        for group in directory['duplicate_content_groups']:
            paths = ', '.join(f'`{path}`' for path in group['paths'])
            lines.append(f'- {group["bytes"]} bytes x{len(group["paths"])}: {paths}')
    lines.append('')
    return '\n'.join(lines) + '\n'


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=pathlib.Path, required=True,
                        help='Extracted package directory to measure')
    parser.add_argument('--archive', type=pathlib.Path, required=True,
                        help='Package ZIP to measure')
    parser.add_argument('--output', type=pathlib.Path, required=True,
                        help='Directory for size-report.json/.md (must differ from --root)')
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    archive = args.archive.resolve(strict=True)
    output = args.output.resolve()
    if output == root or root in output.parents or output in archive.parents:
        raise SystemExit('Report output must live outside the measured tree')
    output.mkdir(parents=True, exist_ok=True)
    try:
        import subprocess
        source_commit = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=str(root),
                                       capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        source_commit = None
    directory = scan_directory(root)
    report = scan_archive(archive)
    report['source_commit'] = source_commit
    document = {
        'measured': {'directory': directory, 'archive': report},
    }
    (output / 'size-report.json').write_text(
        json.dumps(document, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    (output / 'size-report.md').write_text(
        render_markdown(directory, report, source_commit or 'unknown'), encoding='utf-8')
    print(f'Wrote {output / "size-report.json"} and size-report.md')


if __name__ == '__main__':
    main()

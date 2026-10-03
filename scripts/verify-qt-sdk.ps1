<#
.SYNOPSIS
Reproduces the official Qt SDK archive-to-file-hash verification.
.DESCRIPTION
Requires Python 3 and py7zr (also installed by aqtinstall). Downloads the pinned
archive once, verifies its SHA-256, then independently extracts every manifest
entry and verifies the recorded file SHA-256 values. No SDK files are modified.
#>
param([string] $ArchivePath = '', [string] $SdkOutputPath = '')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$ManifestPath = Join-Path $Repository 'licenses\Qt\sdk-6.8.3-msvc2022-x64.json'
$Manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
if ([string]::IsNullOrWhiteSpace($ArchivePath)) { $ArchivePath = Join-Path $Repository 'target\qt-source\qtbase-sdk.7z' }
$ArchivePath = [IO.Path]::GetFullPath($ArchivePath)
$Python = Get-Command python.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $Python) { throw 'Python 3 with py7zr is required; install aqtinstall or python -m pip install --user py7zr.' }
& $Python.Source -c 'import py7zr'
if ($LASTEXITCODE -ne 0) { throw 'Install the build dependency with: python -m pip install --user py7zr' }

if (-not (Test-Path -LiteralPath $ArchivePath -PathType Leaf)) {
    New-Item -ItemType Directory -Path (Split-Path $ArchivePath) -Force | Out-Null
    $PartialPath = $ArchivePath + '.download'
    Write-Host 'Downloading the pinned official Qt binary archive for verification...'
    Invoke-WebRequest -UseBasicParsing -Uri $Manifest.archive_url -OutFile $PartialPath -TimeoutSec 900
    $Actual = (Get-FileHash -LiteralPath $PartialPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Actual -ne $Manifest.archive_sha256) { throw 'Downloaded Qt SDK archive SHA-256 mismatch.' }
    Move-Item -LiteralPath $PartialPath -Destination $ArchivePath
}

$Verifier = @'
import hashlib
import json
import pathlib
import sys
import tempfile
import contextlib
import py7zr

def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8-sig'))
archive = pathlib.Path(sys.argv[2]).resolve(strict=True)
if manifest['schema_version'] != 1 or digest(archive) != manifest['archive_sha256']:
    raise SystemExit('Qt SDK archive SHA-256 or manifest schema mismatch')
entries = manifest['files_sha256']
if not entries:
    raise SystemExit('Empty SDK file manifest')
for name in entries:
    path = pathlib.PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or ':' in name or '\\' in name:
        raise SystemExit('Unsafe SDK manifest path')
    if path.parts[0] not in ('bin', 'plugins'):
        raise SystemExit('Unexpected SDK manifest root')

cache = archive.parent.resolve(strict=True)
with contextlib.ExitStack() as cleanup:
    full_sdk = sys.argv[3] != '-'
    if full_sdk:
        output = pathlib.Path(sys.argv[3]).resolve()
        allowed = pathlib.Path(sys.argv[4]).resolve(strict=True) / 'target'
        if not output.is_relative_to(allowed) or output == allowed or output.exists():
            raise SystemExit('SDK extraction requires a new isolated directory under repository target/')
        output.mkdir(parents=True)
    else:
        temporary = cleanup.enter_context(tempfile.TemporaryDirectory(prefix='qt-verify-', dir=cache))
        output = pathlib.Path(temporary).resolve(strict=True)
        # Automatic cleanup is confined to this cache-owned directory.
        if not output.is_relative_to(cache) or output == cache:
            raise SystemExit('Unsafe verification directory')
    with py7zr.SevenZipFile(archive, 'r') as sdk:
        names = sdk.getnames()
        for name in entries:
            if names.count(name) != 1:
                raise SystemExit('Missing or duplicate archive member: ' + name)
        if full_sdk:
            # Every header, import/static library, CMake configuration, tool and
            # plugin used by the build originates from the verified archive.
            sdk.extractall(path=output)
        else:
            sdk.extract(path=output, targets=list(entries))
    for name, expected in entries.items():
        extracted = (output / name).resolve(strict=True)
        if not extracted.is_relative_to(output) or digest(extracted) != expected:
            raise SystemExit('Qt SDK manifest hash mismatch: ' + name)
print('Verified official Qt archive SHA-256 and all ' + str(len(entries)) + ' SDK file hashes.')
if full_sdk:
    print('Extracted complete, unmodified SDK for this build: ' + str(output))
'@
$ExtractionArgument = if ($SdkOutputPath) { [IO.Path]::GetFullPath($SdkOutputPath) } else { '-' }
& $Python.Source -c $Verifier $ManifestPath $ArchivePath $ExtractionArgument $Repository
if ($LASTEXITCODE -ne 0) { throw 'Official Qt SDK archive-to-manifest verification failed.' }

<#
.SYNOPSIS
Collect read-only acceptance evidence for one final release EXE.

.DESCRIPTION
Development-stage helper invoked manually for the A01 acceptance matrix.
It binds the evidence to the actual SHA-256 of the final executable (not
just a source version string) and records the execution environment and
the Authenticode facts of that file. It deliberately does NOT launch the
real Desktop, does not authorize or revoke anything, does not touch a
real Codex Home, does not read auth data, does not scan the machine and
does not upload anything. The only executed command is the EXE's own
`build-info` metadata subcommand, which has no configuration side
effects.

.EXAMPLE
powershell -NoProfile -File scripts\collect-release-evidence.ps1 `
    -Executable dist\releases\<version>-<commit>\CodexProxyGuard.exe `
    -EvidenceDirectory target\evidence\W01 `
    -TestId W01
#>
param(
    [Parameter(Mandatory = $true)] [string] $Executable,
    [Parameter(Mandatory = $true)] [string] $EvidenceDirectory,
    [string] $TestId = ''
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ExecutablePath = [IO.Path]::GetFullPath($Executable)
if (-not (Test-Path -LiteralPath $ExecutablePath -PathType Leaf)) {
    throw "Executable not found: $ExecutablePath"
}
$EvidencePath = [IO.Path]::GetFullPath($EvidenceDirectory)
if (Test-Path -LiteralPath $EvidencePath) {
    throw "Evidence directory already exists, refusing to overwrite: $EvidencePath"
}
New-Item -ItemType Directory -Path $EvidencePath -Force | Out-Null

function Get-Hash([string] $Path) {
    $Stream = [IO.File]::OpenRead($Path)
    $Hasher = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($Hasher.ComputeHash($Stream))).Replace('-', '').ToLowerInvariant() }
    finally { $Hasher.Dispose(); $Stream.Dispose() }
}

$StartedUtc = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$ExeHash = Get-Hash $ExecutablePath
$ExeSize = (Get-Item -LiteralPath $ExecutablePath).Length

# Fixed metadata command only - no configuration side effects.
$BuildInfoCommand = 'build-info'
$BuildInfoRaw = ''
$BuildInfo = $null
try {
    $BuildInfoRaw = (& $ExecutablePath $BuildInfoCommand | Out-String).Trim()
    $BuildInfo = $BuildInfoRaw | ConvertFrom-Json
} catch {
    $BuildInfoRaw = "EXECUTION FAILED: $($_.Exception.Message)"
}

$Os = Get-CimInstance Win32_OperatingSystem
$Terminal = if ($Env:WT_SESSION) { 'Windows Terminal' }
            elseif ($Env:TERM_PROGRAM) { "unknown ($Env:TERM_PROGRAM)" }
            elseif ($Env:ConEmuANSI) { "unknown (ConEmu)" } else { 'unknown' }

$Signature = Get-AuthenticodeSignature -LiteralPath $ExecutablePath
$Evidence = [ordered]@{
    test_id = $TestId
    started_utc = $StartedUtc
    ended_utc = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    executable = [ordered]@{
        path = $ExecutablePath
        size_bytes = $ExeSize
        sha256 = $ExeHash
        build_info_command = $BuildInfoCommand
        build_info = $BuildInfo
        build_info_raw = $BuildInfoRaw
    }
    environment = [ordered]@{
        os_version = $Os.Version
        os_build = $Os.BuildNumber
        os_architecture = $Os.OSArchitecture
        terminal = $Terminal
        user_profile = [Environment]::UserName
        machine = $Env:COMPUTERNAME
    }
    authenticode = [ordered]@{
        status = [string]$Signature.Status
        signer_subject = if ($Signature.SignerCertificate) { [string]$Signature.SignerCertificate.Subject } else { '' }
        signer_thumbprint = if ($Signature.SignerCertificate) { [string]$Signature.SignerCertificate.Thumbprint } else { '' }
        timestamp_signer = if ($Signature.TimeStamperCertificate) { [string]$Signature.TimeStamperCertificate.Subject } else { '' }
        signature_type = if ($Signature.PSObject.Properties['SignatureType']) { [string]$Signature.SignatureType } else { 'unknown' }
    }
    notes = 'read-only collection; no Desktop launch, no Home access, no network'
}
$EvidenceFile = Join-Path $EvidencePath 'evidence.json'
[IO.File]::WriteAllText($EvidenceFile,
    ($Evidence | ConvertTo-Json -Depth 6) + [Environment]::NewLine,
    (New-Object Text.UTF8Encoding($false)))
Write-Host "evidence written: $EvidenceFile"
Write-Host "  exe sha256=$ExeHash ($ExeSize bytes)"

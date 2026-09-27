<#
.SYNOPSIS
    Reads registered Codex Desktop application metadata, exact executable
    registration, and optional per-process identity observations.

.DESCRIPTION
    Strictly read-only: this script never starts, stops, activates, or
    debugs any application, never reads environment blocks or business
    command lines, and never prints file contents. For each requested PID it
    reports package identity, application identity (AUMID), token elevation,
    and top-level window ownership (via GetWindowThreadProcessId) so a popup
    can be attributed to the process that actually owns it.
.PARAMETER ProcessId
    Exact process IDs to observe. The script never starts or stops an application.
#>
param([int[]] $ProcessId = @())

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class PackageIdentityProbe {
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool CloseHandle(IntPtr handle);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetPackageFullName(IntPtr process, ref uint length, StringBuilder name);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetApplicationUserModelId(IntPtr process, ref uint length, StringBuilder aumid);

    [DllImport("advapi32.dll", SetLastError = true)]
    public static extern bool OpenProcessToken(IntPtr process, uint desiredAccess, out IntPtr token);

    [DllImport("advapi32.dll", SetLastError = true)]
    public static extern bool GetTokenInformation(IntPtr token, int infoClass, out uint info, uint infoLength, out uint returnedLength);
}
'@

Add-Type -Namespace Win32 -Name Windows -MemberDefinition @'
[DllImport("user32.dll")]
public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
[DllImport("user32.dll")]
public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder text, int maxCount);
[DllImport("user32.dll")]
public static extern int GetWindowTextLength(IntPtr hWnd);
[DllImport("user32.dll")]
public static extern bool IsWindowVisible(IntPtr hWnd);
'@

$applications = @(
    foreach ($name in @('OpenAI.Codex', 'OpenAI.ChatGPT-Desktop')) {
        foreach ($package in @(Get-AppxPackage -Name $name)) {
            [xml] $manifest = Get-AppxPackageManifest -Package $package.PackageFullName
            $apps = @($manifest.SelectNodes("/*[local-name()='Package']/*[local-name()='Applications']/*[local-name()='Application']"))
            if ($apps.Count -gt 16) { throw 'APPX_APPLICATION_LIMIT: too many registered applications' }
            foreach ($app in $apps) {
                $aliases = @($app.SelectNodes(".//*[local-name()='Extension' and @Category='windows.appExecutionAlias']//*[local-name()='ExecutionAlias']") | ForEach-Object { [string] $_.GetAttribute('Alias') })
                # The exact executable file Windows would run for this entry
                # (read-only existence and version probe; never executed).
                $executable = Join-Path $package.InstallLocation ([string] $app.GetAttribute('Executable'))
                $executableDetail = $null
                if (Test-Path -LiteralPath $executable -PathType Leaf) {
                    $executableDetail = [ordered]@{
                        exists     = $true
                        size_bytes = (Get-Item -LiteralPath $executable).Length
                    }
                } else {
                    $executableDetail = [ordered]@{ exists = $false }
                }
                [pscustomobject]@{
                    PackageName       = [string] $package.Name
                    PackageFullName   = [string] $package.PackageFullName
                    PackageFamilyName = [string] $package.PackageFamilyName
                    Version           = [string] $package.Version
                    Architecture      = [string] $package.Architecture
                    Status            = [string] $package.Status
                    InstallLocation   = [string] $package.InstallLocation
                    ApplicationId     = [string] $app.GetAttribute('Id')
                    Aumid             = "$($package.PackageFamilyName)!$($app.GetAttribute('Id'))"
                    Executable        = [string] $app.GetAttribute('Executable')
                    ExecutableDetail  = $executableDetail
                    EntryPoint        = [string] $app.GetAttribute('EntryPoint')
                    RuntimeBehavior   = [string] (@($app.Attributes | Where-Object LocalName -eq 'RuntimeBehavior' | Select-Object -First 1 -ExpandProperty Value) -join '')
                    TrustLevel        = [string] (@($app.Attributes | Where-Object LocalName -eq 'TrustLevel' | Select-Object -First 1 -ExpandProperty Value) -join '')
                    ExecutionAliases  = $aliases
                }
            }
        }
    }
)

# Ownership of visible top-level windows, so an unexpected popup can be
# attributed to the exact process that created it. Titles may be redacted
# downstream; only the prefix is kept here.
$windowsByPid = @{}
$collector = {
    param([IntPtr] $hWnd, [IntPtr] $lParam)
    if ([Win32.Windows]::IsWindowVisible($hWnd)) {
        $ownerPid = 0
        [void] [Win32.Windows]::GetWindowThreadProcessId($hWnd, [ref] $ownerPid)
        if ($ownerPid -ne 0) {
            $length = [Win32.Windows]::GetWindowTextLength($hWnd)
            if ($length -gt 0) {
                $text = [Text.StringBuilder]::new($length + 1)
                [void] [Win32.Windows]::GetWindowText($hWnd, $text, $text.Capacity)
                $title = $text.ToString()
                if ($title.Length -gt 60) { $title = $title.Substring(0, 60) + '...' }
                if (-not $windowsByPid.ContainsKey([int] $ownerPid)) {
                    $windowsByPid[[int] $ownerPid] = @()
                }
                $windowsByPid[[int] $ownerPid] += $title
            }
        }
    }
    return $true
}
[void] [Win32.Windows]::EnumWindows($collector, [IntPtr]::Zero)

function Get-TokenElevation([IntPtr] $ProcessHandle) {
    $token = [IntPtr]::Zero
    if (-not [PackageIdentityProbe]::OpenProcessToken($ProcessHandle, 0x0008, [ref] $token)) {
        return 'query_failed'
    }
    try {
        $info = [uint32] 0
        $returned = [uint32] 0
        if (-not [PackageIdentityProbe]::GetTokenInformation($token, 20, [ref] $info, 4, [ref] $returned)) {
            return 'query_failed'
        }
        if ($info -ne 0) { return 'elevated' } else { return 'not_elevated' }
    } finally {
        [void] [PackageIdentityProbe]::CloseHandle($token)
    }
}

function Get-ProcessAumid([IntPtr] $ProcessHandle) {
    [uint32] $length = 0
    $status = [PackageIdentityProbe]::GetApplicationUserModelId($ProcessHandle, [ref] $length, $null)
    if ($status -eq 15700 -or $status -eq 15703) { return $null }
    if ($status -ne 122 -or $length -lt 2 -or $length -gt 1024) {
        return "#query_failed:$status"
    }
    $buffer = [Text.StringBuilder]::new([int] $length)
    $status = [PackageIdentityProbe]::GetApplicationUserModelId($ProcessHandle, [ref] $length, $buffer)
    if ($status -ne 0) { return "#query_failed:$status" }
    return $buffer.ToString()
}

$identities = @(
    foreach ($pidValue in $ProcessId) {
        if ($pidValue -le 0) { throw 'ProcessId must be positive' }
        $handle = [PackageIdentityProbe]::OpenProcess(0x1000, $false, $pidValue)
        if ($handle -eq [IntPtr]::Zero) {
            [pscustomobject]@{ ProcessId = $pidValue; State = 'open_failed'; Status = [Runtime.InteropServices.Marshal]::GetLastWin32Error(); PackageFullName = $null; Aumid = $null; Elevation = 'query_failed'; VisibleWindowTitles = @() }
            continue
        }
        try {
            [uint32] $length = 0
            $status = [PackageIdentityProbe]::GetPackageFullName($handle, [ref] $length, $null)
            $packageFullName = $null
            $state = 'query_failed'
            if ($status -eq 15700) {
                $state = 'unpackaged'
            } elseif ($status -eq 122 -and $length -ge 2 -and $length -le 1024) {
                $buffer = [Text.StringBuilder]::new([int] $length)
                $status = [PackageIdentityProbe]::GetPackageFullName($handle, [ref] $length, $buffer)
                if ($status -eq 0) {
                    $state = 'packaged'
                    $packageFullName = $buffer.ToString()
                }
            }
            [pscustomobject]@{
                ProcessId           = $pidValue
                State               = $state
                Status              = $status
                PackageFullName     = $packageFullName
                Aumid               = Get-ProcessAumid $handle
                Elevation           = Get-TokenElevation $handle
                VisibleWindowTitles = @($windowsByPid[[int] $pidValue])
            }
        } finally {
            [void] [PackageIdentityProbe]::CloseHandle($handle)
        }
    }
)

[pscustomobject]@{
    OsVersion                 = [Environment]::OSVersion.Version.ToString()
    PowerShellVersion         = $PSVersionTable.PSVersion.ToString()
    HasApplicationActivation  = $true
    HasPackageContextCommand  = [bool] (Get-Command Invoke-CommandInDesktopPackage -ErrorAction SilentlyContinue)
    Applications              = $applications
    ProcessIdentities         = $identities
} | ConvertTo-Json -Depth 7

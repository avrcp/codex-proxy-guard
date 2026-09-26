<#
.SYNOPSIS
    Reads registered Codex Desktop application metadata and optional process package identities.
.PARAMETER ProcessId
    Exact process IDs to query. The script never starts or stops an application.
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
}
'@

$applications = @(
    foreach ($name in @('OpenAI.Codex', 'OpenAI.ChatGPT-Desktop')) {
        foreach ($package in @(Get-AppxPackage -Name $name)) {
            [xml] $manifest = Get-AppxPackageManifest -Package $package.PackageFullName
            $apps = @($manifest.SelectNodes("/*[local-name()='Package']/*[local-name()='Applications']/*[local-name()='Application']"))
            if ($apps.Count -gt 16) { throw 'APPX_APPLICATION_LIMIT: too many registered applications' }
            foreach ($app in $apps) {
                $aliases = @($app.SelectNodes(".//*[local-name()='Extension' and @Category='windows.appExecutionAlias']//*[local-name()='ExecutionAlias']") | ForEach-Object { [string] $_.GetAttribute('Alias') })
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
                    EntryPoint        = [string] $app.GetAttribute('EntryPoint')
                    RuntimeBehavior   = [string] (@($app.Attributes | Where-Object LocalName -eq 'RuntimeBehavior' | Select-Object -First 1 -ExpandProperty Value) -join '')
                    TrustLevel        = [string] (@($app.Attributes | Where-Object LocalName -eq 'TrustLevel' | Select-Object -First 1 -ExpandProperty Value) -join '')
                    ExecutionAliases  = $aliases
                }
            }
        }
    }
)

$identities = @(
    foreach ($pidValue in $ProcessId) {
        if ($pidValue -le 0) { throw 'ProcessId must be positive' }
        $handle = [PackageIdentityProbe]::OpenProcess(0x1000, $false, $pidValue)
        if ($handle -eq [IntPtr]::Zero) {
            [pscustomobject]@{ ProcessId = $pidValue; State = 'open_failed'; Status = [Runtime.InteropServices.Marshal]::GetLastWin32Error(); PackageFullName = $null }
            continue
        }
        try {
            [uint32] $length = 0
            $status = [PackageIdentityProbe]::GetPackageFullName($handle, [ref] $length, $null)
            if ($status -eq 15700) {
                [pscustomobject]@{ ProcessId = $pidValue; State = 'unpackaged'; Status = $status; PackageFullName = $null }
            } elseif ($status -ne 122 -or $length -lt 2 -or $length -gt 1024) {
                [pscustomobject]@{ ProcessId = $pidValue; State = 'query_failed'; Status = $status; PackageFullName = $null }
            } else {
                $buffer = [Text.StringBuilder]::new([int] $length)
                $status = [PackageIdentityProbe]::GetPackageFullName($handle, [ref] $length, $buffer)
                [pscustomobject]@{
                    ProcessId       = $pidValue
                    State           = if ($status -eq 0) { 'packaged' } else { 'query_failed' }
                    Status          = $status
                    PackageFullName = if ($status -eq 0) { $buffer.ToString() } else { $null }
                }
            }
        } finally {
            [void] [PackageIdentityProbe]::CloseHandle($handle)
        }
    }
)

[pscustomobject]@{
    OsVersion                = [Environment]::OSVersion.Version.ToString()
    PowerShellVersion        = $PSVersionTable.PSVersion.ToString()
    HasPackageContextCommand = [bool] (Get-Command Invoke-CommandInDesktopPackage -ErrorAction SilentlyContinue)
    Applications             = $applications
    ProcessIdentities        = $identities
} | ConvertTo-Json -Depth 7

# Bounded, read-only APPX discovery for Codex Proxy Guard.
#
# Output contract (consumed verbatim by backend/src/platform.cpp):
#   * a single fixed-schema envelope: {"schema_version":1,"records":[...]};
#   * the envelope is emitted even when no supported package is installed
#     (records = []), so "no package" is always distinguishable from a failed
#     or silent script;
#   * applications[] is always an array of objects (explicit -Depth is
#     mandatory: ConvertTo-Json's default of 2 truncates this nesting);
#   * optional manifest attributes (RuntimeBehavior, TrustLevel) are emitted
#     as JSON null when the manifest omits them -- absence is legal protocol
#     state, not an error;
#   * stdout is strict UTF-8; a successful run always produces exactly one
#     JSON document.
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

function Read-ManifestAttribute($element, [string]$name) {
  $attribute = @($element.Attributes | Where-Object { $_.LocalName -eq $name })
  if ($attribute.Count -gt 1) { throw "APPX_DISCOVERY_INVALID: ambiguous manifest attribute $name" }
  if ($attribute.Count -eq 1) { return [string]$attribute[0].Value }
  return $null
}

$records = @(
  foreach ($name in @('OpenAI.Codex', 'OpenAI.ChatGPT-Desktop')) {
    $package = Get-AppxPackage -Name $name | Sort-Object Version -Descending | Select-Object -First 1
    if ($null -ne $package) {
      $manifest = Get-AppxPackageManifest -Package $package.PackageFullName
      $applications = @(
        $manifest.Package.Applications.Application | ForEach-Object {
          [PSCustomObject]@{
            application_id = [string]$_.Id
            manifest_executable = [string]$_.Executable
            entry_point = [string]$_.EntryPoint
            runtime_behavior = Read-ManifestAttribute $_ 'RuntimeBehavior'
            trust_level = Read-ManifestAttribute $_ 'TrustLevel'
          }
        }
      )
      if ($applications.Count -gt 16) { throw 'APPX_DISCOVERY_INVALID: package has too many applications' }
      [PSCustomObject]@{
        package_name = [string]$package.Name
        package_full_name = [string]$package.PackageFullName
        package_family_name = [string]$package.PackageFamilyName
        package_version = [string]$package.Version
        architecture = [string]$package.Architecture
        install_location = [string]$package.InstallLocation
        applications = $applications
      }
    }
  }
)

$payload = [PSCustomObject]@{
  schema_version = 1
  records = @($records)
}

$payload | ConvertTo-Json -Depth 8 -Compress

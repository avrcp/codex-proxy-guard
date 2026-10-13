<#
.SYNOPSIS
Authenticode signature facts for the canonical release pipeline.

.DESCRIPTION
Dot-sourced by build-portable.ps1. Publisher identity, the signer thumbprint
and the timestamp fact come from the actual signature object
(Get-AuthenticodeSignature), never from parsing signtool's localized,
multi-line text output. The caller still runs `signtool verify /pa /all`
(exit-code based) and scripts/verify-package.py still performs an independent
WinVerifyTrust pass; the three layers do not cancel each other out.
#>
Set-StrictMode -Version Latest

function Get-SignatureMetadata {
    param(
        [Parameter(Mandatory = $true)] [string] $Path,
        [string] $ExpectedThumbprint = ''
    )
    $Signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ([string]$Signature.Status -ne 'Valid') {
        throw "Authenticode signature of $Path is '$($Signature.Status)', not Valid."
    }
    if (-not $Signature.SignerCertificate) {
        throw "Authenticode signature of $Path has no signer certificate."
    }
    if ($Signature.PSObject.Properties['SignatureType'] -and
        [string]$Signature.SignatureType -ne 'Embedded') {
        # This project signs the built EXE with embedded Authenticode; a file
        # that merely matches a system catalog is not evidence of that.
        throw "Signature of $Path is '$($Signature.SignatureType)', not an embedded Authenticode signature."
    }
    $Signer = ('' + $Signature.SignerCertificate.Thumbprint).ToLowerInvariant() -replace '[^0-9a-f]', ''
    if ($ExpectedThumbprint) {
        $Expected = $ExpectedThumbprint.ToLowerInvariant() -replace '[^0-9a-f]', ''
        if ($Signer -ne $Expected) {
            throw "Signer thumbprint $Signer does not match the requested certificate $Expected."
        }
    }
    [pscustomobject]@{
        publisher = [string]$Signature.SignerCertificate.Subject
        signer_thumbprint = $Signer
        timestamp_present = [bool]$Signature.TimeStamperCertificate
        signature_type = if ($Signature.PSObject.Properties['SignatureType']) {
            [string]$Signature.SignatureType
        } else {
            'unknown'
        }
    }
}

<#
.SYNOPSIS
  Creates the self-signed identity certificate used for tpt-focus sparse-package
  development and CI.

.DESCRIPTION
  The certificate is written to build/certs/:
    tpt-focus-dev.cer  - public certificate (import into TrustedPeople to trust)
    tpt-focus-dev.pfx  - private key used by SignTool (gitignored)

  Subject must stay in sync with Identity/@Publisher in
  packaging/windows/AppxManifest.xml (default: CN=TPT Solutions).

.EXAMPLE
  ./scripts/windows/new-dev-cert.ps1
  ./scripts/windows/new-dev-cert.ps1 -Subject "CN=TPT Solutions" -ValidYears 3
#>
[CmdletBinding()]
param(
    [string]$Subject = "CN=TPT Solutions",
    [int]$ValidYears = 2,
    [string]$OutDir = (Join-Path $PSScriptRoot "..\..\build\certs"),
    [string]$Password = "tpt-focus-dev"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$existing = Get-ChildItem Cert:\CurrentUser\My -ErrorAction SilentlyContinue |
    Where-Object { $_.Subject -eq $Subject -and $_.HasPrivateKey }

if ($existing) {
    Write-Host "Reusing existing certificate for $Subject ($($existing[0].Thumbprint))"
    $cert = $existing[0]
} else {
    Write-Host "Creating self-signed certificate for $Subject"
    $cert = New-SelfSignedCertificate `
        -Type Custom `
        -Subject $Subject `
        -KeyUsage DigitalSignature `
        -KeyLength 2048 `
        -KeyAlgorithm RSA `
        -HashAlgorithm SHA256 `
        -CertStoreLocation Cert:\CurrentUser\My `
        -NotAfter (Get-Date).AddYears($ValidYears) `
        -TextExtension @(
            "2.5.29.37={text}1.3.6.1.5.5.7.3.3",  # code signing EKU
            "2.5.29.19={text}"                     # basic constraints (subject only)
        )
}

$cerPath = Join-Path $OutDir "tpt-focus-dev.cer"
$pfxPath = Join-Path $OutDir "tpt-focus-dev.pfx"

Export-Certificate -Cert $cert -FilePath $cerPath -Force | Out-Null
$secure = ConvertTo-SecureString -String $Password -Force -AsPlainText
Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $secure -Force | Out-Null

Write-Host ""
Write-Host "Certificate : $($cert.Thumbprint)"
Write-Host "Public cert : $cerPath"
Write-Host "Private key : $pfxPath  (gitignored)"
Write-Host ""
Write-Host "To trust it for local package deployment:"
Write-Host "  Import-Certificate -FilePath `"$cerPath`" -CertStoreLocation Cert:\CurrentUser\TrustedPeople\"

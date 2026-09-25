<#
.SYNOPSIS
  Builds, packs and signs the tpt-focus sparse package, then deploys it so the
  tray process gains package identity (required for UserNotificationListener).

.EXAMPLE
  ./scripts/windows/package-sparse.ps1 -DevCert
  ./scripts/windows/package-sparse.ps1 -CertPath release\tpt-focus-release.pfx -Password $env:CERT_PASSWORD
#>
[CmdletBinding(DefaultParameterSetName = "DevCert")]
param(
    [Parameter(ParameterSetName = "DevCert")]
    [switch]$DevCert,

    [Parameter(ParameterSetName = "Release", Mandatory)]
    [string]$CertPath,

    [Parameter(ParameterSetName = "Release")]
    [string]$Password,

    [string]$Configuration = "release",
    [string]$OutDir = (Join-Path $PSScriptRoot "..\..\build\windows"),
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
$manifest = Join-Path $repoRoot "packaging\windows\AppxManifest.xml"
$staging = Join-Path $OutDir "staging"

if (-not $SkipBuild) {
    Write-Host "==> cargo build --release"
    cargo build --release --workspace
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
New-Item -ItemType Directory -Force -Path $staging | Out-Null

Write-Host "==> staging binaries"
Copy-Item (Join-Path $repoRoot "target\release\tpt-focus-tray.exe") $staging
Copy-Item (Join-Path $repoRoot "target\release\tpt-focus.exe") $staging

$appx = Join-Path $OutDir "tpt-focus.appx"
if (Test-Path $appx) { Remove-Item -Force $appx }

Write-Host "==> MakeAppx pack"
& MakeAppx pack /p $appx /f $manifest /d $staging /o
if ($LASTEXITCODE -ne 0) { throw "MakeAppx failed" }

if ($DevCert) {
    $certDir = Join-Path $repoRoot "build\certs"
    if (-not (Test-Path (Join-Path $certDir "tpt-focus-dev.pfx"))) {
        & (Join-Path $PSScriptRoot "new-dev-cert.ps1")
    }
    $CertPath = Join-Path $certDir "tpt-focus-dev.pfx"
    $Password = "tpt-focus-dev"
}

Write-Host "==> SignTool"
$secure = if ($Password) { ConvertTo-SecureString $Password -Force -AsPlainText } else { $null }
& SignTool sign /fd SHA256 /f $CertPath $(if ($secure) { "/p", $Password }) $appx
if ($LASTEXITCODE -ne 0) { throw "SignTool failed" }

Write-Host "==> Add-AppxPackage (external location: $staging)"
$existing = Get-AppxPackage -Name "TPT.Focus" -ErrorAction SilentlyContinue
if ($existing) { Remove-AppxPackage -Package $existing.PackageFullName }

Add-AppxPackage -Path $appx -ExternalLocation $staging
if ($LASTEXITCODE -ne 0) { throw "Add-AppxPackage failed" }

Write-Host ""
Write-Host "Package identity installed. Run target\release\tpt-focus-tray.exe to"
Write-Host "exercise UserNotificationListener consent."

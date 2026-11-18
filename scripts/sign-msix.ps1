<#
.SYNOPSIS
  Make dist\Swirvo-<version>-x64-test.msix: a copy of the package signed with a
  throwaway self-signed certificate, so you can install it on your own PC before
  submitting the unsigned original to the Store.

.DESCRIPTION
  Store submissions don't need this: upload the unsigned .msix and Microsoft signs it.
  The test certificate's subject must equal the manifest's Publisher (identity.env).

  To install the signed package (needs an administrator PowerShell once, to trust the
  certificate):
    Import-Certificate -FilePath dist\swirvo-test.cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople
    Add-AppxPackage dist\Swirvo-<version>-x64-test.msix
  Remove it again with:  Get-AppxPackage *Swirvo* | Remove-AppxPackage

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\sign-msix.ps1
#>
param(
  [string]$Msix = "",
  [string]$Publisher = ""
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $Publisher) {
  $line = Get-Content (Join-Path $root "packaging\msix\identity.env") | Where-Object { $_ -match '^PUBLISHER=' }
  $Publisher = ($line -replace '^PUBLISHER=', '').Trim('"')
}
if (-not $Msix) {
  $Msix = Get-ChildItem (Join-Path $root "dist") -Filter "Swirvo-*-x64.msix" | Where-Object { $_.Name -notlike "*-test.msix" } | Sort-Object LastWriteTime | Select-Object -Last 1 -ExpandProperty FullName
}
if (-not $Msix -or -not (Test-Path $Msix)) { throw "No MSIX found: run scripts/build-msix.sh first." }
# Sign a copy: the unsigned original is the one to upload to the Store.
$Signed = $Msix -replace '\.msix$', '-test.msix'
Copy-Item $Msix $Signed -Force

$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName | Select-Object -Last 1 -ExpandProperty FullName
if (-not $signtool) { throw "signtool.exe not found (install the Windows SDK)." }

# Reuse the test certificate for this Publisher if there is one.
$cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Subject -eq $Publisher -and $_.FriendlyName -eq "Swirvo MSIX test" } | Select-Object -First 1
if (-not $cert) {
  $cert = New-SelfSignedCertificate -Type Custom -Subject $Publisher -KeyUsage DigitalSignature `
    -FriendlyName "Swirvo MSIX test" -CertStoreLocation Cert:\CurrentUser\My `
    -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
}
$cer = Join-Path $root "dist\swirvo-test.cer"
Export-Certificate -Cert $cert -FilePath $cer | Out-Null
& $signtool sign /fd SHA256 /sha1 $cert.Thumbprint /s My $Signed
if ($LASTEXITCODE -ne 0) { throw "signtool failed" }
Write-Host "Signed copy for local testing: $Signed (upload the unsigned $Msix to the Store)."
Write-Host "Trust the test certificate once (administrator PowerShell):"
Write-Host "  Import-Certificate -FilePath `"$cer`" -CertStoreLocation Cert:\LocalMachine\TrustedPeople"
Write-Host "Then install:  Add-AppxPackage `"$Signed`""

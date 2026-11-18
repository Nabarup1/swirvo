<#
.SYNOPSIS
  Code-sign Swirvo.exe and swirvo-cli.exe for direct download (website, GitHub
  Releases, winget). The Microsoft Store does NOT need this: it signs MSIX packages for
  free.

.DESCRIPTION
  Unsigned downloads get the "Windows protected your PC" (SmartScreen) warning and more
  antivirus false positives. Two ways to sign:

  1. An OV code-signing certificate (individuals in India: about $300-500 a year from a
     CA such as Sectigo, DigiCert, GlobalSign or SSL.com). Since 2023 the private key
     must live on a hardware token or in the CA's cloud signing service; install the
     token's driver, and the certificate shows up in your Windows certificate store.
       powershell -File scripts\sign-exe.ps1 -Thumbprint <cert thumbprint>

  2. Azure Trusted Signing (about $10/month), if your account is eligible (individual
     developers: US and Canada only at the time of writing). Needs the Trusted Signing
     dlib and a metadata.json naming your account and certificate profile:
       powershell -File scripts\sign-exe.ps1 -AzureDlib <path\Azure.CodeSigning.Dlib.dll> -AzureMetadata <metadata.json>

  Every signature gets an RFC 3161 timestamp, so it stays valid after the certificate
  expires. SmartScreen reputation still builds up over the first weeks of downloads.
#>
param(
  [string]$Thumbprint = "",
  [string]$AzureDlib = "",
  [string]$AzureMetadata = "",
  [string]$Dir = "",
  [string]$TimestampUrl = "http://timestamp.digicert.com"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $Dir) { $Dir = Join-Path $root "dist\Swirvo" }
$files = @("Swirvo.exe", "swirvo-cli.exe") | ForEach-Object { Join-Path $Dir $_ } | Where-Object { Test-Path $_ }
if (-not $files) { throw "Nothing to sign in ${Dir}: run scripts/build-windows-exe.sh first." }
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName | Select-Object -Last 1 -ExpandProperty FullName
if (-not $signtool) { throw "signtool.exe not found (install the Windows SDK)." }

if ($AzureDlib) {
  if (-not $AzureMetadata) { throw "-AzureMetadata is needed with -AzureDlib" }
  & $signtool sign /v /fd SHA256 /tr $TimestampUrl /td SHA256 /dlib $AzureDlib /dmdf $AzureMetadata @files
} elseif ($Thumbprint) {
  & $signtool sign /v /fd SHA256 /tr $TimestampUrl /td SHA256 /sha1 $Thumbprint @files
} else {
  throw "Give -Thumbprint (an OV certificate in your store) or -AzureDlib/-AzureMetadata. See Get-Help .\scripts\sign-exe.ps1 -Full"
}
if ($LASTEXITCODE -ne 0) { throw "signtool failed" }
& $signtool verify /pa /v @files

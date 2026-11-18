# Swirvo: Windows 11 developer setup.
# Run from an elevated PowerShell after reading it. Idempotent: packages already installed are skipped.
# Open a NEW terminal afterwards so PATH changes apply.

$ErrorActionPreference = 'Stop'

function Install-WingetPackage([string]$Id, [string]$Override = '') {
    Write-Host "==> $Id"
    $wingetArgs = @('install', '--id', $Id, '-e', '--accept-package-agreements', '--accept-source-agreements', '--silent')
    if ($Override) { $wingetArgs += @('--override', $Override) }
    & winget @wingetArgs
    if ($LASTEXITCODE -ne 0) {
        # A non-zero code is normal when the package is already installed; check the message above.
        Write-Warning "winget exited with $LASTEXITCODE for $Id"
    }
}

# MSVC toolset + Windows 11 SDK: needed to link Rust (MSVC target) and to build C/C++ dependencies.
Install-WingetPackage 'Microsoft.VisualStudio.2022.BuildTools' `
    '--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.Windows11SDK.26100 --includeRecommended'

$packages = @(
    'Rustlang.Rustup',
    'Kitware.CMake',
    'Ninja-build.Ninja',
    'LLVM.LLVM',                               # clang/libclang for bindgen
    'Python.Python.3.12',
    'Gyan.FFmpeg',                             # dev/testing only; we ship our own LGPL build
    'TheDocumentFoundation.LibreOffice',       # dev/testing of the Office pack
    'JohnMacFarlane.Pandoc',                   # dev/testing of the Docs pack
    'Microsoft.Sysinternals.ProcessMonitor',
    'Microsoft.Sysinternals.ProcessExplorer',
    'Microsoft.AccessibilityInsights'
)
foreach ($p in $packages) { Install-WingetPackage $p }

# Refresh PATH for this session so rustup/cargo are found.
$env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')

if (Get-Command rustup -ErrorAction SilentlyContinue) {
    rustup default stable-x86_64-pc-windows-msvc
    rustup component add rustfmt clippy
    rustup target add aarch64-pc-windows-msvc
    cargo install --locked cargo-nextest cargo-deny cargo-about just
    cargo install --locked tauri-cli --version '^2'
} else {
    Write-Warning 'rustup not on PATH yet. Open a new terminal and re-run this script to install the cargo tools.'
}

# vcpkg for native libraries (libvips and friends).
$vcpkg = 'C:\dev\vcpkg'
if (-not (Test-Path $vcpkg)) {
    New-Item -ItemType Directory -Force -Path 'C:\dev' | Out-Null
    git clone https://github.com/microsoft/vcpkg $vcpkg
    & "$vcpkg\bootstrap-vcpkg.bat" -disableMetrics
}
[Environment]::SetEnvironmentVariable('VCPKG_ROOT', $vcpkg, 'User')

Write-Host ''
Write-Host 'Done. Install these by hand when needed:'
Write-Host '  * Windows Performance Toolkit (Windows ADK) for WPR/WPA latency traces'
Write-Host '  * PresentMon for frame timing'
Write-Host '  * MSIX Packaging Tool (Microsoft Store) for packaging experiments'

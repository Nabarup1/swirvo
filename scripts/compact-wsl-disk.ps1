# Give disk space back to Windows after deleting files inside WSL.
#
# WSL keeps each distro in a virtual disk file (ext4.vhdx) that grows on demand
# but never shrinks by itself. Deleting files in Linux frees space *inside* the
# disk only; this script compacts the file so Windows gets that space back.
#
# Run from an elevated PowerShell (Run as administrator):
#   powershell -ExecutionPolicy Bypass -File \\wsl.localhost\Ubuntu\home\<you>\swirvo\scripts\compact-wsl-disk.ps1
#
# It shuts WSL down first (every distro, VS Code remote windows, Docker Desktop),
# so save your work. Nothing inside Linux is changed or deleted: the disk is
# attached read-only and only unused blocks are dropped. Works on Windows Home
# (diskpart); uses Optimize-VHD instead when Hyper-V's module is installed.

#Requires -RunAsAdministrator
param(
    [string]$Distro = 'Ubuntu',
    [string]$Vhd = ''          # Override when the distro is registered under another user.
)

$ErrorActionPreference = 'Stop'

function Get-FreeGB { [math]::Round((Get-PSDrive C).Free / 1GB, 1) }

if (-not $Vhd) {
    $key = Get-ChildItem HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss |
        Where-Object { $_.GetValue('DistributionName') -eq $Distro }
    if (-not $key) { throw "No WSL distro named '$Distro' for this user. Pass -Vhd <path-to-ext4.vhdx>." }
    $Vhd = Join-Path $key.GetValue('BasePath') 'ext4.vhdx'
}
if (-not (Test-Path -LiteralPath $Vhd)) { throw "Disk file not found: $Vhd" }

$before = (Get-Item -LiteralPath $Vhd).Length
Write-Host ("Disk file: {0}" -f $Vhd)
Write-Host ("Before: {0:N1} GB on disk, C: has {1} GB free" -f ($before / 1GB), (Get-FreeGB))

# 1. Tell the virtual disk which blocks Linux no longer uses (safe, online).
Write-Host '==> Trimming unused blocks inside the distro'
& wsl.exe -d $Distro -u root -- fstrim -av
if ($LASTEXITCODE -ne 0) { Write-Warning "fstrim exited with $LASTEXITCODE; continuing (the disk is mounted with discard)." }

# 2. Stop WSL so the file is not in use.
Write-Host '==> Shutting down WSL'
& wsl.exe --shutdown
for ($i = 0; $i -lt 30; $i++) {
    $running = (& wsl.exe -l --running -q 2>$null) -replace "`0", '' | Where-Object { $_.Trim() -eq $Distro }
    if (-not $running) { break }
    Start-Sleep -Seconds 1
}
Start-Sleep -Seconds 3

# 3. Compact.
if (Get-Command Optimize-VHD -ErrorAction SilentlyContinue) {
    Write-Host '==> Compacting with Optimize-VHD'
    Optimize-VHD -Path $Vhd -Mode Full
} else {
    Write-Host '==> Compacting with diskpart (a few minutes for a large disk)'
    $script = Join-Path $env:TEMP 'compact-wsl-disk.txt'
    @(
        "select vdisk file=`"$Vhd`"",
        'attach vdisk readonly',
        'compact vdisk',
        'detach vdisk'
    ) | Set-Content -LiteralPath $script -Encoding ascii
    try {
        & diskpart.exe /s $script
        if ($LASTEXITCODE -ne 0) { throw "diskpart exited with $LASTEXITCODE" }
    } finally {
        Remove-Item -LiteralPath $script -ErrorAction SilentlyContinue
    }
}

$after = (Get-Item -LiteralPath $Vhd).Length
Write-Host ("After:  {0:N1} GB on disk ({1:N1} GB returned), C: has {2} GB free" -f ($after / 1GB), (($before - $after) / 1GB), (Get-FreeGB))
Write-Host 'Done. Open your WSL terminal or VS Code again to start the distro.'

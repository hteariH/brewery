# A Windows Dev Drive for CI, after Zed's script/setup-dev-driver.ps1: a ReFS VHDX that
# Defender doesn't scan, holding CARGO_HOME, RUSTUP_HOME and the cargo target directory.
# `target` in the checkout becomes a junction to the drive, so every `target/...` path in
# the workflow (artifact uploads, the exe checks) keeps working. Run it after the checkout
# and before anything that uses cargo; it writes the new environment to GITHUB_ENV.
param(
    # Dynamic VHDX: it only takes the space that is written. The release-fast build needs ~30 GB.
    [int64]$SizeBytes = 100GB
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$workspace = $env:GITHUB_WORKSPACE
if (-not $workspace) { throw "GITHUB_WORKSPACE is not set" }
Get-PSDrive -PSProvider FileSystem | Format-Table Name, @{ n = 'Used GB'; e = { [math]::Round($_.Used / 1GB) } }, @{ n = 'Free GB'; e = { [math]::Round($_.Free / 1GB) } }

# On the checkout's drive (D: on hosted runners).
$vhdx = Join-Path (Split-Path -Qualifier $workspace) "brewery_dev_drive.vhdx"
$volume = New-VHD -Path $vhdx -SizeBytes $SizeBytes -Dynamic |
    Mount-VHD -Passthru |
    Initialize-Disk -Passthru |
    New-Partition -AssignDriveLetter -UseMaximumSize |
    Format-Volume -DevDrive -Confirm:$false -Force

# Trusted, and no antivirus filter on Dev Drives
# (https://learn.microsoft.com/en-us/windows/dev-drive/#how-do-i-configure-additional-filters-on-dev-drive).
fsutil devdrv trust "$($volume.DriveLetter):"
fsutil devdrv enable /disallowAv
# Remount so the filter settings apply; the drive letter can change.
Dismount-VHD -Path $vhdx
Mount-VHD -Path $vhdx
$letter = (Get-VHD -Path $vhdx | Get-Disk | Get-Partition | Get-Volume | Where-Object DriveLetter).DriveLetter
if (-not $letter) { throw "the Dev Drive has no drive letter after remounting" }
$drive = "${letter}:"
fsutil devdrv query $drive
Write-Output "Dev Drive at $drive ($vhdx)"
try {
    Get-MpComputerStatus | Format-List RealTimeProtectionEnabled, AntivirusEnabled
} catch {
    Write-Output "Defender status unavailable: $_"
}

# Cargo (with rustup's proxies in bin) moves over. The toolchain from rust-toolchain.toml
# is installed into the new RUSTUP_HOME, on the drive as well.
$cargoHome = "$drive\.cargo"
$rustupHome = "$drive\.rustup"
New-Item -Path "$cargoHome\bin" -ItemType Directory -Force | Out-Null
New-Item -Path $rustupHome -ItemType Directory -Force | Out-Null
$oldCargo = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { "$env:USERPROFILE\.cargo" }
Copy-Item -Path "$oldCargo\*" -Destination "$cargoHome\" -Recurse -Force
$env:CARGO_HOME = $cargoHome
$env:RUSTUP_HOME = $rustupHome
$env:PATH = "$cargoHome\bin;$env:PATH"
Push-Location $workspace
try {
    rustup toolchain install
    rustup show active-toolchain
} finally {
    Pop-Location
}

$target = "$drive\target"
New-Item -Path $target -ItemType Directory -Force | Out-Null
$link = Join-Path $workspace "target"
if (Test-Path $link) { throw "$link already exists; run this before anything writes to target" }
New-Item -ItemType Junction -Path $link -Target $target | Out-Null

Add-Content -Path $env:GITHUB_ENV -Value @(
    "DEV_DRIVE=$drive"
    "CARGO_HOME=$cargoHome"
    "RUSTUP_HOME=$rustupHome"
)
Add-Content -Path $env:GITHUB_PATH -Value "$cargoHome\bin"

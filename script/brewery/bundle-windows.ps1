# Builds Brewery's Windows installer (Inno Setup) from already-built binaries and the offline
# bundle. A Brewery-owned replacement for script/bundle-windows.ps1: no cargo build, no remote
# server, no appx context menu, no AMD AGS, no Sentry, no signing (the hook stays in brewery.iss).
#
#   powershell -File script/brewery/bundle-windows.ps1 `
#     -App target/release-fast/brewery.exe -Cli target/release-fast/cli.exe `
#     -Helper target/release-fast/auto_update_helper.exe -Bundle target/bundle/windows-x64
#
# Output: <Out>/Brewery-<version>-x86_64-setup.exe (other channels: Brewery-<channel>-...).
# The only download is the ConPTY NuGet package (pinned, SHA-256 checked, cached).
[CmdletBinding()]
Param(
    # brewery.exe from `cargo build -p zed` (installed as Brewery.exe)
    [Parameter(Mandatory = $true)][string]$App,
    # cli.exe from `cargo build -p cli` (installed as bin\brewery.exe)
    [Parameter(Mandatory = $true)][string]$Cli,
    # auto_update_helper.exe (installed as tools\auto_update_helper.exe)
    [Parameter(Mandatory = $true)][string]$Helper,
    # script/brewery/fetch_bundle_deps.py output for windows-x64
    [Parameter(Mandatory = $true)][string]$Bundle,
    # Defaults to crates/zed/BREWERY_VERSION
    [string]$Version,
    # Defaults to crates/zed/RELEASE_CHANNEL; must match the channel the binaries were built for
    [ValidateSet('', 'stable', 'preview', 'nightly', 'dev')][string]$Channel,
    # Output directory for the installer; defaults to target/installer
    [string]$Out,
    # Zed's generated third-party licences (assets/licenses.md from script/generate-licenses)
    [string]$RustLicenses,
    # Download cache, shared with fetch_bundle_deps.py
    [string]$Cache,
    # ISCC.exe; defaults to PATH, then the usual Inno Setup 6 install locations
    [string]$Iscc
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Workspace = (Resolve-Path "$PSScriptRoot\..\..").Path
$Resources = Join-Path $Workspace 'crates\zed\resources\windows'

# microsoft/terminal's ConPTY package, the same release script/bundle-windows.ps1 uses.
$ConptyUrl = 'https://github.com/microsoft/terminal/releases/download/v1.25.1912.0/Microsoft.Windows.Console.ConPTY.1.25.260710002-preview.nupkg'
$ConptySha256 = '05fe9b571ea4fb198f5012405cb39a132cf23eee50feaa496524c149b2502692'

function Fail([string]$Message) {
    throw "bundle-windows: $Message"
}

function Require-File([string]$Path, [string]$What) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { Fail "$What not found: $Path" }
    (Resolve-Path -LiteralPath $Path).Path
}

function Write-Text([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text.Replace("`r`n", "`n"), (New-Object Text.UTF8Encoding($false)))
}

function Copy-Licence([string]$From, [string]$ToDir, [string]$Name) {
    New-Item -ItemType Directory -Force -Path $ToDir | Out-Null
    Copy-Item -LiteralPath $From -Destination (Join-Path $ToDir $Name) -Force
}

# --- inputs ------------------------------------------------------------------------------

if (-not $Version) { $Version = (Get-Content (Join-Path $Workspace 'crates\zed\BREWERY_VERSION') -TotalCount 1).Trim() }
if (-not $Channel) { $Channel = (Get-Content (Join-Path $Workspace 'crates\zed\RELEASE_CHANNEL') -TotalCount 1).Trim() }
$NumericVersion = $Version -replace '[-+].*$', ''
if ($NumericVersion -notmatch '^\d+\.\d+\.\d+$') { Fail "version '$Version' is not x.y.z[-suffix]" }
if (-not $Out) { $Out = Join-Path $Workspace 'target\installer' }
if (-not $Cache) { $Cache = Join-Path $HOME '.cache\brewery-bundle' }

$App = Require-File $App 'app (brewery.exe)'
$Cli = Require-File $Cli 'CLI (cli.exe)'
$Helper = Require-File $Helper 'update helper (auto_update_helper.exe)'
if (-not (Test-Path -LiteralPath $Bundle -PathType Container)) { Fail "bundle directory not found: $Bundle" }
$Bundle = (Resolve-Path -LiteralPath $Bundle).Path
foreach ($entry in @('jre\bin\java.exe', 'brewery-daemon\brewery-daemon.jar', 'jdtls\brewery-jdtls.cmd', 'extensions\java', 'licenses\README.txt')) {
    if (-not (Test-Path -LiteralPath (Join-Path $Bundle $entry))) { Fail "bundle is incomplete, missing $entry (run fetch_bundle_deps.py with --daemon-jar)" }
}
if ($RustLicenses) {
    $RustLicenses = Require-File $RustLicenses 'Rust licences (licenses.md)'
} else {
    Write-Warning 'no -RustLicenses: the installer lacks the Rust dependency licences, do not release it'
}

# The binaries carry the channel's product name and the version (windows_resources): an
# installer for one channel must not ship another channel's build.
$DisplayName = switch ($Channel) { 'stable' { 'Brewery' } 'preview' { 'Brewery Preview' } 'nightly' { 'Brewery Nightly' } default { 'Brewery Dev' } }
foreach ($exe in @($App, $Cli, $Helper)) {
    $info = (Get-Item -LiteralPath $exe).VersionInfo
    if ($info.ProductName -ne $DisplayName) { Fail "$exe has ProductName '$($info.ProductName)', expected '$DisplayName' (channel $Channel)" }
}
$appVersion = (Get-Item -LiteralPath $App).VersionInfo.FileVersion
if ($appVersion -ne $NumericVersion) { Fail "$App has FileVersion '$appVersion', expected '$NumericVersion'" }

if (-not $Iscc) {
    $found = Get-Command iscc -ErrorAction SilentlyContinue
    $candidates = @()
    if ($found) { $candidates += $found.Source }
    $candidates += @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe")
    $Iscc = $candidates | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
    if (-not $Iscc) { Fail 'ISCC.exe (Inno Setup 6) not found; pass -Iscc or install it (winget install JRSoftware.InnoSetup)' }
}

Write-Host "bundle-windows: Brewery $Version ($Channel), bundle $Bundle"

# --- ConPTY ------------------------------------------------------------------------------

New-Item -ItemType Directory -Force -Path $Cache | Out-Null
$conptyPkg = Join-Path $Cache ("conpty-" + $ConptySha256.Substring(0, 12) + ".nupkg")
if (-not (Test-Path -LiteralPath $conptyPkg) -or (Get-FileHash -Algorithm SHA256 -LiteralPath $conptyPkg).Hash -ne $ConptySha256) {
    Write-Host "  fetch   $ConptyUrl"
    $partial = "$conptyPkg.part"
    Invoke-WebRequest -UseBasicParsing -Uri $ConptyUrl -OutFile $partial
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $partial).Hash
    if ($actual -ne $ConptySha256) {
        Remove-Item -LiteralPath $partial -Force
        Fail "ConPTY package checksum mismatch: got $actual, expected $ConptySha256"
    }
    Move-Item -LiteralPath $partial -Destination $conptyPkg -Force
} else {
    Write-Host '  cached  ConPTY'
}

# --- payload: the tree as installed ------------------------------------------------------

$stage = Join-Path $Workspace 'target\inno\x86_64'
$payload = Join-Path $stage 'payload'
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
foreach ($dir in @('bin', 'tools', 'x64', 'arm64', 'conpty')) { New-Item -ItemType Directory -Force -Path (Join-Path $payload $dir) | Out-Null }

Copy-Item -LiteralPath $App -Destination (Join-Path $payload 'Brewery.exe')
Copy-Item -LiteralPath $Cli -Destination (Join-Path $payload 'bin\brewery.exe')
Write-Text (Join-Path $payload 'bin\brewery') (Get-Content -Raw -LiteralPath (Join-Path $Resources 'brewery.sh'))
Copy-Item -LiteralPath $Helper -Destination (Join-Path $payload 'tools\auto_update_helper.exe')

Add-Type -AssemblyName System.IO.Compression.FileSystem
$conptyDir = Join-Path $stage 'conpty'
[IO.Compression.ZipFile]::ExtractToDirectory($conptyPkg, $conptyDir)
Copy-Item -LiteralPath (Join-Path $conptyDir 'runtimes\win-x64\native\conpty.dll') -Destination (Join-Path $payload 'conpty.dll')
Copy-Item -LiteralPath (Join-Path $conptyDir 'build\native\runtimes\x64\OpenConsole.exe') -Destination (Join-Path $payload 'x64\OpenConsole.exe')
# As Zed does: an x64 build on an Arm64 machine uses the native Arm64 console host.
Copy-Item -LiteralPath (Join-Path $conptyDir 'build\native\runtimes\arm64\OpenConsole.exe') -Destination (Join-Path $payload 'arm64\OpenConsole.exe')
Remove-Item -LiteralPath (Join-Path $payload 'conpty') -Force

foreach ($dir in @('jre', 'brewery-daemon', 'jdtls', 'extensions', 'licenses')) {
    Copy-Item -LiteralPath (Join-Path $Bundle $dir) -Destination (Join-Path $payload $dir) -Recurse
}

# --- licences: the bundle's, plus the app's own --------------------------------------------

$licenses = Join-Path $payload 'licenses'
Copy-Licence (Join-Path $Workspace 'LICENSE-GPL') (Join-Path $licenses 'brewery') 'LICENSE-GPL.txt'
Copy-Licence (Join-Path $Workspace 'brewery-daemon\LICENSE') (Join-Path $licenses 'brewery-daemon') 'LICENSE.txt'
Copy-Licence (Join-Path $Workspace 'crates\brewery_theme\icons\NOTICE') (Join-Path $licenses 'intellij-icons') 'NOTICE.txt'
Copy-Licence (Join-Path $Workspace 'LICENSE-APACHE') (Join-Path $licenses 'intellij-icons') 'LICENSE-APACHE.txt'
Copy-Licence (Join-Path $Workspace 'script\brewery\licenses\microsoft-terminal-LICENSE.txt') (Join-Path $licenses 'conpty') 'LICENSE.txt'
$fontNames = @()
foreach ($font in Get-ChildItem -Directory (Join-Path $Workspace 'assets\fonts')) {
    $texts = @(Get-ChildItem -File -LiteralPath $font.FullName -Filter '*.txt')
    if ($texts.Count -eq 0) { Fail "font $($font.Name) has no licence text" }
    foreach ($text in $texts) { Copy-Licence $text.FullName (Join-Path $licenses "fonts\$($font.Name)") $text.Name }
    $fontNames += $font.Name
}
$rustLine = '  Not included in this build (no licenses.md was passed to bundle-windows.ps1).'
if ($RustLicenses) {
    Copy-Licence $RustLicenses (Join-Path $licenses 'zed') 'licenses.md'
    $rustLine = '  Files:   licenses/zed/licenses.md'
}

$readme = Join-Path $licenses 'README.txt'
$appIndex = @"

Brewery $Version (Brewery.exe, bin\brewery.exe, tools\auto_update_helper.exe)
  License: GPL-3.0-or-later
  Source:  https://github.com/hteariH/brewery
  A fork of Zed (https://github.com/zed-industries/zed), GPL-3.0-or-later.
  Files:   licenses/brewery/LICENSE-GPL.txt
  The brewery-daemon's own Apache-2.0 text: licenses/brewery-daemon/LICENSE.txt

Rust crates, themes and icons compiled into Brewery.exe
$rustLine

IntelliJ Platform icons (JetBrains/intellij-community)
  License: Apache-2.0
  Files:   licenses/intellij-icons/

Fonts ($($fontNames -join ', '))
  License: SIL Open Font License 1.1
  Files:   licenses/fonts/

ConPTY and OpenConsole (microsoft/terminal v1.25.1912.0)
  License: MIT
  Files:   licenses/conpty/LICENSE.txt
"@
Write-Text $readme ((Get-Content -Raw -LiteralPath $readme) + $appIndex)

# --- Inno Setup ------------------------------------------------------------------------------

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path
$setupName = if ($Channel -eq 'stable') { "Brewery-$Version-x86_64-setup" } else { "Brewery-$Channel-$Version-x86_64-setup" }
$innoArgs = @(
    '/Qp',
    "/dChannel=$Channel",
    "/dVersion=$Version",
    "/dNumericVersion=$NumericVersion",
    "/dPayloadDir=$payload",
    "/dResourcesDir=$Resources",
    "/dOutputDir=$Out",
    "/dAppSetupName=$setupName",
    (Join-Path $Resources 'brewery.iss')
)
Write-Host "  iscc    $Iscc"
& $Iscc @innoArgs
Write-Host ''
if ($LASTEXITCODE -ne 0) { Fail "ISCC failed with exit code $LASTEXITCODE" }

$setup = Join-Path $Out "$setupName.exe"
if (-not (Test-Path -LiteralPath $setup)) { Fail "ISCC did not produce $setup" }
$size = (Get-Item -LiteralPath $setup).Length
Write-Host ("bundle-windows: {0} ({1:N1} MB)" -f $setup, ($size / 1MB))
if ($env:GITHUB_OUTPUT) { Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "setup=$setup" }

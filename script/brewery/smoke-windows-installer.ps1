# Smoke test for the Brewery Windows installer: silent per-user install into a temp dir, file
# and registry asserts, `bin\brewery.exe --version`, `jre\bin\java -version`, an update-mode run
# (/update=true) checked against the layout the update helper expects, the helper's swap itself
# (--launch false, so the app is never started), then a silent uninstall.
#
#   powershell -File script/brewery/smoke-windows-installer.ps1 -Setup target/installer/Brewery-0.1.0-x86_64-setup.exe
#
# -SkipShellIntegration deselects PATH, file associations and the Explorer menus, for runs on
# a developer machine; CI runs with the installer's default tasks and checks those too.
[CmdletBinding()]
Param(
    [Parameter(Mandatory = $true)][string]$Setup,
    # Defaults to crates/zed/BREWERY_VERSION
    [string]$Version,
    [switch]$SkipShellIntegration,
    [switch]$SkipHelper
)

$ErrorActionPreference = 'Stop'
$Workspace = (Resolve-Path "$PSScriptRoot\..\..").Path
if (-not $Version) { $Version = (Get-Content (Join-Path $Workspace 'crates\zed\BREWERY_VERSION') -TotalCount 1).Trim() }
$Setup = (Resolve-Path -LiteralPath $Setup).Path

# RUNNER_TEMP on CI: the runner's %TEMP% is an 8.3 path (RUNNER~1), which the CLI prints long.
$tempBase = $env:RUNNER_TEMP
if (-not $tempBase) { $tempBase = [IO.Path]::GetTempPath() }
$root = Join-Path $tempBase ("brewery-smoke-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
$dir = Join-Path $root 'Brewery'
$logs = Join-Path $root 'logs'
New-Item -ItemType Directory -Force -Path $logs | Out-Null
$failures = New-Object Collections.Generic.List[string]

function Check([bool]$Condition, [string]$Message) {
    if ($Condition) { Write-Host "  ok    $Message" } else { Write-Host "  FAIL  $Message"; $script:failures.Add($Message) }
}

function Invoke-Captured([string]$File, [string[]]$Arguments) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $File
    $info.Arguments = ($Arguments | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) -join ' '
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.CreateNoWindow = $true
    $process = [Diagnostics.Process]::Start($info)
    $stderr = $process.StandardError.ReadToEndAsync()
    $stdout = $process.StandardOutput.ReadToEnd()
    $process.WaitForExit()
    [pscustomobject]@{ ExitCode = $process.ExitCode; Output = ($stdout + $stderr.Result).Trim() }
}

function Run-Setup([string[]]$Arguments, [string]$Log, [string]$Exe = $Setup) {
    $all = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', "/LOG=`"$Log`"") + $Arguments
    $process = Start-Process -FilePath $Exe -ArgumentList $all -Wait -PassThru
    $process.ExitCode
}

# Relative paths of every file under $Base, lowercase, sorted.
function Get-Tree([string]$Base) {
    if (-not (Test-Path -LiteralPath $Base)) { return @() }
    $prefix = (Resolve-Path -LiteralPath $Base).Path.TrimEnd('\').Length + 1
    @(Get-ChildItem -LiteralPath $Base -Recurse -File -Force | ForEach-Object { $_.FullName.Substring($prefix).ToLowerInvariant() } | Sort-Object)
}

function Get-RegValue([string]$Key, [string]$Name) {
    $handle = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Key)
    if (-not $handle) { return $null }
    try { $handle.GetValue($Name, $null, 'DoNotExpandEnvironmentNames') } finally { $handle.Close() }
}

function Get-UserPath { [string](Get-RegValue 'Environment' 'Path') }

function Find-UninstallKey {
    $base = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
    Get-ChildItem $base -ErrorAction SilentlyContinue | Where-Object {
        $location = (Get-ItemProperty -LiteralPath $_.PSPath -ErrorAction SilentlyContinue).InstallLocation
        $location -and ($location.TrimEnd('\') -ieq $dir)
    } | Select-Object -First 1
}

# Everything the update helper swaps, as a fresh install lays it out (brewery.rs jobs()).
$SwappedFiles = @('brewery.exe', 'bin\brewery.exe', 'bin\brewery', 'conpty.dll', 'x64\openconsole.exe', 'arm64\openconsole.exe')
$SwappedDirs = @('jre', 'brewery-daemon', 'jdtls', 'extensions')
function Select-Swapped([string[]]$Tree) {
    @($Tree | Where-Object { ($SwappedFiles -contains $_) -or ($SwappedDirs -contains ($_ -split '\\')[0]) })
}

$pathBefore = Get-UserPath
$tasks = @()
if ($SkipShellIntegration) { $tasks = @('/MERGETASKS=!addtopath,!associatewithfiles,!addcontextmenufiles,!addcontextmenufolders') }

try {
    Write-Host "smoke: $Setup"
    Write-Host "smoke: install into $dir"
    $code = Run-Setup (@("/DIR=`"$dir`"") + $tasks) (Join-Path $logs 'install.log')
    Check ($code -eq 0) "setup exited with $code"

    $top = @(Get-ChildItem -LiteralPath $dir -Force | ForEach-Object { $_.Name.ToLowerInvariant() } | Sort-Object)
    $expectedTop = @('arm64', 'bin', 'brewery-daemon', 'brewery.exe', 'conpty.dll', 'extensions', 'jdtls', 'jre', 'licenses', 'tools', 'unins000.dat', 'unins000.exe', 'x64') | Sort-Object
    Check ((Compare-Object $expectedTop $top | Measure-Object).Count -eq 0) "top level is exactly: $($expectedTop -join ' ') (got: $($top -join ' '))"
    foreach ($file in @('Brewery.exe', 'bin\brewery.exe', 'bin\brewery', 'tools\auto_update_helper.exe', 'conpty.dll', 'x64\OpenConsole.exe',
            'jre\bin\java.exe', 'brewery-daemon\brewery-daemon.jar', 'jdtls\brewery-jdtls.cmd', 'jdtls\tools\lombok.jar',
            'jdtls\tools\gradle-lsp-bridge.exe', 'licenses\gradle-ls\NOTICE.txt', 'licenses\README.txt', 'licenses\brewery\LICENSE-GPL.txt', 'licenses\jre\LICENSE', 'licenses\conpty\LICENSE.txt')) {
        Check (Test-Path -LiteralPath (Join-Path $dir $file) -PathType Leaf) "installed $file"
    }
    Check (@(Get-ChildItem -LiteralPath (Join-Path $dir 'extensions\java') -Force).Count -gt 0) 'installed extensions\java\'
    Check (-not (Select-String -LiteralPath (Join-Path $dir 'bin\brewery') -Pattern "`r" -SimpleMatch -Quiet)) 'bin\brewery (WSL shim) has LF line endings'

    $cli = Invoke-Captured (Join-Path $dir 'bin\brewery.exe') @('--version')
    Write-Host "        brewery --version: $($cli.Output)"
    Check (($cli.ExitCode -eq 0) -and ($cli.Output -match '^Brewery ') -and $cli.Output.Contains($Version)) "bin\brewery.exe --version names Brewery $Version"
    Check ($cli.Output.ToLowerInvariant().Contains((Join-Path $dir 'Brewery.exe').ToLowerInvariant())) 'the CLI finds the installed Brewery.exe'
    $java = Invoke-Captured (Join-Path $dir 'jre\bin\java.exe') @('-version')
    Write-Host "        java -version: $(($java.Output -split "`n")[0])"
    Check (($java.ExitCode -eq 0) -and ($java.Output -match 'version "21\.')) 'jre\bin\java -version is 21'

    $uninstallKey = Find-UninstallKey
    Check ($null -ne $uninstallKey) 'uninstall entry in HKCU'
    if ($uninstallKey) {
        $props = Get-ItemProperty -LiteralPath $uninstallKey.PSPath
        Write-Host "        $($uninstallKey.PSChildName): $($props.DisplayName) $($props.DisplayVersion), $($props.Publisher)"
        Check ($props.DisplayName -like 'Brewery*') "DisplayName is Brewery ($($props.DisplayName))"
        Check ($props.DisplayVersion -eq $Version) "DisplayVersion is $Version"
        Check ($props.Publisher -eq 'Brewery contributors') 'Publisher is Brewery contributors'
    }
    $zedCommand = [string](Get-RegValue 'Software\Classes\zed\shell\open\command' '')
    Check (-not $zedCommand.ToLowerInvariant().Contains($dir.ToLowerInvariant())) 'zed:// is not registered to this install'
    if (-not $SkipShellIntegration) {
        Check ((Get-UserPath).ToLowerInvariant().Contains("$dir\bin".ToLowerInvariant())) 'bin\ added to the user PATH'
        $shell = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes\*\shell')
        $verbs = @(); if ($shell) { $verbs = $shell.GetSubKeyNames() | Where-Object { $_ -like 'Brewery*' }; $shell.Close() }
        Check ($verbs.Count -gt 0) "file context menu verb ($($verbs -join ', '))"
        foreach ($verb in $verbs) {
            Check (([string](Get-RegValue "Software\Classes\*\shell\$verb\command" '')).Contains("$dir\Brewery.exe")) "  $verb runs Brewery.exe"
            Check (([string](Get-RegValue "Software\Classes\directory\shell\$verb\command" '')).Contains("$dir\Brewery.exe")) "  folder menu $verb runs Brewery.exe"
            Check ($null -ne (Get-RegValue 'Software\Classes\.java\OpenWithProgids' "$verb.java")) "  .java lists $verb.java under OpenWithProgids"
        }
    }

    # --- update mode: what the app does after downloading the next version ------------------
    Write-Host 'smoke: update mode (/update=true)'
    $before = Get-Tree $dir
    # The app saves the download as updates\Brewery-setup.exe and runs it from there.
    $updateSetup = Join-Path $dir 'updates\Brewery-setup.exe'
    New-Item -ItemType Directory -Force -Path (Split-Path $updateSetup) | Out-Null
    Copy-Item -LiteralPath $Setup -Destination $updateSetup
    $code = Run-Setup @('/update=true', '/MERGETASKS=!desktopicon') (Join-Path $logs 'update.log') $updateSetup
    Check ($code -eq 0) "update-mode setup exited with $code"
    Check ($null -ne (Find-UninstallKey)) 'uninstall entry still in HKCU'
    $installTree = Get-Tree (Join-Path $dir 'install')
    $expectedInstall = Select-Swapped $before
    $diff = @(Compare-Object $expectedInstall $installTree)
    Check ($diff.Count -eq 0) "install\ holds exactly the swapped files ($($installTree.Count) files)"
    if ($diff.Count -gt 0) { $diff | Select-Object -First 20 | ForEach-Object { Write-Host "        $($_.SideIndicator) $($_.InputObject)" } }
    foreach ($file in @('Brewery.exe', 'bin\brewery.exe', 'bin\brewery', 'conpty.dll', 'x64\OpenConsole.exe', 'jre\bin\java.exe', 'brewery-daemon\brewery-daemon.jar', 'jdtls\brewery-jdtls.cmd')) {
        Check (Test-Path -LiteralPath (Join-Path $dir "install\$file") -PathType Leaf) "install\$file"
    }
    foreach ($entry in @('tools', 'licenses', 'unins000.exe')) {
        Check (-not (Test-Path -LiteralPath (Join-Path $dir "install\$entry"))) "no install\$entry (written in place)"
    }
    $flag = Join-Path $dir 'updates\versions.txt'
    Check ((Test-Path -LiteralPath $flag) -and ((Get-Content -LiteralPath $flag -TotalCount 1).Trim() -eq $Version)) "updates\versions.txt says $Version"
    Check (@(Compare-Object $before (Get-Tree $dir | Where-Object { -not ($_.StartsWith('install\') -or $_.StartsWith('updates\')) })).Count -eq 0) 'the installed tree is untouched outside install\ and updates\'

    if (-not $SkipHelper) {
        Write-Host 'smoke: update helper (--launch false)'
        $helper = Start-Process -FilePath (Join-Path $dir 'tools\auto_update_helper.exe') -ArgumentList @('--launch', 'false') -PassThru
        if (-not $helper.WaitForExit(120000)) { $helper.Kill(); Check $false 'update helper finished within 120 s' }
        $helperLog = Join-Path $dir 'tools\auto_update_helper.log'
        $logText = ''; if (Test-Path -LiteralPath $helperLog) { $logText = Get-Content -Raw -LiteralPath $helperLog }
        Copy-Item -LiteralPath $helperLog -Destination $logs -ErrorAction SilentlyContinue
        foreach ($leftover in @('install', 'updates', 'old')) {
            Check (-not (Test-Path -LiteralPath (Join-Path $dir $leftover))) "no $leftover\ after the swap"
        }
        $after = @(Get-Tree $dir | Where-Object { $_ -ne 'tools\auto_update_helper.log' })
        Check (@(Compare-Object $before $after).Count -eq 0) 'the swapped tree matches the fresh install'
        Check (-not ($logText -match 'rollback|Autoupdate failed|inconsistent')) 'helper log reports no failure or rollback'
        $cli = Invoke-Captured (Join-Path $dir 'bin\brewery.exe') @('--version')
        Check (($cli.ExitCode -eq 0) -and $cli.Output.Contains($Version)) 'bin\brewery.exe --version after the swap'
    }
}
finally {
    # --- uninstall ------------------------------------------------------------------------------
    $uninstaller = Join-Path $dir 'unins000.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        Write-Host 'smoke: uninstall'
        $process = Start-Process -FilePath $uninstaller -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$(Join-Path $logs 'uninstall.log')`"") -Wait -PassThru
        Check ($process.ExitCode -eq 0) "uninstaller exited with $($process.ExitCode)"
        # The uninstaller finishes from a temp copy of itself; give it a moment to remove the dir.
        $deadline = (Get-Date).AddSeconds(60)
        while ((Test-Path -LiteralPath $dir) -and ((Get-Date) -lt $deadline)) { Start-Sleep -Milliseconds 500 }
        Check (-not (Test-Path -LiteralPath $dir)) 'install directory is gone'
        if (Test-Path -LiteralPath $dir) { Get-ChildItem -LiteralPath $dir -Recurse -Force | Select-Object -First 20 | ForEach-Object { Write-Host "        left: $($_.FullName)" } }
        Check ($null -eq (Find-UninstallKey)) 'uninstall entry removed'
        Check (-not (Get-UserPath).ToLowerInvariant().Contains("$dir\bin".ToLowerInvariant())) 'bin\ is not on the user PATH'
        if (-not $SkipShellIntegration) {
            $entries = { param($p) @($p -split ';' | Where-Object { $_ }) -join ';' }
            Check ((& $entries (Get-UserPath)) -eq (& $entries $pathBefore)) 'user PATH has the same entries as before'
            Check ($null -eq [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes\*\shell\BreweryDev') -and
                $null -eq [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes\*\shell\Brewery')) 'file context menu removed'
        }
    } else {
        Check $false "no uninstaller at $uninstaller"
    }
    Write-Host "smoke: logs in $logs"
}

if ($failures.Count -gt 0) {
    Write-Host "smoke: $($failures.Count) check(s) failed"
    exit 1
}
Write-Host 'smoke: all checks passed'

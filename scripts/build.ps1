<#
.SYNOPSIS
  Build RapidRAW+ for Windows ARM64, Windows x64 or Linux (Fedora) x64.

.EXAMPLE
  .\scripts\build.ps1 -Target win-arm64
  .\scripts\build.ps1 -Target win-x64
  .\scripts\build.ps1 -Target linux-x64     # via Podman/Docker (Fedora container) when not on Linux
  .\scripts\build.ps1                        # interactive menu
  .\scripts\build.ps1 -Target win-x64 -InstallDeps   # install missing Rust via winget first
#>
[CmdletBinding()]
param(
    [ValidateSet('win-arm64', 'win-x64', 'linux-x64')]
    [string]$Target,
    [switch]$InstallDeps,
    [switch]$DebugBuild,
    [string]$Features = ''
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

if (-not $Target) {
    Write-Host 'Seleziona il target di build:'
    Write-Host '  1) Windows ARM64  (aarch64-pc-windows-msvc)'
    Write-Host '  2) Windows x64    (x86_64-pc-windows-msvc)'
    Write-Host '  3) Linux x64      (Fedora, x86_64-unknown-linux-gnu)'
    switch (Read-Host 'Scelta [1-3]') {
        '1' { $Target = 'win-arm64' }
        '2' { $Target = 'win-x64' }
        '3' { $Target = 'linux-x64' }
        default { throw 'Scelta non valida.' }
    }
}

function Test-Cmd($name) { [bool](Get-Command $name -ErrorAction SilentlyContinue) }

function Invoke-Checked {
    param([string]$Exe, [string[]]$Arguments)
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "'$Exe $($Arguments -join ' ')' e' fallito (exit $LASTEXITCODE)." }
}

function Update-PathFromRegistry {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (Test-Path $cargoBin) { $env:Path += ";$cargoBin" }
}

# ---------------------------------------------------------------- Windows
function Build-Windows([string]$RustTarget) {
    if (-not (Test-Cmd 'rustup')) {
        if ($InstallDeps -and (Test-Cmd 'winget')) {
            Write-Host '>> Installo rustup via winget...'
            Invoke-Checked winget @('install', '--id', 'Rustlang.Rustup', '-e', '--accept-package-agreements', '--accept-source-agreements')
            Update-PathFromRegistry
        }
        if (-not (Test-Cmd 'rustup')) {
            throw "Rust non trovato. Installalo con:  winget install Rustlang.Rustup   (oppure rilancia con -InstallDeps)"
        }
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) {
        throw "Visual Studio Build Tools non trovati. Installa il workload 'Sviluppo di applicazioni desktop con C++' (con i componenti MSVC per ARM64 e x64)."
    }
    if (-not (Test-Cmd 'node')) { throw 'Node.js non trovato (serve v22+). winget install OpenJS.NodeJS.LTS' }

    # ring / aws-lc-sys need clang on Windows ARM64
    if ($RustTarget -like 'aarch64*') {
        $llvm = 'C:/Program Files/LLVM/bin'
        if (-not (Test-Cmd 'clang') -and (Test-Path $llvm)) { $env:Path += ";$llvm" }
        if (-not (Test-Cmd 'clang')) {
            if ($InstallDeps -and (Test-Cmd 'winget')) {
                Invoke-Checked winget @('install', '--id', 'LLVM.LLVM', '-e', '--accept-package-agreements', '--accept-source-agreements')
                if (Test-Path $llvm) { $env:Path += ";$llvm" }
            }
            if (-not (Test-Cmd 'clang')) { throw 'clang non trovato (serve per la build ARM64). winget install LLVM.LLVM  (oppure -InstallDeps)' }
        }
    }

    # cargo's built-in libgit2 can fail to reach GitHub on some setups; use the system git instead
    $env:CARGO_NET_GIT_FETCH_WITH_CLI = 'true'

    Write-Host ">> rustup target add $RustTarget"
    Invoke-Checked rustup @('target', 'add', $RustTarget)   # rust-toolchain.toml fissa anche la versione di Rust

    if (-not (Test-Path (Join-Path $Root 'node_modules'))) {
        Write-Host '>> npm install'
        Invoke-Checked npm @('install')
    }

    $tauriArgs = @('tauri', 'build', '--target', $RustTarget, '--bundles', 'nsis')
    if ($DebugBuild) { $tauriArgs += '--debug' }
    if ($Features) { $tauriArgs += @('--features', $Features) }
    Write-Host ">> npx $($tauriArgs -join ' ')"
    Invoke-Checked npx $tauriArgs

    $profileDir = if ($DebugBuild) { 'debug' } else { 'release' }
    Write-Host "`nFatto. Installer in: src-tauri\target\$RustTarget\$profileDir\bundle\nsis" -ForegroundColor Green
}

# ---------------------------------------------------------------- Linux
function Build-LinuxViaContainer {
    $engine = @('podman', 'docker') | Where-Object { Test-Cmd $_ } | Select-Object -First 1
    if (-not $engine) {
        throw @"
Per compilare Linux x64 da Windows serve Podman o Docker (container Fedora x86_64).
Su un PC ARM64 il container gira in emulazione (lento ma funziona).
Alternative: eseguire scripts/build.sh su una macchina Fedora x86_64, oppure usare GitHub Actions
(.github/workflows/rapidraw-plus-build.yml, se presente).
Installa con:  winget install RedHat.Podman   oppure   winget install Docker.DockerDesktop
"@
    }
    $tauriArgs = @('run', '--rm', '--platform', 'linux/amd64',
        '-v', "${Root}:/src", '-w', '/src',
        '-e', "RR_FEATURES=$Features", '-e', "RR_DEBUG=$([int]$DebugBuild.IsPresent)",
        'registry.fedoraproject.org/fedora:latest',
        'bash', 'scripts/build.sh')
    Write-Host ">> $engine $($tauriArgs -join ' ')"
    Invoke-Checked $engine $tauriArgs
    Write-Host "`nFatto. Pacchetti (rpm/deb/AppImage) in: src-tauri/target/release/bundle" -ForegroundColor Green
}

switch ($Target) {
    'win-arm64' { Build-Windows 'aarch64-pc-windows-msvc' }
    'win-x64'   { Build-Windows 'x86_64-pc-windows-msvc' }
    'linux-x64' { Build-LinuxViaContainer }
}

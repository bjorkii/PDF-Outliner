# Builds the ui crate in release mode for x86_64-pc-windows-msvc, stages the
# exe together with the given pdfium.dll, and compiles an Inno Setup installer
# (setup.exe, scripts/windows-installer.iss) for distribution. Unsigned (no
# code-signing certificate) — SmartScreen warns when the installer runs
# ("More info" -> "Run anyway"), which is expected for this deployment tier.
#
# Usage: package-windows.ps1 -PdfiumDllPath <path-to-pdfium.dll> [-VersionTag v0.1.4]
#   VersionTag is used in the installer filename. Defaults to "v<Cargo.toml version>"
#   for convenient local/ad-hoc runs; CI always passes the actual release tag
#   (the git tag is the single source of truth for release versions —
#   Cargo.toml's version is not bumped per release and will drift).

param(
    [Parameter(Mandatory = $true)][string]$PdfiumDllPath,
    [string]$VersionTag = ""
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $PdfiumDllPath)) {
    throw "pdfium dll not found: $PdfiumDllPath"
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

# 창 제목에 넣을 버전(crates/ui/build.rs) — 인자로 받은 태그만 넘긴다(아래 Cargo.toml 기반 기본값을
# 채우기 전). 비어 있으면 build.rs가 git describe를 쓴다.
$env:PDF_OUTLINER_VERSION = $VersionTag

if ([string]::IsNullOrEmpty($VersionTag)) {
    $CargoVersion = (Select-String -Path (Join-Path $RepoRoot "Cargo.toml") -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
    $VersionTag = "v$CargoVersion"
}

$Target = "x86_64-pc-windows-msvc"

Write-Host "==> Building PDF-Outliner release binary for $Target"
cargo build --release --target $Target -p ui

$DistDir = Join-Path $RepoRoot "dist"
$PkgDir = Join-Path $DistDir "PDF Outliner"
if (Test-Path $PkgDir) { Remove-Item -Recurse -Force $PkgDir }
New-Item -ItemType Directory -Path $PkgDir | Out-Null

Copy-Item (Join-Path $RepoRoot "target\$Target\release\PDF-Outliner.exe") (Join-Path $PkgDir "PDF-Outliner.exe")
Copy-Item $PdfiumDllPath (Join-Path $PkgDir "pdfium.dll")

# Distribution format: installer (2026-09-14, was .zip) — Inno Setup 6.
$Iscc = Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"
if (-not (Test-Path $Iscc)) {
    Write-Host "==> Inno Setup not found, installing via Chocolatey"
    choco install innosetup -y --no-progress
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup install failed (exit code $LASTEXITCODE)" }
}
if (-not (Test-Path $Iscc)) { throw "ISCC.exe not found: $Iscc" }

$SetupBaseName = "PDF-Outliner-$VersionTag-windows-x64-setup"
$SetupPath = Join-Path $DistDir "$SetupBaseName.exe"
if (Test-Path $SetupPath) { Remove-Item $SetupPath }

Write-Host "==> Compiling installer $SetupBaseName.exe"
& $Iscc `
    "/DAppVersion=$($VersionTag.TrimStart('v'))" `
    "/DSourceDir=$PkgDir" `
    "/DOutputDir=$DistDir" `
    "/DOutputBaseFilename=$SetupBaseName" `
    "/DIconFile=$(Join-Path $RepoRoot 'assets\icon\icon.ico')" `
    (Join-Path $PSScriptRoot "windows-installer.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed (exit code $LASTEXITCODE)" }
if (-not (Test-Path $SetupPath)) { throw "installer not produced: $SetupPath" }

Write-Host "==> Done: $SetupPath"

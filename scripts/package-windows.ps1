# Builds the ui crate in release mode for x86_64-pc-windows-msvc, bundles the
# exe together with the given pdfium.dll into a folder, and zips it for
# distribution. Unsigned (no code-signing certificate) — SmartScreen will warn
# on first run, which is expected for this deployment tier.
#
# Usage: package-windows.ps1 -PdfiumDllPath <path-to-pdfium.dll> [-VersionTag v0.1.4]
#   VersionTag is used in the zip filename. Defaults to "v<Cargo.toml version>"
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

$ZipPath = Join-Path $DistDir "PDF-Outliner-$VersionTag-windows-x64.zip"
if (Test-Path $ZipPath) { Remove-Item $ZipPath }
Compress-Archive -Path (Join-Path $PkgDir "*") -DestinationPath $ZipPath

Write-Host "==> Done: $ZipPath"

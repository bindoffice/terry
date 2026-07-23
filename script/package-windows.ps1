# Build a portable Terry zip for Windows.
param(
    [string]$Target = "",
    [string]$Version = $env:VERSION
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $Root

if (-not $Version -or $Version -eq "") {
    $line = Get-Content "Cargo.toml" | Where-Object { $_ -match '^version\s*=\s*"' } | Select-Object -First 1
    if ($line -match '"([^"]+)"') { $Version = $Matches[1] } else { $Version = "0.0.0" }
}

if (-not $Target -or $Target -eq "") {
    $Target = (rustc -vV | Select-String "^host: ").ToString().Substring(6).Trim()
}

$Arch = $Target.Split("-")[0]
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }

Write-Host "==> Building terry ($Target)…"
$env:ZED_BUNDLE = "true"
$env:RELEASE_VERSION = $Version
cargo build --release --package terry --target $Target
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Bin = Join-Path $TargetDir "$Target/release/terry.exe"
if (-not (Test-Path $Bin)) {
    $Bin = Join-Path $TargetDir "release/terry.exe"
}
if (-not (Test-Path $Bin)) {
    throw "terry.exe not found"
}

$Stage = Join-Path ([System.IO.Path]::GetTempPath()) ("terry-win-" + [guid]::NewGuid().ToString("n"))
$BundleName = "Terry-$Version-windows-$Arch"
$Dest = Join-Path $Stage $BundleName
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
Copy-Item $Bin (Join-Path $Dest "terry.exe")
if (Test-Path "LICENSE-GPL") {
    Copy-Item "LICENSE-GPL" (Join-Path $Dest "LICENSE")
} elseif (Test-Path "LICENSE") {
    Copy-Item "LICENSE" (Join-Path $Dest "LICENSE")
}
if (Test-Path "resources/app-icon.png") {
    Copy-Item "resources/app-icon.png" (Join-Path $Dest "app-icon.png")
}

@"
Terry $Version

Run terry.exe from this folder.
"@ | Set-Content -Path (Join-Path $Dest "README.txt") -Encoding UTF8

$OutDir = Join-Path $TargetDir "release"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$ZipPath = Join-Path $OutDir "$BundleName.zip"
if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }

Compress-Archive -Path $Dest -DestinationPath $ZipPath -Force
Remove-Item -Recurse -Force $Stage

Write-Host "==> Wrote $ZipPath"
Write-Output $ZipPath

# Build the Windows package (PowerShell):  packaging\windows\build-zip.ps1 [version]
# Needs: Rust (MSVC toolchain). Output: dist\ascuta-desktop-<version>-windows-x86_64.zip
param([string]$Version = "")
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")
if (-not $Version) {
  $line = Select-String -Path Cargo.toml -Pattern '^version' | Select-Object -First 1
  $Version = "v" + ($line.Line -replace '.*"(.*)".*', '$1')
}
cargo build --release --locked --features "capture sherpa"
$pkg = "ascuta-desktop-$Version-windows-x86_64"
$dir = "dist\$pkg"
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item target\release\ascuta-desktop.exe $dir
Get-ChildItem target\release\*.dll | Copy-Item -Destination $dir
Copy-Item packaging\windows\install.ps1 $dir
Copy-Item README.md $dir
Compress-Archive -Path $dir -DestinationPath "dist\$pkg.zip" -Force
(Get-FileHash "dist\$pkg.zip" -Algorithm SHA256).Hash.ToLower() + "  $pkg.zip" | Out-File "dist\$pkg.zip.sha256" -Encoding ascii
Get-Item "dist\$pkg.zip"

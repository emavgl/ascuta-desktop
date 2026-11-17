# Build the Windows package (PowerShell):  packaging\windows\build-zip.ps1 [version]
# Needs: Rust (MSVC toolchain). Output: dist\ascuta-desktop-<version>-windows-x86_64.zip
param([string]$Version = "")
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")
if ($Version -notmatch '^v?\d+\.\d+\.\d+$') {
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

# Bundle the MSVC runtime app-locally so the package also works on Windows
# machines without the Visual C++ Redistributable installed.
$crt = $null
if ($env:VCToolsRedistDir) {
  $crt = Get-ChildItem "$env:VCToolsRedistDir\x64\Microsoft.VC*.CRT" -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | Select-Object -First 1
}
if (-not $crt) {
  $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
  if (Test-Path $vswhere) {
    $vs = & $vswhere -latest -products * -property installationPath 2>$null
    if ($vs) {
      $crt = Get-ChildItem (Join-Path $vs "VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT") -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | Select-Object -First 1
    }
  }
}
if ($crt) {
  Get-ChildItem (Join-Path $crt.FullName "*.dll") | Copy-Item -Destination $dir
  Write-Host "Bundled MSVC runtime from $($crt.FullName)"
} else {
  Write-Warning "MSVC runtime DLLs not found; users may need the Visual C++ Redistributable."
}

Copy-Item packaging\windows\app.ico $dir
Copy-Item packaging\windows\install.ps1 $dir
Copy-Item packaging\windows\install.cmd $dir
Copy-Item README.md $dir
Compress-Archive -Path $dir -DestinationPath "dist\$pkg.zip" -Force
(Get-FileHash "dist\$pkg.zip" -Algorithm SHA256).Hash.ToLower() + "  $pkg.zip" | Out-File "dist\$pkg.zip.sha256" -Encoding ascii

# Stable names, mirroring the Linux assets used by the public release.
Copy-Item "dist\$pkg.zip" "dist\ascuta-desktop-windows-x86_64.zip" -Force
(Get-FileHash "dist\ascuta-desktop-windows-x86_64.zip" -Algorithm SHA256).Hash.ToLower() + "  ascuta-desktop-windows-x86_64.zip" |
  Out-File "dist\ascuta-desktop-windows-x86_64.zip.sha256" -Encoding ascii
Copy-Item packaging\windows\install-windows.ps1 "dist\install-windows.ps1" -Force

Get-Item "dist\$pkg.zip", "dist\ascuta-desktop-windows-x86_64.zip", "dist\install-windows.ps1"

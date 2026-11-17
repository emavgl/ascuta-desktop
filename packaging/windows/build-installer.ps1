# Builds the single-file Windows installer (NSIS).
#   packaging\windows\build-installer.ps1 [version]
# Needs: NSIS 3 (makensis) and the unpacked package in dist\ (see build-zip.ps1).
param([string]$Version = "")
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $root
if ($Version -notmatch '^v?\d+\.\d+\.\d+$') {
  $line = Select-String -Path Cargo.toml -Pattern '^version' | Select-Object -First 1
  $Version = "v" + ($line.Line -replace '.*"(.*)".*', '$1')
}
$num = $Version.TrimStart("v")
$pkg = "ascuta-desktop-$Version-windows-x86_64"
$src = Join-Path $root "dist\$pkg"
$out = Join-Path $root "dist\$pkg-setup.exe"
$stable = Join-Path $root "dist\ascuta-desktop-windows-x86_64-setup.exe"

if (-not (Test-Path (Join-Path $src "ascuta-desktop.exe"))) {
  throw "$src not found; run packaging\windows\build-zip.ps1 first"
}
$makensis = $null
$cmd = Get-Command makensis -ErrorAction SilentlyContinue
if ($cmd) {
  $makensis = $cmd.Source
} else {
  # The Chocolatey package installs NSIS but does not shim makensis.
  $candidates = @(
    (Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe"),
    (Join-Path $env:ProgramFiles "NSIS\makensis.exe"),
    "C:\ProgramData\chocolatey\bin\makensis.exe"
  )
  foreach ($candidate in $candidates) {
    if (Test-Path $candidate) { $makensis = $candidate; break }
  }
}
if (-not $makensis) {
  throw "makensis not found; install NSIS 3 and run again"
}

Copy-Item packaging\windows\app.ico $src -Force
Write-Host "Installer source: $src"
Get-ChildItem -Force $src | Select-Object Length, Name | Format-Table | Out-String | Write-Host
Push-Location packaging\windows
try {
  & $makensis /V2 "-DVERSION=$num" "-DSOURCE_DIR=$src" "-DOUTFILE=$out" installer.nsi
  if ($LASTEXITCODE -ne 0) { throw "makensis failed with exit code $LASTEXITCODE" }
}
finally {
  Pop-Location
}

Copy-Item $out $stable -Force
(Get-FileHash $out -Algorithm SHA256).Hash.ToLower() + "  $pkg-setup.exe" |
  Out-File "$out.sha256" -Encoding ascii
(Get-FileHash $stable -Algorithm SHA256).Hash.ToLower() + "  ascuta-desktop-windows-x86_64-setup.exe" |
  Out-File "$stable.sha256" -Encoding ascii
Get-Item $out, $stable

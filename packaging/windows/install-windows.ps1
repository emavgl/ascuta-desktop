# One-line Windows installer for the prebuilt package.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -Command `
#     "irm https://github.com/emavgl/ascuta-website/releases/download/desktop-latest/install-windows.ps1 | iex"
#
# Downloads the release zip, verifies its SHA-256 and runs the bundled
# per-user installer (no administrator rights needed).
$ErrorActionPreference = "Stop"

$releases = "https://github.com/emavgl/ascuta-website/releases/download/desktop-latest"
$zipName = "ascuta-desktop-windows-x86_64.zip"
$tmp = Join-Path $env:TEMP ("ascuta-install-" + [guid]::NewGuid().ToString("N"))

try {
  New-Item -ItemType Directory -Force $tmp | Out-Null
  $zip = Join-Path $tmp $zipName
  $sha = "$zip.sha256"

  Write-Host "Downloading $zipName..."
  Invoke-WebRequest -UseBasicParsing -Uri "$releases/$zipName" -OutFile $zip
  Invoke-WebRequest -UseBasicParsing -Uri "$releases/$zipName.sha256" -OutFile $sha

  $expected = ((Get-Content $sha -Raw).Trim() -split '\s+')[0].ToLower()
  $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
  if ($actual -ne $expected) {
    throw "SHA-256 mismatch for $zipName (expected $expected, got $actual)"
  }
  Write-Host "Checksum OK."

  Expand-Archive -Path $zip -DestinationPath $tmp -Force
  $installer = Get-ChildItem $tmp -Recurse -Filter install.ps1 | Select-Object -First 1
  if (-not $installer) {
    throw "install.ps1 not found inside $zipName"
  }
  & powershell -NoProfile -ExecutionPolicy Bypass -File $installer.FullName
  if ($LASTEXITCODE -ne 0) {
    throw "Installer failed with exit code $LASTEXITCODE"
  }
}
finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# Per-user install (no admin):  right-click > Run with PowerShell
$ErrorActionPreference = "Stop"
$src = $PSScriptRoot
$dest = Join-Path $env:LOCALAPPDATA "Ascuta"
if ($args -contains "--uninstall") {
  Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
  Remove-Item -Force (Join-Path ([Environment]::GetFolderPath("Programs")) "Ascuta Desktop.lnk") -ErrorAction SilentlyContinue
  Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "Ascuta" -ErrorAction SilentlyContinue
  Write-Host "Ascuta removed. Settings/models stay in %APPDATA%\ascuta - delete them if you like."
  exit 0
}
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $src "ascuta-desktop.exe") $dest -Force
Get-ChildItem $src -Filter *.dll | Copy-Item -Destination $dest -Force
# Start-menu shortcut (opens the settings window).
$lnk = Join-Path ([Environment]::GetFolderPath("Programs")) "Ascuta Desktop.lnk"
$sh = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
$sh.TargetPath = Join-Path $dest "ascuta-desktop.exe"
$sh.Arguments = "--gui"
$sh.WorkingDirectory = $dest
$sh.Save()
Write-Host "Installed in $dest. Start it from the Start menu: 'Ascuta Desktop'."
Write-Host "The first run downloads the speech model (~640 MB)."

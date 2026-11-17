; Ascuta Desktop — per-user Windows installer (NSIS 3).
; Build: packaging/windows/build-installer.sh [version]
;   or:  makensis -DVERSION=0.2.1 -DSOURCE_DIR=<package> -DOUTFILE=<setup.exe> installer.nsi

Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma
RequestExecutionLevel user
XPStyle on
BrandingText "Ascuta Desktop"

!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !define VERSION "0.2.5"
!endif
!ifndef SOURCE_DIR
  !define SOURCE_DIR "..\..\dist\ascuta-desktop-v${VERSION}-windows-x86_64"
!endif
!ifndef OUTFILE
  !define OUTFILE "..\..\dist\ascuta-desktop-v${VERSION}-windows-x86_64-setup.exe"
!endif

Name "Ascuta Desktop ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Ascuta"
InstallDirRegKey HKCU "Software\Ascuta\Desktop" "InstallDir"

VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "Ascuta Desktop"
VIAddVersionKey /LANG=1033 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "FileDescription" "Ascuta Desktop installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${VERSION}.0"
VIAddVersionKey /LANG=1033 "CompanyName" "Ascuta"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Apache-2.0"

!define MUI_ICON "app.ico"
!define MUI_UNICON "app.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\ascuta-desktop.exe"
!define MUI_FINISHPAGE_RUN_PARAMETERS "--gui"
!define MUI_FINISHPAGE_RUN_TEXT "Start Ascuta Desktop"
!define MUI_FINISHPAGE_SHOWREADME "$INSTDIR\README.md"
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Open the README"
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  SetRegView 64
FunctionEnd

Function un.onInit
  SetRegView 64
FunctionEnd

Section "Ascuta Desktop" SecMain
  SectionIn RO
  SetShellVarContext current
  SetRegView 64

  DetailPrint "Closing a running Ascuta Desktop..."
  nsExec::Exec 'taskkill /F /IM ascuta-desktop.exe'
  Pop $0
  Sleep 800

  RMDir /r "$INSTDIR"
  CreateDirectory "$INSTDIR"
  SetOutPath "$INSTDIR"
  File /r "${SOURCE_DIR}/*"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateDirectory "$SMPROGRAMS"
  CreateShortcut "$SMPROGRAMS\Ascuta Desktop.lnk" "$INSTDIR\ascuta-desktop.exe" "--gui" "$INSTDIR\app.ico" 0 SW_SHOWNORMAL "" "Ascuta Desktop"

  WriteRegStr HKCU "Software\Ascuta\Desktop" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "DisplayName" "Ascuta Desktop"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "Publisher" "Ascuta"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "DisplayIcon" "$INSTDIR\app.ico"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "URLInfoAbout" "https://ascuta.viglianisi.eu/desktop/"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "NoRepair" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop" "EstimatedSize" 28000

  DetailPrint "Running the self-test..."
  nsExec::Exec '"$INSTDIR\ascuta-desktop.exe" --check'
  Pop $0
  ${If} $0 != "0"
    DetailPrint "Self-test failed (exit code $0)."
    MessageBox MB_ICONEXCLAMATION|MB_OK "Ascuta Desktop was installed, but its self-test failed (exit code $0). You can still start it; see the README for troubleshooting."
  ${Else}
    DetailPrint "Self-test OK."
  ${EndIf}
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  SetRegView 64

  nsExec::Exec 'taskkill /F /IM ascuta-desktop.exe'
  Pop $0
  Sleep 800

  Delete "$SMPROGRAMS\Ascuta Desktop.lnk"
  Delete "$DESKTOP\Ascuta Desktop.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AscutaDesktop"
  DeleteRegKey HKCU "Software\Ascuta\Desktop"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Ascuta"

  IfSilent keep_data
  MessageBox MB_YESNO|MB_ICONQUESTION "Also delete Ascuta settings and downloaded speech models?" IDNO keep_data
  RMDir /r "$APPDATA\ascuta"
  keep_data:
  RMDir /r "$INSTDIR"
SectionEnd

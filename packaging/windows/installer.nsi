; MetroForge Windows installer (NSIS). Built cross-platform by makensis on
; the Linux release runner -- see scripts/package.sh (windows) and
; .github/workflows/release.yml.
;
; Required defines (passed with -D on the makensis command line):
;   VERSION   release version string, e.g. 0.4.4-alpha
;   STAGEDIR  directory holding the staged files to install
;   OUTFILE   absolute path of the setup exe to write

Unicode true
SetCompressor /SOLID lzma

!include "MUI2.nsh"

Name "MetroForge ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\MetroForge"
InstallDirRegKey HKLM "Software\MetroForge" "InstallDir"
RequestExecutionLevel admin

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\MetroForge"

!define MUI_ICON "${STAGEDIR}\icon.ico"
!define MUI_UNICON "${STAGEDIR}\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\metroforge.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch MetroForge"

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "MetroForge (required)" SecCore
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${STAGEDIR}\metroforge.exe"
  File "${STAGEDIR}\OFL.txt"
  File "${STAGEDIR}\README-dist.txt"
  File "${STAGEDIR}\icon.ico"
  ; Bevy resolves its asset root next to the exe, so install assets\ alongside.
  File /r "${STAGEDIR}\assets"

  WriteRegStr HKLM "Software\MetroForge" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Add/Remove Programs entry
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "MetroForge"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "MetroForge"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Start Menu shortcut" SecStartMenu
  CreateDirectory "$SMPROGRAMS\MetroForge"
  CreateShortcut "$SMPROGRAMS\MetroForge\MetroForge.lnk" "$INSTDIR\metroforge.exe" "" "$INSTDIR\icon.ico"
  CreateShortcut "$SMPROGRAMS\MetroForge\Uninstall MetroForge.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\MetroForge.lnk" "$INSTDIR\metroforge.exe" "" "$INSTDIR\icon.ico"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\metroforge.exe"
  Delete "$INSTDIR\OFL.txt"
  Delete "$INSTDIR\README-dist.txt"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\assets"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\MetroForge\MetroForge.lnk"
  Delete "$SMPROGRAMS\MetroForge\Uninstall MetroForge.lnk"
  RMDir "$SMPROGRAMS\MetroForge"
  Delete "$DESKTOP\MetroForge.lnk"
  DeleteRegKey HKLM "${UNINST_KEY}"
  DeleteRegKey HKLM "Software\MetroForge"
SectionEnd

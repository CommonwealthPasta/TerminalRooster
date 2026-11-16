; Windows installer for TerminalRooster (NSIS 3).
;
; Build it with `cargo xtask package windows`, which compiles the release binary
; and passes the defines below. To run makensis by hand after
; `cargo build --release`:
;
;   makensis /DVERSION=0.1.0 packaging\windows\terminalrooster.nsi
;
; Installs for the current user only (no administrator rights needed), into
; %LOCALAPPDATA%\Programs\TerminalRooster. The app's data in
; %APPDATA%\termroost is never touched, so uninstalling keeps projects and
; settings.

Unicode true
SetCompressor /SOLID lzma
ManifestDPIAware true

!ifndef VERSION
  !error "pass the app version, e.g. /DVERSION=0.1.0"
!endif
; VIProductVersion needs four numeric parts.
!ifndef VERSION_QUAD
  !define VERSION_QUAD "${VERSION}.0"
!endif
; Paths are relative to this script (makensis runs in its folder).
!ifndef ROOT
  !define ROOT "..\.."
!endif
!ifndef EXE
  !define EXE "${ROOT}\target\release\terminalrooster.exe"
!endif
!ifndef OUTFILE
  !define OUTFILE "${ROOT}\dist\TerminalRooster-${VERSION}-x64-setup.exe"
!endif

!define APP_NAME "TerminalRooster"
!define EXE_NAME "terminalrooster.exe"
!define PUBLISHER "Emma Davenport"
!define HOMEPAGE "https://github.com/Davenport-Physics/TermRoost"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
!define SETTINGS_KEY "Software\${APP_NAME}"

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

Name "${APP_NAME}"
Caption "${APP_NAME} ${VERSION} Setup"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\${APP_NAME}"
InstallDirRegKey HKCU "${SETTINGS_KEY}" "InstallDir"
BrandingText "${APP_NAME} ${VERSION}"

VIProductVersion "${VERSION_QUAD}"
VIAddVersionKey "ProductName" "${APP_NAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${APP_NAME} Setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "Copyright (c) 2026 ${PUBLISHER}"

!define MUI_ICON "${ROOT}\packaging\icons\terminalrooster.ico"
!define MUI_UNICON "${ROOT}\packaging\icons\terminalrooster.ico"
!define MUI_ABORTWARNING

!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE_NAME}"
!define MUI_FINISHPAGE_RUN_TEXT "Start ${APP_NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${ROOT}\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "${APP_NAME} needs 64-bit Windows 10 (1809) or newer."
    Abort
  ${EndIf}
FunctionEnd

Section "${APP_NAME}" SecApp
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; Overwriting a running exe fails; NSIS then offers Retry once the app is closed.
  File "/oname=${EXE_NAME}" "${EXE}"
  File "/oname=LICENSE.txt" "${ROOT}\LICENSE"
  SetOutPath "$INSTDIR\licenses"
  File "${ROOT}\crates\termroost-iced\fonts\OFL-JetBrainsMono.txt"
  File "${ROOT}\crates\termroost-iced\fonts\OFL-NotoSansSymbols2.txt"
  SetOutPath "$INSTDIR"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}.lnk" "$INSTDIR\${EXE_NAME}" "" "$INSTDIR\${EXE_NAME}" 0

  WriteRegStr HKCU "${SETTINGS_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "${HOMEPAGE}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${EXE_NAME},0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${EXE_NAME}" "" "$INSTDIR\${EXE_NAME}" 0
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecApp} "The ${APP_NAME} app and a Start menu shortcut."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A shortcut on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\${EXE_NAME}"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\licenses\OFL-JetBrainsMono.txt"
  Delete "$INSTDIR\licenses\OFL-NotoSansSymbols2.txt"
  RMDir "$INSTDIR\licenses"
  Delete "$INSTDIR\uninstall.exe"
  ; Only removed when empty, so nothing the user put there is lost.
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APP_NAME}.lnk"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "${SETTINGS_KEY}"
SectionEnd

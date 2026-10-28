; Code-Pruefer – NSIS installer hooks (bundle.windows.nsis.installerHooks).
;
; Adds a page "Container-Engine" (Docker Desktop or Podman) before the
; installation page, stores the choice in HKCU\Software\Code-Pruefer\Engine
; ("docker" | "podman", default "docker") and, if the chosen engine is
; missing, offers to install it with winget (or opens the download page).
; winget is only started via its absolute path in the user's WindowsApps
; folder, never as a bare "winget" (Windows would also look for it next to
; the installer, e.g. in the Downloads folder).
; A failed engine installation never fails the Code-Prüfer installation.
;
; The page is inserted via the macro CODE_PRUEFER_CUSTOM_PAGES, which our
; custom template (windows/installer.nsi) expands right before the
; installation page. Everything that references the template's variables
; ($PassiveMode, $UpdateMode, ...) lives inside macros, because this file is
; included before those variables are declared.

!include LogicLib.nsh
!include nsDialogs.nsh

!define CP_ENGINE_KEY "Software\Code-Pruefer"
!define CP_ENGINE_VALUE "Engine"

!macro CODE_PRUEFER_CUSTOM_PAGES
  Var CpEngine        ; "docker" | "podman"
  Var CpRadioDocker
  Var CpRadioPodman
  Var CpName          ; display name of the chosen engine
  Var CpUrl           ; official download page
  Var CpWingetId      ; winget package id
  Var CpFound         ; "1" if the chosen engine is installed
  Var CpWinget        ; absolute path of winget.exe

  Page custom CpEnginePageCreate CpEnginePageLeave

  Function CpEnginePageCreate
    ${If} $PassiveMode = 1
      Abort
    ${EndIf}
    Call CpLoadEngine
    !insertmacro MUI_HEADER_TEXT "Container-Engine" "Womit sollen die Programme ausgeführt werden?"
    nsDialogs::Create 1018
    Pop $0
    ${If} $0 == error
      Abort
    ${EndIf}
    ${NSD_CreateLabel} 0 0 100% 30u "Der Code-Prüfer führt Programme in isolierten Containern aus. Bitte die gewünschte Container-Engine wählen. Ist sie noch nicht installiert, kann sie im Anschluss automatisch installiert werden."
    Pop $0
    ${NSD_CreateRadioButton} 8u 36u -8u 12u "Docker Desktop (empfohlen)"
    Pop $CpRadioDocker
    ${NSD_CreateLabel} 20u 49u -20u 20u "Verbreitet und einfach. Kostenlos für private Nutzung und Bildung – bitte die Lizenzbedingungen selbst prüfen."
    Pop $0
    ${NSD_CreateRadioButton} 8u 74u -8u 12u "Podman"
    Pop $CpRadioPodman
    ${NSD_CreateLabel} 20u 87u -20u 20u "Open Source. Die Podman-Maschine wird beim ersten Start des Code-Prüfers eingerichtet."
    Pop $0
    ${NSD_CreateLabel} 0 114u 100% 24u "Beide benötigen unter Windows WSL2. Die Engine kann später in der App umgestellt werden; sie wird beim Start des Code-Prüfers automatisch gestartet."
    Pop $0
    ${If} $CpEngine == "podman"
      ${NSD_Check} $CpRadioPodman
    ${Else}
      ${NSD_Check} $CpRadioDocker
    ${EndIf}
    nsDialogs::Show
  FunctionEnd

  Function CpEnginePageLeave
    ${NSD_GetState} $CpRadioPodman $0
    ${If} $0 == ${BST_CHECKED}
      StrCpy $CpEngine "podman"
    ${Else}
      StrCpy $CpEngine "docker"
    ${EndIf}
  FunctionEnd

  ; Keeps an earlier choice (reinstall/update), otherwise Docker.
  Function CpLoadEngine
    ${If} $CpEngine == ""
      ReadRegStr $CpEngine HKCU "${CP_ENGINE_KEY}" "${CP_ENGINE_VALUE}"
    ${EndIf}
    ${If} $CpEngine != "podman"
      StrCpy $CpEngine "docker"
    ${EndIf}
  FunctionEnd

  ; Sets $CpName, $CpUrl, $CpWingetId and $CpFound for $CpEngine.
  Function CpDetectEngine
    StrCpy $CpFound "0"
    ${If} $CpEngine == "podman"
      StrCpy $CpName "Podman"
      StrCpy $CpUrl "https://podman.io/"
      StrCpy $CpWingetId "RedHat.Podman"
      ClearErrors
      SearchPath $0 "podman.exe"
      ${If} $0 != ""
      ${OrIf} ${FileExists} "$PROGRAMFILES64\RedHat\Podman\podman.exe"
        StrCpy $CpFound "1"
      ${EndIf}
    ${Else}
      StrCpy $CpName "Docker Desktop"
      StrCpy $CpUrl "https://www.docker.com/products/docker-desktop/"
      StrCpy $CpWingetId "Docker.DockerDesktop"
      ClearErrors
      SearchPath $0 "docker.exe"
      ${If} $0 != ""
      ${OrIf} ${FileExists} "$PROGRAMFILES64\Docker\Docker\Docker Desktop.exe"
        StrCpy $CpFound "1"
      ${EndIf}
    ${EndIf}
  FunctionEnd

  Function CpOpenDownloadPage
    MessageBox MB_YESNO|MB_ICONQUESTION "Soll die offizielle Download-Seite von $CpName jetzt geöffnet werden?$\r$\n$\r$\n$CpUrl" /SD IDNO IDNO cp_no_page
      ExecShell "open" "$CpUrl"
    cp_no_page:
  FunctionEnd

  Function CpEnginePostInstall
    Call CpLoadEngine
    WriteRegStr HKCU "${CP_ENGINE_KEY}" "${CP_ENGINE_VALUE}" "$CpEngine"
    Call CpDetectEngine
    DetailPrint "Container-Engine: $CpName"
    ${If} $CpFound == "1"
      DetailPrint "$CpName ist bereits installiert."
      Return
    ${EndIf}
    DetailPrint "$CpName ist nicht installiert."
    ; No questions in silent/passive mode (e.g. updates).
    ${If} ${Silent}
    ${OrIf} $PassiveMode = 1
      Return
    ${EndIf}

    MessageBox MB_YESNO|MB_ICONQUESTION "$CpName ist auf diesem Computer nicht installiert. Ohne Container-Engine kann der Code-Prüfer keine Programme ausführen.$\r$\n$\r$\nSoll $CpName jetzt automatisch installiert werden (über winget)?$\r$\n$\r$\nWindows fragt dabei nach Administratorrechten. Die Installation kann einige Minuten dauern und einen Neustart erfordern." IDYES cp_install
      MessageBox MB_OK|MB_ICONINFORMATION "$CpName wird nicht installiert. Sie können es später selbst installieren:$\r$\n$CpUrl"
      Return
    cp_install:

    ; winget ("App-Installer") is missing on some older/managed systems.
    ; Its app execution alias lives in the user's WindowsApps folder.
    StrCpy $CpWinget "$LOCALAPPDATA\Microsoft\WindowsApps\winget.exe"
    StrCpy $0 "nicht gefunden"
    ${If} ${FileExists} "$CpWinget"
      nsExec::Exec '"$CpWinget" --version'
      Pop $0
    ${EndIf}
    ${If} $0 != 0
      DetailPrint "winget ist nicht verfügbar ($CpWinget: $0)."
      MessageBox MB_OK|MB_ICONEXCLAMATION "Die automatische Installation ist nicht möglich, weil winget (App-Installer) auf diesem Computer nicht verfügbar ist.$\r$\n$\r$\nBitte $CpName manuell installieren."
      Call CpOpenDownloadPage
      Return
    ${EndIf}

    DetailPrint "Installiere $CpName über winget – das kann einige Minuten dauern …"
    nsExec::ExecToLog '"$CpWinget" install -e --id $CpWingetId --source winget --accept-package-agreements --accept-source-agreements --disable-interactivity'
    Pop $0
    ${If} $0 == 0
      DetailPrint "$CpName wurde installiert."
      ${If} $CpEngine == "podman"
        MessageBox MB_OK|MB_ICONINFORMATION "$CpName wurde installiert.$\r$\n$\r$\nFalls Windows einen Neustart verlangt, bitte den Computer neu starten. Beim ersten Start richtet der Code-Prüfer die Podman-Maschine ein (benötigt WSL2; das kann einige Minuten dauern)."
      ${Else}
        MessageBox MB_OK|MB_ICONINFORMATION "$CpName wurde installiert.$\r$\n$\r$\nFalls Windows einen Neustart verlangt, bitte den Computer neu starten. Beim ersten Start von Docker Desktop müssen ggf. die Nutzungsbedingungen bestätigt werden. Der Code-Prüfer startet Docker Desktop danach automatisch."
      ${EndIf}
    ${Else}
      DetailPrint "winget-Installation von $CpName nicht erfolgreich (Code $0)."
      MessageBox MB_OK|MB_ICONEXCLAMATION "Die Installation von $CpName wurde nicht erfolgreich abgeschlossen (winget-Code $0).$\r$\n$\r$\nFalls ein Neustart verlangt wurde, bitte den Computer neu starten. Andernfalls $CpName bitte manuell installieren.$\r$\n$\r$\nDer Code-Prüfer selbst wird trotzdem installiert."
      Call CpOpenDownloadPage
    ${EndIf}
  FunctionEnd
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Call CpEnginePostInstall
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Same condition as the template uses for deleting app data.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    DeleteRegValue HKCU "${CP_ENGINE_KEY}" "${CP_ENGINE_VALUE}"
    DeleteRegKey /ifempty HKCU "${CP_ENGINE_KEY}"
  ${EndIf}
!macroend

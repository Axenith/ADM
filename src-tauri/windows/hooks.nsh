; Run by the installer and uninstaller (Tauri NSIS hooks). They tell the browsers where ADM is, so the extension can
; start it, and take that away again on uninstall.
!macro NSIS_HOOK_POSTINSTALL
  nsExec::Exec '"$INSTDIR\adm.exe" --register-native-host'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec '"$INSTDIR\adm.exe" --unregister-native-host'
!macroend

!macro NSIS_HOOK_PREINSTALL
  IfFileExists "$INSTDIR\bwbrowser-proxy.exe" 0 bwbrowser_proxy_preinstall_done

  DetailPrint "Stopping Bwbrowser proxy workers before replacing application files"
  nsExec::ExecToStack '"$SYSDIR\taskkill.exe" /F /T /IM "bwbrowser-proxy.exe"'
  Pop $0
  Pop $1
  Sleep 1000

  ; Removing the old sidecar first prevents NSIS from retaining a same-version
  ; or previously locked executable while updating the main application.
  Delete "$INSTDIR\bwbrowser-proxy.exe"

  bwbrowser_proxy_preinstall_done:
!macroend

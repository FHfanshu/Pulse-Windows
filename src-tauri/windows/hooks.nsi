; Uninstalling Pulse takes back what it put outside its folder: Claude Code's
; status line (restoring the previous command, if any) and the login item.
!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\pulse.exe" --statusline-uninstall'
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Pulse"
!macroend

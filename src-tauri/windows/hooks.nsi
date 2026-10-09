; Uninstalling Pulse takes back what it put outside its folder: Claude Code's
; status line (restoring the previous command, if any) and the login item.
; Not while updating (/UPDATE): an update replaces the files in place and Pulse starts
; again by itself, so the status line and the Run entry stay as they are.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ExecWait '"$INSTDIR\pulse.exe" --statusline-uninstall'
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Pulse"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Pulse"
  ${EndIf}
!macroend

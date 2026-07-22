!macro NSIS_HOOK_PREINSTALL
  IfFileExists "$INSTDIR\lumi-agent.exe" 0 preinstall_agent_stopped
  ExecWait '"$INSTDIR\lumi-agent.exe" --shutdown' $0
  IntCmp $0 0 preinstall_agent_stopped
  MessageBox MB_OK|MB_ICONSTOP "LumiControl could not stop its background agent. Quit LumiControl from the tray and try again."
  Abort
  preinstall_agent_stopped:
!macroend

!macro NSIS_HOOK_POSTINSTALL
  CreateShortCut "$DESKTOP\LumiControl.lnk" "$INSTDIR\LumiControl.exe"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\lumi-agent.exe" 0 preuninstall_agent_stopped
  ExecWait '"$INSTDIR\lumi-agent.exe" --shutdown' $0
  IntCmp $0 0 preuninstall_agent_stopped
  MessageBox MB_OK|MB_ICONSTOP "LumiControl could not stop its background agent. Quit LumiControl from the tray and try again."
  Abort
  preuninstall_agent_stopped:
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "LumiControl Agent"
  Delete "$DESKTOP\LumiControl.lnk"
!macroend

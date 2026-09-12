; Hashgram NSIS installer hooks (Tauri bundler `installerHooks`).
;
; Per-user install, no UAC. The firewall rule for the app binary is attempted
; quietly: it succeeds when the installer happens to run elevated (an admin
; or the MSI path) and is a silent no-op otherwise, in which case Windows
; asks the user on first use. Outbound UDP+TCP 26670 is what the network
; needs; nothing inbound is required for a client.

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Hashgram" program="$INSTDIR\${MAINBINARYNAME}.exe"'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Hashgram" dir=out action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=UDP remoteport=26670 profile=any enable=yes'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Hashgram" dir=out action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=TCP remoteport=26670 profile=any enable=yes'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Default: keep the vault and messages. The 24 words restore an account,
  ; but messages stored only on this PC do not come back.
  ${IfNot} ${Silent}
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON1 \
      "Keep your Hashgram vault and messages on this PC?$\r$\n$\r$\nYes keeps them in %LOCALAPPDATA%\Hashgram\data for a later reinstall (recommended).$\r$\nNo deletes them. You would need your 24 words to get the account back, and messages stored only here would be lost." \
      IDYES +2
    RMDir /r "$LOCALAPPDATA\Hashgram\data"
  ${EndIf}
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Hashgram" program="$INSTDIR\${MAINBINARYNAME}.exe"'
!macroend

; Do not recreate a desktop shortcut during installation or silent updates.
; Tauri's default NSIS template creates one for silent/passive installers,
; which can bring back a shortcut the user intentionally removed.
!macro NSIS_HOOK_PREINSTALL
  StrCpy $NoShortcutMode 1
!macroend

; The built-in shortcut step is skipped together with the desktop shortcut.
; Restore the normal mode and recreate only the Start Menu shortcut so the
; application remains discoverable without changing the user's desktop.
!macro NSIS_HOOK_POSTINSTALL
  StrCpy $NoShortcutMode 0
  !insertmacro MUI_STARTMENU_WRITE_BEGIN Application
    Call CreateOrUpdateStartMenuShortcut
  !insertmacro MUI_STARTMENU_WRITE_END
!macroend

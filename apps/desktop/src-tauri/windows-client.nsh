!define MUI_WELCOMEPAGE_TITLE "Welcome to Zibblu Windows Client Setup"
!define MUI_WELCOMEPAGE_TEXT "Install the Zibblu desktop client, then sign in to Zibblu Online or choose your own server."
!define MUI_LICENSEPAGE_CHECKBOX
!define ZIBBLU_AI_TERMS_FILE "${__FILEDIR__}\windows-ai-terms.txt"

!macro NSIS_HOOK_POSTINSTALL
  ; Silent and passive modes skip the license page.
  IfSilent doneTerms
  StrCmp $PassiveMode 1 doneTerms
  Push "$OUTDIR"
  SetOutPath "$LOCALAPPDATA\${BUNDLEID}"
  File /oname=accepted-ai-terms.txt "${ZIBBLU_AI_TERMS_FILE}"
  Pop $0
  SetOutPath "$0"
doneTerms:
!macroend

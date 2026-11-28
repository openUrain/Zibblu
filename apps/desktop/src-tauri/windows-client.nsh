LangString ZibbluWelcomeTitle 1033 "Welcome to Zibblu Windows Client Setup"
LangString ZibbluWelcomeTitle 2052 "欢迎安装 Zibblu Windows 客户端"
LangString ZibbluWelcomeText 1033 "Install the Zibblu desktop client, then sign in to Zibblu Online or choose your own server."
LangString ZibbluWelcomeText 2052 "安装 Zibblu 桌面客户端后，即可登录 Zibblu 在线服务或选择自己的服务器。"
!define MUI_WELCOMEPAGE_TITLE "$(ZibbluWelcomeTitle)"
!define MUI_WELCOMEPAGE_TEXT "$(ZibbluWelcomeText)"
!define MUI_LICENSEPAGE_CHECKBOX
!define ZIBBLU_AI_TERMS_EN "${__FILEDIR__}\windows-ai-terms.en.txt"
!define ZIBBLU_AI_TERMS_ZH "${__FILEDIR__}\windows-ai-terms.zh-CN.txt"
LicenseLangString ZibbluAiTerms 1033 "${ZIBBLU_AI_TERMS_EN}"
LicenseLangString ZibbluAiTerms 2052 "${ZIBBLU_AI_TERMS_ZH}"

; Tauri supplies a single license path to this MUI macro.
!macroundef MUI_PAGE_LICENSE
!macro MUI_PAGE_LICENSE LICENSEDATA
  !insertmacro MUI_PAGE_INIT
  !insertmacro MUI_PAGEDECLARATION_LICENSE "$(ZibbluAiTerms)"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Silent and passive modes skip the license page.
  IfSilent doneTerms
  StrCmp $PassiveMode 1 doneTerms
  Push "$OUTDIR"
  SetOutPath "$LOCALAPPDATA\${BUNDLEID}"
  StrCmp $LANGUAGE 2052 chineseTerms
  File /oname=accepted-ai-terms.txt "${ZIBBLU_AI_TERMS_EN}"
  Goto restoreOutDir
chineseTerms:
  File /oname=accepted-ai-terms.txt "${ZIBBLU_AI_TERMS_ZH}"
restoreOutDir:
  Pop $0
  SetOutPath "$0"
doneTerms:
!macroend

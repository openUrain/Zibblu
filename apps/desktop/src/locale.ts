export function t(english: string, simplifiedChinese: string) {
  return navigator.language.toLowerCase().startsWith("zh") ? simplifiedChinese : english;
}

export const actions = {
  back: t("Back", "返回"),
  cancel: t("Cancel", "取消"),
  save: t("Save", "保存"),
  saving: t("Saving…", "保存中…"),
  saved: t("Saved", "已保存"),
  retry: t("Retry", "重试"),
};

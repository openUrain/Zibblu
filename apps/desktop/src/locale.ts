import { createContext } from "react";

export type AppLanguage = "zh-CN" | "en";

let language: AppLanguage = globalThis.navigator?.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en";

export const LanguageContext = createContext<AppLanguage>(language);

export function appLanguage() {
  return language;
}

export function applyAppLanguage(preference?: AppLanguage) {
  if (preference) language = preference;
  if (typeof document !== "undefined") document.documentElement.lang = language;
}

export function t(english: string, simplifiedChinese: string) {
  return language === "zh-CN" ? simplifiedChinese : english;
}

export const actions = {
  get back() { return t("Back", "返回"); },
  get cancel() { return t("Cancel", "取消"); },
  get save() { return t("Save", "保存"); },
  get saving() { return t("Saving…", "保存中…"); },
  get saved() { return t("Saved", "已保存"); },
  get retry() { return t("Retry", "重试"); },
};

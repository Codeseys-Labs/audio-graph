import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";

import en from "./locales/en.json";
import pt from "./locales/pt.json";

// Resources are bundled inline — the app is offline-first, so we don't use
// an HTTP backend. Add new locales by importing a JSON file and extending
// the resources object.
const resources = {
  en: { translation: en },
  pt: { translation: pt },
} as const;

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: "en",
    // Auto-detect uses navigator language (matches the OS locale in Tauri
    // WebViews). We fall back to "en" for anything we don't ship yet.
    supportedLngs: ["en", "pt"],
    debug: import.meta.env.DEV,
    interpolation: {
      // React already escapes by default, so i18next doesn't need to.
      escapeValue: false,
    },
    detection: {
      // Cache in localStorage so toggling the OS locale only changes the
      // default on first run; subsequent sessions respect explicit choices
      // once we add a language switcher.
      order: ["localStorage", "navigator"],
      caches: ["localStorage"],
    },
  });

export default i18n;

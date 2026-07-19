import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en/translation.json";

// v2 ships English only. The i18next plumbing stays so every user-facing
// string lives in locales/en/translation.json (the no-literal-string ESLint
// rule keeps it that way), which also keeps the door open for locales later.
i18n.use(initReactI18next).init({
  resources: { en: { translation: en } },
  lng: "en",
  fallbackLng: "en",
  interpolation: {
    escapeValue: false, // React already escapes values
  },
  react: {
    useSuspense: false,
  },
});

export default i18n;

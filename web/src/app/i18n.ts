import { watch } from 'vue';
import { createI18n } from 'vue-i18n';

import aboutEn from '@/features/about/locales/en.json';
import aboutVi from '@/features/about/locales/vi.json';
import convertEn from '@/features/convert/locales/en.json';
import convertVi from '@/features/convert/locales/vi.json';
import en from '@/shared/locales/en.json';
import vi from '@/shared/locales/vi.json';

export const LOCALES = ['en', 'vi'] as const;
export type AppLocale = (typeof LOCALES)[number];

export const LOCALE_STORAGE_KEY = 'backsave.locale';

/** The two storage methods the app uses; `localStorage` satisfies it. */
export interface LocaleStorage {
  getItem: (key: string) => string | null;
  setItem: (key: string, value: string) => void;
}

function isLocale(value: string | null): value is AppLocale {
  return LOCALES.some((locale) => locale === value);
}

/** A remembered choice wins; otherwise any `vi*` browser language picks Vietnamese and everything else English. */
export function pickLocale(browserLanguage: string, remembered: string | null): AppLocale {
  if (isLocale(remembered)) {
    return remembered;
  }
  return browserLanguage.toLowerCase().startsWith('vi') ? 'vi' : 'en';
}

function readRemembered(storage: LocaleStorage): string | null {
  try {
    return storage.getItem(LOCALE_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function createAppI18n(browserLanguage: string, storage: LocaleStorage) {
  const i18n = createI18n({
    legacy: false,
    locale: pickLocale(browserLanguage, readRemembered(storage)),
    fallbackLocale: 'en',
    messages: { en: { ...en, ...convertEn, ...aboutEn }, vi: { ...vi, ...convertVi, ...aboutVi } },
  });
  const reflectLocale = (locale: string) => {
    document.documentElement.lang = locale;
  };
  reflectLocale(i18n.global.locale.value);
  watch(
    i18n.global.locale,
    (locale) => {
      reflectLocale(locale);
      try {
        storage.setItem(LOCALE_STORAGE_KEY, locale);
      } catch {
        // Storage can be blocked (private window, site data off); the choice then lasts for this visit only.
      }
    },
    { flush: 'sync' },
  );
  return i18n;
}

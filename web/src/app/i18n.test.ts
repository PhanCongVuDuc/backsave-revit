import { describe, expect, it } from 'vitest';

import en from '@/shared/locales/en.json';
import vi from '@/shared/locales/vi.json';

import { createAppI18n, pickLocale } from './i18n';

function keysOf(value: object, prefix = ''): string[] {
  return Object.entries(value).flatMap(([key, child]) =>
    typeof child === 'object' && child !== null ? keysOf(child, `${prefix}${key}.`) : [`${prefix}${key}`],
  );
}

describe('pickLocale', () => {
  it('picks vi for vi-VN', () => {
    expect(pickLocale('vi-VN', null)).toBe('vi');
  });

  it('picks en for de-DE', () => {
    expect(pickLocale('de-DE', null)).toBe('en');
  });

  it('prefers a remembered choice over the browser language', () => {
    expect(pickLocale('de-DE', 'vi')).toBe('vi');
  });
});

describe('createAppI18n', () => {
  it('translates with the locale picked from the browser language', () => {
    const i18n = createAppI18n('vi-VN', { getItem: () => null, setItem: () => undefined });
    expect(i18n.global.locale.value).toBe('vi');
    expect(i18n.global.t('nav.about')).toBe(vi.nav.about);
  });

  it('remembers a chosen locale and survives a storage that throws', () => {
    const stored: Record<string, string> = {};
    const i18n = createAppI18n('en-US', {
      getItem: (key) => stored[key] ?? null,
      setItem: (key, value) => {
        stored[key] = value;
      },
    });
    i18n.global.locale.value = 'vi';
    expect(stored['backsave.locale']).toBe('vi');

    const blocked = () => {
      throw new Error('blocked');
    };
    const safe = createAppI18n('en-US', { getItem: blocked, setItem: blocked });
    safe.global.locale.value = 'vi';
    expect(safe.global.locale.value).toBe('vi');
  });
});

describe('remembered locale', () => {
  it('restores a remembered vi', () => {
    const i18n = createAppI18n('en-US', { getItem: () => 'vi', setItem: () => undefined });
    expect(i18n.global.locale.value).toBe('vi');
  });

  it('ignores an invalid stored value', () => {
    const i18n = createAppI18n('en-US', { getItem: () => 'xx', setItem: () => undefined });
    expect(i18n.global.locale.value).toBe('en');
  });

  it('sets the html lang to the active locale and follows changes', () => {
    const i18n = createAppI18n('vi-VN', { getItem: () => null, setItem: () => undefined });
    expect(document.documentElement.lang).toBe('vi');
    i18n.global.locale.value = 'en';
    expect(document.documentElement.lang).toBe('en');
  });
});

describe('locale files', () => {
  it('have identical key sets in en and vi', () => {
    expect(keysOf(vi).toSorted()).toEqual(keysOf(en).toSorted());
  });
});

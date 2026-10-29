import { describe, expect, it } from 'vitest';

import aboutEn from '@/features/about/locales/en.json';
import aboutVi from '@/features/about/locales/vi.json';
import convertEn from '@/features/convert/locales/en.json';
import convertVi from '@/features/convert/locales/vi.json';
import guidesEn from '@/features/guides/locales/en.json';
import guidesVi from '@/features/guides/locales/vi.json';
import sharedEn from '@/shared/locales/en.json';
import sharedVi from '@/shared/locales/vi.json';

import indexHtml from '../../index.html?raw';

function keysOf(value: object, prefix = ''): string[] {
  return Object.entries(value).flatMap(([key, child]) =>
    typeof child === 'object' && child !== null ? keysOf(child, `${prefix}${key}.`) : [`${prefix}${key}`],
  );
}

describe('feature locale files', () => {
  it('convert has identical key sets in en and vi', () => {
    expect(keysOf(convertVi).toSorted()).toEqual(keysOf(convertEn).toSorted());
  });

  it('guides has identical key sets in en and vi', () => {
    expect(keysOf(guidesVi).toSorted()).toEqual(keysOf(guidesEn).toSorted());
  });

  it('about has identical key sets in en and vi', () => {
    expect(keysOf(aboutVi).toSorted()).toEqual(keysOf(aboutEn).toSorted());
  });
});

/** Every string in a locale file. */
function stringsOf(value: unknown): string[] {
  if (typeof value === 'string') {
    return [value];
  }
  return typeof value === 'object' && value !== null ? Object.values(value).flatMap(stringsOf) : [];
}

describe('retention texts', () => {
  const locales = { aboutEn, aboutVi, convertEn, convertVi, guidesEn, guidesVi, sharedEn, sharedVi };
  const ONE_HOUR = /\b1 hour\b|\b1 giờ/;
  const UNFINISHED_UPLOAD = /last data|lần cuối nhận dữ liệu/;

  it('say 1 hour only about an unfinished upload', () => {
    const texts = [...Object.values(locales).flatMap(stringsOf), ...indexHtml.split('\n')];
    const oneHour = texts.filter((text) => ONE_HOUR.test(text));
    expect(oneHour.length).toBeGreaterThan(0);
    for (const text of oneHour) {
      expect(text).toMatch(UNFINISHED_UPLOAD);
    }
  });

  it('say an expired conversion can no longer be reported, and where to reach us', () => {
    expect(convertEn.convert.error.expired).toContain('can no longer be reported');
    expect(convertEn.convert.error.expired).toContain('bottom of the page');
    expect(convertVi.convert.error.expired).toContain('không báo được');
    expect(convertVi.convert.error.expired).toContain('chân trang');
  });

  it('keep the converted file for 2 hours on the home page', () => {
    expect(convertEn.convert.lead).toContain('2 hours');
    expect(convertVi.convert.lead).toContain('2 giờ');
    expect(indexHtml).toContain('Files are deleted after 2 hours.');
  });
});

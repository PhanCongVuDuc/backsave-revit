import { describe, expect, it } from 'vitest';

import aboutEn from '@/features/about/locales/en.json';
import aboutVi from '@/features/about/locales/vi.json';
import convertEn from '@/features/convert/locales/en.json';
import convertVi from '@/features/convert/locales/vi.json';
import guidesEn from '@/features/guides/locales/en.json';
import guidesVi from '@/features/guides/locales/vi.json';

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

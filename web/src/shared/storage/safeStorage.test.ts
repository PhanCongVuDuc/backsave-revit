import { afterEach, describe, expect, it } from 'vitest';

import { safeStorage } from './safeStorage';

const original = Object.getOwnPropertyDescriptor(window, 'localStorage');

afterEach(() => {
  if (original) {
    Object.defineProperty(window, 'localStorage', original);
  }
});

describe('safeStorage', () => {
  it('falls back to memory when reading localStorage throws', () => {
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      get() {
        throw new Error('blocked');
      },
    });
    const storage = safeStorage();
    expect(storage.getItem('k')).toBeNull();
    storage.setItem('k', 'v');
    expect(storage.getItem('k')).toBe('v');
    storage.removeItem('k');
    expect(storage.getItem('k')).toBeNull();
  });

  it('is localStorage when the browser allows it', () => {
    expect(safeStorage()).toBe(window.localStorage);
  });
});

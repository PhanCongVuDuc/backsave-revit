import { afterEach, describe, expect, it } from 'vitest';

import { createBacksaveApp } from './createApp';

const original = Object.getOwnPropertyDescriptor(window, 'localStorage');

afterEach(() => {
  if (original) {
    Object.defineProperty(window, 'localStorage', original);
  }
});

describe('createBacksaveApp', () => {
  it('still mounts, in en, when reading localStorage throws', () => {
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      get() {
        throw new Error('blocked');
      },
    });
    const root = document.createElement('div');
    const app = createBacksaveApp();
    app.mount(root);
    expect(root.querySelector('header')).not.toBeNull();
    expect(document.documentElement.lang).toBe('en');
    app.unmount();
  });
});

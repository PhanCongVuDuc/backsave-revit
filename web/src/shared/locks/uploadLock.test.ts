import { afterEach, describe, expect, it, vi } from 'vitest';

import { holdUploadLock, isUploadLockHeld } from './uploadLock';

/** A minimal LockManager: requests are granted at once, released when their callback's promise settles. */
function fakeLocks() {
  const held = new Set<string>();
  return {
    held,
    request: (name: string, callback: () => Promise<void>) => {
      held.add(name);
      return callback().finally(() => held.delete(name));
    },
    query: () => Promise.resolve({ held: [...held].map((name) => ({ name })), pending: [] }),
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('upload locks', () => {
  it('holds the lock of a job until released, visible to queries', async () => {
    const locks = fakeLocks();
    vi.stubGlobal('navigator', { locks });
    const release = holdUploadLock('abc');
    expect(locks.held.has('backsave-upload-abc')).toBe(true);
    await expect(isUploadLockHeld('abc')).resolves.toBe(true);
    await expect(isUploadLockHeld('other')).resolves.toBe(false);
    release();
    await Promise.resolve();
    await Promise.resolve();
    await expect(isUploadLockHeld('abc')).resolves.toBe(false);
  });

  it('treats a browser without the Locks API as holding nothing', async () => {
    vi.stubGlobal('navigator', {});
    await expect(isUploadLockHeld('abc')).resolves.toBe(false);
    expect(() => {
      holdUploadLock('abc')();
    }).not.toThrow();
  });
});

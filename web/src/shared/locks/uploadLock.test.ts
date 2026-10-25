import { afterEach, describe, expect, it, vi } from 'vitest';

import { acquireUploadLock, isUploadLockHeld } from './uploadLock';

/** A minimal LockManager honoring `ifAvailable`: a taken name is refused with a null lock. */
function fakeLocks() {
  const held = new Set<string>();
  const options: { ifAvailable?: boolean }[] = [];
  return {
    held,
    options,
    request: (name: string, opts: { ifAvailable?: boolean }, callback: (lock: { name: string } | null) => unknown) => {
      options.push(opts);
      if (held.has(name)) {
        return Promise.resolve(callback(null));
      }
      held.add(name);
      return Promise.resolve(callback({ name })).finally(() => held.delete(name));
    },
    query: () => Promise.resolve({ held: [...held].map((name) => ({ name })), pending: [] }),
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('upload locks', () => {
  it('grants the lock to one tab only, until it is released', async () => {
    const locks = fakeLocks();
    vi.stubGlobal('navigator', { locks });
    const release = await acquireUploadLock('abc');
    expect(release).not.toBeNull();
    await expect(isUploadLockHeld('abc')).resolves.toBe(true);
    // A second request (another tab) is refused at once, not queued.
    await expect(acquireUploadLock('abc')).resolves.toBeNull();
    expect(locks.options.every((opts) => opts.ifAvailable === true)).toBe(true);
    await expect(isUploadLockHeld('other')).resolves.toBe(false);
    release?.();
    await vi.waitFor(async () => {
      await expect(isUploadLockHeld('abc')).resolves.toBe(false);
    });
    await expect(acquireUploadLock('abc')).resolves.not.toBeNull();
  });

  it('grants every request in a browser without the Locks API', async () => {
    vi.stubGlobal('navigator', {});
    await expect(isUploadLockHeld('abc')).resolves.toBe(false);
    const release = await acquireUploadLock('abc');
    expect(release).not.toBeNull();
    expect(() => {
      release?.();
    }).not.toThrow();
  });
});

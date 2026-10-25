// One exclusive Web Lock per upload: the tab holding it is the only one that may send data for that job or delete it.
// Without the Locks API (older browsers, tests) every request is granted, which is how the app behaved before locks.

function lockName(jobId: string): string {
  return `backsave-upload-${jobId}`;
}

function locks(): LockManager | null {
  try {
    return typeof navigator !== 'undefined' && 'locks' in navigator ? navigator.locks : null;
  } catch {
    return null;
  }
}

/** A hint only: true when some tab holds the upload lock of this job right now. `acquireUploadLock` decides. */
export async function isUploadLockHeld(jobId: string): Promise<boolean> {
  const manager = locks();
  if (!manager) {
    return false;
  }
  try {
    const state = await manager.query();
    return (state.held ?? []).some((lock) => lock.name === lockName(jobId));
  } catch {
    return false;
  }
}

/**
 * Takes the upload lock of this job if no other tab has it. Resolves to a function that releases it, or to `null` when
 * another tab holds it. The lock is also released when the tab closes.
 */
export function acquireUploadLock(jobId: string): Promise<(() => void) | null> {
  const manager = locks();
  if (!manager) {
    return Promise.resolve(() => undefined);
  }
  return new Promise((resolveGrant) => {
    manager
      .request(lockName(jobId), { ifAvailable: true }, (lock) => {
        if (!lock) {
          resolveGrant(null);
          return undefined;
        }
        // Held until the returned release function is called.
        return new Promise<void>((release) => {
          resolveGrant(() => {
            release();
          });
        });
      })
      .catch(() => {
        resolveGrant(null);
      });
  });
}

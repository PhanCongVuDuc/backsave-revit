// One Web Lock per upload, so other tabs of this browser can tell that an upload is in progress. Without the Locks API
// (older browsers, tests) no lock is ever held.

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

/** True when some tab holds the upload lock of this job. */
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

/** Holds the upload lock of this job until the returned function is called or the tab closes. */
export function holdUploadLock(jobId: string): () => void {
  const manager = locks();
  if (!manager) {
    return () => undefined;
  }
  let release: () => void = () => undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  manager.request(lockName(jobId), () => released).catch(() => undefined);
  return release;
}

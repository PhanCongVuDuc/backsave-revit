import type { KeyValueStorage } from '@/shared/storage/safeStorage';

/** Where this browser lists its unfinished uploads. */
export const UPLOADS_STORAGE_KEY = 'backsave.uploads';
/** The server drops an unfinished upload at the latest 6 hours after it was created. */
export const UPLOAD_ENTRY_MAX_AGE_MS = 6 * 60 * 60 * 1000;

/** The file an upload was started with, as the browser can identify it again later. */
export interface FileFingerprint {
  name: string;
  size: number;
  lastModified: number;
}

export interface UploadEntry extends FileFingerprint {
  jobId: string;
  uploadUrl: string;
  /** Milliseconds since the epoch. */
  createdAt: number;
}

export function fingerprintOf(file: File): FileFingerprint {
  return { name: file.name, size: file.size, lastModified: file.lastModified };
}

function isEntry(value: unknown): value is UploadEntry {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const entry = value as Record<string, unknown>;
  return (
    typeof entry.name === 'string' &&
    typeof entry.size === 'number' &&
    typeof entry.lastModified === 'number' &&
    typeof entry.jobId === 'string' &&
    typeof entry.uploadUrl === 'string' &&
    typeof entry.createdAt === 'number'
  );
}

/**
 * Unfinished uploads keyed by file fingerprint, shared by every tab of this browser, so choosing the same file again can
 * continue its upload. Entries older than 6 hours are dropped on every read. Blocked or broken storage reads as empty.
 */
export function uploadRegistry(storage: KeyValueStorage, now: () => number = Date.now) {
  function read(): UploadEntry[] {
    let parsed: unknown;
    try {
      parsed = JSON.parse(storage.getItem(UPLOADS_STORAGE_KEY) ?? '[]');
    } catch {
      return [];
    }
    if (!Array.isArray(parsed)) {
      return [];
    }
    const oldest = now() - UPLOAD_ENTRY_MAX_AGE_MS;
    return parsed.filter(isEntry).filter((entry) => entry.createdAt > oldest);
  }

  function write(entries: UploadEntry[]) {
    try {
      if (entries.length === 0) {
        storage.removeItem(UPLOADS_STORAGE_KEY);
      } else {
        storage.setItem(UPLOADS_STORAGE_KEY, JSON.stringify(entries));
      }
    } catch {
      // Blocked storage only costs continuing the upload after a reload.
    }
  }

  const same = (a: FileFingerprint, b: FileFingerprint) =>
    a.name === b.name && a.size === b.size && a.lastModified === b.lastModified;

  return {
    /** The unfinished upload of this file, if any. */
    find(file: FileFingerprint): UploadEntry | null {
      const entries = read();
      write(entries);
      return entries.find((entry) => same(entry, file)) ?? null;
    },
    /** Records the upload of `file`; an older entry for the same file is replaced. */
    add(file: FileFingerprint, jobId: string, uploadUrl: string) {
      const entries = read().filter((entry) => !same(entry, file) && entry.jobId !== jobId);
      entries.push({
        name: file.name,
        size: file.size,
        lastModified: file.lastModified,
        jobId,
        uploadUrl,
        createdAt: now(),
      });
      write(entries);
    },
    remove(jobId: string) {
      write(read().filter((entry) => entry.jobId !== jobId));
    },
    all(): UploadEntry[] {
      return read();
    },
    /** Writes back only the entries that are still recent. */
    prune() {
      write(read());
    },
  };
}

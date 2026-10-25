import { describe, expect, it } from 'vitest';

import type { KeyValueStorage } from '@/shared/storage/safeStorage';

import { UPLOAD_ENTRY_MAX_AGE_MS, uploadRegistry, UPLOADS_STORAGE_KEY } from './uploadRegistry';

function memory(): KeyValueStorage & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => {
      data.set(key, value);
    },
    removeItem: (key) => {
      data.delete(key);
    },
  };
}

const A = { name: 'a.rvt', size: 10, lastModified: 1 };
const B = { name: 'b.rvt', size: 10, lastModified: 1 };
const ID_A = 'a'.repeat(32);
const ID_B = 'b'.repeat(32);

describe('uploadRegistry', () => {
  it('finds an upload only by the full fingerprint', () => {
    const registry = uploadRegistry(memory(), () => 1000);
    registry.add(A, ID_A, `/api/uploads/${ID_A}`);
    expect(registry.find(A)).toMatchObject({ jobId: ID_A, uploadUrl: `/api/uploads/${ID_A}`, createdAt: 1000 });
    expect(registry.find({ ...A, lastModified: 2 })).toBeNull();
    expect(registry.find({ ...A, size: 11 })).toBeNull();
    expect(registry.find(B)).toBeNull();
  });

  it('keeps one entry per file and removes by job', () => {
    const storage = memory();
    const registry = uploadRegistry(storage, () => 1000);
    registry.add(A, ID_A, 'u1');
    registry.add(B, ID_B, 'u2');
    registry.add(A, 'c'.repeat(32), 'u3');
    expect(registry.all().map((entry) => entry.uploadUrl)).toEqual(['u2', 'u3']);
    registry.remove(ID_B);
    registry.remove('c'.repeat(32));
    expect(registry.all()).toEqual([]);
    expect(storage.data.has(UPLOADS_STORAGE_KEY)).toBe(false);
  });

  it('drops entries older than 6 hours', () => {
    let now = 0;
    const registry = uploadRegistry(memory(), () => now);
    registry.add(A, ID_A, 'u1');
    now = UPLOAD_ENTRY_MAX_AGE_MS - 1;
    registry.add(B, ID_B, 'u2');
    expect(registry.find(A)).not.toBeNull();
    now = UPLOAD_ENTRY_MAX_AGE_MS;
    expect(registry.find(A)).toBeNull();
    expect(registry.all().map((entry) => entry.jobId)).toEqual([ID_B]);
  });

  it('reads broken or foreign data as empty', () => {
    const storage = memory();
    const registry = uploadRegistry(storage);
    storage.setItem(UPLOADS_STORAGE_KEY, '{nope');
    expect(registry.all()).toEqual([]);
    storage.setItem(UPLOADS_STORAGE_KEY, JSON.stringify([{ name: 'x' }, 3]));
    expect(registry.all()).toEqual([]);
  });

  it('survives storage that throws', () => {
    const broken: KeyValueStorage = {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
      removeItem: () => {
        throw new Error('blocked');
      },
    };
    const registry = uploadRegistry(broken);
    expect(() => {
      registry.add(A, ID_A, 'u');
    }).not.toThrow();
    expect(registry.find(A)).toBeNull();
  });
});

import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { test as base } from '@playwright/test';
import CFB from 'cfb';

function utf16le(text: string): Buffer {
  return Buffer.from(text, 'utf16le');
}

/** A real OLE file: a `BasicFileInfo` stream naming the format year, plus a marker stream when the engine stub must misbehave. */
export function revitFileBytes(year: number, stubMode?: string): Buffer {
  const container = CFB.utils.cfb_new();
  const info = Buffer.concat([
    Buffer.alloc(4, 1),
    utf16le(`Worksharing: Not enabled\r\nUsername: \r\nFormat: ${String(year)}\r\nBuild: 26.0.4.409\r\n`),
  ]);
  CFB.utils.cfb_add(container, '/BasicFileInfo', info);
  if (stubMode) {
    CFB.utils.cfb_add(container, '/Marker', Buffer.from(`stub-mode=${stubMode}`, 'ascii'));
  }
  return Buffer.from(CFB.write(container, { type: 'array' }) as ArrayLike<number>);
}

const directory = mkdtempSync(join(tmpdir(), 'backsave-e2e-files-'));

/** Writes a Revit-like file with the given name and returns its path. The stub mode comes from the content, not the name. */
export function revitFile(name: string, year: number, stubMode?: string): string {
  mkdirSync(directory, { recursive: true });
  const path = join(directory, name);
  writeFileSync(path, revitFileBytes(year, stubMode));
  return path;
}

let counter = 0;

/**
 * Every test gets its own client address (the server trusts the last X-Forwarded-For entry), so the per-address job limits
 * never couple tests together.
 */
export const test = base.extend<{ clientIp: string }>({
  clientIp: async ({}, use, testInfo) => {
    counter += 1;
    await use(`10.${String(testInfo.workerIndex % 250)}.${String(counter >> 8)}.${String(counter & 255)}`);
  },
  extraHTTPHeaders: async ({ clientIp }, use) => {
    await use({ 'X-Forwarded-For': clientIp });
  },
});

export { expect } from '@playwright/test';

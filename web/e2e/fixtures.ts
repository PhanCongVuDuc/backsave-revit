import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { expect, test as base } from '@playwright/test';
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

const directory = mkdtempSync(join(tmpdir(), 'revit-downgrade-e2e-files-'));

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
export const test = base.extend<{ clientIp: string; noPolicyViolations: undefined }, { filesDir: string }>({
  // The generated files live for one worker and are removed with it.
  filesDir: [
    async ({}, use) => {
      await use(directory);
      rmSync(directory, { recursive: true, force: true });
    },
    { scope: 'worker', auto: true },
  ],
  // The built app must run under the server's Content-Security-Policy: any violation fails the test.
  noPolicyViolations: [
    async ({ page }, use) => {
      const violations: string[] = [];
      page.on('console', (message) => {
        if (message.type() === 'error' && /Content Security Policy/i.test(message.text())) {
          violations.push(message.text());
        }
      });
      await use(undefined);
      expect(violations).toEqual([]);
    },
    { auto: true },
  ],
  clientIp: async ({}, use, testInfo) => {
    counter += 1;
    await use(`10.${String(testInfo.workerIndex % 250)}.${String(counter >> 8)}.${String(counter & 255)}`);
  },
  extraHTTPHeaders: async ({ clientIp }, use) => {
    await use({ 'X-Forwarded-For': clientIp });
  },
});

export { expect };

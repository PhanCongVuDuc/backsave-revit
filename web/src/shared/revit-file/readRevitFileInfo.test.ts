import { openAsBlob } from 'node:fs';
import { readdir } from 'node:fs/promises';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { basicFileInfo, countingFile, fileOf, makeOle, makeOleV4, utf16le } from './makeOle.testutil';
import { readStream } from './ole';
import { readRevitFileInfo, RevitFileError } from './readRevitFileInfo';

const MIB = 1024 * 1024;

async function errorCodeOf(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
    return 'no error';
  } catch (error) {
    return error instanceof RevitFileError ? error.code : `unexpected ${String(error)}`;
  }
}

describe('readRevitFileInfo', () => {
  it('reads_year_and_kind_from_generated_ole', async () => {
    const bytes = makeOle([['BasicFileInfo', basicFileInfo(2026, { padTo: 6000 })]]);
    await expect(readRevitFileInfo(fileOf(bytes, 'tower.rvt'))).resolves.toEqual({ kind: 'rvt', year: 2026 });
  });

  it('reads a small BasicFileInfo from the mini stream', async () => {
    const stream = basicFileInfo(2024);
    expect(stream.length).toBeLessThan(4096);
    const bytes = makeOle([
      ['Contents', new Uint8Array(300).fill(7)],
      ['BasicFileInfo', stream],
    ]);
    await expect(readRevitFileInfo(fileOf(bytes, 'Door.RFA'))).resolves.toEqual({ kind: 'rfa', year: 2024 });
  });

  it('finds the year when the text starts at an odd offset', async () => {
    const bytes = makeOle([['BasicFileInfo', basicFileInfo(2025, { prefix: 5 })]]);
    await expect(readRevitFileInfo(fileOf(bytes, 'template.rte'))).resolves.toEqual({ kind: 'rte', year: 2025 });
  });

  it('reads a version 4 file with 4096-byte sectors', async () => {
    const bytes = makeOleV4('BasicFileInfo', basicFileInfo(2027, { padTo: 9000 }));
    await expect(readRevitFileInfo(fileOf(bytes, 'family.rft'))).resolves.toEqual({ kind: 'rft', year: 2027 });
  });

  it('follows the DIFAT chain when the FAT needs more than 109 sectors', async () => {
    const bytes = makeOle([
      ['Filler', new Uint8Array(8 * MIB)],
      ['BasicFileInfo', basicFileInfo(2023, { padTo: 5000 })],
    ]);
    const view = new DataView(bytes.buffer);
    expect(view.getUint32(0x48, true)).toBeGreaterThan(0);
    await expect(readRevitFileInfo(fileOf(bytes, 'big.rvt'))).resolves.toEqual({ kind: 'rvt', year: 2023 });
  });

  it('big_file_reads_only_slices', async () => {
    const bytes = makeOle([
      ['Contents', new Uint8Array(64 * 1024).fill(3)],
      ['BasicFileInfo', basicFileInfo(2026)],
    ]);
    const { file, counter } = countingFile(bytes, 'huge.rvt', 900 * MIB);
    await expect(readRevitFileInfo(file)).resolves.toEqual({ kind: 'rvt', year: 2026 });
    expect(counter.requested).toBeLessThan(MIB);
  });

  it('office_ole_is_not_revit', async () => {
    const bytes = makeOle([
      ['WordDocument', new Uint8Array(5000).fill(1)],
      ['SummaryInformation', new Uint8Array(200)],
    ]);
    await expect(errorCodeOf(readRevitFileInfo(fileOf(bytes, 'letter.rvt')))).resolves.toBe('not_revit');
  });

  it('rejects a BasicFileInfo without a format year as not Revit', async () => {
    const bytes = makeOle([['BasicFileInfo', utf16le('Worksharing: Not enabled\r\n')]]);
    await expect(errorCodeOf(readRevitFileInfo(fileOf(bytes, 'odd.rvt')))).resolves.toBe('not_revit');
  });

  it('rejects a file without the OLE signature', async () => {
    const bytes = new TextEncoder().encode('PK\u0003\u0004 this is a zip, not a Revit file'.repeat(40));
    await expect(errorCodeOf(readRevitFileInfo(fileOf(bytes, 'archive.rvt')))).resolves.toBe('not_ole');
    await expect(errorCodeOf(readRevitFileInfo(fileOf(new Uint8Array(0), 'empty.rvt')))).resolves.toBe('not_ole');
  });

  it('truncated_file_is_unreadable', async () => {
    const bytes = makeOle([['BasicFileInfo', basicFileInfo(2026, { padTo: 6000 })]]);
    for (const length of [100, 600, 2000, bytes.length - 1000]) {
      await expect(errorCodeOf(readRevitFileInfo(fileOf(bytes.slice(0, length), 'cut.rvt')))).resolves.toBe(
        'unreadable',
      );
    }
  });

  it('reports a FAT cycle as unreadable instead of looping', async () => {
    const bytes = makeOleV4('BasicFileInfo', basicFileInfo(2026, { padTo: 9000 }));
    const view = new DataView(bytes.buffer);
    // FAT entry of the stream's second sector (sector 3) points back to its first (sector 2).
    view.setUint32(4096 + 3 * 4, 2, true);
    await expect(errorCodeOf(readRevitFileInfo(fileOf(bytes, 'loop.rvt')))).resolves.toBe('unreadable');
  });

  it('reports an out-of-range sector number as unreadable', async () => {
    const bytes = makeOleV4('BasicFileInfo', basicFileInfo(2026, { padTo: 9000 }));
    const view = new DataView(bytes.buffer);
    view.setUint32(4096 * 2 + 128 + 0x74, 999_999, true);
    await expect(errorCodeOf(readRevitFileInfo(fileOf(bytes, 'far.rvt')))).resolves.toBe('unreadable');
  });

  it('bad_extension_rejected_before_read', async () => {
    const { file, counter } = countingFile(new Uint8Array(0), 'drawing.dwg', 1000);
    await expect(errorCodeOf(readRevitFileInfo(file))).resolves.toBe('bad_extension');
    const noExtension = countingFile(new Uint8Array(0), 'rvt', 1000);
    await expect(errorCodeOf(readRevitFileInfo(noExtension.file))).resolves.toBe('bad_extension');
    expect(counter.slices + noExtension.counter.slices).toBe(0);
  });

  it('over_1gib_rejected_before_read', async () => {
    const { file, counter } = countingFile(new Uint8Array(0), 'tower.rvt', 1_073_741_825);
    await expect(errorCodeOf(readRevitFileInfo(file))).resolves.toBe('too_large');
    expect(counter.slices).toBe(0);
  });

  it('accepts a file of exactly 1 GiB', async () => {
    const bytes = makeOle([['BasicFileInfo', basicFileInfo(2026)]]);
    const { file } = countingFile(bytes, 'tower.rvt', 1_073_741_824);
    await expect(readRevitFileInfo(file)).resolves.toEqual({ kind: 'rvt', year: 2026 });
  });

  const samples = process.env.REVIT_DOWNGRADE_SAMPLES ?? '';

  it.skipIf(samples === '')('real_samples_2023_2027', async () => {
    const names = (await readdir(samples)).filter((name) => /_(\d{4})\.(?:rvt|rte|rfa|rft)$/i.test(name));
    expect(names.length).toBeGreaterThan(0);
    for (const name of names) {
      const expected = Number(/_(\d{4})\.\w+$/.exec(name)?.[1]);
      const blob = await openAsBlob(join(samples, name));
      let requested = 0;
      const file = {
        name,
        size: blob.size,
        slice: (start: number, end: number) => {
          requested += end - start;
          return blob.slice(start, end);
        },
      };
      const info = await readRevitFileInfo(file);
      expect({ name, year: info.year }).toEqual({ name, year: expected });
      expect(requested).toBeLessThan(MIB);
    }
  });
});

describe('readStream', () => {
  it('returns the exact bytes of a named stream', async () => {
    const content = new Uint8Array(7000).map((_, index) => index % 251);
    const bytes = makeOle([
      ['First', new Uint8Array(100)],
      ['Wanted', content],
    ]);
    await expect(readStream(fileOf(bytes, 'x.rvt'), 'Wanted')).resolves.toEqual(content);
  });

  it('matches stream names case-sensitively', async () => {
    const bytes = makeOle([['basicfileinfo', basicFileInfo(2026)]]);
    await expect(errorCodeOf(readRevitFileInfo(fileOf(bytes, 'lower.rvt')))).resolves.toBe('not_revit');
  });
});

// A minimal Compound File Binary (OLE) reader that pulls one stream out of a file through `blob.slice(a, b).arrayBuffer()`
// only, so a 1 GiB model costs a few kilobytes of reads. Layout per [MS-CFB]: a 512-byte header, a FAT located through the
// DIFAT, a directory of 128-byte entries, and a mini stream (64-byte sectors, own MiniFAT) for streams under the cutoff.

export type OleErrorCode = 'not_ole' | 'unreadable' | 'not_found';

export class OleError extends Error {
  readonly code: OleErrorCode;

  constructor(code: OleErrorCode, message: string) {
    super(message);
    this.name = 'OleError';
    this.code = code;
  }
}

const SIGNATURE = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const HEADER_SIZE = 512;
const HEADER_DIFAT_ENTRIES = 109;
const MAX_REGULAR_SECTOR = 0xff_ff_ff_fa;
const END_OF_CHAIN = 0xff_ff_ff_fe;
const DIRECTORY_ENTRY_SIZE = 128;
const MINI_SECTOR_SIZE = 64;
const TYPE_STREAM = 2;
const TYPE_ROOT = 5;

/** A stream larger than this is not read; callers here only need small metadata streams. */
const DEFAULT_MAX_STREAM = 1024 * 1024;

type Slicer = Pick<Blob, 'size' | 'slice'>;

interface DirectoryEntry {
  name: string;
  type: number;
  start: number;
  size: number;
}

function unreadable(message: string): never {
  throw new OleError('unreadable', message);
}

async function readRange(blob: Slicer, start: number, length: number): Promise<Uint8Array> {
  if (start < 0 || start + length > blob.size) {
    unreadable(`range ${String(start)}+${String(length)} lies outside the file`);
  }
  const bytes = new Uint8Array(await blob.slice(start, start + length).arrayBuffer());
  if (bytes.length !== length) {
    unreadable(`short read at ${String(start)}`);
  }
  return bytes;
}

/** Merges ranges that touch, so a contiguous run of sectors costs one read. */
async function readRanges(blob: Slicer, ranges: [start: number, length: number][]): Promise<Uint8Array> {
  const total = ranges.reduce((sum, [, length]) => sum + length, 0);
  const out = new Uint8Array(total);
  let written = 0;
  let index = 0;
  while (index < ranges.length) {
    const first = ranges[index];
    if (!first) {
      break;
    }
    const [start] = first;
    let length = first[1];
    index += 1;
    for (let next = ranges[index]; next && next[0] === start + length; next = ranges[index]) {
      length += next[1];
      index += 1;
    }
    out.set(await readRange(blob, start, length), written);
    written += length;
  }
  return out;
}

class OleReader {
  private readonly fatCache = new Map<number, DataView>();
  private readonly difatSectors: number[] = [];
  private readonly difatViews: DataView[] = [];
  private nextDifatSector: number;
  private readonly sectorCount: number;

  constructor(
    private readonly blob: Slicer,
    private readonly header: DataView,
    private readonly sectorSize: number,
  ) {
    this.nextDifatSector = header.getUint32(0x44, true);
    this.sectorCount = Math.floor((blob.size - sectorSize + sectorSize - 1) / sectorSize);
  }

  private get entriesPerSector() {
    return this.sectorSize / 4;
  }

  private checkSector(sector: number) {
    if (sector > MAX_REGULAR_SECTOR || sector >= this.sectorCount) {
      unreadable(`sector ${String(sector)} is out of range`);
    }
  }

  sectorOffset(sector: number): number {
    this.checkSector(sector);
    return (sector + 1) * this.sectorSize;
  }

  private async readSector(sector: number): Promise<DataView> {
    const bytes = await readRange(this.blob, this.sectorOffset(sector), this.sectorSize);
    return new DataView(bytes.buffer);
  }

  /** The sector number of FAT sector `index`, from the header DIFAT or, beyond 109, the DIFAT sector chain. */
  private async fatSectorNumber(index: number): Promise<number> {
    if (index < HEADER_DIFAT_ENTRIES) {
      return this.header.getUint32(0x4c + index * 4, true);
    }
    const perDifat = this.entriesPerSector - 1;
    const difatIndex = Math.floor((index - HEADER_DIFAT_ENTRIES) / perDifat);
    const declared = this.header.getUint32(0x48, true);
    while (this.difatSectors.length <= difatIndex) {
      if (this.difatSectors.length >= declared || this.difatSectors.length >= this.sectorCount) {
        unreadable('the DIFAT chain is shorter than the FAT needs');
      }
      const sector = this.nextDifatSector;
      if (this.difatSectors.includes(sector)) {
        unreadable('the DIFAT chain loops');
      }
      const view = await this.readSector(sector);
      this.difatSectors.push(sector);
      this.difatViews.push(view);
      this.nextDifatSector = view.getUint32(perDifat * 4, true);
    }
    const view = this.difatViews[difatIndex] ?? unreadable('missing DIFAT sector');
    return view.getUint32(((index - HEADER_DIFAT_ENTRIES) % perDifat) * 4, true);
  }

  async nextSector(sector: number): Promise<number> {
    this.checkSector(sector);
    const fatIndex = Math.floor(sector / this.entriesPerSector);
    let fat = this.fatCache.get(fatIndex);
    if (!fat) {
      if (fatIndex >= this.header.getUint32(0x2c, true)) {
        unreadable(`sector ${String(sector)} has no FAT entry`);
      }
      fat = await this.readSector(await this.fatSectorNumber(fatIndex));
      this.fatCache.set(fatIndex, fat);
    }
    return fat.getUint32((sector % this.entriesPerSector) * 4, true);
  }

  /** Walks a FAT chain lazily: `at(i)` is the i-th sector of the chain; cycles and bad links are unreadable. */
  chain(start: number) {
    const sectors: number[] = [];
    const seen = new Set<number>();
    let ended = start === END_OF_CHAIN;
    const at = async (index: number): Promise<number | null> => {
      while (sectors.length <= index && !ended) {
        const last = sectors.at(-1);
        const sector = last === undefined ? start : await this.nextSector(last);
        if (sector === END_OF_CHAIN) {
          ended = true;
          break;
        }
        this.checkSector(sector);
        if (seen.has(sector)) {
          unreadable('a sector chain loops');
        }
        seen.add(sector);
        sectors.push(sector);
      }
      return sectors[index] ?? null;
    };
    return at;
  }

  async regularStream(start: number, size: number): Promise<Uint8Array> {
    const at = this.chain(start);
    const ranges: [number, number][] = [];
    for (let offset = 0, index = 0; offset < size; offset += this.sectorSize, index += 1) {
      const sector = (await at(index)) ?? unreadable('a stream ends before its declared size');
      ranges.push([this.sectorOffset(sector), Math.min(this.sectorSize, size - offset)]);
    }
    return readRanges(this.blob, ranges);
  }

  async miniStream(root: DirectoryEntry, start: number, size: number): Promise<Uint8Array> {
    const miniFatAt = this.chain(this.header.getUint32(0x3c, true));
    const miniFatSectors = this.header.getUint32(0x40, true);
    const containerAt = this.chain(root.start);
    const miniFatCache = new Map<number, DataView>();
    const miniSectorCount = Math.ceil(root.size / MINI_SECTOR_SIZE);
    const nextMini = async (mini: number): Promise<number> => {
      const index = Math.floor(mini / this.entriesPerSector);
      if (index >= miniFatSectors) {
        unreadable(`mini sector ${String(mini)} has no MiniFAT entry`);
      }
      let view = miniFatCache.get(index);
      if (!view) {
        const sector = (await miniFatAt(index)) ?? unreadable('the MiniFAT chain is too short');
        view = await this.readSector(sector);
        miniFatCache.set(index, view);
      }
      return view.getUint32((mini % this.entriesPerSector) * 4, true);
    };
    const ranges: [number, number][] = [];
    const seen = new Set<number>();
    let mini = start;
    for (let offset = 0; offset < size; offset += MINI_SECTOR_SIZE) {
      if (mini >= miniSectorCount || seen.has(mini)) {
        unreadable(`mini sector ${String(mini)} is out of range or loops`);
      }
      seen.add(mini);
      const byteInContainer = mini * MINI_SECTOR_SIZE;
      const containerSector =
        (await containerAt(Math.floor(byteInContainer / this.sectorSize))) ??
        unreadable('the mini stream is too short');
      ranges.push([
        this.sectorOffset(containerSector) + (byteInContainer % this.sectorSize),
        Math.min(MINI_SECTOR_SIZE, size - offset),
      ]);
      if (offset + MINI_SECTOR_SIZE < size) {
        mini = await nextMini(mini);
      }
    }
    return readRanges(this.blob, ranges);
  }

  /** Reads directory sectors one by one until `name` is found; the root entry is always entry 0. */
  async find(name: string): Promise<{ root: DirectoryEntry; entry: DirectoryEntry | null }> {
    const at = this.chain(this.header.getUint32(0x30, true));
    const major = this.header.getUint16(0x1a, true);
    let root: DirectoryEntry | null = null;
    for (let index = 0; ; index += 1) {
      const sector = await at(index);
      if (sector === null) {
        return { root: root ?? unreadable('the directory is empty'), entry: null };
      }
      const view = await this.readSector(sector);
      for (let offset = 0; offset < this.sectorSize; offset += DIRECTORY_ENTRY_SIZE) {
        const entry = parseEntry(view, offset, major);
        if (root === null) {
          if (entry.type !== TYPE_ROOT) {
            unreadable('the first directory entry is not the root');
          }
          root = entry;
        } else if (entry.type === TYPE_STREAM && entry.name === name) {
          return { root, entry };
        }
      }
    }
  }

  get miniStreamCutoff() {
    return this.header.getUint32(0x38, true);
  }
}

function parseEntry(view: DataView, offset: number, major: number): DirectoryEntry {
  const nameBytes = Math.min(view.getUint16(offset + 0x40, true), 64);
  let name = '';
  for (let at = 0; at + 2 <= nameBytes - 2; at += 2) {
    name += String.fromCharCode(view.getUint16(offset + at, true));
  }
  const low = view.getUint32(offset + 0x78, true);
  // Version 3 files may leave garbage in the high half of the size; version 4 files use all 64 bits.
  const high = major === 4 ? view.getUint32(offset + 0x7c, true) : 0;
  if (high !== 0) {
    unreadable('a stream is larger than 4 GiB');
  }
  return { name, type: view.getUint8(offset + 0x42), start: view.getUint32(offset + 0x74, true), size: low };
}

async function openOle(blob: Slicer): Promise<OleReader> {
  const head = new Uint8Array(await blob.slice(0, HEADER_SIZE).arrayBuffer());
  if (head.length < SIGNATURE.length || SIGNATURE.some((byte, index) => head[index] !== byte)) {
    throw new OleError('not_ole', 'the file is not a Compound File (OLE) container');
  }
  if (head.length < HEADER_SIZE) {
    unreadable('the header is truncated');
  }
  const header = new DataView(head.buffer);
  const major = header.getUint16(0x1a, true);
  const shift = header.getUint16(0x1e, true);
  if (!((major === 3 && shift === 9) || (major === 4 && shift === 12)) || header.getUint16(0x20, true) !== 6) {
    unreadable(`unsupported version ${String(major)} with sector shift ${String(shift)}`);
  }
  return new OleReader(blob, header, 1 << shift);
}

/**
 * The bytes of the stream called `name` (case-sensitive, anywhere in the directory). Throws `OleError`: `not_ole` without
 * the signature, `not_found` without the stream, `unreadable` for anything truncated, looping or out of range.
 */
export async function readStream(blob: Slicer, name: string, options: { maxBytes?: number } = {}): Promise<Uint8Array> {
  try {
    const reader = await openOle(blob);
    const { root, entry } = await reader.find(name);
    if (!entry) {
      throw new OleError('not_found', `no stream named ${name}`);
    }
    if (entry.size > (options.maxBytes ?? DEFAULT_MAX_STREAM)) {
      unreadable(`stream ${name} is larger than expected`);
    }
    return entry.size < reader.miniStreamCutoff
      ? await reader.miniStream(root, entry.start, entry.size)
      : await reader.regularStream(entry.start, entry.size);
  } catch (error) {
    if (error instanceof OleError) {
      throw error;
    }
    // A file that changed or vanished while being read rejects its arrayBuffer() promise.
    throw new OleError('unreadable', error instanceof Error ? error.message : String(error));
  }
}

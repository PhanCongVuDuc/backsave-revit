// Builds real Compound File Binary (OLE) files for the reader tests. Not shipped: only *.test.ts files import it.
import CFB from 'cfb';

export function utf16le(text: string): Uint8Array {
  const bytes = new Uint8Array(text.length * 2);
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    bytes[index * 2] = code & 0xff;
    bytes[index * 2 + 1] = code >> 8;
  }
  return bytes;
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((total, part) => total + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

/** The content of a `BasicFileInfo` stream: a short binary prefix, then UTF-16LE text naming the format year. */
export function basicFileInfo(year: number, options: { padTo?: number; prefix?: number } = {}): Uint8Array {
  const prefix = new Uint8Array(options.prefix ?? 4).fill(1);
  const text = utf16le(`Worksharing: Not enabled\r\nUsername: \r\nFormat: ${String(year)}\r\nBuild: 26.0.4.409\r\n`);
  const body = concat(prefix, text);
  const padTo = options.padTo ?? 0;
  return body.length >= padTo ? body : concat(body, new Uint8Array(padTo - body.length));
}

/** A version 3 (512-byte sector) OLE file written by the `cfb` package, holding the given streams in order. */
export function makeOle(streams: [name: string, content: Uint8Array][]): Uint8Array {
  const container = CFB.utils.cfb_new();
  for (const [name, content] of streams) {
    CFB.utils.cfb_add(container, `/${name}`, content);
  }
  return new Uint8Array(CFB.write(container, { type: 'array' }) as ArrayLike<number>);
}

const FREE = 0xff_ff_ff_ff;
const END_OF_CHAIN = 0xff_ff_ff_fe;
const FAT_SECTOR = 0xff_ff_ff_fd;
const NO_STREAM = 0xff_ff_ff_ff;

function writeName(view: DataView, offset: number, name: string) {
  for (let index = 0; index < name.length; index += 1) {
    view.setUint16(offset + index * 2, name.charCodeAt(index), true);
  }
  view.setUint16(offset + 0x40, (name.length + 1) * 2, true);
}

/**
 * A version 4 (4096-byte sector) OLE file with one stream in the regular FAT, written by hand because `cfb` writes version 3
 * only. Layout: header sector, sector 0 = FAT, sector 1 = directory, sectors 2… = the stream.
 */
export function makeOleV4(name: string, content: Uint8Array): Uint8Array {
  const size = 4096;
  if (content.length < size) {
    throw new Error('a version 4 test stream must be at least 4096 bytes to sit in the regular FAT');
  }
  const dataSectors = Math.ceil(content.length / size);
  const file = new Uint8Array(size * (3 + dataSectors));
  const view = new DataView(file.buffer);
  file.set([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1], 0);
  view.setUint16(0x18, 0x3e, true);
  view.setUint16(0x1a, 4, true);
  view.setUint16(0x1c, 0xff_fe, true);
  view.setUint16(0x1e, 12, true);
  view.setUint16(0x20, 6, true);
  view.setUint32(0x28, 1, true);
  view.setUint32(0x2c, 1, true);
  view.setUint32(0x30, 1, true);
  view.setUint32(0x38, 4096, true);
  view.setUint32(0x3c, END_OF_CHAIN, true);
  view.setUint32(0x44, END_OF_CHAIN, true);
  for (let index = 0; index < 109; index += 1) {
    view.setUint32(0x4c + index * 4, index === 0 ? 0 : FREE, true);
  }
  const fat = size;
  for (let index = 0; index < size / 4; index += 1) {
    view.setUint32(fat + index * 4, FREE, true);
  }
  view.setUint32(fat, FAT_SECTOR, true);
  view.setUint32(fat + 4, END_OF_CHAIN, true);
  for (let index = 0; index < dataSectors; index += 1) {
    const sector = 2 + index;
    view.setUint32(fat + sector * 4, index === dataSectors - 1 ? END_OF_CHAIN : sector + 1, true);
  }
  const directory = size * 2;
  for (let entry = 0; entry < size / 128; entry += 1) {
    const at = directory + entry * 128;
    view.setUint32(at + 0x44, NO_STREAM, true);
    view.setUint32(at + 0x48, NO_STREAM, true);
    view.setUint32(at + 0x4c, NO_STREAM, true);
  }
  writeName(view, directory, 'Root Entry');
  view.setUint8(directory + 0x42, 5);
  view.setUint32(directory + 0x4c, 1, true);
  view.setUint32(directory + 0x74, END_OF_CHAIN, true);
  writeName(view, directory + 128, name);
  view.setUint8(directory + 128 + 0x42, 2);
  view.setUint32(directory + 128 + 0x74, 2, true);
  view.setUint32(directory + 128 + 0x78, content.length, true);
  file.set(content, size * 3);
  return file;
}

export function fileOf(bytes: Uint8Array, name: string): File {
  return new File([bytes.slice().buffer], name);
}

/**
 * A Blob-like file that claims `claimedSize` bytes and is backed by `bytes` (reads past the end return nothing), counting
 * every byte the reader asks for through `slice`.
 */
export function countingFile(bytes: Uint8Array, name: string, claimedSize: number) {
  const counter = { requested: 0, slices: 0 };
  const sliceOf = (start: number, end: number): Blob => {
    counter.requested += Math.max(0, end - start);
    counter.slices += 1;
    return new Blob([bytes.slice(Math.min(start, bytes.length), Math.min(end, bytes.length))]);
  };
  const file = {
    name,
    size: claimedSize,
    type: '',
    slice: (start = 0, end = claimedSize) => sliceOf(start, end),
    arrayBuffer: () => Promise.reject(new Error('the reader must never read the whole file')),
  };
  return { file: file as unknown as File, counter };
}

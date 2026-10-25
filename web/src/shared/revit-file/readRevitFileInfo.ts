import { OleError, readStream } from './ole';

export const REVIT_KINDS = ['rvt', 'rte', 'rfa', 'rft'] as const;
export type RevitFileKind = (typeof REVIT_KINDS)[number];

export interface RevitFileInfo {
  kind: RevitFileKind;
  year: number;
}

export type RevitFileErrorCode = 'bad_extension' | 'too_large' | 'not_ole' | 'not_revit' | 'unreadable';

export class RevitFileError extends Error {
  readonly code: RevitFileErrorCode;

  constructor(code: RevitFileErrorCode, message: string) {
    super(message);
    this.name = 'RevitFileError';
    this.code = code;
  }
}

/** 1 GiB, the largest upload the server accepts. */
export const MAX_FILE_BYTES = 1_073_741_824;

const YEAR_PATTERN = /Format: (\d{4})/;

function kindOf(name: string): RevitFileKind | null {
  const dot = name.lastIndexOf('.');
  if (dot <= 0) {
    return null;
  }
  const extension = name.slice(dot + 1).toLowerCase();
  return REVIT_KINDS.find((kind) => kind === extension) ?? null;
}

/** The text is UTF-16LE, but the binary prefix before it may leave it at an odd offset, so both alignments are tried. */
function yearIn(stream: Uint8Array): number | null {
  const decoder = new TextDecoder('utf-16le');
  for (const shift of [0, 1]) {
    const match = YEAR_PATTERN.exec(decoder.decode(stream.subarray(shift)));
    if (match?.[1]) {
      return Number(match[1]);
    }
  }
  return null;
}

/**
 * The kind (from the extension) and the format year (from the `BasicFileInfo` stream) of a Revit file, read in the browser
 * through a few small slices. Throws `RevitFileError`. Whether the year can be converted is the caller's question.
 */
export async function readRevitFileInfo(file: Pick<Blob, 'size' | 'slice'> & { name: string }): Promise<RevitFileInfo> {
  const kind = kindOf(file.name);
  if (!kind) {
    throw new RevitFileError('bad_extension', 'only .rvt, .rte, .rfa and .rft files are accepted');
  }
  if (file.size > MAX_FILE_BYTES) {
    throw new RevitFileError('too_large', 'the file is larger than 1 GiB');
  }
  let stream: Uint8Array;
  try {
    stream = await readStream(file, 'BasicFileInfo');
  } catch (error) {
    if (error instanceof OleError) {
      switch (error.code) {
        case 'not_ole':
          throw new RevitFileError('not_ole', error.message);
        case 'not_found':
          throw new RevitFileError('not_revit', 'the file has no BasicFileInfo stream');
        case 'unreadable':
          throw new RevitFileError('unreadable', error.message);
      }
    }
    throw new RevitFileError('unreadable', error instanceof Error ? error.message : String(error));
  }
  const year = yearIn(stream);
  if (year === null) {
    throw new RevitFileError('not_revit', 'BasicFileInfo names no format year');
  }
  return { kind, year };
}

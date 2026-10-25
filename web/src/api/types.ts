// The server's /api, in camelCase. The wire format (snake_case) is mapped in client.ts.

export type FileKind = 'rvt' | 'rte' | 'rfa' | 'rft';

/** A conversion the server can do: [from year, to year]. */
export type YearPair = [from: number, to: number];

export interface ServerInfo {
  maxBytes: number;
  outputTtlS: number;
  pairs: YearPair[];
  version: string;
}

export interface CreateJobRequest {
  name: string;
  size: number;
  kind: FileKind;
  year: number;
  target: number;
}

export interface CreatedJob {
  jobId: string;
  uploadUrl: string;
}

export type JobState = 'waiting_upload' | 'queued' | 'running' | 'done' | 'failed' | 'cancelled' | 'expired';

export type JobErrorKind = 'refused' | 'corrupt' | 'failed' | 'crashed' | 'timed_out' | 'mismatch' | 'disk_full';

export interface JobStatus {
  state: JobState;
  position: number | null;
  elapsedS: number;
  estimateS: number;
  /** RFC 3339 UTC. */
  expiresAt: string | null;
  error: { kind: JobErrorKind; message: string } | null;
  result: { fileName: string; size: number; droppedCount: number } | null;
  reportCode: string | null;
  /** What the job was created with; the job id is the capability that reveals them. */
  name: string;
  size: number;
  kind: FileKind;
  year: number;
  target: number;
}

export type DropReason = 'no_rule' | 'rule_broken' | 'cascade';

export interface DroppedElement {
  elementId: number;
  class: string;
  reason: DropReason;
  cascadeFrom: number | null;
}

export interface UploadOptions {
  onProgress?: (bytesSent: number, bytesTotal: number) => void;
  signal?: AbortSignal;
}

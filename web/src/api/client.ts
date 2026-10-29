import type {
  CreatedJob,
  CreateJobRequest,
  DroppedElement,
  DropReason,
  FileKind,
  JobErrorKind,
  JobState,
  JobStatus,
  ServerInfo,
  UsageStats,
  YearPair,
} from './types';

/**
 * A failed request. `status` is the HTTP status (0 when no response arrived); `code` is the server's error code, or one of
 * `http_<status>`, `network`, `bad_response`, `aborted`, `job_gone`, `upload_failed`; `retryAfterS` says when to retry.
 */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly retryAfterS: number | null;

  constructor(status: number, code: string, message: string, retryAfterS: number | null = null) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.retryAfterS = retryAfterS;
  }
}

interface WireError {
  error?: { code?: unknown; message?: unknown; retry_after_s?: unknown };
}

/** The `Retry-After` header holds delta-seconds or an HTTP date. */
function parseRetryAfter(header: string | null | undefined): number | null {
  const value = header?.trim();
  if (!value) {
    return null;
  }
  if (/^\d+$/.test(value)) {
    return Number(value);
  }
  const date = Date.parse(value);
  return Number.isNaN(date) ? null : Math.max(0, Math.ceil((date - Date.now()) / 1000));
}

/** The ApiError for an error response, from its status, body text and `Retry-After` header. */
export function apiErrorFrom(status: number, body: string, retryAfterHeader: string | null | undefined): ApiError {
  let parsed: WireError | null;
  try {
    parsed = JSON.parse(body) as WireError | null;
  } catch {
    parsed = null;
  }
  const wire = parsed?.error;
  const code = typeof wire?.code === 'string' ? wire.code : `http_${String(status)}`;
  const message = typeof wire?.message === 'string' ? wire.message : `HTTP ${String(status)}`;
  const retryAfterS = typeof wire?.retry_after_s === 'number' ? wire.retry_after_s : parseRetryAfter(retryAfterHeader);
  return new ApiError(status, code, message, retryAfterS);
}

async function request(method: string, path: string, body?: unknown): Promise<Response> {
  const init: RequestInit =
    body === undefined
      ? { method, headers: { Accept: 'application/json' } }
      : {
          method,
          headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        };
  let response: Response;
  try {
    response = await fetch(path, init);
  } catch (error) {
    throw new ApiError(0, 'network', error instanceof Error ? error.message : String(error));
  }
  if (!response.ok) {
    const text = await response.text().catch(() => '');
    throw apiErrorFrom(response.status, text, response.headers.get('Retry-After'));
  }
  return response;
}

async function requestJson<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await request(method, path, body);
  try {
    return (await response.json()) as T;
  } catch {
    throw new ApiError(response.status, 'bad_response', 'the server sent a response that is not JSON');
  }
}

function jobPath(id: string, rest = ''): string {
  return `/api/jobs/${encodeURIComponent(id)}${rest}`;
}

export async function getInfo(): Promise<ServerInfo> {
  const wire = await requestJson<{ max_bytes: number; output_ttl_s: number; pairs: YearPair[]; version: string }>(
    'GET',
    '/api/info',
  );
  return { maxBytes: wire.max_bytes, outputTtlS: wire.output_ttl_s, pairs: wire.pairs, version: wire.version };
}

export async function createJob(job: CreateJobRequest): Promise<CreatedJob> {
  const wire = await requestJson<{ job_id: string; upload_url: string }>('POST', '/api/jobs', {
    name: job.name,
    size: job.size,
    kind: job.kind,
    year: job.year,
    target: job.target,
  });
  return { jobId: wire.job_id, uploadUrl: wire.upload_url };
}

interface WireJob {
  state: JobState;
  position: number | null;
  elapsed_s: number;
  estimate_s: number;
  expires_at: string | null;
  error: { kind: JobErrorKind; message: string } | null;
  result: { file_name: string; size: number; dropped_count: number } | null;
  report_code: string | null;
  name: string;
  size: number;
  kind: FileKind;
  year: number;
  target: number;
}

export async function getJob(id: string): Promise<JobStatus> {
  const wire = await requestJson<WireJob>('GET', jobPath(id));
  return {
    state: wire.state,
    position: wire.position,
    elapsedS: wire.elapsed_s,
    estimateS: wire.estimate_s,
    expiresAt: wire.expires_at,
    error: wire.error ? { kind: wire.error.kind, message: wire.error.message } : null,
    result: wire.result
      ? { fileName: wire.result.file_name, size: wire.result.size, droppedCount: wire.result.dropped_count }
      : null,
    reportCode: wire.report_code,
    name: wire.name,
    size: wire.size,
    kind: wire.kind,
    year: wire.year,
    target: wire.target,
  };
}

interface WireDropped {
  element_id: number;
  class: string;
  reason: DropReason;
  cascade_from: number | null;
}

export async function getDropped(id: string): Promise<DroppedElement[]> {
  const wire = await requestJson<WireDropped[]>('GET', jobPath(id, '/dropped'));
  return wire.map((item) => ({
    elementId: item.element_id,
    class: item.class,
    reason: item.reason,
    cascadeFrom: item.cascade_from,
  }));
}

export async function deleteJob(id: string): Promise<void> {
  await request('DELETE', jobPath(id));
}

/** Sends the failed job's input for review, with the user's consent; resolves to the report code. */
export async function sendReport(id: string): Promise<string> {
  const wire = await requestJson<{ report_code: string }>('POST', jobPath(id, '/report'), { consent: true });
  return wire.report_code;
}

/**
 * Opens (or resumes) the upload of the original for a "does not open in Revit" report on a done job, with the user's
 * consent; resolves to the tus upload URL. `size` must be the size of the file the job was made from.
 */
export async function startOpenReport(id: string, size: number, note?: string): Promise<string> {
  const body = note === undefined ? { consent: true, size } : { consent: true, size, note };
  const wire = await requestJson<{ upload_url: string }>('POST', jobPath(id, '/open-report'), body);
  return wire.upload_url;
}

/** Makes the "does not open in Revit" report once its upload is complete; resolves to the report code. */
export async function finishOpenReport(id: string): Promise<string> {
  const wire = await requestJson<{ report_code: string }>('POST', jobPath(id, '/open-report/finish'));
  return wire.report_code;
}

const STATS_COUNTS = ['people', 'succeeded', 'failed', 'contributed'] as const;

function isCount(value: unknown): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= 0;
}

/** The public usage totals, or `null` when they cannot be had for any reason. Never rejects. */
export async function getStats(): Promise<UsageStats | null> {
  try {
    const wire = await requestJson<Record<string, unknown> | null>('GET', '/api/stats');
    if (typeof wire !== 'object' || wire === null) {
      return null;
    }
    const { since } = wire;
    if (typeof since !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(since)) {
      return null;
    }
    const [people, succeeded, failed, contributed] = STATS_COUNTS.map((key) => wire[key]);
    if (!isCount(people) || !isCount(succeeded) || !isCount(failed) || !isCount(contributed)) {
      return null;
    }
    return { since, people, succeeded, failed, contributed };
  } catch {
    return null;
  }
}

/** The tus upload of a job (the server creates it together with the job). */
export function jobUploadUrl(id: string): string {
  return `/api/uploads/${encodeURIComponent(id)}`;
}

/** Where the browser downloads a finished job's file. */
export function jobFileUrl(id: string): string {
  return jobPath(id, '/file');
}

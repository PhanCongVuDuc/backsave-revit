import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  ApiError,
  createJob,
  deleteJob,
  finishOpenReport,
  getDropped,
  getInfo,
  getJob,
  getStats,
  sendReport,
  startOpenReport,
  uploadFile,
} from './index';
import { shouldRetryUpload } from './upload';

interface FakeUpload {
  file: unknown;
  options: {
    uploadUrl?: string;
    chunkSize?: number;
    retryDelays?: number[];
    removeFingerprintOnSuccess?: boolean;
    onProgress?: (sent: number, total: number) => void;
    onSuccess?: () => void;
    onError?: (error: Error) => void;
    onShouldRetry?: (error: Error, attempt: number, options: unknown) => boolean;
  };
  start: ReturnType<typeof vi.fn>;
  abort: ReturnType<typeof vi.fn>;
}

const tus = vi.hoisted(() => ({ uploads: [] as FakeUpload[] }));

vi.mock('tus-js-client', () => {
  class DetailedError extends Error {
    originalResponse: { getStatus: () => number; getBody: () => string } | null = null;
  }
  class Upload {
    start = vi.fn();
    abort = vi.fn(() => Promise.resolve());
    constructor(
      public file: unknown,
      public options: FakeUpload['options'],
    ) {
      tus.uploads.push(this);
    }
  }
  return { Upload, DetailedError };
});

function json(status: number, body: unknown, headers: Record<string, string> = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  });
}

async function apiErrorOf(promise: Promise<unknown>): Promise<ApiError> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof ApiError) {
      return error;
    }
    throw error;
  }
  throw new Error('expected an ApiError');
}

const fetchMock = vi.fn<typeof fetch>();

beforeEach(() => {
  tus.uploads = [];
  vi.stubGlobal('fetch', fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('api client', () => {
  it('api_error_carries_retry_after', async () => {
    fetchMock.mockResolvedValueOnce(
      json(
        429,
        { error: { code: 'hourly_limit', message: 'Too many jobs this hour.', retry_after_s: 1800 } },
        { 'Retry-After': '1800' },
      ),
    );
    const error = await apiErrorOf(createJob({ name: 'a.rvt', size: 10, kind: 'rvt', year: 2026, target: 2025 }));
    expect(error).toMatchObject({
      status: 429,
      code: 'hourly_limit',
      message: 'Too many jobs this hour.',
      retryAfterS: 1800,
    });
  });

  it('falls back to the Retry-After header when the body has no retry_after_s', async () => {
    fetchMock.mockResolvedValueOnce(
      json(503, { error: { code: 'queue_full', message: 'Busy.', retry_after_s: null } }, { 'Retry-After': '60' }),
    );
    const error = await apiErrorOf(createJob({ name: 'a.rvt', size: 10, kind: 'rvt', year: 2026, target: 2025 }));
    expect(error).toMatchObject({ status: 503, code: 'queue_full', retryAfterS: 60 });
  });

  it('gives a non-JSON error a code from its status', async () => {
    fetchMock.mockResolvedValueOnce(new Response('Bad Gateway', { status: 502 }));
    const error = await apiErrorOf(getInfo());
    expect(error).toMatchObject({ status: 502, code: 'http_502', retryAfterS: null });
  });

  it('turns a network failure into status 0', async () => {
    fetchMock.mockRejectedValueOnce(new TypeError('Failed to fetch'));
    const error = await apiErrorOf(getInfo());
    expect(error).toMatchObject({ status: 0, code: 'network' });
  });

  it('reads /api/info', async () => {
    fetchMock.mockResolvedValueOnce(
      json(200, { max_bytes: 1_073_741_824, output_ttl_s: 3600, pairs: [[2026, 2025]], version: '0.1.0' }),
    );
    await expect(getInfo()).resolves.toEqual({
      maxBytes: 1_073_741_824,
      outputTtlS: 3600,
      pairs: [[2026, 2025]],
      version: '0.1.0',
    });
    expect(fetchMock).toHaveBeenCalledWith('/api/info', expect.objectContaining({ method: 'GET' }));
  });

  it('creates a job with a JSON body', async () => {
    fetchMock.mockResolvedValueOnce(
      json(201, { job_id: 'a'.repeat(32), upload_url: `/api/uploads/${'a'.repeat(32)}` }),
    );
    const request = { name: 'tower.rvt', size: 1234, kind: 'rvt', year: 2026, target: 2024 } as const;
    await expect(createJob(request)).resolves.toEqual({
      jobId: 'a'.repeat(32),
      uploadUrl: `/api/uploads/${'a'.repeat(32)}`,
    });
    const [url, init] = fetchMock.mock.calls[0] ?? [];
    expect(url).toBe('/api/jobs');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(init?.body as string)).toEqual(request);
  });

  it('reads a job status', async () => {
    fetchMock.mockResolvedValueOnce(
      json(200, {
        state: 'done',
        position: null,
        elapsed_s: 12,
        estimate_s: 30,
        expires_at: '2026-10-25T12:00:00Z',
        error: null,
        result: { file_name: 'tower_2024.rvt', size: 999, dropped_count: 2 },
        report_code: null,
        name: 'tower.rvt',
        size: 1234,
        kind: 'rvt',
        year: 2026,
        target: 2024,
      }),
    );
    await expect(getJob('b'.repeat(32))).resolves.toEqual({
      state: 'done',
      position: null,
      elapsedS: 12,
      estimateS: 30,
      expiresAt: '2026-10-25T12:00:00Z',
      error: null,
      result: { fileName: 'tower_2024.rvt', size: 999, droppedCount: 2 },
      reportCode: null,
      name: 'tower.rvt',
      size: 1234,
      kind: 'rvt',
      year: 2026,
      target: 2024,
    });
    expect(fetchMock.mock.calls[0]?.[0]).toBe(`/api/jobs/${'b'.repeat(32)}`);
  });

  it('reads the dropped list, deletes and reports', async () => {
    const id = 'c'.repeat(32);
    fetchMock
      .mockResolvedValueOnce(json(200, [{ element_id: 7, class: 'Wall', reason: 'cascade', cascade_from: 3 }]))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(json(201, { report_code: 'R-ABC123' }));
    await expect(getDropped(id)).resolves.toEqual([{ elementId: 7, class: 'Wall', reason: 'cascade', cascadeFrom: 3 }]);
    await expect(deleteJob(id)).resolves.toBeUndefined();
    await expect(sendReport(id)).resolves.toBe('R-ABC123');
    expect(fetchMock.mock.calls.map(([url, init]) => `${String(init?.method)} ${url as string}`)).toEqual([
      `GET /api/jobs/${id}/dropped`,
      `DELETE /api/jobs/${id}`,
      `POST /api/jobs/${id}/report`,
    ]);
    expect(JSON.parse(fetchMock.mock.calls[2]?.[1]?.body as string)).toEqual({ consent: true });
  });

  it('opens and finishes a "does not open" report', async () => {
    const id = 'd'.repeat(32);
    fetchMock
      .mockResolvedValueOnce(json(201, { upload_url: `/api/jobs/${id}/open-report/upload` }))
      .mockResolvedValueOnce(json(200, { upload_url: `/api/jobs/${id}/open-report/upload` }))
      .mockResolvedValueOnce(json(201, { report_code: 'R-7K2M9Q' }));
    await expect(startOpenReport(id, 1234, 'Revit says corrupt')).resolves.toBe(`/api/jobs/${id}/open-report/upload`);
    await expect(startOpenReport(id, 1234)).resolves.toBe(`/api/jobs/${id}/open-report/upload`);
    await expect(finishOpenReport(id)).resolves.toBe('R-7K2M9Q');
    expect(fetchMock.mock.calls.map(([url, init]) => `${String(init?.method)} ${url as string}`)).toEqual([
      `POST /api/jobs/${id}/open-report`,
      `POST /api/jobs/${id}/open-report`,
      `POST /api/jobs/${id}/open-report/finish`,
    ]);
    expect(JSON.parse(fetchMock.mock.calls[0]?.[1]?.body as string)).toEqual({
      consent: true,
      size: 1234,
      note: 'Revit says corrupt',
    });
    expect(JSON.parse(fetchMock.mock.calls[1]?.[1]?.body as string)).toEqual({ consent: true, size: 1234 });
  });

  it('rejects a refused "does not open" report with the server code', async () => {
    fetchMock
      .mockResolvedValueOnce(
        json(409, { error: { code: 'size_mismatch', message: 'Not the original.', retry_after_s: null } }),
      )
      .mockResolvedValueOnce(
        json(409, { error: { code: 'already_reported', message: 'Already reported.', retry_after_s: null } }),
      );
    const started = await apiErrorOf(startOpenReport('e'.repeat(32), 1));
    expect([started.status, started.code]).toEqual([409, 'size_mismatch']);
    const finished = await apiErrorOf(finishOpenReport('e'.repeat(32)));
    expect([finished.status, finished.code]).toEqual([409, 'already_reported']);
  });
});

describe('getStats', () => {
  const totals = { since: '2026-10-29', people: 7, succeeded: 10, failed: 3, contributed: 1 };

  it('reads /api/stats', async () => {
    fetchMock.mockResolvedValueOnce(json(200, totals));
    await expect(getStats()).resolves.toEqual(totals);
    expect(fetchMock).toHaveBeenCalledWith('/api/stats', expect.objectContaining({ method: 'GET' }));
  });

  it('gives null when the statistics are unavailable', async () => {
    fetchMock.mockResolvedValueOnce(
      json(503, { error: { code: 'stats_unavailable', message: 'Unavailable.', retry_after_s: null } }),
    );
    await expect(getStats()).resolves.toBeNull();
  });

  it('gives null on a network failure', async () => {
    fetchMock.mockRejectedValueOnce(new TypeError('Failed to fetch'));
    await expect(getStats()).resolves.toBeNull();
  });

  it('gives null for a body that is not JSON', async () => {
    fetchMock.mockResolvedValueOnce(new Response('<html>', { status: 200 }));
    await expect(getStats()).resolves.toBeNull();
  });

  it.each([
    ['a missing field', { since: '2026-10-29', people: 7, succeeded: 10, failed: 3 }],
    ['a count that is not a number', { ...totals, succeeded: '10' }],
    ['a negative count', { ...totals, failed: -1 }],
    ['a date that is not YYYY-MM-DD', { ...totals, since: 'yesterday' }],
    ['null', null],
  ])('gives null for %s', async (_case, body) => {
    fetchMock.mockResolvedValueOnce(json(200, body));
    await expect(getStats()).resolves.toBeNull();
  });
});

describe('uploadFile', () => {
  const file = new File(['abc'], 'tower.rvt');

  function lastUpload(): FakeUpload {
    const upload = tus.uploads.at(-1);
    if (!upload) {
      throw new Error('no upload started');
    }
    return upload;
  }

  it('resumes the existing upload with the agreed options and reports progress', async () => {
    const onProgress = vi.fn();
    const done = uploadFile(file, '/api/uploads/x', { onProgress });
    const upload = lastUpload();
    expect(upload.start).toHaveBeenCalledOnce();
    expect(upload.options).toMatchObject({
      uploadUrl: '/api/uploads/x',
      chunkSize: 4 * 1024 * 1024,
      retryDelays: [0, 1000, 3000, 5000, 10000],
      removeFingerprintOnSuccess: true,
    });
    upload.options.onProgress?.(1, 3);
    expect(onProgress).toHaveBeenCalledWith(1, 3);
    upload.options.onSuccess?.();
    await expect(done).resolves.toBeUndefined();
  });

  it('rejects with job_gone when the server says 404 or 410', async () => {
    const { DetailedError } = await import('tus-js-client');
    for (const status of [404, 410]) {
      const done = uploadFile(file, '/api/uploads/x');
      const error = new DetailedError('gone');
      error.originalResponse = { getStatus: () => status, getBody: () => '', getHeader: () => undefined } as never;
      lastUpload().options.onError?.(error);
      await expect(apiErrorOf(done)).resolves.toMatchObject({ status, code: 'job_gone' });
    }
  });

  it('rejects with upload_failed once retries are exhausted', async () => {
    const done = uploadFile(file, '/api/uploads/x');
    lastUpload().options.onError?.(new Error('network down'));
    await expect(apiErrorOf(done)).resolves.toMatchObject({ status: 0, code: 'upload_failed' });
  });

  it('keeps the server error code of another refusal', async () => {
    const { DetailedError } = await import('tus-js-client');
    const done = uploadFile(file, '/api/uploads/x');
    const error = new DetailedError('closed');
    error.originalResponse = {
      getStatus: () => 409,
      getHeader: () => undefined,
      getBody: () => JSON.stringify({ error: { code: 'upload_closed', message: 'closed', retry_after_s: null } }),
    } as never;
    lastUpload().options.onError?.(error);
    await expect(apiErrorOf(done)).resolves.toMatchObject({ status: 409, code: 'upload_closed' });
  });

  it('aborts on the signal', async () => {
    const controller = new AbortController();
    const done = uploadFile(file, '/api/uploads/x', { signal: controller.signal });
    controller.abort();
    expect(lastUpload().abort).toHaveBeenCalledOnce();
    await expect(apiErrorOf(done)).resolves.toMatchObject({ code: 'aborted' });
  });

  it('does not start when the signal is already aborted', async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(apiErrorOf(uploadFile(file, '/api/uploads/x', { signal: controller.signal }))).resolves.toMatchObject({
      code: 'aborted',
    });
    expect(tus.uploads).toHaveLength(0);
  });
});

describe('shouldRetryUpload', () => {
  async function detailed(status: number, code?: string): Promise<Error> {
    const { DetailedError } = await import('tus-js-client');
    const error = new DetailedError('http');
    const body = code ? JSON.stringify({ error: { code, message: code, retry_after_s: null } }) : '';
    error.originalResponse = { getStatus: () => status, getBody: () => body, getHeader: () => undefined } as never;
    return error;
  }

  it('does not retry answers that cannot change: full disk, gone, closed', async () => {
    for (const status of [404, 410, 507]) {
      expect(shouldRetryUpload(await detailed(status))).toBe(false);
    }
    expect(shouldRetryUpload(await detailed(409, 'upload_closed'))).toBe(false);
  });

  it('retries network failures, server errors and offset conflicts', async () => {
    expect(shouldRetryUpload(new Error('offline'))).toBe(true);
    expect(shouldRetryUpload(await detailed(502))).toBe(true);
    expect(shouldRetryUpload(await detailed(409, 'offset_mismatch'))).toBe(true);
    expect(shouldRetryUpload(await detailed(409, 'upload_busy'))).toBe(true);
    expect(shouldRetryUpload(await detailed(423))).toBe(true);
  });

  it('does not retry other client errors', async () => {
    expect(shouldRetryUpload(await detailed(413, 'too_large'))).toBe(false);
    expect(shouldRetryUpload(await detailed(415))).toBe(false);
  });

  it('is the upload retry policy', () => {
    void uploadFile(new File(['a'], 'a.rvt'), '/api/uploads/x');
    expect(tus.uploads.at(-1)?.options.onShouldRetry).toBeDefined();
  });
});

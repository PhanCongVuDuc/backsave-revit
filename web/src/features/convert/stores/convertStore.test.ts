import { createPinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import * as api from '@/api';
import { ApiError, type JobStatus, type ServerInfo } from '@/api';
import { basicFileInfo, fileOf, makeOle } from '@/shared/revit-file/makeOle.testutil';

import { JOB_STORAGE_KEY, POLL_INTERVAL_MS, useConvertStore } from './convertStore';

vi.mock('@/api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/api')>();
  return {
    ...actual,
    getInfo: vi.fn(),
    createJob: vi.fn(),
    getJob: vi.fn(),
    getDropped: vi.fn(),
    deleteJob: vi.fn(),
    sendReport: vi.fn(),
    uploadFile: vi.fn(),
  };
});

const mocked = vi.mocked(api);

const JOB = '0123456789abcdef0123456789abcdef';

function serverInfo(pairs: [number, number][]): ServerInfo {
  return { maxBytes: 1_073_741_824, outputTtlS: 3600, pairs, version: '0.1.0' };
}

function status(overrides: Partial<JobStatus>): JobStatus {
  return {
    state: 'queued',
    position: null,
    elapsedS: 0,
    estimateS: 0,
    expiresAt: null,
    error: null,
    result: null,
    reportCode: null,
    ...overrides,
  };
}

function revitFile(year: number, name = 'tower.rvt'): File {
  return fileOf(makeOle([['BasicFileInfo', basicFileInfo(year)]]), name);
}

/** Lets pending promise callbacks run; timers stay frozen. */
async function settle() {
  for (let round = 0; round < 20; round += 1) {
    await Promise.resolve();
  }
}

async function readyStore(year = 2026, pairs: [number, number][] = [[2026, 2025]]) {
  mocked.getInfo.mockResolvedValue(serverInfo(pairs));
  const store = useConvertStore();
  await store.chooseFile(revitFile(year));
  return store;
}

const ALL_PAIRS: [number, number][] = [
  [2026, 2025],
  [2026, 2024],
  [2026, 2023],
  [2025, 2024],
];

beforeEach(() => {
  setActivePinia(createPinia());
  localStorage.clear();
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  mocked.createJob.mockResolvedValue({ jobId: JOB, uploadUrl: `/api/uploads/${JOB}` });
  mocked.uploadFile.mockResolvedValue(undefined);
  mocked.deleteJob.mockResolvedValue(undefined);
  mocked.getDropped.mockResolvedValue([]);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('choosing a file', () => {
  it('default_target_is_year_below', async () => {
    const store = await readyStore(2026, ALL_PAIRS);
    expect(store.phase).toBe('ready');
    expect(store.info).toEqual({ kind: 'rvt', year: 2026 });
    expect(store.targets).toEqual([2025, 2024, 2023]);
    expect(store.target).toBe(2025);
  });

  it('default_target_falls_back_to_highest_allowed', async () => {
    const store = await readyStore(2026, [
      [2026, 2023],
      [2026, 2024],
    ]);
    expect(store.targets).toEqual([2024, 2023]);
    expect(store.target).toBe(2024);
  });

  it('unsupported_year_sets_error', async () => {
    const store = await readyStore(2022, ALL_PAIRS);
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('unsupported_year');
    expect(store.target).toBeNull();
  });

  it('shows the reader error for a file that is not a Revit model', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    const store = useConvertStore();
    await store.chooseFile(fileOf(new Uint8Array(10), 'plan.dwg'));
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('bad_extension');
  });

  it('lets the user pick only an allowed target', async () => {
    const store = await readyStore(2026, ALL_PAIRS);
    store.setTarget(2023);
    expect(store.target).toBe(2023);
    store.setTarget(2019);
    expect(store.target).toBe(2023);
  });
});

describe('submitting', () => {
  it('submit_twice_creates_one_job', async () => {
    const store = await readyStore();
    mocked.getJob.mockResolvedValue(status({ state: 'queued', position: 2 }));
    await Promise.all([store.submit(), store.submit()]);
    expect(mocked.createJob).toHaveBeenCalledExactlyOnceWith({
      name: 'tower.rvt',
      size: store.file?.size,
      kind: 'rvt',
      year: 2026,
      target: 2025,
    });
    expect(mocked.uploadFile).toHaveBeenCalledOnce();
  });

  it('ignores submit outside the ready phase', async () => {
    const store = useConvertStore();
    await store.submit();
    expect(mocked.createJob).not.toHaveBeenCalled();
  });

  it('uploads, remembers the job, then polls every 2 s until done', async () => {
    const store = await readyStore();
    let finishUpload: () => void = () => undefined;
    mocked.uploadFile.mockImplementation((_file, _url, options) => {
      options?.onProgress?.(50, 100);
      return new Promise((resolve) => {
        finishUpload = () => {
          resolve();
        };
      });
    });
    mocked.getJob
      .mockResolvedValueOnce(status({ state: 'queued', position: 1, estimateS: 40 }))
      .mockResolvedValueOnce(status({ state: 'running', elapsedS: 2, estimateS: 40 }))
      .mockResolvedValueOnce(
        status({
          state: 'done',
          elapsedS: 5,
          expiresAt: '2026-10-25T13:00:00Z',
          result: { fileName: 'tower.rvt', size: 9, droppedCount: 1 },
        }),
      );
    mocked.getDropped.mockResolvedValue([{ elementId: 4, class: 'Wall', reason: 'no_rule', cascadeFrom: null }]);

    const submitted = store.submit();
    await settle();
    expect(store.phase).toBe('uploading');
    expect(store.progress).toBe(0.5);
    expect(store.jobId).toBe(JOB);
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBe(JOB);
    expect(mocked.uploadFile).toHaveBeenCalledWith(store.file, `/api/uploads/${JOB}`, expect.anything());

    finishUpload();
    await submitted;
    await settle();
    expect(store.phase).toBe('queued');
    expect(store.position).toBe(1);

    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    await settle();
    expect(store.phase).toBe('running');
    expect(store.elapsedS).toBe(2);

    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    await settle();
    expect(store.phase).toBe('done');
    expect(store.expiresAt).toBe('2026-10-25T13:00:00Z');
    expect(store.dropped).toEqual([{ elementId: 4, class: 'Wall', reason: 'no_rule', cascadeFrom: null }]);

    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS * 5);
    expect(mocked.getJob).toHaveBeenCalledTimes(3);
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBe(JOB);
  });

  it('shows the server refusal with its retry time', async () => {
    const store = await readyStore();
    mocked.createJob.mockRejectedValue(new ApiError(429, 'hourly_limit', 'Too many.', 1800));
    await store.submit();
    expect(store.phase).toBe('error');
    expect(store.error).toEqual({ code: 'hourly_limit', message: 'Too many.', retryAfterS: 1800 });
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBeNull();
  });

  it('keeps the job after the upload gives up, so it can be resumed later', async () => {
    const store = await readyStore();
    mocked.uploadFile.mockRejectedValue(new ApiError(0, 'upload_failed', 'offline'));
    await store.submit();
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('upload_failed');
    expect(store.jobId).toBe(JOB);
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBe(JOB);

    await store.chooseFile(revitFile(2026, 'other.rvt'));
    expect(mocked.deleteJob).toHaveBeenCalledWith(JOB);
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBeNull();
    expect(store.phase).toBe('ready');
  });

  it('warns before leaving the page only while uploading', async () => {
    const store = await readyStore();
    let finishUpload: () => void = () => undefined;
    mocked.uploadFile.mockReturnValue(
      new Promise((resolve) => {
        finishUpload = () => {
          resolve();
        };
      }),
    );
    mocked.getJob.mockResolvedValue(status({ state: 'queued' }));
    const leave = () => {
      const event = new Event('beforeunload', { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    };
    expect(leave()).toBe(false);
    const submitted = store.submit();
    await settle();
    expect(leave()).toBe(true);
    finishUpload();
    await submitted;
    expect(leave()).toBe(false);
  });
});

describe('job outcomes', () => {
  it('job_gone_shows_restart_notice', async () => {
    const store = await readyStore();
    mocked.getJob.mockRejectedValue(new ApiError(404, 'not_found', 'no such job'));
    await store.submit();
    await settle();
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('server_restarted');
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBeNull();
  });

  it('shows expired on 410 and forgets the job', async () => {
    localStorage.setItem(JOB_STORAGE_KEY, JOB);
    mocked.getJob.mockRejectedValue(new ApiError(410, 'expired', 'gone'));
    const store = useConvertStore();
    await expect(store.resume()).resolves.toBe(true);
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('expired');
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBeNull();
  });

  it('keeps polling through a network blip', async () => {
    const store = await readyStore();
    mocked.getJob
      .mockResolvedValueOnce(status({ state: 'running' }))
      .mockRejectedValueOnce(new ApiError(0, 'network', 'offline'))
      .mockResolvedValueOnce(status({ state: 'running', elapsedS: 4 }));
    await store.submit();
    await settle();
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    expect(store.phase).toBe('running');
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    await settle();
    expect(store.elapsedS).toBe(4);
  });

  it('shows a failed job with its error kind and sends a report on consent', async () => {
    const store = await readyStore();
    mocked.getJob.mockResolvedValue(status({ state: 'failed', error: { kind: 'corrupt', message: 'bad file' } }));
    mocked.sendReport.mockResolvedValue('R-ABC123');
    await store.submit();
    await settle();
    expect(store.phase).toBe('failed');
    expect(store.error).toEqual({ code: 'corrupt', message: 'bad file', retryAfterS: null });
    await expect(store.consentReport()).resolves.toBe(true);
    expect(store.reportCode).toBe('R-ABC123');
    await expect(store.consentReport()).resolves.toBe(false);
    expect(mocked.sendReport).toHaveBeenCalledExactlyOnceWith(JOB);
  });

  it('consent_report_error_is_exposed', async () => {
    mocked.getJob.mockResolvedValue(status({ state: 'failed', error: { kind: 'crashed', message: 'boom' } }));
    const store = useConvertStore();
    await store.resume(JOB);
    mocked.sendReport.mockRejectedValueOnce(new ApiError(410, 'expired', 'gone'));
    await expect(store.consentReport()).resolves.toBe(false);
    expect(store.reportError).toBe('expired');
    mocked.sendReport.mockRejectedValueOnce(new ApiError(409, 'already_reported', 'sent'));
    await expect(store.consentReport()).resolves.toBe(false);
    expect(store.reportError).toBe('already_reported');
    mocked.sendReport.mockResolvedValueOnce('R-XYZ789');
    await expect(store.consentReport()).resolves.toBe(true);
    expect(store.reportError).toBeNull();
    expect(store.reportCode).toBe('R-XYZ789');
    expect(store.phase).toBe('failed');
  });

  it('dropped_load_error_is_exposed', async () => {
    mocked.getJob.mockResolvedValue(status({ state: 'done', result: { fileName: 'a.rvt', size: 1, droppedCount: 3 } }));
    mocked.getDropped.mockRejectedValue(new ApiError(0, 'network', 'offline'));
    const store = useConvertStore();
    await store.resume(JOB);
    expect(store.phase).toBe('done');
    expect(store.dropped).toBeNull();
    expect(store.droppedError).toBe('network');
  });
});

describe('retained jobs', () => {
  it('retry_upload_resumes_same_job', async () => {
    const store = await readyStore();
    mocked.uploadFile.mockRejectedValueOnce(new ApiError(0, 'upload_failed', 'offline'));
    mocked.getJob.mockResolvedValue(status({ state: 'queued', position: 1 }));
    await store.submit();
    expect(store.error?.code).toBe('upload_failed');
    await store.retryUpload();
    expect(mocked.createJob).toHaveBeenCalledOnce();
    expect(mocked.uploadFile).toHaveBeenCalledTimes(2);
    expect(mocked.uploadFile).toHaveBeenLastCalledWith(store.file, `/api/uploads/${JOB}`, expect.anything());
    expect(store.phase).toBe('queued');
    expect(store.error).toBeNull();
    expect(store.jobId).toBe(JOB);
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBe(JOB);
  });

  it('ignores retryUpload unless the upload gave up', async () => {
    const store = await readyStore();
    await store.retryUpload();
    expect(mocked.uploadFile).not.toHaveBeenCalled();
    expect(store.phase).toBe('ready');
  });

  it('choose_file_keeps_running_job', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    mocked.getJob.mockRejectedValue(new ApiError(400, 'bad_request', 'odd answer'));
    const store = useConvertStore();
    await store.resume(JOB);
    expect(store.phase).toBe('error');
    await store.chooseFile(revitFile(2026));
    expect(store.phase).toBe('ready');
    expect(mocked.deleteJob).not.toHaveBeenCalled();
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBe(JOB);
  });
});

describe('resume, cancel and delete', () => {
  it('resume_restores_done_job', async () => {
    mocked.getJob.mockResolvedValue(
      status({
        state: 'done',
        elapsedS: 30,
        estimateS: 25,
        expiresAt: '2026-10-25T14:00:00Z',
        result: { fileName: 'tower.rvt', size: 10, droppedCount: 0 },
      }),
    );
    const store = useConvertStore();
    await expect(store.resume(JOB)).resolves.toBe(true);
    expect(store.phase).toBe('done');
    expect(store.jobId).toBe(JOB);
    expect(store.elapsedS).toBe(30);
    expect(store.expiresAt).toBe('2026-10-25T14:00:00Z');
    expect(store.dropped).toEqual([]);
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBe(JOB);
  });

  it('resumes the remembered job when no id is given, and ignores a malformed id', async () => {
    const store = useConvertStore();
    await expect(store.resume()).resolves.toBe(false);
    await expect(store.resume('not-a-job')).resolves.toBe(false);
    expect(mocked.getJob).not.toHaveBeenCalled();
    localStorage.setItem(JOB_STORAGE_KEY, JOB);
    mocked.getJob.mockResolvedValue(status({ state: 'running' }));
    await store.resume();
    expect(store.phase).toBe('running');
    expect(mocked.getJob).toHaveBeenCalledWith(JOB);
  });

  it('resume_waiting_upload_does_not_delete_job', async () => {
    mocked.getJob.mockResolvedValue(status({ state: 'waiting_upload' }));
    const store = useConvertStore();
    await store.resume(JOB);
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('upload_interrupted');
    expect(mocked.deleteJob).not.toHaveBeenCalled();
    expect(store.jobId).toBeNull();
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBeNull();
  });

  it('cancel aborts the upload, deletes the job and returns to pick', async () => {
    const store = await readyStore();
    let signal: AbortSignal | undefined;
    mocked.uploadFile.mockImplementation((_file, _url, options) => {
      signal = options?.signal;
      return new Promise((_resolve, reject) => {
        options?.signal?.addEventListener('abort', () => {
          reject(new ApiError(0, 'aborted', 'cancelled'));
        });
      });
    });
    const submitted = store.submit();
    await settle();
    await store.cancel();
    await submitted;
    expect(signal?.aborted).toBe(true);
    expect(mocked.deleteJob).toHaveBeenCalledWith(JOB);
    expect(store.phase).toBe('pick');
    expect(store.file).toBeNull();
    expect(store.error).toBeNull();
    expect(store.jobId).toBeNull();
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBeNull();
  });

  it('cancel after the upload gave up deletes the waiting job', async () => {
    const store = await readyStore();
    mocked.uploadFile.mockRejectedValue(new ApiError(0, 'upload_failed', 'offline'));
    await store.submit();
    await store.cancel();
    expect(mocked.deleteJob).toHaveBeenCalledWith(JOB);
    expect(store.phase).toBe('pick');
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBeNull();
  });

  it('cancel stops polling', async () => {
    const store = await readyStore();
    mocked.getJob.mockResolvedValue(status({ state: 'queued' }));
    await store.submit();
    await settle();
    await store.cancel();
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS * 3);
    expect(mocked.getJob).toHaveBeenCalledOnce();
    expect(store.phase).toBe('pick');
  });

  it('deleteNow removes a done job and forgets it', async () => {
    mocked.getJob.mockResolvedValue(status({ state: 'done', result: { fileName: 'a.rvt', size: 1, droppedCount: 0 } }));
    const store = useConvertStore();
    await store.resume(JOB);
    await store.deleteNow();
    expect(mocked.deleteJob).toHaveBeenCalledWith(JOB);
    expect(store.phase).toBe('pick');
    expect(store.jobId).toBeNull();
    expect(localStorage.getItem(JOB_STORAGE_KEY)).toBeNull();
  });

  it('exposes the result and the download URL of a done job', async () => {
    mocked.getJob.mockResolvedValue(
      status({ state: 'done', result: { fileName: 'a_R2025.rvt', size: 42, droppedCount: 0 } }),
    );
    const store = useConvertStore();
    await store.resume(JOB);
    expect(store.result).toEqual({ fileName: 'a_R2025.rvt', size: 42, droppedCount: 0 });
    expect(store.downloadUrl).toBe(api.jobFileUrl(JOB));
    await store.deleteNow();
    expect(store.result).toBeNull();
    expect(store.downloadUrl).toBeNull();
  });

  it('$reset returns to the initial state', async () => {
    const store = await readyStore();
    store.$reset();
    expect(store.phase).toBe('pick');
    expect(store.file).toBeNull();
    expect(store.info).toBeNull();
    expect(store.targets).toEqual([]);
  });
});

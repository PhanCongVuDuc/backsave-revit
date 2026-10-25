import { createPinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import * as api from '@/api';
import { ApiError, type JobStatus, type ServerInfo } from '@/api';
import { basicFileInfo, fileOf, makeOle } from '@/shared/revit-file/makeOle.testutil';

import { POLL_INTERVAL_MS, useConvertStore } from './convertStore';
import { UPLOAD_ENTRY_MAX_AGE_MS, UPLOADS_STORAGE_KEY } from './uploadRegistry';

// A shared stand-in for the browser's lock manager: every store in a test (one per tab) sees the same locks.
const lock = vi.hoisted(() => ({
  taken: new Set<string>(),
  /** Grants refused even though the hint said the lock was free (another tab took it in between). */
  refuse: new Set<string>(),
  held: [] as string[],
  released: [] as string[],
}));

vi.mock('@/shared/locks/uploadLock', () => ({
  isUploadLockHeld: (jobId: string) => Promise.resolve(lock.taken.has(jobId)),
  acquireUploadLock: (jobId: string) => {
    if (lock.taken.has(jobId) || lock.refuse.has(jobId)) {
      return Promise.resolve(null);
    }
    lock.taken.add(jobId);
    lock.held.push(jobId);
    return Promise.resolve(() => {
      lock.taken.delete(jobId);
      lock.released.push(jobId);
    });
  },
}));

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
    name: 'tower.rvt',
    size: 1000,
    kind: 'rvt',
    year: 2026,
    target: 2025,
    ...overrides,
  };
}

const LAST_MODIFIED = 1_790_000_000_000;

function revitFile(year: number, name = 'tower.rvt', lastModified = LAST_MODIFIED): File {
  return new File([fileOf(makeOle([['BasicFileInfo', basicFileInfo(year)]]), name)], name, { lastModified });
}

/** Job ids in this browser's list of unfinished uploads. */
function storedJobs(): string[] {
  const raw = localStorage.getItem(UPLOADS_STORAGE_KEY);
  return raw ? (JSON.parse(raw) as { jobId: string }[]).map((entry) => entry.jobId) : [];
}

/** Puts an unfinished upload of `file` for `jobId` in the list, as an earlier visit would have. */
function storeUpload(file: File, jobId: string, createdAt = Date.now()) {
  const entries = JSON.parse(localStorage.getItem(UPLOADS_STORAGE_KEY) ?? '[]') as unknown[];
  entries.push({
    name: file.name,
    size: file.size,
    lastModified: file.lastModified,
    jobId,
    uploadUrl: `/api/uploads/${jobId}`,
    createdAt,
  });
  localStorage.setItem(UPLOADS_STORAGE_KEY, JSON.stringify(entries));
}

const OTHER = 'ffffffffffffffffffffffffffffffff';

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
  lock.taken.clear();
  lock.refuse.clear();
  lock.held = [];
  lock.released = [];
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
          expiresAt: '2099-10-25T13:00:00Z',
          result: { fileName: 'tower.rvt', size: 9, droppedCount: 1 },
        }),
      );
    mocked.getDropped.mockResolvedValue([{ elementId: 4, class: 'Wall', reason: 'no_rule', cascadeFrom: null }]);

    const submitted = store.submit();
    await settle();
    expect(store.phase).toBe('uploading');
    expect(store.progress).toBe(0.5);
    expect(store.jobId).toBe(JOB);
    expect(storedJobs()).toEqual([JOB]);
    expect(lock.held).toEqual([JOB]);
    expect(mocked.uploadFile).toHaveBeenCalledWith(store.file, `/api/uploads/${JOB}`, expect.anything());

    finishUpload();
    await submitted;
    await settle();
    expect(store.phase).toBe('queued');
    expect(store.position).toBe(1);
    // The upload is complete: nothing left to continue, and other tabs no longer see it in progress.
    expect(storedJobs()).toEqual([]);
    expect(lock.released).toEqual([JOB]);

    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    await settle();
    expect(store.phase).toBe('running');
    expect(store.elapsedS).toBe(2);

    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    await settle();
    expect(store.phase).toBe('done');
    expect(store.expiresAt).toBe('2099-10-25T13:00:00Z');
    expect(store.dropped).toEqual([{ elementId: 4, class: 'Wall', reason: 'no_rule', cascadeFrom: null }]);

    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS * 5);
    expect(mocked.getJob).toHaveBeenCalledTimes(3);
  });

  it('shows the server refusal with its retry time', async () => {
    const store = await readyStore();
    mocked.createJob.mockRejectedValue(new ApiError(429, 'hourly_limit', 'Too many.', 1800));
    await store.submit();
    expect(store.phase).toBe('error');
    expect(store.error).toEqual({ code: 'hourly_limit', message: 'Too many.', retryAfterS: 1800 });
    expect(storedJobs()).toEqual([]);
  });

  it('keeps the job after the upload gives up, so it can be resumed later', async () => {
    const store = await readyStore();
    mocked.uploadFile.mockRejectedValue(new ApiError(0, 'upload_failed', 'offline'));
    mocked.getJob.mockResolvedValue(status({ state: 'waiting_upload' }));
    await store.submit();
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('upload_failed');
    expect(store.jobId).toBe(JOB);
    expect(storedJobs()).toEqual([JOB]);

    await store.chooseFile(revitFile(2026, 'other.rvt'));
    expect(mocked.deleteJob).toHaveBeenCalledWith(JOB);
    expect(storedJobs()).toEqual([]);
    expect(store.phase).toBe('ready');
  });

  it('shows disk_full when the server could not store the upload', async () => {
    const store = await readyStore();
    mocked.uploadFile.mockRejectedValue(new ApiError(507, 'storage_error', 'could not store'));
    mocked.getJob.mockResolvedValue(
      status({ state: 'failed', error: { kind: 'disk_full', message: 'The server could not store the file.' } }),
    );
    await store.submit();
    await settle();
    expect(mocked.getJob).toHaveBeenCalledExactlyOnceWith(JOB);
    expect(store.phase).toBe('failed');
    expect(store.error?.code).toBe('disk_full');
  });

  it('follows the job when the upload was closed because it already finished', async () => {
    const store = await readyStore();
    mocked.uploadFile.mockRejectedValue(new ApiError(409, 'upload_closed', 'closed'));
    mocked.getJob.mockResolvedValue(status({ state: 'queued', position: 1 }));
    await store.submit();
    await settle();
    expect(store.phase).toBe('queued');
    expect(store.position).toBe(1);
  });

  it('keeps the upload error when the job is still waiting for its data', async () => {
    const store = await readyStore();
    mocked.uploadFile.mockRejectedValue(new ApiError(0, 'upload_failed', 'offline'));
    mocked.getJob.mockResolvedValue(status({ state: 'waiting_upload' }));
    await store.submit();
    await settle();
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('upload_failed');
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
    expect(storedJobs()).toEqual([]);
  });

  it('shows expired on 410 and forgets the job', async () => {
    storeUpload(revitFile(2026), JOB);
    mocked.getJob.mockRejectedValue(new ApiError(410, 'expired', 'gone'));
    const store = useConvertStore();
    await expect(store.resume(JOB)).resolves.toBe(true);
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('expired');
    expect(storedJobs()).toEqual([]);
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
    mocked.getJob
      .mockResolvedValueOnce(status({ state: 'waiting_upload' }))
      .mockResolvedValue(status({ state: 'queued', position: 1 }));
    await store.submit();
    expect(store.error?.code).toBe('upload_failed');
    await store.retryUpload();
    expect(mocked.createJob).toHaveBeenCalledOnce();
    expect(mocked.uploadFile).toHaveBeenCalledTimes(2);
    expect(mocked.uploadFile).toHaveBeenLastCalledWith(store.file, `/api/uploads/${JOB}`, expect.anything());
    expect(store.phase).toBe('queued');
    expect(store.error).toBeNull();
    expect(store.jobId).toBe(JOB);
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
  });

  it('opening the start page follows nothing, and a malformed id is ignored', async () => {
    storeUpload(revitFile(2026), JOB);
    const store = useConvertStore();
    await expect(store.resume()).resolves.toBe(false);
    await expect(store.resume('not-a-job')).resolves.toBe(false);
    expect(mocked.getJob).not.toHaveBeenCalled();
    expect(store.phase).toBe('pick');
  });

  it('a link to a job still waiting for its upload shows the normal card and touches nothing', async () => {
    storeUpload(revitFile(2026), JOB);
    mocked.getJob.mockResolvedValue(status({ state: 'waiting_upload' }));
    const store = useConvertStore();
    await store.resume(JOB);
    expect(store.phase).toBe('pick');
    expect(store.jobId).toBeNull();
    await store.cancel();
    expect(mocked.deleteJob).not.toHaveBeenCalled();
    expect(storedJobs()).toEqual([JOB]);
  });

  it('continues the earlier upload only when its button is pressed', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    storeUpload(revitFile(2026), JOB);
    mocked.getJob
      .mockResolvedValueOnce(status({ state: 'waiting_upload', year: 2026, target: 2024 }))
      .mockResolvedValue(status({ state: 'queued', position: 0 }));
    const store = useConvertStore();
    const same = revitFile(2026);
    await store.chooseFile(same);
    // Choosing the file sends nothing: the card waits for the button.
    expect(store.phase).toBe('ready');
    expect(store.continuing).toBe(true);
    expect(store.target).toBe(2024);
    expect(store.targets).toEqual([2024]);
    expect(store.jobId).toBeNull();
    expect(mocked.uploadFile).not.toHaveBeenCalled();
    expect(lock.held).toEqual([]);

    await store.submit();
    await settle();
    expect(mocked.createJob).not.toHaveBeenCalled();
    expect(mocked.uploadFile).toHaveBeenCalledWith(same, `/api/uploads/${JOB}`, expect.anything());
    expect(store.jobId).toBe(JOB);
    expect(lock.held).toEqual([JOB]);
    expect(store.phase).toBe('queued');
    expect(lock.released).toEqual([JOB]);
    expect(storedJobs()).toEqual([]);
  });

  it('lock_not_granted_means_other_tab', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    storeUpload(revitFile(2026), JOB);
    mocked.getJob.mockResolvedValue(status({ state: 'waiting_upload' }));
    const store = useConvertStore();
    await store.chooseFile(revitFile(2026));
    expect(store.phase).toBe('ready');
    // Another tab takes the lock between the hint and the press.
    lock.refuse.add(JOB);
    await store.submit();
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('upload_in_other_tab');
    expect(mocked.uploadFile).not.toHaveBeenCalled();
    expect(store.jobId).toBeNull();
    await store.cancel();
    expect(mocked.deleteJob).not.toHaveBeenCalled();
  });

  it('failed_upload_keeps_lock_so_other_tab_cannot_take_over', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    const file = revitFile(2026);
    const tabA = useConvertStore();
    await tabA.chooseFile(file);
    mocked.uploadFile.mockRejectedValueOnce(new ApiError(0, 'upload_failed', 'offline'));
    mocked.getJob.mockResolvedValue(status({ state: 'waiting_upload' }));
    await tabA.submit();
    expect(tabA.error?.code).toBe('upload_failed');
    // Tab A still holds the job's lock while it offers Resume.
    expect(lock.taken.has(JOB)).toBe(true);
    expect(lock.released).toEqual([]);

    // Tab B, same browser, chooses the same file: it may not continue A's job.
    setActivePinia(createPinia());
    const tabB = useConvertStore();
    await tabB.chooseFile(file);
    expect(tabB.phase).toBe('error');
    expect(tabB.error?.code).toBe('upload_in_other_tab');
    expect(mocked.uploadFile).toHaveBeenCalledOnce();

    // Tab A can still resume its own upload.
    mocked.getJob.mockResolvedValue(status({ state: 'queued' }));
    await tabA.retryUpload();
    expect(mocked.uploadFile).toHaveBeenCalledTimes(2);
    expect(tabA.phase).toBe('queued');
    expect(lock.taken.has(JOB)).toBe(false);
  });

  it('cancel_never_deletes_job_continued_elsewhere', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    storeUpload(revitFile(2026), JOB);
    mocked.getJob.mockResolvedValueOnce(status({ state: 'waiting_upload' }));
    const store = useConvertStore();
    await store.chooseFile(revitFile(2026));
    // Another tab finished this upload first: the server closes it and the job is already queued.
    mocked.uploadFile.mockRejectedValueOnce(new ApiError(409, 'upload_closed', 'closed'));
    mocked.getJob.mockResolvedValue(status({ state: 'queued' }));
    await store.submit();
    await settle();
    expect(store.phase).toBe('queued');
    expect(store.ownsJob).toBe(false);
    await store.cancel();
    expect(mocked.deleteJob).not.toHaveBeenCalled();
  });

  it('a new file after a failed upload deletes the old job only with its lock', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    const store = await readyStore();
    mocked.uploadFile.mockRejectedValueOnce(new ApiError(0, 'upload_failed', 'offline'));
    mocked.getJob.mockResolvedValue(status({ state: 'waiting_upload' }));
    await store.submit();
    expect(lock.taken.has(JOB)).toBe(true);
    await store.chooseFile(revitFile(2026, 'other.rvt'));
    expect(mocked.deleteJob).toHaveBeenCalledWith(JOB);
    expect(lock.taken.has(JOB)).toBe(false);
  });

  it('prunes old entries as soon as the store starts', () => {
    storeUpload(revitFile(2026), JOB, Date.now() - UPLOAD_ENTRY_MAX_AGE_MS - 1);
    useConvertStore();
    expect(localStorage.getItem(UPLOADS_STORAGE_KEY)).toBeNull();
  });

  it('starts a new job for a different file, or the same name with another date', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    storeUpload(revitFile(2026), JOB);
    const store = useConvertStore();
    await store.chooseFile(revitFile(2026, 'other.rvt'));
    expect(store.phase).toBe('ready');
    await store.chooseFile(revitFile(2026, 'tower.rvt', LAST_MODIFIED + 1));
    expect(store.phase).toBe('ready');
    expect(mocked.getJob).not.toHaveBeenCalled();
    expect(storedJobs()).toEqual([JOB]);
  });

  it('says so when another tab is uploading the same file, and deletes nothing', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    storeUpload(revitFile(2026), JOB);
    lock.taken.add(JOB);
    mocked.getJob.mockResolvedValue(status({ state: 'waiting_upload' }));
    const store = useConvertStore();
    await store.chooseFile(revitFile(2026));
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('upload_in_other_tab');
    expect(mocked.uploadFile).not.toHaveBeenCalled();
    expect(store.jobId).toBeNull();
    await store.cancel();
    expect(mocked.deleteJob).not.toHaveBeenCalled();
    expect(storedJobs()).toEqual([JOB]);
  });

  it('drops the entry and starts afresh when the earlier job is no longer waiting or is gone', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    storeUpload(revitFile(2026), JOB);
    mocked.getJob.mockResolvedValueOnce(status({ state: 'queued' }));
    const store = useConvertStore();
    await store.chooseFile(revitFile(2026));
    expect(store.phase).toBe('ready');
    expect(storedJobs()).toEqual([]);

    storeUpload(revitFile(2026), OTHER);
    mocked.getJob.mockRejectedValueOnce(new ApiError(404, 'not_found', 'gone'));
    await store.chooseFile(revitFile(2026));
    expect(store.phase).toBe('ready');
    expect(storedJobs()).toEqual([]);
    expect(mocked.deleteJob).not.toHaveBeenCalled();
  });

  it('keeps the entry when the server cannot be asked', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    storeUpload(revitFile(2026), JOB);
    mocked.getJob.mockRejectedValueOnce(new ApiError(0, 'network', 'offline'));
    const store = useConvertStore();
    await store.chooseFile(revitFile(2026));
    expect(store.phase).toBe('ready');
    expect(storedJobs()).toEqual([JOB]);
  });

  it('prunes entries older than 6 hours', async () => {
    mocked.getInfo.mockResolvedValue(serverInfo(ALL_PAIRS));
    storeUpload(revitFile(2026), JOB, Date.now() - UPLOAD_ENTRY_MAX_AGE_MS - 1);
    storeUpload(revitFile(2026, 'b.rvt'), OTHER);
    const store = useConvertStore();
    await store.chooseFile(revitFile(2026));
    expect(mocked.getJob).not.toHaveBeenCalled();
    expect(store.phase).toBe('ready');
    expect(storedJobs()).toEqual([OTHER]);
  });

  it('never deletes a job it only follows from a link', async () => {
    mocked.getJob.mockResolvedValue(status({ state: 'running' }));
    const store = useConvertStore();
    await store.resume(OTHER);
    expect(store.ownsJob).toBe(false);
    await store.cancel();
    expect(mocked.deleteJob).not.toHaveBeenCalled();
    expect(store.phase).toBe('pick');
  });

  it('a reopened running job shows its own versions', async () => {
    mocked.getJob.mockResolvedValue(status({ state: 'running', kind: 'rfa', year: 2026, target: 2024 }));
    const store = useConvertStore();
    await store.resume(JOB);
    expect(store.phase).toBe('running');
    expect(store.info).toEqual({ kind: 'rfa', year: 2026 });
    expect(store.target).toBe(2024);
  });

  it('re-checks a done job when its download expires', async () => {
    const expiresAt = new Date(Date.now() + 5000).toISOString();
    mocked.getJob.mockResolvedValueOnce(
      status({ state: 'done', expiresAt, result: { fileName: 'a_R2025.rvt', size: 3, droppedCount: 0 } }),
    );
    const store = useConvertStore();
    await store.resume(JOB);
    expect(store.phase).toBe('done');
    mocked.getJob.mockRejectedValue(new ApiError(410, 'expired', 'gone'));
    await vi.advanceTimersByTimeAsync(4000);
    expect(mocked.getJob).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(mocked.getJob).toHaveBeenCalledTimes(2);
    expect(store.phase).toBe('error');
    expect(store.error?.code).toBe('expired');
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
    expect(storedJobs()).toEqual([]);
  });

  it('cancel after the upload gave up deletes the waiting job', async () => {
    const store = await readyStore();
    mocked.uploadFile.mockRejectedValue(new ApiError(0, 'upload_failed', 'offline'));
    await store.submit();
    await store.cancel();
    expect(mocked.deleteJob).toHaveBeenCalledWith(JOB);
    expect(store.phase).toBe('pick');
    expect(storedJobs()).toEqual([]);
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

import { defineStore } from 'pinia';
import { ref, shallowRef, watch } from 'vue';

import {
  ApiError,
  createJob,
  deleteJob,
  type DroppedElement,
  getDropped,
  getInfo,
  getJob,
  type JobStatus,
  sendReport,
  type ServerInfo,
  uploadFile,
} from '@/api';
import { readRevitFileInfo, RevitFileError, type RevitFileInfo } from '@/shared/revit-file/readRevitFileInfo';
import { safeStorage } from '@/shared/storage/safeStorage';

export type ConvertPhase = 'pick' | 'ready' | 'uploading' | 'queued' | 'running' | 'done' | 'failed' | 'error';

/**
 * What went wrong. In phase `error`: a reader code (`bad_extension`, `too_large`, `not_ole`, `not_revit`, `unreadable`),
 * `unsupported_year`, `server_restarted`, `expired`, `cancelled`, `upload_interrupted`, or an API code (`hourly_limit`,
 * `upload_failed`, `job_gone`, `network`, …). In phase `failed`: the job's error kind (`refused`, `corrupt`, …).
 */
export interface ConvertError {
  code: string;
  message: string | null;
  retryAfterS: number | null;
}

export const JOB_STORAGE_KEY = 'backsave.job';
export const POLL_INTERVAL_MS = 2000;

const JOB_ID = /^[0-9a-f]{32}$/;

export const useConvertStore = defineStore('convert', () => {
  const phase = ref<ConvertPhase>('pick');
  const file = shallowRef<File | null>(null);
  const info = ref<RevitFileInfo | null>(null);
  const target = ref<number | null>(null);
  const targets = ref<number[]>([]);
  /** Upload progress, 0 to 1. */
  const progress = ref(0);
  const position = ref<number | null>(null);
  const elapsedS = ref(0);
  const estimateS = ref(0);
  const expiresAt = ref<string | null>(null);
  const dropped = ref<DroppedElement[] | null>(null);
  const error = ref<ConvertError | null>(null);
  const reportCode = ref<string | null>(null);
  /** Why the last report request failed: the ApiError code, or `network`. */
  const reportError = ref<string | null>(null);
  const jobId = ref<string | null>(null);
  /** Why the dropped list of a done job could not be loaded; `dropped` stays null then. */
  const droppedError = ref<string | null>(null);

  const serverInfo = shallowRef<ServerInfo | null>(null);
  let loading: Promise<ServerInfo> | null = null;

  // Every reset, cancel, new file or new job starts a new session; a late answer from an older one is dropped.
  let session = 0;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let uploadAbort: AbortController | null = null;
  let uploadedThisSession = false;
  // The tus URL of the current job, kept so an upload that gave up can be resumed with retryUpload().
  let uploadUrl: string | null = null;
  const storage = safeStorage();

  function remember(id: string) {
    try {
      storage.setItem(JOB_STORAGE_KEY, id);
    } catch {
      // Blocked storage only costs the resume after a reload.
    }
  }

  function forget() {
    try {
      storage.removeItem(JOB_STORAGE_KEY);
    } catch {
      // Nothing to forget when storage is blocked.
    }
  }

  function recall(): string | null {
    try {
      return storage.getItem(JOB_STORAGE_KEY);
    } catch {
      return null;
    }
  }

  function onBeforeUnload(event: BeforeUnloadEvent) {
    event.preventDefault();
  }

  watch(
    phase,
    (now, before) => {
      if (now === 'uploading') {
        window.addEventListener('beforeunload', onBeforeUnload);
      } else if (before === 'uploading') {
        window.removeEventListener('beforeunload', onBeforeUnload);
      }
    },
    { flush: 'sync' },
  );

  function fail(code: string, message: string | null = null, retryAfterS: number | null = null) {
    error.value = { code, message, retryAfterS };
    phase.value = 'error';
  }

  function codeOf(cause: unknown): string {
    return cause instanceof ApiError ? cause.code : 'network';
  }

  function failFrom(cause: unknown) {
    if (cause instanceof ApiError) {
      fail(cause.code, cause.message, cause.retryAfterS);
    } else {
      fail('unknown', cause instanceof Error ? cause.message : String(cause));
    }
  }

  /** Ends the current session: no more polls, no upload, and late answers are ignored. */
  function stop() {
    session += 1;
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
    uploadAbort?.abort();
    uploadAbort = null;
    uploadedThisSession = false;
  }

  function clearJob() {
    progress.value = 0;
    position.value = null;
    elapsedS.value = 0;
    estimateS.value = 0;
    expiresAt.value = null;
    dropped.value = null;
    error.value = null;
    reportCode.value = null;
    reportError.value = null;
    droppedError.value = null;
    jobId.value = null;
    uploadUrl = null;
  }

  function clearAll() {
    clearJob();
    file.value = null;
    info.value = null;
    target.value = null;
    targets.value = [];
    phase.value = 'pick';
  }

  async function load(): Promise<ServerInfo | null> {
    if (serverInfo.value) {
      return serverInfo.value;
    }
    loading ??= getInfo().finally(() => {
      loading = null;
    });
    try {
      serverInfo.value = await loading;
      return serverInfo.value;
    } catch (cause) {
      failFrom(cause);
      return null;
    }
  }

  async function chooseFile(chosen: File): Promise<void> {
    if (phase.value !== 'pick' && phase.value !== 'ready' && phase.value !== 'error') {
      return;
    }
    // A job left behind by an upload that gave up is abandoned with the new choice. Any other retained job may still be
    // running (for example after an unexpected polling error), so it is left alone and stays resumable.
    const abandoned = phase.value === 'error' && error.value?.code === 'upload_failed' ? jobId.value : null;
    stop();
    const token = session;
    clearAll();
    file.value = chosen;
    if (abandoned) {
      forget();
      void deleteJob(abandoned).catch(() => undefined);
    }
    let read: RevitFileInfo;
    try {
      read = await readRevitFileInfo(chosen);
    } catch (cause) {
      if (token === session) {
        fail(cause instanceof RevitFileError ? cause.code : 'unreadable');
      }
      return;
    }
    const server = await load();
    if (token !== session || !server) {
      return;
    }
    info.value = read;
    // A fresh array, so sorting in place touches nothing shared.
    const allowed = [...new Set(server.pairs.filter(([from]) => from === read.year).map(([, to]) => to))];
    allowed.sort((a, b) => b - a);
    const highest = allowed[0];
    if (highest === undefined) {
      fail('unsupported_year');
      return;
    }
    targets.value = allowed;
    target.value = allowed.includes(read.year - 1) ? read.year - 1 : highest;
    phase.value = 'ready';
  }

  function setTarget(year: number) {
    if (phase.value === 'ready' && targets.value.includes(year)) {
      target.value = year;
    }
  }

  function schedulePoll(token: number) {
    pollTimer = setTimeout(() => {
      pollTimer = null;
      void poll(token);
    }, POLL_INTERVAL_MS);
  }

  async function poll(token: number): Promise<void> {
    const id = jobId.value;
    if (!id) {
      return;
    }
    let current: JobStatus;
    try {
      current = await getJob(id);
    } catch (cause) {
      if (token !== session) {
        return;
      }
      if (cause instanceof ApiError && cause.status === 404) {
        forget();
        fail('server_restarted');
      } else if (cause instanceof ApiError && cause.status === 410) {
        forget();
        fail('expired');
      } else if (
        cause instanceof ApiError &&
        (cause.status === 0 || cause.status === 429 || cause.status >= 500) &&
        (phase.value === 'queued' || phase.value === 'running')
      ) {
        schedulePoll(token);
      } else {
        failFrom(cause);
      }
      return;
    }
    if (token === session) {
      await apply(id, current, token);
    }
  }

  async function apply(id: string, current: JobStatus, token: number): Promise<void> {
    position.value = current.position;
    elapsedS.value = current.elapsedS;
    estimateS.value = current.estimateS;
    expiresAt.value = current.expiresAt;
    reportCode.value = current.reportCode;
    switch (current.state) {
      case 'waiting_upload':
        if (uploadedThisSession) {
          phase.value = 'queued';
          schedulePoll(token);
          return;
        }
        // Resumed while the upload is unfinished: the page that holds the file may be another tab still uploading, so the
        // job is not deleted here; it is only forgotten, and the server expires it if nobody finishes the upload.
        forget();
        jobId.value = null;
        fail('upload_interrupted');
        return;
      case 'queued':
      case 'running':
        phase.value = current.state;
        schedulePoll(token);
        return;
      case 'done':
        phase.value = 'done';
        if (current.result?.droppedCount === 0) {
          dropped.value = [];
          return;
        }
        try {
          const list = await getDropped(id);
          if (token === session) {
            dropped.value = list;
            droppedError.value = null;
          }
        } catch (cause) {
          // The download still works without the list; the view says the list could not be loaded.
          if (token === session) {
            droppedError.value = codeOf(cause);
          }
        }
        return;
      case 'failed':
        error.value = {
          code: current.error?.kind ?? 'failed',
          message: current.error?.message ?? null,
          retryAfterS: null,
        };
        phase.value = 'failed';
        return;
      case 'cancelled':
        forget();
        jobId.value = null;
        fail('cancelled');
        return;
      case 'expired':
        forget();
        fail('expired');
        return;
    }
  }

  async function submit(): Promise<void> {
    const chosen = file.value;
    const read = info.value;
    const year = target.value;
    if (phase.value !== 'ready' || !chosen || !read || year === null) {
      return;
    }
    stop();
    const token = session;
    phase.value = 'uploading';
    progress.value = 0;
    error.value = null;
    let created: { jobId: string; uploadUrl: string };
    try {
      created = await createJob({
        name: chosen.name,
        size: chosen.size,
        kind: read.kind,
        year: read.year,
        target: year,
      });
    } catch (cause) {
      if (token === session) {
        failFrom(cause);
      }
      return;
    }
    if (token !== session) {
      // Cancelled while the job was being created: do not leave it waiting for an upload.
      await deleteJob(created.jobId).catch(() => undefined);
      return;
    }
    jobId.value = created.jobId;
    uploadUrl = created.uploadUrl;
    remember(created.jobId);
    await runUpload(token, chosen, created.uploadUrl);
  }

  async function runUpload(token: number, chosen: File, url: string): Promise<void> {
    phase.value = 'uploading';
    error.value = null;
    const controller = new AbortController();
    uploadAbort = controller;
    try {
      await uploadFile(chosen, url, {
        signal: controller.signal,
        onProgress: (sent, total) => {
          if (token === session) {
            progress.value = total > 0 ? sent / total : 0;
          }
        },
      });
    } catch (cause) {
      if (token !== session) {
        return;
      }
      uploadAbort = null;
      if (cause instanceof ApiError && cause.code === 'job_gone') {
        forget();
      }
      // `upload_failed` keeps the job and its id so the upload can be resumed.
      failFrom(cause);
      return;
    }
    if (token !== session) {
      return;
    }
    uploadAbort = null;
    uploadedThisSession = true;
    progress.value = 1;
    phase.value = 'queued';
    await poll(token);
  }

  /** Resumes an upload that gave up (`upload_failed`) for the same job; tus continues from the server's offset. */
  async function retryUpload(): Promise<void> {
    const chosen = file.value;
    const url = uploadUrl;
    if (phase.value !== 'error' || error.value?.code !== 'upload_failed' || !chosen || !jobId.value || !url) {
      return;
    }
    stop();
    await runUpload(session, chosen, url);
  }

  async function cancel(): Promise<void> {
    const id = jobId.value;
    const active = phase.value === 'uploading' || phase.value === 'queued' || phase.value === 'running';
    stop();
    forget();
    clearAll();
    if (id && active) {
      await deleteJob(id).catch(() => undefined);
    }
  }

  async function deleteNow(): Promise<void> {
    const id = jobId.value;
    if (!id) {
      return;
    }
    stop();
    const token = session;
    try {
      await deleteJob(id);
    } catch (cause) {
      const gone = cause instanceof ApiError && (cause.status === 404 || cause.status === 410);
      if (!gone) {
        if (token === session) {
          failFrom(cause);
        }
        return;
      }
    }
    if (token === session) {
      forget();
      clearAll();
    }
  }

  /** Sends the failed job's file for review, once; resolves to whether a report code was received. */
  async function consentReport(): Promise<boolean> {
    const id = jobId.value;
    if (phase.value !== 'failed' || !id || reportCode.value !== null) {
      return false;
    }
    const token = session;
    reportError.value = null;
    try {
      const code = await sendReport(id);
      if (token === session) {
        reportCode.value = code;
      }
      return true;
    } catch (cause) {
      if (token === session) {
        reportError.value = codeOf(cause);
      }
      return false;
    }
  }

  /** Follows a job from the `/job/:id` link or, without an id, the one remembered in storage. */
  async function resume(id?: string): Promise<boolean> {
    const candidate = id ?? recall();
    if (!candidate || !JOB_ID.test(candidate)) {
      return false;
    }
    stop();
    const token = session;
    clearAll();
    jobId.value = candidate;
    remember(candidate);
    await poll(token);
    return true;
  }

  function $reset() {
    stop();
    clearAll();
    serverInfo.value = null;
    loading = null;
  }

  return {
    phase,
    file,
    info,
    target,
    targets,
    progress,
    position,
    elapsedS,
    estimateS,
    expiresAt,
    dropped,
    error,
    reportCode,
    reportError,
    jobId,
    droppedError,
    load,
    chooseFile,
    setTarget,
    submit,
    retryUpload,
    cancel,
    deleteNow,
    consentReport,
    resume,
    $reset,
  };
});

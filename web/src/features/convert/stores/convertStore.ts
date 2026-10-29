import { defineStore } from 'pinia';
import { computed, ref, shallowRef, watch } from 'vue';

import {
  ApiError,
  createJob,
  deleteJob,
  type DroppedElement,
  finishOpenReport,
  getDropped,
  getInfo,
  getJob,
  jobFileUrl,
  type JobStatus,
  sendReport,
  type ServerInfo,
  startOpenReport,
  uploadFile,
} from '@/api';
import { readRevitFileInfo, RevitFileError, type RevitFileInfo } from '@/shared/revit-file/readRevitFileInfo';
import { acquireUploadLock, isUploadLockHeld } from '@/shared/locks/uploadLock';
import { safeStorage } from '@/shared/storage/safeStorage';

import { type UploadEntry, uploadRegistry } from './uploadRegistry';

export type ConvertPhase = 'pick' | 'ready' | 'uploading' | 'queued' | 'running' | 'done' | 'failed' | 'error';

/**
 * What went wrong. In phase `error`: a reader code (`bad_extension`, `too_large`, `not_ole`, `not_revit`, `unreadable`),
 * `unsupported_year`, `server_restarted`, `expired`, `cancelled`, `upload_in_other_tab`, or an API code (`hourly_limit`,
 * `upload_failed`, `job_gone`, `network`, …). In phase `failed`: the job's error kind (`refused`, `corrupt`, …).
 */
export interface ConvertError {
  code: string;
  message: string | null;
  retryAfterS: number | null;
}

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
  /** The size of the file the current job was made from, from the server. */
  const jobSize = ref<number | null>(null);
  /** A "does not open in Revit" report is being sent. */
  const unopenableSending = ref(false);
  /** Upload progress of the original sent with that report, 0 to 1. */
  const unopenableProgress = ref(0);
  /**
   * Why the last "does not open" report failed: `needs_file` (no file to send), an API code (`size_mismatch`,
   * `already_reported`, `expired`, `not_found`, `storage_error`, `uploads_in_progress`, `server_full`, …), or `network`.
   */
  const unopenableError = ref<string | null>(null);
  const jobId = ref<string | null>(null);
  /** Why the dropped list of a done job could not be loaded; `dropped` stays null then. */
  const droppedError = ref<string | null>(null);
  /** The finished job's output file, from the server. */
  const result = ref<JobStatus['result']>(null);
  /** The card continues an upload this browser started earlier for the same file (pressing the button sends it). */
  const continuing = ref(false);
  /** This tab created the job, or continued its upload; only such a job is deleted by `cancel`. */
  const ownsJob = ref(false);
  const downloadUrl = computed(() => (jobId.value !== null && result.value !== null ? jobFileUrl(jobId.value) : null));
  /** The tab still holds the file the job was made from, so a report can send it without asking for it again. */
  const canReuseFile = computed(
    () => file.value !== null && jobSize.value !== null && file.value.size === jobSize.value,
  );

  const serverInfo = shallowRef<ServerInfo | null>(null);
  let loading: Promise<ServerInfo> | null = null;

  // Every reset, cancel, new file or new job starts a new session; a late answer from an older one is dropped.
  let session = 0;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  // Fires when a done job's download expires, to check it once more.
  let expiryTimer: ReturnType<typeof setTimeout> | null = null;
  // The expiry time already scheduled, so a server that still says done after it does not cause a loop.
  let expiryScheduledFor: string | null = null;
  let uploadAbort: AbortController | null = null;
  // The upload of the original for a "does not open" report.
  let reportAbort: AbortController | null = null;
  let uploadedThisSession = false;
  // The tus URL of the current job, kept so an upload that gave up can be resumed with retryUpload().
  let uploadUrl: string | null = null;
  const uploads = uploadRegistry(safeStorage());
  // Entries of uploads that were never finished go after 6 hours, even if no file is ever chosen again.
  uploads.prune();
  // The earlier upload the ready card would continue when its button is pressed.
  let pendingContinue: { jobId: string; uploadUrl: string } | null = null;
  // The job whose exclusive upload lock this tab holds, from the start of its upload until it is complete, cancelled or
  // left for another file. Only this tab may then send data for that job or delete it.
  let lockedJob: string | null = null;
  let releaseLock: (() => void) | null = null;

  function holdsLock(id: string): boolean {
    return lockedJob === id;
  }

  /** Takes the upload lock of `id`; false when another tab holds it. */
  async function lock(id: string): Promise<boolean> {
    if (holdsLock(id)) {
      return true;
    }
    unlock();
    const release = await acquireUploadLock(id);
    if (!release) {
      return false;
    }
    lockedJob = id;
    releaseLock = release;
    return true;
  }

  function unlock() {
    releaseLock?.();
    releaseLock = null;
    lockedJob = null;
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
    if (expiryTimer !== null) {
      clearTimeout(expiryTimer);
      expiryTimer = null;
    }
    expiryScheduledFor = null;
    uploadAbort?.abort();
    uploadAbort = null;
    reportAbort?.abort();
    reportAbort = null;
    unopenableSending.value = false;
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
    jobSize.value = null;
    unopenableSending.value = false;
    unopenableProgress.value = 0;
    unopenableError.value = null;
    droppedError.value = null;
    result.value = null;
    jobId.value = null;
    uploadUrl = null;
    continuing.value = false;
    ownsJob.value = false;
    pendingContinue = null;
    expiryScheduledFor = null;
    unlock();
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
    const failedId = phase.value === 'error' && error.value?.code === 'upload_failed' ? jobId.value : null;
    // Deleted only while this tab still holds its upload lock: otherwise another tab may be uploading it now.
    const abandoned = failedId && ownsJob.value && holdsLock(failedId) ? failedId : null;
    stop();
    const token = session;
    clearAll();
    file.value = chosen;
    if (abandoned) {
      uploads.remove(abandoned);
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
    const earlier = uploads.find(chosen);
    if (earlier) {
      const outcome = await checkEarlierUpload(earlier);
      if (token !== session) {
        return;
      }
      if (outcome === 'busy') {
        fail('upload_in_other_tab');
        return;
      }
      if (outcome !== 'new') {
        // The same file's upload is still waiting on the server: the button continues it with the same job. Nothing is
        // sent before it is pressed.
        targets.value = [outcome.target];
        target.value = outcome.target;
        pendingContinue = { jobId: earlier.jobId, uploadUrl: earlier.uploadUrl };
        continuing.value = true;
        phase.value = 'ready';
        return;
      }
    }
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

  /**
   * Whether an upload this browser started earlier for the same file can be continued: `busy` when another tab seems to
   * be uploading it (a hint; the lock taken on pressing the button decides), `new` when its job is gone or no longer
   * waiting (the entry is dropped then), else the job's status.
   */
  async function checkEarlierUpload(entry: UploadEntry): Promise<JobStatus | 'busy' | 'new'> {
    let current: JobStatus;
    try {
      current = await getJob(entry.jobId);
    } catch (cause) {
      if (cause instanceof ApiError && (cause.status === 404 || cause.status === 410)) {
        uploads.remove(entry.jobId);
      }
      return 'new';
    }
    if (current.state !== 'waiting_upload') {
      uploads.remove(entry.jobId);
      return 'new';
    }
    return (await isUploadLockHeld(entry.jobId)) ? 'busy' : current;
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
        uploads.remove(id);
        fail('server_restarted');
      } else if (cause instanceof ApiError && cause.status === 410) {
        uploads.remove(id);
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

  /** Checks a done job once more when its download expires, so the page says so instead of keeping a dead link. */
  function scheduleExpiryCheck(token: number, at: string | null) {
    const when = at === null ? Number.NaN : Date.parse(at);
    if (Number.isNaN(when) || at === expiryScheduledFor) {
      return;
    }
    expiryScheduledFor = at;
    if (expiryTimer !== null) {
      clearTimeout(expiryTimer);
    }
    const delay = Math.min(Math.max(0, when - Date.now()) + 1000, 2 ** 31 - 1);
    expiryTimer = setTimeout(() => {
      expiryTimer = null;
      if (token === session) {
        void poll(token);
      }
    }, delay);
  }

  async function apply(id: string, current: JobStatus, token: number): Promise<void> {
    // A reopened job knows its versions only from the server.
    info.value ??= { kind: current.kind, year: current.year };
    target.value ??= current.target;
    position.value = current.position;
    elapsedS.value = current.elapsedS;
    estimateS.value = current.estimateS;
    expiresAt.value = current.expiresAt;
    reportCode.value = current.reportCode;
    jobSize.value = current.size;
    result.value = current.result;
    if (current.state !== 'waiting_upload') {
      uploads.remove(id);
    }
    switch (current.state) {
      case 'waiting_upload':
        if (uploadedThisSession) {
          phase.value = 'queued';
          schedulePoll(token);
          return;
        }
        // Opened from a link while its upload is unfinished: the tab holding the file may still be uploading, so the job is
        // left alone and the card starts empty. Choosing the same file in this browser continues it (see chooseFile).
        clearAll();
        return;
      case 'queued':
      case 'running':
        phase.value = current.state;
        schedulePoll(token);
        return;
      case 'done':
        phase.value = 'done';
        scheduleExpiryCheck(token, current.expiresAt);
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
        jobId.value = null;
        fail('cancelled');
        return;
      case 'expired':
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
    const earlier = pendingContinue;
    if (earlier) {
      pendingContinue = null;
      if (!(await lock(earlier.jobId))) {
        if (token === session) {
          continuing.value = false;
          fail('upload_in_other_tab');
        }
        return;
      }
      if (token !== session) {
        return;
      }
      jobId.value = earlier.jobId;
      uploadUrl = earlier.uploadUrl;
      ownsJob.value = true;
      await runUpload(token, chosen, earlier.uploadUrl);
      return;
    }
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
    ownsJob.value = true;
    uploads.add(chosen, created.jobId, created.uploadUrl);
    // A new job is unknown to other tabs, so the lock is always free.
    await lock(created.jobId);
    if (token !== session) {
      return;
    }
    await runUpload(token, chosen, created.uploadUrl);
  }

  async function runUpload(token: number, chosen: File, url: string): Promise<void> {
    phase.value = 'uploading';
    error.value = null;
    const controller = new AbortController();
    uploadAbort = controller;
    const id = jobId.value;
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
      // The server may have moved the job on (failed for a full disk, or queued after another tab finished the upload):
      // follow it rather than showing the bare upload error.
      let current: JobStatus | null = null;
      if (id) {
        try {
          current = await getJob(id);
        } catch {
          current = null;
        }
      }
      if (token !== session) {
        return;
      }
      if (id && current && current.state !== 'waiting_upload') {
        unlock();
        // A continued upload that another tab finished first is not this tab's job.
        if (continuing.value) {
          ownsJob.value = false;
        }
        await apply(id, current, token);
        return;
      }
      if (cause instanceof ApiError && cause.code === 'job_gone' && id) {
        unlock();
        uploads.remove(id);
      }
      // `upload_failed` keeps the job, its id and its lock so the upload can be resumed here and nowhere else.
      failFrom(cause);
      return;
    }
    if (token !== session) {
      return;
    }
    uploadAbort = null;
    unlock();
    if (id) {
      uploads.remove(id);
    }
    uploadedThisSession = true;
    progress.value = 1;
    phase.value = 'queued';
    await poll(token);
  }

  /** Resumes an upload that gave up (`upload_failed`) for the same job; tus continues from the server's offset. */
  async function retryUpload(): Promise<void> {
    const chosen = file.value;
    const url = uploadUrl;
    const id = jobId.value;
    if (phase.value !== 'error' || error.value?.code !== 'upload_failed' || !chosen || !id || !url || !holdsLock(id)) {
      return;
    }
    stop();
    await runUpload(session, chosen, url);
  }

  async function cancel(): Promise<void> {
    const id = jobId.value;
    // Only this tab's own job is deleted: while it still waits for data, only with its upload lock (another tab may be
    // uploading it otherwise); once queued or running after this tab's upload, nobody else can take it over. A job this
    // tab only follows (opened from a link) is never deleted here: the card just goes back to the start.
    const uploadingHere =
      phase.value === 'uploading' || (phase.value === 'error' && error.value?.code === 'upload_failed');
    const pastUpload = phase.value === 'queued' || phase.value === 'running';
    const mayDelete =
      id !== null && ownsJob.value && ((uploadingHere && holdsLock(id)) || (pastUpload && uploadedThisSession));
    stop();
    clearAll();
    if (id && mayDelete) {
      uploads.remove(id);
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
      uploads.remove(id);
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

  /**
   * Reports that the done job's file does not open in Revit, once: sends the original (`picked`, else the file this tab
   * still holds for the job) and resolves to whether a report code was received. Without a file to send, sets
   * `unopenableError` to `needs_file`.
   */
  async function reportUnopenable(note: string, picked?: File): Promise<boolean> {
    const id = jobId.value;
    if (phase.value !== 'done' || !id || reportCode.value !== null || unopenableSending.value) {
      return false;
    }
    const chosen = picked ?? (canReuseFile.value ? file.value : null);
    if (!chosen) {
      unopenableError.value = 'needs_file';
      return false;
    }
    const token = session;
    const controller = new AbortController();
    reportAbort = controller;
    unopenableSending.value = true;
    unopenableProgress.value = 0;
    unopenableError.value = null;
    const trimmed = note.trim();
    try {
      const url = await startOpenReport(id, chosen.size, trimmed === '' ? undefined : trimmed);
      await uploadFile(chosen, url, {
        signal: controller.signal,
        onProgress: (sent, total) => {
          if (token === session) {
            unopenableProgress.value = total > 0 ? sent / total : 0;
          }
        },
      });
      const code = await finishOpenReport(id);
      if (token === session) {
        reportCode.value = code;
      }
      return true;
    } catch (cause) {
      if (token === session) {
        const failure = await reportFailureCode(id, cause, token);
        if (token === session) {
          unopenableError.value = failure;
        }
      }
      return false;
    } finally {
      if (token === session) {
        reportAbort = null;
        unopenableSending.value = false;
      }
    }
  }

  /**
   * The code a failed "does not open" report shows. An upload refused as gone asks the server whether the job is; an
   * upload closed under it, or a report refused as already sent (another tab won), asks for the job's report code so
   * that it can be shown (`token` guards against a session that changed meanwhile).
   */
  async function reportFailureCode(id: string, cause: unknown, token: number): Promise<string> {
    const code = codeOf(cause);
    if (code === 'upload_closed' || code === 'already_reported') {
      try {
        const current = await getJob(id);
        if (current.reportCode !== null) {
          if (token === session) {
            reportCode.value = current.reportCode;
          }
          return 'already_reported';
        }
      } catch {
        // The answer below still says what happened to this request.
      }
      return code;
    }
    if (code !== 'job_gone') {
      return code;
    }
    try {
      await getJob(id);
      return 'upload_failed';
    } catch (again) {
      return again instanceof ApiError && again.status === 410 ? 'expired' : codeOf(again);
    }
  }

  /** Follows the job of a `/job/:id` link: view and download, never deleted unless the user presses Delete now. */
  async function resume(id?: string): Promise<boolean> {
    if (!id || !JOB_ID.test(id)) {
      return false;
    }
    stop();
    const token = session;
    clearAll();
    jobId.value = id;
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
    jobSize,
    canReuseFile,
    unopenableSending,
    unopenableProgress,
    unopenableError,
    jobId,
    continuing,
    ownsJob,
    droppedError,
    result,
    downloadUrl,
    load,
    chooseFile,
    setTarget,
    submit,
    retryUpload,
    cancel,
    deleteNow,
    consentReport,
    reportUnopenable,
    resume,
    $reset,
  };
});

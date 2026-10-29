import { DetailedError, Upload } from 'tus-js-client';

import { ApiError, apiErrorFrom } from './client';
import type { UploadOptions } from './types';

// Small requests: behind a tunnel or proxy a large request body slows down and is cut once the server has not answered
// within about 100 s (Cloudflare 524), so a 50 MiB chunk could never finish. 4 MiB requests finish in seconds.
export const UPLOAD_CHUNK_SIZE = 4 * 1024 * 1024;
export const UPLOAD_RETRY_DELAYS = [0, 1000, 3000, 5000, 10_000];

function uploadErrorOf(error: Error): ApiError {
  const response = error instanceof DetailedError ? error.originalResponse : null;
  if (!response) {
    return new ApiError(0, 'upload_failed', error.message);
  }
  const status = response.getStatus();
  if (status === 404 || status === 410) {
    return new ApiError(status, 'job_gone', 'the job no longer exists on the server');
  }
  const parsed = apiErrorFrom(status, response.getBody(), response.getHeader('Retry-After'));
  return parsed.code === `http_${String(status)}`
    ? new ApiError(status, 'upload_failed', error.message, parsed.retryAfterS)
    : parsed;
}

/** Answers that a retry cannot change: the job is gone, the disk is full, or the upload is closed. */
const FINAL_STATUSES = new Set([404, 410, 507]);

/**
 * The retry policy for upload requests. Network failures, server errors, 423 and 409 offset or busy conflicts are retried
 * (tus then asks the server for its offset); a full disk, a gone job, a closed upload and other client errors are not.
 */
export function shouldRetryUpload(error: Error): boolean {
  const response = error instanceof DetailedError ? error.originalResponse : null;
  if (!response) {
    return true;
  }
  const status = response.getStatus();
  if (FINAL_STATUSES.has(status)) {
    return false;
  }
  if (status === 409) {
    return apiErrorFrom(status, response.getBody(), null).code !== 'upload_closed';
  }
  return status < 400 || status >= 500 || status === 423;
}

/**
 * Sends `file` to the tus upload the server already created at `uploadUrl`. Rejects with an ApiError: `aborted` when
 * `signal` fires, `job_gone` on 404/410, the server's code for another refusal, `upload_failed` once retries run out.
 */
export function uploadFile(file: Blob, uploadUrl: string, options: UploadOptions = {}): Promise<void> {
  const { onProgress, signal } = options;
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new ApiError(0, 'aborted', 'the upload was cancelled'));
      return;
    }
    const upload = new Upload(file, {
      uploadUrl,
      chunkSize: UPLOAD_CHUNK_SIZE,
      retryDelays: UPLOAD_RETRY_DELAYS,
      removeFingerprintOnSuccess: true,
      onShouldRetry: (error) => shouldRetryUpload(error),
      onProgress: (sent, total) => {
        onProgress?.(sent, total);
      },
      onSuccess: () => {
        signal?.removeEventListener('abort', onAbort);
        resolve();
      },
      onError: (error) => {
        signal?.removeEventListener('abort', onAbort);
        reject(uploadErrorOf(error));
      },
    });
    function onAbort() {
      void upload.abort();
      reject(new ApiError(0, 'aborted', 'the upload was cancelled'));
    }
    signal?.addEventListener('abort', onAbort, { once: true });
    upload.start();
  });
}

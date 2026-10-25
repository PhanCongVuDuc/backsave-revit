import { DetailedError, Upload } from 'tus-js-client';

import { ApiError, apiErrorFrom } from './client';
import type { UploadOptions } from './types';

export const UPLOAD_CHUNK_SIZE = 50 * 1024 * 1024;
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

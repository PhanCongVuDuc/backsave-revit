//! Resumable uploads: the core of the tus 1.0 protocol (OPTIONS, HEAD, PATCH).
//!
//! Uploads are created together with their job (`POST /api/jobs`), so the creation extension is not offered.

use crate::AppState;
use crate::jobs::{Job, JobId};
use crate::model::{ApiError, ErrorKind, JobState};
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{Next, from_fn};
use axum::response::{IntoResponse, Response};
use axum::routing::{options, patch};
use http_body_util::BodyExt;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tokio::io::AsyncWriteExt;

const TUS_VERSION: &str = "1.0.0";
const OFFSET_CONTENT_TYPE: &str = "application/offset+octet-stream";
const TUS_RESUMABLE: HeaderName = HeaderName::from_static("tus-resumable");
const UPLOAD_OFFSET: HeaderName = HeaderName::from_static("upload-offset");
const UPLOAD_LENGTH: HeaderName = HeaderName::from_static("upload-length");

/// Routes for `/api/uploads/{id}`.
pub fn routes() -> Router<AppState> {
    let write = patch(patch_upload).layer(DefaultBodyLimit::disable());
    Router::new()
        .route(
            "/api/uploads/{id}",
            options(options_upload).head(head_upload).merge(write),
        )
        .layer(from_fn(add_tus_header))
}

async fn add_tus_header(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert(TUS_RESUMABLE, HeaderValue::from_static(TUS_VERSION));
    resp
}

/// Ids of uploads with a PATCH in flight; keeps two requests from interleaving their writes.
fn busy_set() -> &'static Mutex<HashSet<JobId>> {
    static SET: OnceLock<Mutex<HashSet<JobId>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

struct BusyGuard(JobId);

impl BusyGuard {
    fn acquire(id: JobId) -> Option<BusyGuard> {
        let mut set = busy_set().lock().unwrap_or_else(|e| e.into_inner());
        set.insert(id).then(|| BusyGuard(id))
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        busy_set()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "Upload not found.")
}

fn closed() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "upload_closed",
        "This upload is no longer accepting data.",
    )
}

fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "too_large",
        "The data goes past the declared file size.",
    )
}

/// Finds the job for a path id. Unknown, malformed and expired ids are all "not found".
fn find_job(state: &AppState, raw: &str) -> Result<Job, ApiError> {
    let id: JobId = raw.parse().map_err(|_| not_found())?;
    match state.store.get(&id) {
        Some(job) if job.state != JobState::Expired => Ok(job),
        _ => Err(not_found()),
    }
}

async fn options_upload() -> Response {
    let mut resp = StatusCode::NO_CONTENT.into_response();
    let h = resp.headers_mut();
    h.insert("tus-version", HeaderValue::from_static(TUS_VERSION));
    h.insert("tus-max-size", HeaderValue::from(crate::config::MAX_BYTES));
    h.insert("tus-extension", HeaderValue::from_static(""));
    resp
}

async fn head_upload(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> Result<Response, ApiError> {
    let job = find_job(&state, &raw)?;
    let mut resp = StatusCode::OK.into_response();
    let h = resp.headers_mut();
    h.insert(UPLOAD_OFFSET, HeaderValue::from(job.received));
    h.insert(UPLOAD_LENGTH, HeaderValue::from(job.size));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// Deletes the partial input, fails the job as "disk full" and builds the 507 answer.
fn storage_error(state: &AppState, job: &Job) -> ApiError {
    let path = state.store.input_path(job);
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
        let _ = std::fs::remove_file(&path);
    }
    let message = "The server could not store the file.";
    state
        .store
        .set_failed_upload(&job.id, ErrorKind::DiskFull, message.into());
    ApiError::new(StatusCode::INSUFFICIENT_STORAGE, "storage_error", message)
}

async fn patch_upload(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let job = find_job(&state, &raw)?;
    if job.state != JobState::WaitingUpload {
        return Err(closed());
    }
    let header_str = |name: &HeaderName| headers.get(name).and_then(|v| v.to_str().ok());
    if header_str(&TUS_RESUMABLE) != Some(TUS_VERSION) {
        return Err(ApiError::new(
            StatusCode::PRECONDITION_FAILED,
            "bad_tus_version",
            "Unsupported tus version.",
        ));
    }
    let content_type = header_str(&header::CONTENT_TYPE)
        .and_then(|v| v.split(';').next())
        .map(str::trim);
    if !content_type.is_some_and(|t| t.eq_ignore_ascii_case(OFFSET_CONTENT_TYPE)) {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "bad_content_type",
            "Content-Type must be application/offset+octet-stream.",
        ));
    }
    let offset: u64 = header_str(&UPLOAD_OFFSET)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "bad_offset",
                "Missing or invalid Upload-Offset.",
            )
        })?;

    let _guard = BusyGuard::acquire(job.id).ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "upload_busy",
            "Another request is writing this upload.",
        )
    })?;
    // Read again under the guard: a request that just finished may have moved the offset or closed the upload.
    let job = find_job(&state, &raw)?;
    if job.state != JobState::WaitingUpload {
        return Err(closed());
    }
    if offset != job.received {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "offset_mismatch",
            "Upload-Offset does not match the server.",
        ));
    }
    let declared = header_str(&header::CONTENT_LENGTH).and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > job.size - job.received) {
        return Err(too_large());
    }

    let path = state.store.input_path(&job);
    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    {
        Ok(f) => f,
        Err(_) => return Err(storage_error(&state, &job)),
    };

    let mut body = body;
    let mut received = job.received;
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            // The client went away; what was written stays and HEAD reports it.
            let _ = file.flush().await;
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "upload_interrupted",
                "The upload was interrupted.",
            ));
        };
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        if received + chunk.len() as u64 > job.size {
            let _ = file.flush().await;
            return Err(too_large());
        }
        if file.write_all(&chunk).await.is_err() {
            drop(file);
            return Err(storage_error(&state, &job));
        }
        received = state.store.add_received(&job.id, chunk.len() as u64);
    }
    if file.flush().await.is_err() {
        drop(file);
        return Err(storage_error(&state, &job));
    }
    drop(file);
    if received == job.size {
        state.store.mark_uploaded(&job.id);
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut()
        .insert(UPLOAD_OFFSET, HeaderValue::from(received));
    Ok(resp)
}

//! Resumable uploads: the core of the tus 1.0 protocol (OPTIONS, HEAD, PATCH).
//!
//! Two kinds of upload share one write path: a job's input at `/api/uploads/{id}`, created together with its job
//! (`POST /api/jobs`), and the original sent again for a "does not open" report at `/api/jobs/{id}/open-report/upload`,
//! opened by `POST /api/jobs/{id}/open-report`. Neither offers the creation extension.

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
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tokio::io::{AsyncWrite, AsyncWriteExt};

const TUS_VERSION: &str = "1.0.0";
const OFFSET_CONTENT_TYPE: &str = "application/offset+octet-stream";
const TUS_RESUMABLE: HeaderName = HeaderName::from_static("tus-resumable");
const UPLOAD_OFFSET: HeaderName = HeaderName::from_static("upload-offset");
const UPLOAD_LENGTH: HeaderName = HeaderName::from_static("upload-length");

/// Routes for `/api/uploads/{id}` and `/api/jobs/{id}/open-report/upload`.
pub fn routes() -> Router<AppState> {
    let write = patch(patch_upload).layer(DefaultBodyLimit::disable());
    let report_write = patch(patch_report_upload).layer(DefaultBodyLimit::disable());
    Router::new()
        .route(
            "/api/uploads/{id}",
            options(options_upload).head(head_upload).merge(write),
        )
        .route(
            "/api/jobs/{id}/open-report/upload",
            options(options_upload)
                .head(head_report_upload)
                .merge(report_write),
        )
        .layer(from_fn(add_tus_header))
}

/// What an upload writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The input of a job waiting for its file.
    Input,
    /// The original sent again for a "does not open" report on a done job.
    ReportInput,
}

impl Target {
    /// Finds the job for a path id. For an input, unknown, malformed and expired ids are all "not found"; for a report
    /// input, an expired job is "expired" and a job without an open report is "not found".
    fn find(self, state: &AppState, raw: &str) -> Result<Job, ApiError> {
        let id: JobId = raw.parse().map_err(|_| not_found())?;
        let job = state.store.get(&id);
        match (self, job) {
            (Target::Input, Some(job)) if job.state != JobState::Expired => Ok(job),
            (Target::ReportInput, Some(job)) if job.state == JobState::Expired => Err(
                ApiError::new(StatusCode::GONE, "expired", "This job has expired."),
            ),
            (Target::ReportInput, Some(job)) => Ok(job),
            _ => Err(not_found()),
        }
    }

    /// Whether the upload takes data now. A finished, failed or cancelled job's input, or a report that is being
    /// written or already exists, takes no more: saying so stops the client from resuming at a stale offset.
    fn accepting(self, job: &Job) -> Result<(), ApiError> {
        match self {
            Target::Input if job.state == JobState::WaitingUpload => Ok(()),
            Target::Input => Err(closed()),
            Target::ReportInput if job.reporting || job.report_code.is_some() => Err(closed()),
            Target::ReportInput if job.open_report.is_none() => Err(not_found()),
            Target::ReportInput if job.state == JobState::Done => Ok(()),
            Target::ReportInput => Err(closed()),
        }
    }

    fn received(self, job: &Job) -> u64 {
        match self {
            Target::Input => job.received,
            Target::ReportInput => job.open_report.as_ref().map_or(0, |o| o.received),
        }
    }

    fn path(self, state: &AppState, job: &Job) -> PathBuf {
        match self {
            Target::Input => state.store.input_path(job),
            Target::ReportInput => state.store.report_input_path(job),
        }
    }

    /// Counts accepted bytes and returns the new total.
    fn add_received(self, state: &AppState, id: &JobId, n: u64) -> u64 {
        match self {
            Target::Input => state.store.add_received(id, n),
            Target::ReportInput => state.store.open_report_add_received(id, n),
        }
    }

    /// Gives up after the file could not be stored and builds the 507 answer. A job's input fails the job as "disk
    /// full"; a report input only closes the open report (the job stays done) and the user can start it again.
    fn storage_error(self, state: &AppState, job: &Job) -> ApiError {
        let message = "The server could not store the file.";
        match self {
            Target::Input => {
                remove_partial(&state.store.input_path(job));
                tracing::warn!(
                    job_id = %job.id,
                    kind = "disk_full",
                    size_mib = job.size.div_ceil(1 << 20),
                    "upload could not be stored"
                );
                state
                    .store
                    .set_failed_upload(&job.id, ErrorKind::DiskFull, message.into());
            }
            Target::ReportInput => {
                remove_partial(&state.store.report_input_path(job));
                tracing::warn!(
                    job_id = %job.id,
                    size_mib = job.size.div_ceil(1 << 20),
                    "report upload could not be stored"
                );
                state.store.open_report_close(&job.id);
            }
        }
        ApiError::new(StatusCode::INSUFFICIENT_STORAGE, "storage_error", message)
    }

    /// Runs once every byte has arrived.
    fn complete(self, state: &AppState, id: &JobId) {
        match self {
            Target::Input => state.store.mark_uploaded(id),
            // The report is made by `POST /api/jobs/{id}/open-report/finish`.
            Target::ReportInput => {}
        }
    }
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
    head(&state, &raw, Target::Input)
}

async fn head_report_upload(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> Result<Response, ApiError> {
    head(&state, &raw, Target::ReportInput)
}

fn head(state: &AppState, raw: &str, target: Target) -> Result<Response, ApiError> {
    let job = target.find(state, raw)?;
    target.accepting(&job)?;
    let mut resp = StatusCode::OK.into_response();
    let h = resp.headers_mut();
    h.insert(UPLOAD_OFFSET, HeaderValue::from(target.received(&job)));
    h.insert(UPLOAD_LENGTH, HeaderValue::from(job.size));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// Deletes a partial file, if it is one.
fn remove_partial(path: &std::path::Path) {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && let Err(e) = std::fs::remove_file(path)
    {
        tracing::warn!(error = ?e.kind(), "could not remove partial upload");
    }
}

async fn patch_upload(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    patch_target(&state, &raw, &headers, body, Target::Input).await
}

async fn patch_report_upload(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    patch_target(&state, &raw, &headers, body, Target::ReportInput).await
}

async fn patch_target(
    state: &AppState,
    raw: &str,
    headers: &HeaderMap,
    body: Body,
    target: Target,
) -> Result<Response, ApiError> {
    let job = target.find(state, raw)?;
    target.accepting(&job)?;
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
    let job = target.find(state, raw)?;
    target.accepting(&job)?;
    let already = target.received(&job);
    if offset != already {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "offset_mismatch",
            "Upload-Offset does not match the server.",
        ));
    }
    let declared = header_str(&header::CONTENT_LENGTH).and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > job.size - already) {
        return Err(too_large());
    }

    let path = target.path(state, &job);
    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    {
        Ok(f) => f,
        Err(_) => return Err(target.storage_error(state, &job)),
    };
    let received = write_body(state, &job, target, &mut file, body).await?;
    drop(file);
    if received == job.size {
        target.complete(state, &job.id);
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut()
        .insert(UPLOAD_OFFSET, HeaderValue::from(received));
    Ok(resp)
}

/// Appends the body to `writer`, flushing each chunk before it is counted, so `received` only ever covers
/// bytes the writer accepted. Any write or flush failure removes the partial file and gives up on the upload
/// (see `Target::storage_error`).
async fn write_body<W>(
    state: &AppState,
    job: &Job,
    target: Target,
    writer: &mut W,
    mut body: Body,
) -> Result<u64, ApiError>
where
    W: AsyncWrite + Unpin,
{
    let mut received = target.received(job);
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            // The client went away; what was counted stays on disk and HEAD reports it.
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
            return Err(too_large());
        }
        if writer.write_all(&chunk).await.is_err() || writer.flush().await.is_err() {
            return Err(target.storage_error(state, job));
        }
        received = target.add_received(state, &job.id, chunk.len() as u64);
    }
    Ok(received)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::config::Config;
    use crate::engine::Engine;
    use crate::jobs::{JobStore, NewJob};
    use crate::model::FileKind;
    use std::io;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    /// Accepts the first write, fails every write after it.
    struct FailsOnSecondWrite(u32);

    impl AsyncWrite for FailsOnSecondWrite {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.0 += 1;
            if self.0 > 1 {
                return Poll::Ready(Err(io::Error::other("disk full")));
            }
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn test_state(dir: &tempfile::TempDir) -> AppState {
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
        ));
        let config =
            Config::from_lookup(|k| (k == "REVIT_DOWNGRADE_ENGINE").then(|| "x".to_string()))
                .unwrap();
        AppState {
            store: Arc::new(JobStore::new(dir.path().to_path_buf(), clock.clone())),
            engine: Arc::new(Engine::new("x")),
            config: Arc::new(config),
            pairs: Arc::new(vec![(2026, 2024)]),
            clock,
            reports: Arc::new(crate::reports::Reports::new(dir.path().join("reports"))),
            stats: Arc::new(crate::stats::Stats::open(
                dir.path().join("stats"),
                chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            )),
            engine_version: Arc::new("x".to_string()),
        }
    }

    fn new_job(state: &AppState) -> Job {
        let req = NewJob {
            name: "a.rvt".into(),
            size: 100,
            kind: FileKind::Rvt,
            year: 2026,
            target: 2024,
        };
        state
            .store
            .create(req, std::net::IpAddr::from([10, 0, 0, 1]), &state.pairs)
            .unwrap()
    }

    /// A body of two chunks, "one" then "two".
    async fn two_chunks() -> Body {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, io::Error>>(4);
        tx.send(Ok(b"one".to_vec())).await.unwrap();
        tx.send(Ok(b"two".to_vec())).await.unwrap();
        drop(tx);
        Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        }))
    }

    #[tokio::test]
    async fn failed_second_write_removes_partial_file_and_fails_job() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(&dir);
        let job = new_job(&state);
        // Bytes from an earlier chunk are already on disk.
        let input = state.store.input_path(&job);
        std::fs::write(&input, b"earlier").unwrap();

        let mut writer = FailsOnSecondWrite(0);
        let err = write_body(&state, &job, Target::Input, &mut writer, two_chunks().await)
            .await
            .unwrap_err();

        assert_eq!(err.status, StatusCode::INSUFFICIENT_STORAGE);
        let after = state.store.get(&job.id).unwrap();
        assert_eq!(after.state, JobState::Failed);
        assert_eq!(after.error.unwrap().kind, ErrorKind::DiskFull);
        assert_eq!(after.received, 3, "only the accepted chunk is counted");
        assert!(!input.exists(), "partial input must be removed");
    }

    #[tokio::test]
    async fn failed_report_write_removes_partial_file_and_keeps_job_done() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(&dir);
        let job = new_job(&state);
        state.store.mark_uploaded(&job.id);
        state.store.take_next().unwrap();
        state.store.finish_done(&job.id, Vec::new(), 10);
        let ip = std::net::IpAddr::from([10, 0, 0, 1]);
        state
            .store
            .open_report_start(&job.id, 100, None, ip)
            .unwrap();
        let job = state.store.get(&job.id).unwrap();
        let partial = state.store.report_input_path(&job);
        std::fs::write(&partial, b"earlier").unwrap();

        let mut writer = FailsOnSecondWrite(0);
        let err = write_body(
            &state,
            &job,
            Target::ReportInput,
            &mut writer,
            two_chunks().await,
        )
        .await
        .unwrap_err();

        assert_eq!(err.status, StatusCode::INSUFFICIENT_STORAGE);
        assert_eq!(err.code, "storage_error");
        let after = state.store.get(&job.id).unwrap();
        assert_eq!(after.state, JobState::Done);
        assert!(after.error.is_none());
        assert!(after.open_report.is_none());
        assert!(!partial.exists(), "partial report input must be removed");
    }
}

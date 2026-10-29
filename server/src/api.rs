//! JSON API under `/api`: server info, usage statistics, job creation, status, downloads and
//! deletion.

use crate::AppState;
use crate::config::IpCidr;
use crate::jobs::{CancelOutcome, Job, JobId, NewJob, ReportClaim};
use crate::model::{ApiError, DroppedElement, ErrorKind, FileKind, JobState, estimate};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use tokio_util::io::ReaderStream;

const MAX_NAME_CHARS: usize = 255;
const NO_STORE: &str = "no-store";
/// The public totals may be up to a minute old in browsers and proxies.
const STATS_CACHE: &str = "public, max-age=60";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/stats", get(stats))
        .route("/api/jobs", post(create_job))
        .route("/api/jobs/{id}", get(get_job).delete(delete_job))
        .route("/api/jobs/{id}/file", get(download_file))
        .route("/api/jobs/{id}/dropped", get(dropped))
        .route("/api/jobs/{id}/report", post(report_job))
        .route("/api", any(api_not_found))
        .route("/api/{*rest}", any(api_not_found))
}

async fn api_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "Not found.")
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "Job not found.")
}

fn expired() -> ApiError {
    ApiError::new(StatusCode::GONE, "expired", "This job has expired.")
}

/// Picks the address used for rate limits: the socket peer, or with a trusted proxy the last
/// valid address in `X-Forwarded-For`. With a list of trusted proxies, the header counts only
/// when the socket peer is in that list.
pub fn client_ip(
    trust_proxy: bool,
    trusted: &[IpCidr],
    headers: &HeaderMap,
    peer: SocketAddr,
) -> IpAddr {
    let from_proxy = trusted.is_empty() || trusted.iter().any(|c| c.contains(peer.ip()));
    if trust_proxy && from_proxy {
        let forwarded = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .rev()
            .find_map(|part| part.trim().parse::<IpAddr>().ok());
        if let Some(ip) = forwarded {
            return ip;
        }
    }
    peer.ip()
}

async fn info(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "max_bytes": s.config.max_bytes,
        "output_ttl_s": 3600,
        "pairs": &*s.pairs,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Public usage totals, served from memory. While the statistics are unavailable the answer is
/// 503, never zeros.
async fn stats(State(s): State<AppState>) -> Response {
    match s.stats.snapshot() {
        Some(snapshot) => ([(header::CACHE_CONTROL, STATS_CACHE)], Json(snapshot)).into_response(),
        None => {
            let mut resp = ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "stats_unavailable",
                "Usage statistics are not available.",
            )
            .into_response();
            resp.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
            resp
        }
    }
}

#[derive(Deserialize)]
struct CreateRequest {
    name: String,
    size: u64,
    kind: FileKind,
    year: u16,
    target: u16,
}

async fn create_job(
    State(s): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<CreateRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(req) = body.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "The request body is not valid.",
        )
    })?;
    let chars = req.name.chars().count();
    if chars == 0 || chars > MAX_NAME_CHARS {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_name",
            "The file name must be 1 to 255 characters.",
        ));
    }
    if req.size == 0 {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_size",
            "The file is empty.",
        ));
    }
    let ip = client_ip(
        s.config.trust_proxy,
        &s.config.trusted_proxies,
        &headers,
        peer,
    );
    let job = s.store.create(
        NewJob {
            name: req.name,
            size: req.size,
            kind: req.kind,
            year: req.year,
            target: req.target,
        },
        ip,
        &s.pairs,
    )?;
    let body = json!({
        "job_id": job.id.to_string(),
        "upload_url": format!("/api/uploads/{}", job.id),
    });
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

fn load_job(s: &AppState, raw: &str) -> Result<Job, ApiError> {
    let id: JobId = raw.parse().map_err(|_| not_found())?;
    s.store.get(&id).ok_or_else(not_found)
}

#[derive(Serialize)]
struct ErrorOut {
    kind: ErrorKind,
    message: String,
}

#[derive(Serialize)]
struct ResultOut {
    file_name: String,
    size: u64,
    dropped_count: usize,
}

#[derive(Serialize)]
struct JobOut {
    state: JobState,
    position: Option<usize>,
    elapsed_s: i64,
    estimate_s: u64,
    expires_at: Option<String>,
    error: Option<ErrorOut>,
    result: Option<ResultOut>,
    report_code: Option<String>,
    /// The job id is the capability: its holder sees the request it made, to resume an upload.
    name: String,
    size: u64,
    kind: FileKind,
    year: u16,
    target: u16,
}

async fn get_job(
    State(s): State<AppState>,
    Path(raw): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let job = load_job(&s, &raw)?;
    if job.state == JobState::Expired {
        return Err(expired());
    }
    let position = match job.state {
        JobState::Queued | JobState::Running => s.store.position(&job.id),
        _ => None,
    };
    // Elapsed time stops growing once the job has finished.
    let elapsed_s = job.started_at.map_or(0, |start| {
        let end = job.finished_at.unwrap_or_else(|| s.clock.now());
        (end - start).num_seconds().max(0)
    });
    let steps = u32::from(job.year.saturating_sub(job.target));
    let result = if job.state == JobState::Done {
        let size = tokio::fs::metadata(s.store.output_path(&job))
            .await
            .map_or(0, |m| m.len());
        Some(ResultOut {
            file_name: download_name(&job),
            size,
            dropped_count: job.dropped.len(),
        })
    } else {
        None
    };
    Ok((
        [(header::CACHE_CONTROL, NO_STORE)],
        Json(JobOut {
            state: job.state,
            position,
            elapsed_s,
            estimate_s: estimate(job.size, steps).as_secs(),
            expires_at: job
                .expires_at
                .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true)),
            error: job.error.map(|e| ErrorOut {
                kind: e.kind,
                message: e.message,
            }),
            result,
            report_code: job.report_code.clone(),
            name: job.name.clone(),
            size: job.size,
            kind: job.kind,
            year: job.year,
            target: job.target,
        }),
    ))
}

/// `<stem>_R<target>.<ext>` where stem is the original name without its last extension.
fn download_name(job: &Job) -> String {
    // Keep the user's extension only when it is the job kind's own; otherwise append it.
    let stem = match job.name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.eq_ignore_ascii_case(job.kind.ext()) => {
            return format!("{stem}_R{}.{ext}", job.target);
        }
        _ => job.name.as_str(),
    };
    format!("{stem}_R{}.{}", job.target, job.kind.ext())
}

fn ascii_fallback(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// RFC 5987 percent-encoding: only `attr-char` bytes stay as they are.
fn percent_encode_5987(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn content_disposition(name: &str) -> String {
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        ascii_fallback(name),
        percent_encode_5987(name)
    )
}

/// Loads a finished job for download: 410 when expired, 409 until it is done.
fn done_job(s: &AppState, raw: &str) -> Result<Job, ApiError> {
    let job = load_job(s, raw)?;
    match job.state {
        JobState::Done => Ok(job),
        JobState::Expired => Err(expired()),
        _ => Err(ApiError::new(
            StatusCode::CONFLICT,
            "not_ready",
            "The result is not ready.",
        )),
    }
}

async fn download_file(
    State(s): State<AppState>,
    Path(raw): Path<String>,
) -> Result<Response, ApiError> {
    let job = done_job(&s, &raw)?;
    let file = tokio::fs::File::open(s.store.output_path(&job))
        .await
        .map_err(|_| expired())?;
    let len = file.metadata().await.map_err(|_| expired())?.len();
    let mut resp = Response::new(Body::from_stream(ReaderStream::new(file)));
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(&download_name(&job)))
            .expect("header value is ASCII"),
    );
    Ok(resp)
}

async fn dropped(
    State(s): State<AppState>,
    Path(raw): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let dropped: Vec<DroppedElement> = done_job(&s, &raw)?.dropped;
    Ok(([(header::CACHE_CONTROL, NO_STORE)], Json(dropped)))
}

async fn delete_job(
    State(s): State<AppState>,
    Path(raw): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id: JobId = raw.parse().map_err(|_| not_found())?;
    // The decision is made atomically by the store; a running job is removed by its worker
    // once the engine has let go of the files.
    match s.store.cancel(&id) {
        CancelOutcome::NotFound => return Err(not_found()),
        CancelOutcome::WasRunning => {}
        CancelOutcome::Other => s.store.remove(&id),
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Opt-in failure report: moves the failed job's input and log to the reports directory.
async fn report_job(
    State(s): State<AppState>,
    Path(raw): Path<String>,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    let consent =
        matches!(&body, Ok(Json(v)) if v.get("consent") == Some(&serde_json::Value::Bool(true)));
    if !consent {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "consent_required",
            "Sending a report needs your consent.",
        ));
    }
    let id: JobId = raw.parse().map_err(|_| not_found())?;
    // The claim is atomic: of two concurrent requests only one proceeds.
    let job = match s.store.claim_report(&id) {
        ReportClaim::Claimed(job) => *job,
        ReportClaim::NotFound => return Err(not_found()),
        ReportClaim::Expired => return Err(expired()),
        ReportClaim::NotFailed => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "not_failed",
                "Only a failed conversion can be reported.",
            ));
        }
        ReportClaim::AlreadyReported => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "already_reported",
                "This conversion was already reported.",
            ));
        }
    };
    let input = s.store.input_path(&job);
    let log = s.store.log_path(&job);
    let (reports, stats, version, now) = (
        s.reports.clone(),
        s.stats.clone(),
        s.engine_version.clone(),
        s.clock.now(),
    );
    let made = {
        let job = job.clone();
        tokio::task::spawn_blocking(move || {
            let made = reports.create(&job, &input, &log, &version, now);
            if made.is_ok() {
                stats.record_contribution();
                reports.enforce(now);
            }
            made
        })
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("report task panicked")))
    };
    let code = match made {
        Ok(code) => code,
        Err(e) => {
            s.store.release_report(&job.id);
            return Err(if e.kind() == std::io::ErrorKind::NotFound {
                expired()
            } else {
                internal()
            });
        }
    };
    if !s.store.set_report_code(&job.id, code.clone()) {
        // The job vanished while the report was being written; the report itself is kept.
        tracing::warn!("report written for a job that no longer exists");
    }
    Ok((StatusCode::CREATED, Json(json!({ "report_code": code }))).into_response())
}

fn internal() -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "Could not save the report.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> SocketAddr {
        "192.0.2.1:5000".parse().unwrap()
    }

    fn xff(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", v.parse().unwrap());
        h
    }

    #[test]
    fn forwarded_for_used_only_when_trusted() {
        let h = xff("203.0.113.9, 198.51.100.4");
        assert_eq!(client_ip(false, &[], &h, peer()), peer().ip());
        assert_eq!(
            client_ip(true, &[], &h, peer()),
            "198.51.100.4".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn forwarded_for_honored_only_from_listed_proxies() {
        let h = xff("203.0.113.9");
        let list: Vec<IpCidr> = vec!["192.0.2.0/24".parse().unwrap()];
        // The peer 192.0.2.1 is in the list: the header counts.
        assert_eq!(
            client_ip(true, &list, &h, peer()),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        // A peer outside the list cannot pick its own address.
        let other: SocketAddr = "198.51.100.7:5000".parse().unwrap();
        assert_eq!(client_ip(true, &list, &h, other), other.ip());
        // The list alone does not turn trust on.
        assert_eq!(client_ip(false, &list, &h, peer()), peer().ip());
    }

    #[test]
    fn forwarded_for_skips_invalid_and_falls_back() {
        assert_eq!(
            client_ip(true, &[], &xff("203.0.113.9, garbage"), peer()),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        assert_eq!(client_ip(true, &[], &xff("nope"), peer()), peer().ip());
        assert_eq!(client_ip(true, &[], &HeaderMap::new(), peer()), peer().ip());
        assert_eq!(
            client_ip(true, &[], &xff("2001:db8::1"), peer()),
            "2001:db8::1".parse::<IpAddr>().unwrap()
        );
    }

    fn job_with(name: &str) -> Job {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::jobs::JobStore::new(
            dir.path().to_path_buf(),
            std::sync::Arc::new(crate::clock::SystemClock),
        );
        let ip = "10.0.0.1".parse().unwrap();
        let new = NewJob {
            name: "x.rvt".into(),
            size: 10,
            kind: FileKind::Rvt,
            year: 2026,
            target: 2025,
        };
        let mut job = store.create(new, ip, &[(2026, 2025)]).unwrap();
        job.name = name.into();
        job
    }

    #[test]
    fn download_name_keeps_only_matching_extension() {
        assert_eq!(download_name(&job_with("House.rvt")), "House_R2025.rvt");
        assert_eq!(download_name(&job_with("House.RVT")), "House_R2025.RVT");
        assert_eq!(
            download_name(&job_with("my.project.v2")),
            "my.project.v2_R2025.rvt"
        );
    }

    #[test]
    fn content_disposition_encodes_unicode_name() {
        assert_eq!(
            content_disposition("Grüße \"naïve\" (v2)_R2025.rvt"),
            "attachment; filename=\"Gr__e _na_ve_ (v2)_R2025.rvt\"; filename*=UTF-8''Gr%C3%BC%C3%9Fe%20%22na%C3%AFve%22%20%28v2%29_R2025.rvt"
        );
    }
}

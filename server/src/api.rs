//! JSON API under `/api`: server info, job creation, status, downloads and deletion.

use crate::AppState;
use crate::jobs::{Job, JobId, NewJob};
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

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/jobs", post(create_job))
        .route("/api/jobs/{id}", get(get_job).delete(delete_job))
        .route("/api/jobs/{id}/file", get(download_file))
        .route("/api/jobs/{id}/dropped", get(dropped))
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
/// valid address in `X-Forwarded-For`.
pub fn client_ip(trust_proxy: bool, headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    if trust_proxy {
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
    let ip = client_ip(s.config.trust_proxy, &headers, peer);
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
}

async fn get_job(
    State(s): State<AppState>,
    Path(raw): Path<String>,
) -> Result<Json<JobOut>, ApiError> {
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
    Ok(Json(JobOut {
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
        report_code: job.report_code,
    }))
}

/// `<stem>_R<target>.<ext>` where stem is the original name without its last extension.
fn download_name(job: &Job) -> String {
    let (stem, ext) = match job.name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, ext),
        _ => (job.name.as_str(), job.kind.ext()),
    };
    format!("{stem}_R{}.{ext}", job.target)
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
) -> Result<Json<Vec<DroppedElement>>, ApiError> {
    Ok(Json(done_job(&s, &raw)?.dropped))
}

async fn delete_job(
    State(s): State<AppState>,
    Path(raw): Path<String>,
) -> Result<StatusCode, ApiError> {
    let job = load_job(&s, &raw)?;
    s.store.cancel(&job.id);
    // A running job is removed by its worker once the engine has let go of the files.
    if job.state != JobState::Running {
        s.store.remove(&job.id);
    }
    Ok(StatusCode::NO_CONTENT)
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
        assert_eq!(client_ip(false, &h, peer()), peer().ip());
        assert_eq!(
            client_ip(true, &h, peer()),
            "198.51.100.4".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn forwarded_for_skips_invalid_and_falls_back() {
        assert_eq!(
            client_ip(true, &xff("203.0.113.9, garbage"), peer()),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        assert_eq!(client_ip(true, &xff("nope"), peer()), peer().ip());
        assert_eq!(client_ip(true, &HeaderMap::new(), peer()), peer().ip());
        assert_eq!(
            client_ip(true, &xff("2001:db8::1"), peer()),
            "2001:db8::1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn content_disposition_encodes_unicode_name() {
        assert_eq!(
            content_disposition("Dự án \"cuối\" (v2)_R2025.rvt"),
            "attachment; filename=\"D_ _n _cu_i_ (v2)_R2025.rvt\"; filename*=UTF-8''D%E1%BB%B1%20%C3%A1n%20%22cu%E1%BB%91i%22%20%28v2%29_R2025.rvt"
        );
    }
}

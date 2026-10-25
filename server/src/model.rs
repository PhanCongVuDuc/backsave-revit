//! Types shared across the server: job states, file kinds, errors, time budgets.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    WaitingUpload,
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Rvt,
    Rte,
    Rfa,
    Rft,
}

impl FileKind {
    /// Detects the kind from the file extension (case-insensitive).
    pub fn from_name(name: &str) -> Option<FileKind> {
        let (_, ext) = name.rsplit_once('.')?;
        match ext.to_ascii_lowercase().as_str() {
            "rvt" => Some(FileKind::Rvt),
            "rte" => Some(FileKind::Rte),
            "rfa" => Some(FileKind::Rfa),
            "rft" => Some(FileKind::Rft),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            FileKind::Rvt => "rvt",
            FileKind::Rte => "rte",
            FileKind::Rfa => "rfa",
            FileKind::Rft => "rft",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FileKind::Rvt => "Project",
            FileKind::Rte => "Project template",
            FileKind::Rfa => "Family",
            FileKind::Rft => "Family template",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    NoRule,
    RuleBroken,
    Cascade,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DroppedElement {
    pub element_id: u64,
    pub class: String,
    pub reason: DropReason,
    pub cascade_from: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Refused,
    Corrupt,
    Failed,
    Crashed,
    TimedOut,
    Mismatch,
    DiskFull,
}

/// An API error rendered as `{"error":{"code","message","retry_after_s"}}`.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub retry_after_s: Option<u64>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after_s: None,
        }
    }

    pub fn retry_after(mut self, secs: u64) -> Self {
        self.retry_after_s = Some(secs);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "retry_after_s": self.retry_after_s,
            }
        });
        let mut resp = (self.status, Json(body)).into_response();
        if let Some(s) = self.retry_after_s {
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(s));
        }
        resp
    }
}

fn mib_ceil(bytes: u64) -> u64 {
    bytes.div_ceil(MIB)
}

/// Hard limit for one conversion: 300 s + 4 s x ceil(MiB) x steps.
pub fn time_limit(bytes: u64, steps: u32) -> Duration {
    Duration::from_secs(300 + 4 * mib_ceil(bytes) * u64::from(steps))
}

/// Estimate shown to the user: 1 s x ceil(MiB) x steps.
pub fn estimate(bytes: u64, steps: u32) -> Duration {
    Duration::from_secs(mib_ceil(bytes) * u64::from(steps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn time_limit_rounds_mib_up() {
        assert_eq!(
            time_limit(149 * MIB, 2),
            Duration::from_secs(300 + 4 * 149 * 2)
        );
        assert_eq!(
            time_limit(149 * MIB + 1, 2),
            Duration::from_secs(300 + 4 * 150 * 2)
        );
        assert_eq!(estimate(149 * MIB, 2), Duration::from_secs(298));
        assert_eq!(estimate(1, 1), Duration::from_secs(1));
    }

    #[test]
    fn file_kind_from_name() {
        assert_eq!(FileKind::from_name("A.RVT"), Some(FileKind::Rvt));
        assert_eq!(FileKind::from_name("b.rft"), Some(FileKind::Rft));
        assert_eq!(FileKind::from_name("c.txt"), None);
        assert_eq!(FileKind::from_name("noext"), None);
        assert_eq!(FileKind::Rte.ext(), "rte");
        assert_eq!(FileKind::Rfa.label(), "Family");
        assert_eq!(FileKind::Rvt.label(), "Project");
        assert_eq!(FileKind::Rte.label(), "Project template");
        assert_eq!(FileKind::Rft.label(), "Family template");
    }

    #[test]
    fn enums_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&JobState::WaitingUpload).unwrap(),
            "\"waiting_upload\""
        );
        assert_eq!(
            serde_json::to_string(&ErrorKind::TimedOut).unwrap(),
            "\"timed_out\""
        );
        assert_eq!(
            serde_json::to_string(&DropReason::NoRule).unwrap(),
            "\"no_rule\""
        );
        assert_eq!(serde_json::to_string(&FileKind::Rvt).unwrap(), "\"rvt\"");
        let d = DroppedElement {
            element_id: 7,
            class: "Wall".into(),
            reason: DropReason::Cascade,
            cascade_from: Some(3),
        };
        assert_eq!(
            serde_json::to_value(&d).unwrap(),
            serde_json::json!({"element_id":7,"class":"Wall","reason":"cascade","cascade_from":3})
        );
    }

    #[tokio::test]
    async fn api_error_json_shape() {
        let r = ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "Slow down")
            .retry_after(42)
            .into_response();
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(r.headers()["retry-after"], "42");
        let body = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"error":{"code":"rate_limited","message":"Slow down","retry_after_s":42}})
        );

        let r = ApiError::new(StatusCode::NOT_FOUND, "not_found", "x").into_response();
        assert!(r.headers().get("retry-after").is_none());
        let body = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["retry_after_s"], serde_json::Value::Null);
    }
}

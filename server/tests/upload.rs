mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use backsave_server::AppState;
use backsave_server::clock::ManualClock;
use backsave_server::jobs::{Job, NewJob};
use backsave_server::model::{ErrorKind, FileKind, JobState};
use backsave_server::upload;
use std::net::IpAddr;
use std::sync::Arc;
use tower::ServiceExt;

const OFFSET_CT: &str = "application/offset+octet-stream";

struct Fixture {
    state: AppState,
    app: Router,
    _dir: tempfile::TempDir,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
    ));
    let state = common::test_state(dir.path(), clock);
    let app = Router::new()
        .merge(upload::routes())
        .with_state(state.clone());
    Fixture {
        state,
        app,
        _dir: dir,
    }
}

fn make_job(f: &Fixture, size: u64) -> Job {
    let req = NewJob {
        name: "House.rvt".into(),
        size,
        kind: FileKind::Rvt,
        year: 2026,
        target: 2024,
    };
    f.state
        .store
        .create(req, IpAddr::from([10, 0, 0, 1]), &f.state.pairs)
        .unwrap()
}

fn patch(
    id: &str,
    offset: u64,
    ct: Option<&str>,
    tus: Option<&str>,
    body: Vec<u8>,
) -> Request<Body> {
    let mut b = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/uploads/{id}"));
    if let Some(ct) = ct {
        b = b.header("content-type", ct);
    }
    if let Some(t) = tus {
        b = b.header("tus-resumable", t);
    }
    b.header("upload-offset", offset.to_string())
        .body(Body::from(body))
        .unwrap()
}

fn good_patch(id: &str, offset: u64, body: Vec<u8>) -> Request<Body> {
    patch(id, offset, Some(OFFSET_CT), Some("1.0.0"), body)
}

fn simple(method: Method, id: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(format!("/api/uploads/{id}"))
        .body(Body::empty())
        .unwrap()
}

fn hdr(resp: &axum::response::Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn options_advertises_tus() {
    let f = setup();
    let id = make_job(&f, 10).id.to_string();
    let resp = f
        .app
        .clone()
        .oneshot(simple(Method::OPTIONS, &id))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(hdr(&resp, "tus-resumable").as_deref(), Some("1.0.0"));
    assert_eq!(hdr(&resp, "tus-version").as_deref(), Some("1.0.0"));
    assert_eq!(hdr(&resp, "tus-max-size").as_deref(), Some("1073741824"));
    assert_eq!(hdr(&resp, "tus-extension").as_deref(), Some(""));
}

#[tokio::test]
async fn patch_two_chunks_then_queued() {
    let f = setup();
    let job = make_job(&f, 10);
    let id = job.id.to_string();
    let r1 = f
        .app
        .clone()
        .oneshot(good_patch(&id, 0, b"hello".to_vec()))
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::NO_CONTENT);
    assert_eq!(hdr(&r1, "upload-offset").as_deref(), Some("5"));
    assert_eq!(hdr(&r1, "tus-resumable").as_deref(), Some("1.0.0"));
    assert_eq!(
        f.state.store.get(&job.id).unwrap().state,
        JobState::WaitingUpload
    );
    let r2 = f
        .app
        .clone()
        .oneshot(good_patch(&id, 5, b"world".to_vec()))
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::NO_CONTENT);
    assert_eq!(hdr(&r2, "upload-offset").as_deref(), Some("10"));
    let after = f.state.store.get(&job.id).unwrap();
    assert_eq!(after.state, JobState::Queued);
    assert_eq!(
        std::fs::read(f.state.store.input_path(&after)).unwrap(),
        b"helloworld"
    );
    // Further data is refused once the upload is complete.
    let r3 = f
        .app
        .clone()
        .oneshot(good_patch(&id, 10, b"x".to_vec()))
        .await
        .unwrap();
    assert_eq!(r3.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn head_reports_offset_after_partial() {
    let f = setup();
    let id = make_job(&f, 10).id.to_string();
    let r = f
        .app
        .clone()
        .oneshot(simple(Method::HEAD, &id))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(hdr(&r, "upload-offset").as_deref(), Some("0"));
    f.app
        .clone()
        .oneshot(good_patch(&id, 0, b"abc".to_vec()))
        .await
        .unwrap();
    let r = f
        .app
        .clone()
        .oneshot(simple(Method::HEAD, &id))
        .await
        .unwrap();
    assert_eq!(hdr(&r, "upload-offset").as_deref(), Some("3"));
    assert_eq!(hdr(&r, "upload-length").as_deref(), Some("10"));
    assert_eq!(hdr(&r, "cache-control").as_deref(), Some("no-store"));
    assert_eq!(hdr(&r, "tus-resumable").as_deref(), Some("1.0.0"));
}

#[tokio::test]
async fn patch_wrong_offset_is_409() {
    let f = setup();
    let id = make_job(&f, 10).id.to_string();
    let r = f
        .app
        .clone()
        .oneshot(good_patch(&id, 4, b"abc".to_vec()))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    assert_eq!(hdr(&r, "tus-resumable").as_deref(), Some("1.0.0"));
}

#[tokio::test]
async fn patch_past_length_is_413() {
    let f = setup();
    let job = make_job(&f, 4);
    let id = job.id.to_string();
    let r = f
        .app
        .clone()
        .oneshot(good_patch(&id, 0, b"too long".to_vec()))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let j = f.state.store.get(&job.id).unwrap();
    assert!(j.received <= j.size);
    assert_eq!(j.state, JobState::WaitingUpload);
}

#[tokio::test]
async fn patch_wrong_content_type_is_415() {
    let f = setup();
    let id = make_job(&f, 10).id.to_string();
    let wrong = patch(
        &id,
        0,
        Some("application/octet-stream"),
        Some("1.0.0"),
        b"abc".to_vec(),
    );
    assert_eq!(
        f.app.clone().oneshot(wrong).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let missing = patch(&id, 0, None, Some("1.0.0"), b"abc".to_vec());
    assert_eq!(
        f.app.clone().oneshot(missing).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn patch_bad_tus_version_is_412() {
    let f = setup();
    let id = make_job(&f, 10).id.to_string();
    let none = patch(&id, 0, Some(OFFSET_CT), None, b"a".to_vec());
    assert_eq!(
        f.app.clone().oneshot(none).await.unwrap().status(),
        StatusCode::PRECONDITION_FAILED
    );
    let old = patch(&id, 0, Some(OFFSET_CT), Some("0.2.2"), b"a".to_vec());
    assert_eq!(
        f.app.clone().oneshot(old).await.unwrap().status(),
        StatusCode::PRECONDITION_FAILED
    );
}

#[tokio::test]
async fn unknown_or_malformed_upload_is_404() {
    let f = setup();
    let unknown = "0123456789abcdef0123456789abcdef";
    let r = f
        .app
        .clone()
        .oneshot(simple(Method::HEAD, unknown))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert_eq!(hdr(&r, "tus-resumable").as_deref(), Some("1.0.0"));
    let r = f
        .app
        .clone()
        .oneshot(good_patch(unknown, 0, b"a".to_vec()))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = f
        .app
        .clone()
        .oneshot(simple(Method::HEAD, "..%2F..%2Fetc"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_unknown_upload_is_404() {
    let f = setup();
    let r = f
        .app
        .clone()
        .oneshot(simple(Method::HEAD, "ffffffffffffffffffffffffffffffff"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn patch_to_closed_upload_is_409() {
    let f = setup();
    let job = make_job(&f, 10);
    f.state.store.cancel(&job.id);
    let r = f
        .app
        .clone()
        .oneshot(good_patch(&job.id.to_string(), 0, b"a".to_vec()))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn concurrent_patch_for_same_upload_is_409_busy() {
    let f = setup();
    let id = make_job(&f, 10).id.to_string();
    // A body that stays open keeps the first request inside the handler.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    tx.send(Ok(b"abc".to_vec())).await.unwrap();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/uploads/{id}"))
        .header("content-type", OFFSET_CT)
        .header("tus-resumable", "1.0.0")
        .header("upload-offset", "0")
        .body(Body::from_stream(stream))
        .unwrap();
    let app = f.app.clone();
    let first = tokio::spawn(async move { app.oneshot(req).await.unwrap() });
    // Wait until the first chunk has landed, then race a second request.
    for _ in 0..100 {
        let r = f
            .app
            .clone()
            .oneshot(simple(Method::HEAD, &id))
            .await
            .unwrap();
        if hdr(&r, "upload-offset").as_deref() == Some("3") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let second = f
        .app
        .clone()
        .oneshot(good_patch(&id, 3, b"def".to_vec()))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    drop(tx);
    let first = first.await.unwrap();
    assert_eq!(first.status(), StatusCode::NO_CONTENT);
    assert_eq!(hdr(&first, "upload-offset").as_deref(), Some("3"));
}

#[tokio::test]
async fn patch_write_error_returns_507_and_removes_partial() {
    let f = setup();
    let job = make_job(&f, 10);
    let id = job.id.to_string();
    // A directory at the input path makes opening the file for writing fail.
    let input = f.state.store.input_path(&job);
    std::fs::create_dir_all(&input).unwrap();
    let r = f
        .app
        .clone()
        .oneshot(good_patch(&id, 0, b"abc".to_vec()))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(hdr(&r, "tus-resumable").as_deref(), Some("1.0.0"));
    let after = f.state.store.get(&job.id).unwrap();
    assert_eq!(after.state, JobState::Failed);
    assert_eq!(after.error.unwrap().kind, ErrorKind::DiskFull);
    assert!(!input.is_file());
}

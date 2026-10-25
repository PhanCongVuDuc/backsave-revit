mod common;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use backsave_server::cleanup::{sweep, wipe_jobs_dir};
use backsave_server::clock::ManualClock;
use backsave_server::jobs::{Job, NewJob};
use backsave_server::model::{ErrorKind, FileKind, JobState};
use backsave_server::{AppState, app};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    clock: Arc<ManualClock>,
    app: Router,
    dir: tempfile::TempDir,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("reports")).unwrap();
    let clock = Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
    ));
    let state = common::test_state(dir.path(), clock.clone());
    let app = app(state.clone());
    Fixture {
        state,
        clock,
        app,
        dir,
    }
}

fn run_sweep(f: &Fixture) {
    sweep(&f.state.store, &f.state.reports, f.state.clock.now());
}

fn new_job(f: &Fixture, last: u8) -> Job {
    let req = NewJob {
        name: "Secret House.rvt".into(),
        size: 2048,
        kind: FileKind::Rvt,
        year: 2026,
        target: 2025,
    };
    f.state
        .store
        .create(
            req,
            std::net::IpAddr::from([10, 0, 0, last]),
            &f.state.pairs,
        )
        .unwrap()
}

/// A job that is queued with its input on disk.
fn queued_job(f: &Fixture, last: u8) -> Job {
    let job = new_job(f, last);
    std::fs::write(f.state.store.input_path(&job), b"input bytes").unwrap();
    f.state.store.mark_uploaded(&job.id);
    job
}

/// A job that ran and failed: input and log are on disk.
fn failed_job(f: &Fixture, last: u8) -> Job {
    let job = queued_job(f, last);
    let running = f.state.store.take_next().unwrap();
    assert_eq!(running.id, job.id);
    std::fs::write(f.state.store.log_path(&job), b"{\"type\":\"run\"}\n").unwrap();
    f.state.store.finish(
        &job.id,
        Err((ErrorKind::Failed, "The conversion failed.".into())),
    );
    f.state.store.get(&job.id).unwrap()
}

fn done_job(f: &Fixture, last: u8) -> Job {
    let job = queued_job(f, last);
    f.state.store.take_next().unwrap();
    std::fs::remove_file(f.state.store.input_path(&job)).unwrap();
    std::fs::write(f.state.store.output_path(&job), b"output bytes").unwrap();
    f.state.store.finish(&job.id, Ok(Vec::new()));
    f.state.store.get(&job.id).unwrap()
}

fn advance(f: &Fixture, secs: u64) {
    f.clock.advance(Duration::from_secs(secs));
}

fn post_report(id: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/jobs/{id}/report"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn send(f: &Fixture, req: Request<Body>) -> axum::response::Response {
    let mut req = req;
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 4000))));
    f.app.clone().oneshot(req).await.unwrap()
}

async fn body_json(resp: axum::response::Response) -> Value {
    let b = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&b).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn report_dirs(f: &Fixture) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(f.dir.path().join("reports"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect()
}

#[test]
fn waiting_upload_removed_after_1h() {
    let f = setup();
    let job = new_job(&f, 1);
    advance(&f, 3600);
    run_sweep(&f);
    assert!(
        f.state.store.get(&job.id).is_some(),
        "exactly 1 h is not over"
    );
    advance(&f, 1);
    run_sweep(&f);
    assert!(f.state.store.get(&job.id).is_none());
    assert!(!f.state.store.job_dir(&job).exists());
}

#[tokio::test]
async fn done_output_removed_after_1h_then_410() {
    let f = setup();
    let job = done_job(&f, 1);
    let id = job.id.to_string();
    advance(&f, 3600);
    run_sweep(&f);
    assert_eq!(f.state.store.get(&job.id).unwrap().state, JobState::Done);
    advance(&f, 1);
    run_sweep(&f);
    assert_eq!(f.state.store.get(&job.id).unwrap().state, JobState::Expired);
    assert!(!f.state.store.job_dir(&job).exists());
    assert_eq!(
        send(&f, get(&format!("/api/jobs/{id}"))).await.status(),
        StatusCode::GONE
    );
    assert_eq!(
        send(&f, get(&format!("/api/jobs/{id}/file")))
            .await
            .status(),
        StatusCode::GONE
    );
    // The record stays for another 24 h, then goes.
    advance(&f, 24 * 3600);
    run_sweep(&f);
    assert!(f.state.store.get(&job.id).is_some());
    advance(&f, 1);
    run_sweep(&f);
    assert!(f.state.store.get(&job.id).is_none());
}

#[test]
fn failed_input_removed_after_15_min_without_consent() {
    let f = setup();
    let job = failed_job(&f, 1);
    advance(&f, 15 * 60);
    run_sweep(&f);
    assert!(f.state.store.input_path(&job).exists());
    advance(&f, 1);
    run_sweep(&f);
    assert_eq!(f.state.store.get(&job.id).unwrap().state, JobState::Expired);
    assert!(!f.state.store.job_dir(&job).exists());
}

#[test]
fn cancelled_stray_job_removed() {
    let f = setup();
    let job = new_job(&f, 1);
    f.state.store.cancel(&job.id);
    run_sweep(&f);
    assert!(f.state.store.get(&job.id).is_none());
    assert!(!f.state.store.job_dir(&job).exists());
}

#[test]
fn queued_and_running_untouched_by_sweep() {
    let f = setup();
    let running = queued_job(&f, 1);
    f.state.store.take_next().unwrap();
    let queued = queued_job(&f, 2);
    advance(&f, 30 * 24 * 3600);
    run_sweep(&f);
    assert_eq!(
        f.state.store.get(&running.id).unwrap().state,
        JobState::Running
    );
    assert_eq!(
        f.state.store.get(&queued.id).unwrap().state,
        JobState::Queued
    );
    assert!(f.state.store.input_path(&running).exists());
    assert!(f.state.store.input_path(&queued).exists());
}

#[tokio::test]
async fn consent_moves_input_to_reports_without_name() {
    let f = setup();
    let job = failed_job(&f, 1);
    let id = job.id.to_string();
    let resp = send(&f, post_report(&id, r#"{"consent":true}"#)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let code = body_json(resp).await["report_code"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(code.starts_with("R-") && code.len() == 8, "{code}");
    let dir = f.dir.path().join("reports").join(&code);
    assert_eq!(
        std::fs::read(dir.join("input.rvt")).unwrap(),
        b"input bytes"
    );
    assert!(dir.join("log.jsonl").is_file());
    assert!(!f.state.store.input_path(&job).exists());
    let meta: Value =
        serde_json::from_slice(&std::fs::read(dir.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["kind"], "rvt");
    assert_eq!(meta["error_kind"], "failed");
    assert_eq!(meta["size"], 11);
    assert!(meta["engine_version"].is_string());
    assert!(meta["failed_at"].is_string());
    // Sweeping later keeps the report even though the job expires.
    advance(&f, 16 * 60);
    run_sweep(&f);
    assert!(dir.join("input.rvt").is_file());
}

#[tokio::test]
async fn report_code_shown_in_job_json() {
    let f = setup();
    let id = failed_job(&f, 1).id.to_string();
    let v = body_json(send(&f, get(&format!("/api/jobs/{id}"))).await).await;
    assert_eq!(v["report_code"], Value::Null);
    let resp = send(&f, post_report(&id, r#"{"consent":true}"#)).await;
    let code = body_json(resp).await["report_code"].clone();
    let v = body_json(send(&f, get(&format!("/api/jobs/{id}"))).await).await;
    assert_eq!(v["report_code"], code);
}

#[tokio::test]
async fn report_on_done_job_is_409() {
    let f = setup();
    let id = done_job(&f, 1).id.to_string();
    let resp = send(&f, post_report(&id, r#"{"consent":true}"#)).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(resp).await["error"]["code"], "not_failed");
    assert!(report_dirs(&f).is_empty());
}

#[tokio::test]
async fn report_requires_exact_true() {
    let f = setup();
    let job = failed_job(&f, 1);
    let id = job.id.to_string();
    for body in [
        "{}",
        r#"{"consent":false}"#,
        r#"{"consent":"true"}"#,
        r#"{"consent":1}"#,
        r#"{"consent":null}"#,
        "not json",
        "",
    ] {
        let resp = send(&f, post_report(&id, body)).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body_json(resp).await["error"]["code"], "consent_required");
    }
    assert!(f.state.store.input_path(&job).exists());
    assert!(report_dirs(&f).is_empty());
}

#[tokio::test]
async fn report_twice_is_409() {
    let f = setup();
    let id = failed_job(&f, 1).id.to_string();
    let first = send(&f, post_report(&id, r#"{"consent":true}"#)).await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let second = send(&f, post_report(&id, r#"{"consent":true}"#)).await;
    assert_eq!(second.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(second).await["error"]["code"], "already_reported");
    assert_eq!(report_dirs(&f).len(), 1);
}

#[tokio::test]
async fn report_unknown_or_expired_job() {
    let f = setup();
    let resp = send(&f, post_report("zz", r#"{"consent":true}"#)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = send(&f, post_report(&"0".repeat(32), r#"{"consent":true}"#)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let job = failed_job(&f, 1);
    std::fs::remove_file(f.state.store.input_path(&job)).unwrap();
    let resp = send(&f, post_report(&job.id.to_string(), r#"{"consent":true}"#)).await;
    assert_eq!(resp.status(), StatusCode::GONE);
    assert_eq!(body_json(resp).await["error"]["code"], "expired");
}

#[test]
fn startup_wipes_jobs_dir() {
    let d = tempfile::tempdir().unwrap();
    let jobs = d.path().join("jobs");
    std::fs::create_dir_all(jobs.join("abc")).unwrap();
    std::fs::write(jobs.join("abc").join("in.rvt"), b"x").unwrap();
    std::fs::create_dir_all(d.path().join("reports").join("R-AAAAAA")).unwrap();
    wipe_jobs_dir(d.path()).unwrap();
    assert!(jobs.is_dir());
    assert_eq!(std::fs::read_dir(&jobs).unwrap().count(), 0);
    assert!(d.path().join("reports").join("R-AAAAAA").is_dir());
    // Also fine when nothing exists yet.
    let e = tempfile::tempdir().unwrap();
    wipe_jobs_dir(e.path()).unwrap();
    assert!(e.path().join("jobs").is_dir());
}

#[tokio::test]
async fn sweep_also_enforces_report_retention() {
    let f = setup();
    let id = failed_job(&f, 1).id.to_string();
    let resp = send(&f, post_report(&id, r#"{"consent":true}"#)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    advance(&f, 30 * 24 * 3600 + 1);
    run_sweep(&f);
    assert!(report_dirs(&f).is_empty());
}

#[test]
fn cancelled_running_job_left_for_worker() {
    let f = setup();
    let job = queued_job(&f, 1);
    f.state.store.take_next().unwrap();
    f.state.store.cancel(&job.id);
    assert_eq!(
        f.state.store.get(&job.id).unwrap().state,
        JobState::Cancelled
    );
    advance(&f, 24 * 3600);
    run_sweep(&f);
    assert!(f.state.store.get(&job.id).is_some());
    assert!(f.state.store.input_path(&job).exists());
}

#[test]
fn orphan_dir_removed_on_next_sweep() {
    let f = setup();
    let jobs = f.dir.path().join("jobs");
    let unknown = jobs.join("0".repeat(32));
    std::fs::create_dir_all(&unknown).unwrap();
    std::fs::write(unknown.join("in.rvt"), b"x").unwrap();
    let junk = jobs.join("junk");
    std::fs::create_dir_all(&junk).unwrap();
    std::fs::write(jobs.join("stray.txt"), b"x").unwrap();
    run_sweep(&f);
    assert!(!unknown.exists());
    assert!(!junk.exists());
    assert!(!jobs.join("stray.txt").exists());
}

#[test]
fn expired_job_dir_retried() {
    let f = setup();
    let job = done_job(&f, 1);
    advance(&f, 3601);
    run_sweep(&f);
    assert_eq!(f.state.store.get(&job.id).unwrap().state, JobState::Expired);
    // Pretend the first delete failed: the directory is back.
    let dir = f.state.store.job_dir(&job);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("out.rvt"), b"left over").unwrap();
    run_sweep(&f);
    assert!(!dir.exists());
    assert!(f.state.store.get(&job.id).is_some(), "record kept for 410");
}

#[test]
fn live_job_dirs_untouched() {
    let f = setup();
    let failed = failed_job(&f, 3);
    let done = done_job(&f, 4);
    let waiting = new_job(&f, 1);
    let queued = queued_job(&f, 2);
    run_sweep(&f);
    for job in [&waiting, &queued, &failed, &done] {
        assert!(f.state.store.job_dir(job).is_dir(), "{:?}", job.state);
    }
}

#[tokio::test]
async fn report_on_expired_job_is_410() {
    let f = setup();
    let job = failed_job(&f, 1);
    advance(&f, 16 * 60);
    run_sweep(&f);
    assert_eq!(f.state.store.get(&job.id).unwrap().state, JobState::Expired);
    let resp = send(&f, post_report(&job.id.to_string(), r#"{"consent":true}"#)).await;
    assert_eq!(resp.status(), StatusCode::GONE);
    assert_eq!(body_json(resp).await["error"]["code"], "expired");
    assert!(report_dirs(&f).is_empty());
}

#[tokio::test]
async fn concurrent_reports_make_one_report() {
    let f = setup();
    let id = failed_job(&f, 1).id.to_string();
    let (a, b) = tokio::join!(
        send(&f, post_report(&id, r#"{"consent":true}"#)),
        send(&f, post_report(&id, r#"{"consent":true}"#)),
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    assert_eq!(report_dirs(&f).len(), 1);
}

#[test]
fn slow_upload_survives_while_data_arrives() {
    let f = setup();
    let job = new_job(&f, 1);
    for _ in 0..3 {
        advance(&f, 50 * 60);
        f.state.store.add_received(&job.id, 1);
        run_sweep(&f);
        assert!(f.state.store.get(&job.id).is_some());
    }
    advance(&f, 3601);
    run_sweep(&f);
    assert!(f.state.store.get(&job.id).is_none());
}

#[test]
fn sweep_prunes_old_client_history() {
    let f = setup();
    let job = new_job(&f, 1);
    f.state.store.remove(&job.id);
    assert_eq!(f.state.store.history_len(), 1);
    advance(&f, 3600);
    run_sweep(&f);
    assert_eq!(f.state.store.history_len(), 0);
}

#[tokio::test]
async fn expired_job_forgets_its_name() {
    let f = setup();
    let job = done_job(&f, 1);
    advance(&f, 3601);
    run_sweep(&f);
    let after = f.state.store.get(&job.id).unwrap();
    assert_eq!(after.state, JobState::Expired);
    assert_eq!(after.name, "");
}

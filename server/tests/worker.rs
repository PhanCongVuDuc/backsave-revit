mod common;

use backsave_server::AppState;
use backsave_server::clock::ManualClock;
use backsave_server::engine::Engine;
use backsave_server::jobs::{Job, JobId, NewJob};
use backsave_server::model::{ErrorKind, FileKind, JobState};
use backsave_server::worker::spawn_workers;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

struct Fixture {
    state: AppState,
    _dir: tempfile::TempDir,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
    ));
    Fixture {
        state: common::test_state(dir.path(), clock),
        _dir: dir,
    }
}

/// Creates a job for `ip_last` and simulates the upload: writes the marker file as the input.
fn queue_job_with(
    f: &Fixture,
    pairs: &[(u16, u16)],
    ip_last: u8,
    declared_year: u16,
    target: u16,
    mode: &str,
    content_year: u32,
) -> Job {
    let job = f
        .state
        .store
        .create(
            NewJob {
                name: "House.rvt".into(),
                size: 1024,
                kind: FileKind::Rvt,
                year: declared_year,
                target,
            },
            IpAddr::from([10, 0, 0, ip_last]),
            pairs,
        )
        .unwrap();
    common::write_marker(
        &f.state.store.job_dir(&job),
        "rvt",
        mode,
        Some(content_year),
    );
    f.state.store.mark_uploaded(&job.id);
    job
}

async fn wait_terminal(f: &Fixture, id: &JobId) -> Job {
    for _ in 0..400 {
        match f.state.store.get(id) {
            Some(j)
                if !matches!(
                    j.state,
                    JobState::WaitingUpload | JobState::Queued | JobState::Running
                ) =>
            {
                return j;
            }
            _ => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    panic!("job did not reach a terminal state");
}

fn queue_job(
    f: &Fixture,
    ip_last: u8,
    declared_year: u16,
    target: u16,
    mode: &str,
    content_year: u32,
) -> Job {
    let pairs = f.state.pairs.clone();
    queue_job_with(
        f,
        &pairs,
        ip_last,
        declared_year,
        target,
        mode,
        content_year,
    )
}

fn start(f: &Fixture, n: usize) {
    spawn_workers(n, f.state.store.clone(), f.state.engine.clone());
}

#[tokio::test]
async fn ok_job_done_input_deleted_output_kept() {
    let f = setup();
    start(&f, 1);
    let job = queue_job(&f, 1, 2026, 2024, "ok", 2026);
    let done = wait_terminal(&f, &job.id).await;
    assert_eq!(done.state, JobState::Done);
    assert_eq!(
        done.expires_at.unwrap() - done.finished_at.unwrap(),
        chrono::Duration::hours(1)
    );
    assert!(!f.state.store.input_path(&job).exists());
    assert!(f.state.store.output_path(&job).exists());
    assert!(f.state.store.log_path(&job).exists());
}

#[tokio::test]
async fn failed_job_keeps_input_15_min() {
    let f = setup();
    start(&f, 1);
    let job = queue_job(&f, 1, 2026, 2024, "fail", 2026);
    let failed = wait_terminal(&f, &job.id).await;
    assert_eq!(failed.state, JobState::Failed);
    let err = failed.error.unwrap();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(err.message, "The conversion failed.");
    assert!(f.state.store.input_path(&job).exists());
    assert!(!f.state.store.output_path(&job).exists());
    let left = failed.expires_at.unwrap() - failed.finished_at.unwrap();
    assert_eq!(left.num_minutes(), 15);
}

#[tokio::test]
async fn mismatch_when_probe_year_differs() {
    let f = setup();
    start(&f, 1);
    let job = queue_job(&f, 1, 2025, 2024, "ok", 2026);
    let failed = wait_terminal(&f, &job.id).await;
    assert_eq!(failed.state, JobState::Failed);
    let err = failed.error.unwrap();
    assert_eq!(err.kind, ErrorKind::Mismatch);
    assert_eq!(err.message, "The file is not what the browser reported.");
    assert!(f.state.store.input_path(&job).exists());
}

#[tokio::test]
async fn refused_message_comes_from_engine() {
    let f = setup();
    start(&f, 1);
    let job = queue_job(&f, 1, 2026, 2024, "refuse", 2026);
    let failed = wait_terminal(&f, &job.id).await;
    let err = failed.error.unwrap();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(!err.message.is_empty());
    assert_ne!(err.message, "The conversion failed.");
}

#[tokio::test]
async fn target_not_offered_is_refused() {
    let f = setup();
    start(&f, 1);
    // The store accepts 2026 -> 2022, but the engine does not offer 2022 for this file.
    let job = queue_job_with(&f, &[(2026, 2022)], 1, 2026, 2022, "ok", 2026);
    let failed = wait_terminal(&f, &job.id).await;
    let err = failed.error.unwrap();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(
        err.message,
        "This conversion is not supported for this file."
    );
    assert!(!f.state.store.output_path(&job).exists());
}

#[tokio::test]
async fn cancel_running_job_kills_and_deletes() {
    let f = setup();
    start(&f, 1);
    let job = queue_job(&f, 1, 2026, 2024, "hang", 2026);
    let pid_file = f.state.store.output_path(&job).with_extension("rvt.pid");
    let pid = common::wait_for_pid(&pid_file).await;
    f.state.store.cancel(&job.id);
    for _ in 0..200 {
        if f.state.store.get(&job.id).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(f.state.store.get(&job.id).is_none());
    assert!(!f.state.store.job_dir(&job).exists());
    assert!(!common::process_alive(pid));
}

#[tokio::test]
async fn cancel_queued_job_never_runs() {
    let f = setup();
    // Token cancelled but state left Queued: a worker that takes it must drop it without running the engine.
    let job = queue_job(&f, 1, 2026, 2024, "ok", 2026);
    job.cancel.cancel();
    // The engine path does not exist, so any attempt to start it would fail the job instead of removing it.
    let missing = Arc::new(Engine::new(f._dir.path().join("no-such-engine")));
    spawn_workers(1, f.state.store.clone(), missing);
    for _ in 0..200 {
        if f.state.store.get(&job.id).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(f.state.store.get(&job.id).is_none());
    assert!(!f.state.store.job_dir(&job).exists());
}

#[tokio::test]
async fn two_workers_run_two_jobs() {
    let f = setup();
    let a = queue_job(&f, 1, 2026, 2024, "hang", 2026);
    let b = queue_job(&f, 2, 2026, 2024, "hang", 2026);
    start(&f, 2);
    // Both hang, so both only get going if two workers run concurrently.
    common::wait_for_pid(&f.state.store.output_path(&a).with_extension("rvt.pid")).await;
    common::wait_for_pid(&f.state.store.output_path(&b).with_extension("rvt.pid")).await;
    assert_eq!(f.state.store.get(&a.id).unwrap().state, JobState::Running);
    assert_eq!(f.state.store.get(&b.id).unwrap().state, JobState::Running);
    f.state.store.cancel(&a.id);
    f.state.store.cancel(&b.id);
}

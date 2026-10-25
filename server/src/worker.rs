//! Background workers that run queued jobs through the conversion engine.

use crate::engine::{DowngradeRequest, Engine, EngineError, public_message};
use crate::jobs::{Job, JobStore};
use crate::model::{ErrorKind, time_limit};
use std::path::Path;
use std::sync::Arc;
use tokio::task::JoinHandle;

const MIB: u64 = 1 << 20;

/// Starts `n` workers; each takes queued jobs one at a time, forever.
pub fn spawn_workers(n: usize, store: Arc<JobStore>, engine: Arc<Engine>) -> Vec<JoinHandle<()>> {
    (0..n)
        .map(|_| {
            let store = store.clone();
            let engine = engine.clone();
            tokio::spawn(async move {
                loop {
                    let job = store.next_job().await;
                    let for_panic = job.clone();
                    // Run each job in its own task so a panic cannot end this loop.
                    let run = tokio::spawn(run_job(store.clone(), engine.clone(), job));
                    if run.await.is_err() {
                        fail(
                            &store,
                            &for_panic,
                            ErrorKind::Crashed,
                            "The conversion engine stopped unexpectedly.".into(),
                            None,
                        );
                    }
                }
            })
        })
        .collect()
}

async fn run_job(store: Arc<JobStore>, engine: Arc<Engine>, job: Job) {
    run_job_inner(&store, &engine, &job).await;
    // A job cancelled while running is removed here, once the engine has released its files,
    // whatever the engine reported.
    if job.cancel.is_cancelled() {
        store.remove(&job.id);
    }
}

async fn run_job_inner(store: &JobStore, engine: &Engine, job: &Job) {
    if job.cancel.is_cancelled() {
        return;
    }
    let input = store.input_path(job);
    let output = store.output_path(job);
    let log = store.log_path(job);

    let probe = match engine.probe(&input, job.cancel.clone()).await {
        Ok(p) => p,
        Err(e) => return finish_error(store, job, e),
    };
    if probe.kind != job.kind || probe.year != job.year {
        return fail(
            store,
            job,
            ErrorKind::Mismatch,
            "The file is not what the browser reported.".into(),
            None,
        );
    }
    if !probe.targets.contains(&job.target) {
        return fail(
            store,
            job,
            ErrorKind::Refused,
            "This conversion is not supported for this file.".into(),
            None,
        );
    }

    let req = DowngradeRequest {
        input: input.clone(),
        output: output.clone(),
        log,
        target: job.target,
        limit: time_limit(job.size, steps(job)),
    };
    match engine.downgrade(req, job.cancel.clone()).await {
        Ok(dropped) => {
            // An engine that reports success must have left a non-empty output file.
            let size = tokio::fs::metadata(&output).await.map_or(0, |m| m.len());
            if size == 0 {
                return fail(
                    store,
                    job,
                    ErrorKind::Failed,
                    "The conversion failed.".into(),
                    Some(0),
                );
            }
            remove_file(&input);
            store.finish_done(&job.id, dropped, size);
        }
        Err(e) => {
            finish_error(store, job, e);
        }
    }
}

fn steps(job: &Job) -> u32 {
    u32::from(job.year.saturating_sub(job.target))
}

fn finish_error(store: &JobStore, job: &Job, e: EngineError) {
    let Some(kind) = e.kind() else {
        store.remove(&job.id);
        return;
    };
    let exit_code = e.exit_code();
    let message = match e {
        EngineError::Refused(m) => public_message(&m, store.data_dir()),
        EngineError::Corrupt => "The file could not be read.".into(),
        EngineError::Crashed(_) => "The conversion engine stopped unexpectedly.".into(),
        EngineError::TimedOut => "The conversion took too long and was stopped.".into(),
        _ => "The conversion failed.".into(),
    };
    fail(store, job, kind, message, exit_code);
}

/// Shared failure exit: logs the failure, drops any partial output, keeps input and log, marks
/// the job failed. The log line names the job, never the file or the engine's message.
fn fail(store: &JobStore, job: &Job, kind: ErrorKind, message: String, exit_code: Option<i32>) {
    tracing::warn!(
        job_id = %job.id,
        kind = kind.as_str(),
        exit_code,
        size_mib = job.size.div_ceil(MIB),
        steps = steps(job),
        "conversion failed"
    );
    remove_file(&store.output_path(job));
    store.finish(&job.id, Err((kind, message)));
}

/// Deletes a file; a missing file is fine, other errors are logged by kind only.
fn remove_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = ?e.kind(), "could not delete a job file");
    }
}

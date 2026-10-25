//! Background workers that run queued jobs through the conversion engine.

use crate::engine::{DowngradeRequest, Engine, EngineError};
use crate::jobs::{Job, JobStore};
use crate::model::{ErrorKind, time_limit};
use std::path::Path;
use std::sync::Arc;
use tokio::task::JoinHandle;

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
                        );
                    }
                }
            })
        })
        .collect()
}

async fn run_job(store: Arc<JobStore>, engine: Arc<Engine>, job: Job) {
    if job.cancel.is_cancelled() {
        store.remove(&job.id);
        return;
    }
    let input = store.input_path(&job);
    let output = store.output_path(&job);
    let log = store.log_path(&job);

    let probe = match engine.probe(&input, job.cancel.clone()).await {
        Ok(p) => p,
        Err(e) => return finish_error(&store, &job, e),
    };
    if probe.kind != job.kind || probe.year != job.year {
        return fail(
            &store,
            &job,
            ErrorKind::Mismatch,
            "The file is not what the browser reported.".into(),
        );
    }
    if !probe.targets.contains(&job.target) {
        return fail(
            &store,
            &job,
            ErrorKind::Refused,
            "This conversion is not supported for this file.".into(),
        );
    }

    let req = DowngradeRequest {
        input: input.clone(),
        output: output.clone(),
        log,
        target: job.target,
        limit: time_limit(job.size, u32::from(job.year.saturating_sub(job.target))),
    };
    match engine.downgrade(req, job.cancel.clone()).await {
        Ok(dropped) => {
            remove_file(&input);
            store.finish(&job.id, Ok(dropped));
        }
        Err(e) => {
            finish_error(&store, &job, e);
        }
    }
}

fn finish_error(store: &JobStore, job: &Job, e: EngineError) {
    let Some(kind) = e.kind() else {
        store.remove(&job.id);
        return;
    };
    let message = match e {
        EngineError::Refused(m) => m,
        EngineError::Corrupt => "The file could not be read.".into(),
        EngineError::Crashed => "The conversion engine stopped unexpectedly.".into(),
        EngineError::TimedOut => "The conversion took too long and was stopped.".into(),
        _ => "The conversion failed.".into(),
    };
    fail(store, job, kind, message);
}

/// Shared failure exit: drops any partial output, keeps input and log, marks the job failed.
fn fail(store: &JobStore, job: &Job, kind: ErrorKind, message: String) {
    remove_file(&store.output_path(job));
    store.finish(&job.id, Err((kind, message)));
}

/// Deletes a file; a missing file is fine, other errors are logged by kind only.
fn remove_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("could not delete a job file: {}", e.kind());
    }
}

//! In-memory job store with per-IP limits and a FIFO queue for the workers.

use crate::clock::Clock;
use crate::config::MAX_BYTES;
use crate::model::{ApiError, DroppedElement, ErrorKind, FileKind, JobState};
use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const MAX_JOBS_PER_HOUR: usize = 5;
const MAX_WAITING_UPLOADS: usize = 2;
const MAX_QUEUED: usize = 20;
const DONE_RETENTION_MIN: i64 = 60;
const FAILED_RETENTION_MIN: i64 = 15;
const EXPIRED_RECORD_HOURS: i64 = 24;

/// A 128-bit random identifier rendered as 32 lowercase hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId([u8; 16]);

impl JobId {
    pub fn random() -> JobId {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).expect("system random source");
        JobId(b)
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseJobIdError;

impl FromStr for JobId {
    type Err = ParseJobIdError;

    /// Accepts exactly 32 lowercase hex characters.
    fn from_str(s: &str) -> Result<JobId, ParseJobIdError> {
        let bytes = s.as_bytes();
        if bytes.len() != 32 {
            return Err(ParseJobIdError);
        }
        let digit = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let mut out = [0u8; 16];
        for (i, o) in out.iter_mut().enumerate() {
            *o = digit(bytes[2 * i]).ok_or(ParseJobIdError)? << 4
                | digit(bytes[2 * i + 1]).ok_or(ParseJobIdError)?;
        }
        Ok(JobId(out))
    }
}

#[derive(Debug, Clone)]
pub struct JobError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub name: String,
    pub size: u64,
    pub kind: FileKind,
    pub year: u16,
    pub target: u16,
    pub ip: IpAddr,
    pub state: JobState,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error: Option<JobError>,
    pub dropped: Vec<DroppedElement>,
    pub received: u64,
    pub cancel: CancellationToken,
    pub report_code: Option<String>,
    /// Insertion order, used to break ties in FIFO ordering.
    seq: u64,
}

#[derive(Debug, Clone)]
pub struct NewJob {
    pub name: String,
    pub size: u64,
    pub kind: FileKind,
    pub year: u16,
    pub target: u16,
}

/// What `JobStore::cancel` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    WasRunning,
    Other,
    NotFound,
}

#[derive(Default)]
struct Inner {
    jobs: HashMap<JobId, Job>,
    /// Creation times per IP, kept apart from the jobs so deleting a job does not reset the limit.
    history: HashMap<IpAddr, VecDeque<DateTime<Utc>>>,
    next_seq: u64,
}

pub struct JobStore {
    data_dir: PathBuf,
    clock: Arc<dyn Clock>,
    inner: Mutex<Inner>,
    notify: Notify,
}

fn api(status: StatusCode, code: &'static str, msg: &str) -> ApiError {
    ApiError::new(status, code, msg)
}

impl JobStore {
    pub fn new(data_dir: PathBuf, clock: Arc<dyn Clock>) -> JobStore {
        JobStore {
            data_dir,
            clock,
            inner: Mutex::new(Inner::default()),
            notify: Notify::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn job_dir(&self, job: &Job) -> PathBuf {
        self.data_dir.join("jobs").join(job.id.to_string())
    }

    pub fn input_path(&self, job: &Job) -> PathBuf {
        self.job_dir(job).join(format!("in.{}", job.kind.ext()))
    }

    pub fn output_path(&self, job: &Job) -> PathBuf {
        self.job_dir(job).join(format!("out.{}", job.kind.ext()))
    }

    pub fn log_path(&self, job: &Job) -> PathBuf {
        self.job_dir(job).join("log.jsonl")
    }

    pub fn create(&self, req: NewJob, ip: IpAddr, pairs: &[(u16, u16)]) -> Result<Job, ApiError> {
        if req.size > MAX_BYTES {
            return Err(api(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "The file is larger than 1 GiB.",
            ));
        }
        if FileKind::from_name(&req.name) != Some(req.kind) {
            return Err(api(
                StatusCode::UNPROCESSABLE_ENTITY,
                "bad_kind",
                "The file extension does not match the file type.",
            ));
        }
        if !pairs.contains(&(req.year, req.target)) {
            return Err(api(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_pair",
                "This conversion is not supported.",
            ));
        }

        let now = self.clock.now();
        let mut g = self.lock();

        let hist = g.history.entry(ip).or_default();
        let window = Duration::minutes(60);
        while hist.front().is_some_and(|t| now - *t >= window) {
            hist.pop_front();
        }
        if hist.len() >= MAX_JOBS_PER_HOUR {
            let oldest = *hist.front().expect("non-empty history");
            let ms = (oldest + window - now).num_milliseconds().max(0) as u64;
            let secs = ms.div_ceil(1000).max(1);
            return Err(api(
                StatusCode::TOO_MANY_REQUESTS,
                "hourly_limit",
                "Hourly limit reached. Try again later.",
            )
            .retry_after(secs));
        }

        let count = |g: &Inner, f: &dyn Fn(&Job) -> bool| {
            g.jobs.values().filter(|j| j.ip == ip && f(j)).count()
        };
        if count(&g, &|j| {
            matches!(j.state, JobState::Queued | JobState::Running)
        }) >= 1
        {
            return Err(api(
                StatusCode::TOO_MANY_REQUESTS,
                "active_job",
                "You already have a conversion in progress.",
            ));
        }
        if count(&g, &|j| j.state == JobState::WaitingUpload) >= MAX_WAITING_UPLOADS {
            return Err(api(
                StatusCode::TOO_MANY_REQUESTS,
                "uploads_in_progress",
                "Too many uploads in progress.",
            ));
        }
        if g.jobs
            .values()
            .filter(|j| j.state == JobState::Queued)
            .count()
            >= MAX_QUEUED
        {
            return Err(api(
                StatusCode::SERVICE_UNAVAILABLE,
                "queue_full",
                "The queue is full. Try again shortly.",
            )
            .retry_after(60));
        }

        let id = JobId::random();
        let seq = g.next_seq;
        let job = Job {
            id,
            name: req.name,
            size: req.size,
            kind: req.kind,
            year: req.year,
            target: req.target,
            ip,
            state: JobState::WaitingUpload,
            created_at: now,
            started_at: None,
            finished_at: None,
            expires_at: None,
            error: None,
            dropped: Vec::new(),
            received: 0,
            cancel: CancellationToken::new(),
            report_code: None,
            seq,
        };
        if std::fs::create_dir_all(self.job_dir(&job)).is_err() {
            return Err(api(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Could not prepare the job.",
            ));
        }
        g.next_seq += 1;
        g.history.entry(ip).or_default().push_back(now);
        g.jobs.insert(id, job.clone());
        Ok(job)
    }

    pub fn get(&self, id: &JobId) -> Option<Job> {
        self.lock().jobs.get(id).cloned()
    }

    /// For a queued job: queued jobs created before it plus running jobs. Zero for any other state.
    pub fn position(&self, id: &JobId) -> Option<usize> {
        let g = self.lock();
        let job = g.jobs.get(id)?;
        if job.state != JobState::Queued {
            return Some(0);
        }
        let ahead = g
            .jobs
            .values()
            .filter(|j| match j.state {
                JobState::Running => true,
                JobState::Queued => (j.created_at, j.seq) < (job.created_at, job.seq),
                _ => false,
            })
            .count();
        Some(ahead)
    }

    /// Adds to the received byte counter and returns the new total.
    pub fn add_received(&self, id: &JobId, n: u64) -> u64 {
        match self.lock().jobs.get_mut(id) {
            Some(j) => {
                j.received += n;
                j.received
            }
            None => 0,
        }
    }

    pub fn mark_uploaded(&self, id: &JobId) {
        let queued = {
            let mut g = self.lock();
            match g.jobs.get_mut(id) {
                Some(j) if j.state == JobState::WaitingUpload => {
                    j.state = JobState::Queued;
                    true
                }
                _ => false,
            }
        };
        if queued {
            self.notify.notify_one();
        }
    }

    /// Marks a job whose upload broke as failed; it is kept for 15 minutes.
    pub fn set_failed_upload(&self, id: &JobId, kind: ErrorKind, message: String) {
        let now = self.clock.now();
        if let Some(j) = self.lock().jobs.get_mut(id)
            && j.state == JobState::WaitingUpload
        {
            j.state = JobState::Failed;
            j.error = Some(JobError { kind, message });
            j.finished_at = Some(now);
            j.expires_at = Some(now + Duration::minutes(FAILED_RETENTION_MIN));
        }
    }

    /// Takes the oldest queued job (FIFO) and marks it running.
    pub fn take_next(&self) -> Option<Job> {
        let now = self.clock.now();
        let mut g = self.lock();
        let id = g
            .jobs
            .values()
            .filter(|j| j.state == JobState::Queued)
            .min_by_key(|j| (j.created_at, j.seq))
            .map(|j| j.id)?;
        let j = g.jobs.get_mut(&id).expect("job present");
        j.state = JobState::Running;
        j.started_at = Some(now);
        Some(j.clone())
    }

    /// Waits until a queued job is available and takes it.
    pub async fn next_job(&self) -> Job {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Register interest before checking, so a notification between the check and the wait is not lost.
            notified.as_mut().enable();
            if let Some(j) = self.take_next() {
                return j;
            }
            notified.await;
        }
    }

    pub fn finish(&self, id: &JobId, outcome: Result<Vec<DroppedElement>, (ErrorKind, String)>) {
        let now = self.clock.now();
        let mut g = self.lock();
        let Some(j) = g.jobs.get_mut(id) else { return };
        if j.state != JobState::Running {
            return;
        }
        j.finished_at = Some(now);
        match outcome {
            Ok(dropped) => {
                j.state = JobState::Done;
                j.dropped = dropped;
                j.expires_at = Some(now + Duration::minutes(DONE_RETENTION_MIN));
            }
            Err((kind, message)) => {
                j.state = JobState::Failed;
                j.error = Some(JobError { kind, message });
                j.expires_at = Some(now + Duration::minutes(FAILED_RETENTION_MIN));
            }
        }
    }

    /// Cancels a job that has not finished yet and signals its token. The result says, decided
    /// atomically under the lock, whether the job was running at that moment (its worker then
    /// removes it once the engine lets go of the files).
    pub fn cancel(&self, id: &JobId) -> CancelOutcome {
        let now = self.clock.now();
        if let Some(j) = self.lock().jobs.get_mut(id) {
            let outcome = if j.state == JobState::Running {
                CancelOutcome::WasRunning
            } else {
                CancelOutcome::Other
            };
            j.cancel.cancel();
            if matches!(
                j.state,
                JobState::WaitingUpload | JobState::Queued | JobState::Running
            ) {
                j.state = JobState::Cancelled;
                j.finished_at = Some(now);
            }
            outcome
        } else {
            CancelOutcome::NotFound
        }
    }

    /// A copy of every job, for the retention sweep.
    pub fn snapshot(&self) -> Vec<Job> {
        self.lock().jobs.values().cloned().collect()
    }

    /// Turns a finished (done or failed) job into `Expired`: its files are deleted and the record
    /// stays for 24 hours so that clients get "gone" rather than "not found". Returns false when
    /// the job is missing or in another state.
    pub fn expire(&self, id: &JobId, now: DateTime<Utc>) -> bool {
        let job = {
            let mut g = self.lock();
            match g.jobs.get_mut(id) {
                Some(j) if matches!(j.state, JobState::Done | JobState::Failed) => {
                    j.state = JobState::Expired;
                    j.expires_at = Some(now + Duration::hours(EXPIRED_RECORD_HOURS));
                    j.dropped = Vec::new();
                    j.clone()
                }
                _ => return false,
            }
        };
        if let Err(e) = std::fs::remove_dir_all(self.job_dir(&job))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("could not remove job directory: {}", e.kind());
        }
        true
    }

    /// Records the report code of a failed job; false when the job is gone or already has one.
    pub fn set_report_code(&self, id: &JobId, code: String) -> bool {
        match self.lock().jobs.get_mut(id) {
            Some(j) if j.report_code.is_none() => {
                j.report_code = Some(code);
                true
            }
            _ => false,
        }
    }

    /// Forgets the job and deletes its directory.
    pub fn remove(&self, id: &JobId) {
        let job = self.lock().jobs.remove(id);
        if let Some(j) = job {
            j.cancel.cancel();
            if let Err(e) = std::fs::remove_dir_all(self.job_dir(&j))
                && e.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!("could not remove job directory: {}", e.kind());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use axum::http::StatusCode;
    use std::time::Duration;

    const GIB: u64 = 1 << 30;

    fn pairs() -> Vec<(u16, u16)> {
        vec![(2026, 2025), (2026, 2024), (2025, 2024)]
    }

    fn setup() -> (JobStore, Arc<ManualClock>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
        ));
        let store = JobStore::new(dir.path().to_path_buf(), clock.clone());
        (store, clock, dir)
    }

    fn req() -> NewJob {
        NewJob {
            name: "House.rvt".into(),
            size: 1000,
            kind: FileKind::Rvt,
            year: 2026,
            target: 2024,
        }
    }

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, n])
    }

    fn err_of(r: Result<Job, ApiError>) -> (StatusCode, &'static str, Option<u64>) {
        let e = r.expect_err("expected an error");
        (e.status, e.code, e.retry_after_s)
    }

    #[test]
    fn job_id_is_32_lowercase_hex_and_parses() {
        let a = JobId::random();
        assert_eq!(a.to_string().len(), 32);
        assert_eq!(a.to_string().parse::<JobId>().unwrap(), a);
        assert_ne!(a, JobId::random());
        assert!("ABCDEF0123456789abcdef0123456789".parse::<JobId>().is_err());
        assert!("abc".parse::<JobId>().is_err());
        assert!("../../etc/passwd/../../../xxxxxx".parse::<JobId>().is_err());
        assert!("0123456789abcdef0123456789abcdeg".parse::<JobId>().is_err());
    }

    #[test]
    fn create_makes_dir_and_waits_for_upload() {
        let (s, _c, d) = setup();
        let j = s.create(req(), ip(1), &pairs()).unwrap();
        assert_eq!(j.state, JobState::WaitingUpload);
        assert!(d.path().join("jobs").join(j.id.to_string()).is_dir());
        assert!(s.input_path(&j).ends_with(format!("{}/in.rvt", j.id)));
        assert!(s.output_path(&j).ends_with(format!("{}/out.rvt", j.id)));
        assert!(s.log_path(&j).ends_with(format!("{}/log.jsonl", j.id)));
        s.remove(&j.id);
        assert!(!s.job_dir(&j).exists());
        assert!(s.get(&j.id).is_none());
    }

    #[test]
    fn create_rejects_over_1gib() {
        let (s, _c, _d) = setup();
        let mut r = req();
        r.size = GIB + 1;
        assert_eq!(
            err_of(s.create(r, ip(1), &pairs())),
            (StatusCode::PAYLOAD_TOO_LARGE, "too_large", None)
        );
        let mut r = req();
        r.size = GIB;
        assert!(s.create(r, ip(1), &pairs()).is_ok());
    }

    #[test]
    fn create_rejects_bad_kind() {
        let (s, _c, _d) = setup();
        let mut r = req();
        r.kind = FileKind::Rfa;
        assert_eq!(
            err_of(s.create(r, ip(1), &pairs())),
            (StatusCode::UNPROCESSABLE_ENTITY, "bad_kind", None)
        );
        let mut r = req();
        r.name = "notes.txt".into();
        assert_eq!(err_of(s.create(r, ip(1), &pairs())).1, "bad_kind");
    }

    #[test]
    fn create_rejects_unsupported_pair() {
        let (s, _c, _d) = setup();
        let mut r = req();
        r.target = 2023;
        assert_eq!(
            err_of(s.create(r, ip(1), &pairs())),
            (StatusCode::UNPROCESSABLE_ENTITY, "unsupported_pair", None)
        );
    }

    #[test]
    fn sixth_job_in_hour_is_429_with_retry_after() {
        let (s, c, _d) = setup();
        for _ in 0..5 {
            let j = s.create(req(), ip(1), &pairs()).unwrap();
            s.cancel(&j.id);
            c.advance(Duration::from_secs(60));
        }
        // The oldest entry is 300 s old, so 3300 s remain.
        assert_eq!(
            err_of(s.create(req(), ip(1), &pairs())),
            (StatusCode::TOO_MANY_REQUESTS, "hourly_limit", Some(3300))
        );
    }

    #[test]
    fn cancel_reports_running_atomically() {
        let (s, _c, _d) = setup();
        assert_eq!(s.cancel(&JobId::random()), CancelOutcome::NotFound);
        let waiting = s.create(req(), ip(1), &pairs()).unwrap();
        assert_eq!(s.cancel(&waiting.id), CancelOutcome::Other);
        let queued = s.create(req(), ip(2), &pairs()).unwrap();
        s.mark_uploaded(&queued.id);
        let running = s.create(req(), ip(3), &pairs()).unwrap();
        s.mark_uploaded(&running.id);
        let taken = s.take_next().unwrap();
        assert_eq!(taken.id, queued.id);
        assert_eq!(s.cancel(&queued.id), CancelOutcome::WasRunning);
        assert_eq!(s.get(&queued.id).unwrap().state, JobState::Cancelled);
        // A cancelled queued job is never taken by a worker.
        assert_eq!(s.cancel(&running.id), CancelOutcome::Other);
        assert!(s.take_next().is_none());
        // Cancelling twice reports that it is no longer running.
        assert_eq!(s.cancel(&queued.id), CancelOutcome::Other);
    }

    #[test]
    fn ok_after_cancel_is_ignored_by_finish_so_worker_must_remove() {
        let (s, _c, _d) = setup();
        let j = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&j.id);
        let taken = s.take_next().unwrap();
        assert_eq!(s.cancel(&j.id), CancelOutcome::WasRunning);
        s.finish(&j.id, Ok(vec![]));
        let after = s.get(&j.id).unwrap();
        assert_eq!(after.state, JobState::Cancelled);
        assert!(taken.cancel.is_cancelled());
    }

    #[test]
    fn deleting_a_job_does_not_reset_hourly_limit() {
        let (s, _c, _d) = setup();
        for _ in 0..5 {
            let j = s.create(req(), ip(1), &pairs()).unwrap();
            s.remove(&j.id);
        }
        assert_eq!(err_of(s.create(req(), ip(1), &pairs())).1, "hourly_limit");
    }

    #[test]
    fn second_active_job_same_ip_is_429() {
        let (s, _c, _d) = setup();
        let j = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&j.id);
        assert_eq!(
            err_of(s.create(req(), ip(1), &pairs())),
            (StatusCode::TOO_MANY_REQUESTS, "active_job", None)
        );
        let t = s.take_next().unwrap();
        assert_eq!(t.id, j.id);
        assert_eq!(err_of(s.create(req(), ip(1), &pairs())).1, "active_job");
        s.finish(&j.id, Ok(vec![]));
        assert!(s.create(req(), ip(1), &pairs()).is_ok());
    }

    #[test]
    fn third_waiting_upload_is_429() {
        let (s, _c, _d) = setup();
        s.create(req(), ip(1), &pairs()).unwrap();
        s.create(req(), ip(1), &pairs()).unwrap();
        assert_eq!(
            err_of(s.create(req(), ip(1), &pairs())),
            (StatusCode::TOO_MANY_REQUESTS, "uploads_in_progress", None)
        );
    }

    #[test]
    fn queue_full_is_503() {
        let (s, _c, _d) = setup();
        for n in 0..20 {
            let j = s.create(req(), ip(n + 1), &pairs()).unwrap();
            s.mark_uploaded(&j.id);
        }
        assert_eq!(
            err_of(s.create(req(), ip(100), &pairs())),
            (StatusCode::SERVICE_UNAVAILABLE, "queue_full", Some(60))
        );
        s.take_next().unwrap();
        assert!(s.create(req(), ip(100), &pairs()).is_ok());
    }

    #[test]
    fn other_ip_not_limited() {
        let (s, _c, _d) = setup();
        let j = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&j.id);
        assert!(s.create(req(), ip(2), &pairs()).is_ok());
    }

    #[test]
    fn check_order_size_before_kind_before_pair() {
        let (s, _c, _d) = setup();
        let mut r = req();
        r.size = GIB + 1;
        r.kind = FileKind::Rfa;
        r.target = 2023;
        assert_eq!(err_of(s.create(r, ip(1), &pairs())).1, "too_large");
        let mut r = req();
        r.kind = FileKind::Rfa;
        r.target = 2023;
        assert_eq!(err_of(s.create(r, ip(1), &pairs())).1, "bad_kind");
    }

    #[test]
    fn position_counts_queued_before_and_running() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        let b = s.create(req(), ip(2), &pairs()).unwrap();
        let c3 = s.create(req(), ip(3), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        s.mark_uploaded(&b.id);
        s.mark_uploaded(&c3.id);
        assert_eq!(s.position(&a.id), Some(0));
        assert_eq!(s.position(&b.id), Some(1));
        assert_eq!(s.position(&c3.id), Some(2));
        assert_eq!(s.take_next().unwrap().id, a.id);
        assert_eq!(s.position(&b.id), Some(1));
        assert_eq!(s.position(&c3.id), Some(2));
        assert_eq!(s.position(&JobId::random()), None);
    }

    #[test]
    fn limits_reset_after_hour() {
        let (s, c, _d) = setup();
        for _ in 0..5 {
            let j = s.create(req(), ip(1), &pairs()).unwrap();
            s.cancel(&j.id);
        }
        assert_eq!(
            err_of(s.create(req(), ip(1), &pairs())),
            (StatusCode::TOO_MANY_REQUESTS, "hourly_limit", Some(3600))
        );
        c.advance(Duration::from_secs(3600));
        assert!(s.create(req(), ip(1), &pairs()).is_ok());
    }

    #[test]
    fn finish_sets_expiry_and_state() {
        let (s, c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        let t0 = c.now();
        s.take_next().unwrap();
        assert_eq!(s.get(&a.id).unwrap().started_at, Some(t0));
        c.advance(Duration::from_secs(10));
        let d = DroppedElement {
            element_id: 1,
            class: "Wall".into(),
            reason: crate::model::DropReason::NoRule,
            cascade_from: None,
        };
        s.finish(&a.id, Ok(vec![d.clone()]));
        let j = s.get(&a.id).unwrap();
        assert_eq!(j.state, JobState::Done);
        assert_eq!(j.dropped, vec![d]);
        assert_eq!(j.finished_at, Some(c.now()));
        assert_eq!(j.expires_at, Some(c.now() + chrono::Duration::hours(1)));

        let b = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&b.id);
        s.take_next().unwrap();
        s.finish(&b.id, Err((ErrorKind::Corrupt, "bad".into())));
        let j = s.get(&b.id).unwrap();
        assert_eq!(j.state, JobState::Failed);
        assert_eq!(j.error.as_ref().unwrap().kind, ErrorKind::Corrupt);
        assert_eq!(j.expires_at, Some(c.now() + chrono::Duration::minutes(15)));
    }

    #[test]
    fn cancel_cancels_token_and_is_not_overridden_by_finish() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        let running = s.take_next().unwrap();
        s.cancel(&a.id);
        assert!(running.cancel.is_cancelled());
        assert_eq!(s.get(&a.id).unwrap().state, JobState::Cancelled);
        s.finish(&a.id, Ok(vec![]));
        assert_eq!(s.get(&a.id).unwrap().state, JobState::Cancelled);
    }

    #[test]
    fn upload_helpers() {
        let (s, c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        assert_eq!(s.add_received(&a.id, 10), 10);
        assert_eq!(s.add_received(&a.id, 5), 15);
        s.set_failed_upload(&a.id, ErrorKind::Failed, "upload broke".into());
        let j = s.get(&a.id).unwrap();
        assert_eq!(j.state, JobState::Failed);
        assert_eq!(j.expires_at, Some(c.now() + chrono::Duration::minutes(15)));
    }

    #[tokio::test]
    async fn next_job_waits_for_upload() {
        let (s, _c, _d) = setup();
        let s = Arc::new(s);
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        let s2 = s.clone();
        let h = tokio::spawn(async move { s2.next_job().await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!h.is_finished());
        s.mark_uploaded(&a.id);
        let got = tokio::time::timeout(Duration::from_secs(2), h)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.id, a.id);
        assert_eq!(got.state, JobState::Running);
    }

    #[tokio::test]
    async fn next_job_sees_job_queued_before_wait() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        let got = tokio::time::timeout(Duration::from_secs(2), s.next_job())
            .await
            .unwrap();
        assert_eq!(got.id, a.id);
    }

    fn done_job(s: &JobStore) -> Job {
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        std::fs::write(s.input_path(&a), b"x").unwrap();
        s.mark_uploaded(&a.id);
        s.take_next().unwrap();
        s.finish(&a.id, Ok(Vec::new()));
        s.get(&a.id).unwrap()
    }

    #[test]
    fn expire_deletes_files_and_keeps_record_for_a_day() {
        let (s, c, _d) = setup();
        let a = done_job(&s);
        let now = c.now();
        assert!(s.expire(&a.id, now));
        let after = s.get(&a.id).unwrap();
        assert_eq!(after.state, JobState::Expired);
        assert_eq!(after.expires_at, Some(now + chrono::Duration::hours(24)));
        assert!(!s.job_dir(&a).exists());
        assert!(!s.expire(&a.id, now), "only done or failed jobs expire");
    }

    #[test]
    fn expire_ignores_queued_and_unknown_jobs() {
        let (s, c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        s.mark_uploaded(&a.id);
        assert!(!s.expire(&a.id, c.now()));
        assert_eq!(s.get(&a.id).unwrap().state, JobState::Queued);
        assert!(s.job_dir(&a).exists());
        assert!(!s.expire(&JobId::random(), c.now()));
    }

    #[test]
    fn set_report_code_only_once() {
        let (s, _c, _d) = setup();
        let a = done_job(&s);
        assert!(s.set_report_code(&a.id, "R-AAAAAA".into()));
        assert!(!s.set_report_code(&a.id, "R-BBBBBB".into()));
        assert_eq!(
            s.get(&a.id).unwrap().report_code.as_deref(),
            Some("R-AAAAAA")
        );
        assert!(!s.set_report_code(&JobId::random(), "R-CCCCCC".into()));
    }

    #[test]
    fn snapshot_lists_every_job() {
        let (s, _c, _d) = setup();
        let a = s.create(req(), ip(1), &pairs()).unwrap();
        let b = s.create(req(), ip(2), &pairs()).unwrap();
        let mut ids: Vec<_> = s.snapshot().iter().map(|j| j.id.to_string()).collect();
        ids.sort();
        let mut want = vec![a.id.to_string(), b.id.to_string()];
        want.sort();
        assert_eq!(ids, want);
    }
}

//! The job queue: states, scheduling with a concurrency limit, cancel and retry.
//!
//! A [`Queue`] starts jobs in FIFO order and never has more than its concurrency limit of
//! executors running at once. A [`JobExecutor`] runs one attempt of a job and reports the
//! in-progress states (`Probing`, `Ready`, `Downloading`, `PostProcessing`) and progress
//! through the [`JobSink`] in its [`JobContext`]; the queue owns `Queued` and the terminal
//! states (`Done`, `Failed`, `Canceled`). States only move forward within an attempt, and a
//! terminal state stays until the job is retried. Every change is published as a
//! [`QueueEvent`].

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinError;

use crate::store::{JobFilter, NewJob, Store, StoreError};
use crate::types::{ErrorCategory, JobPhase, JobState};

/// First id handed out when a persistent insert fails. Far above any SQLite row id the
/// app will reach, and below 2^53 so the webview's JavaScript numbers keep it exact.
const FALLBACK_FIRST_ID: u64 = 1 << 52;

/// Events buffered per subscriber. One that falls further behind misses the oldest events
/// (`RecvError::Lagged`) and can catch up with [`Queue::snapshot`].
const EVENT_BUFFER: usize = 1024;

/// Identifies a job within its queue. Ids increase in enqueue order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub u64);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What a successful attempt produced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobOutcome {
    /// Absolute path of the saved file, when the attempt saved one.
    pub output_path: Option<String>,
    /// Number of parsed comments for a comments-only job.
    #[serde(default)]
    pub comments_count: Option<u64>,
    /// English reasons for a successful skip, when no file was saved.
    #[serde(default)]
    pub skipped: Option<String>,
    /// Child jobs discovered when this attempt probed a list.
    #[serde(default)]
    pub entries: Vec<ListEntry>,
    /// Number of entries reported by the list before the child-job cap.
    #[serde(default)]
    pub list_total: Option<u32>,
    /// Where the video was already saved, when that is why `skipped` is set.
    #[serde(default)]
    pub already_saved: Option<AlreadySaved>,
}

/// Where a video that a job was about to download is already saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlreadySaved {
    /// Unix seconds the copy was saved: when the library recorded it, else the file's
    /// modification time; `None` when neither is known.
    pub saved_at: Option<i64>,
    /// The library remembers the video but its file is gone from disk.
    pub file_missing: bool,
    /// The saved file, or the one the library recorded when `file_missing`.
    pub path: String,
}

/// One child job discovered while probing a list URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    pub url: String,
    #[serde(default)]
    pub playlist_item: Option<u32>,
    #[serde(default)]
    pub title: Option<String>,
}

/// Metadata the queue UI can show after a job has been probed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct JobDetails {
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub site: Option<String>,
    pub published_at: Option<i64>,
    pub duration_s: Option<f64>,
    pub thumbnail_url: Option<String>,
}

/// Why an attempt failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message} ({category:?})")]
pub struct JobFailure {
    /// Kind of failure; decides the next action offered.
    pub category: ErrorCategory,
    /// What happened, e.g. yt-dlp's `ERROR:` line.
    pub message: String,
}

/// A cloneable flag that asks a running job to stop. All clones share one flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    inner: Arc<CancelState>,
}

#[derive(Debug, Default)]
struct CancelState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    /// A token that is not cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels the token and wakes every task waiting in [`cancelled`](Self::cancelled).
    /// Calling it again does nothing.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    /// Whether this token or a clone of it has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Completes once the token is cancelled; at once if it already is.
    pub async fn cancelled(&self) {
        // Created before the flag is checked: `notify_waiters` reaches every `Notified` that
        // exists when it is called, so a `cancel` between the check and the await is seen.
        let notified = self.inner.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

/// Runs one attempt of a job. The queue calls it on a task of its own.
pub trait JobExecutor: Send + Sync + 'static {
    /// Runs the attempt described by `job` to its end.
    ///
    /// Reports in-progress states and progress through `job.sink`, and should return soon
    /// after `job.cancel` is cancelled; the job then ends `Canceled` whatever this returns.
    /// A panic ends the job `Failed` with [`ErrorCategory::Unknown`].
    fn run(&self, job: JobContext) -> impl Future<Output = Result<JobOutcome, JobFailure>> + Send;
}

/// Everything an executor gets for one attempt of a job.
#[derive(Debug)]
pub struct JobContext {
    /// The job.
    pub id: JobId,
    /// The URL it was enqueued with.
    pub url: String,
    /// The options it was enqueued with.
    pub options: serde_json::Value,
    /// 1 for the first attempt, one more for each retry.
    pub attempt: u32,
    /// Where to report in-progress states and progress.
    pub sink: JobSink,
    /// Cancelled when the job is canceled.
    pub cancel: CancelToken,
}

/// Reports one attempt's in-progress states and progress to its queue. Once the attempt
/// has ended, state reports are refused and progress reports are ignored.
#[derive(Clone)]
pub struct JobSink {
    core: Arc<Core>,
    id: JobId,
    attempt: u32,
}

impl JobSink {
    /// Moves the job forward to `state`: `Probing`, `Ready`, `Downloading` or
    /// `PostProcessing`. States may be skipped, and reporting the current state again does
    /// nothing. Going back is refused and published as [`QueueEvent::TransitionRejected`].
    pub fn set_state(&self, state: JobState) -> Result<(), StateError> {
        self.core.report_state(self.id, self.attempt, state)
    }

    /// Names extra work within the current lifecycle state, or clears that name.
    pub fn set_phase(&self, phase: Option<JobPhase>) {
        self.core.report_phase(self.id, self.attempt, phase);
    }

    /// Replaces the job options used by future retries with values discovered during this
    /// attempt. Reports after the attempt ended are ignored.
    pub fn set_options(&self, options: serde_json::Value) {
        self.core.report_options(self.id, self.attempt, options);
    }

    /// Sets the job's progress: the fraction of the whole job done, clamped to 0..=1, or
    /// `None` when unknown (NaN counts as unknown).
    pub fn set_progress(&self, fraction: Option<f64>) {
        self.core.report_progress(self.id, self.attempt, fraction);
    }

    /// Stores metadata discovered while probing this attempt.
    pub fn set_details(&self, details: JobDetails) {
        self.core.report_details(self.id, self.attempt, details);
    }

    /// Stores the current download speed and remaining time. NaN and negative values
    /// count as unknown.
    pub fn set_transfer(&self, speed_bytes_per_s: Option<f64>, eta_s: Option<f64>) {
        self.core
            .report_transfer(self.id, self.attempt, speed_bytes_per_s, eta_s);
    }
}

impl fmt::Debug for JobSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobSink")
            .field("id", &self.id)
            .field("attempt", &self.attempt)
            .finish_non_exhaustive()
    }
}

/// Why a queue operation was refused. Nothing was changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueueError {
    /// No job has this id.
    #[error("no job {0}")]
    UnknownJob(JobId),
    /// Only a `Failed` or `Canceled` job can be retried.
    #[error("job {id} is {state:?}; only failed or canceled jobs can be retried")]
    NotRetryable { id: JobId, state: JobState },
    /// Only a `Done` job that was skipped because its video is already saved can be
    /// saved again.
    #[error(
        "job {id} is {state:?}; only jobs skipped because the video is already saved can be saved again"
    )]
    NotSavable { id: JobId, state: JobState },
    /// The job has already ended.
    #[error("job {id} has already ended ({state:?})")]
    AlreadyEnded { id: JobId, state: JobState },
    /// Only a job that has ended (`Done`, `Failed` or `Canceled`) can be removed.
    #[error("job {id} is {state:?}; only finished jobs can be removed")]
    NotEnded { id: JobId, state: JobState },
    /// Only a `Done` job has an output path to change.
    #[error("job {id} is {state:?}; only done jobs have an output path")]
    NotDone { id: JobId, state: JobState },
}

/// What a Downloads-list removal should do with a queue row at the instant it is
/// inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalPreparation {
    /// Probing, downloading and post-processing rows stay in the queue.
    Kept,
    /// The row is terminal (or was waiting and has now been canceled) and can be removed.
    Removable,
    /// A queued or ready executor was asked to cancel; wait for its `Finished` event.
    Waiting,
}

/// Why finished rows could not be removed from both the persistent and in-memory queue.
#[derive(Debug, thiserror::Error)]
pub enum RemoveJobsError {
    #[error(transparent)]
    Queue(#[from] QueueError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Why a state report was refused. The job's state was not changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// `Queued` and the terminal states are set by the queue only.
    #[error("{0:?} is set by the queue, not reported by an executor")]
    NotReportable(JobState),
    /// States only move forward within an attempt.
    #[error("job {id} cannot go back from {from:?} to {to:?}")]
    Backward {
        id: JobId,
        from: JobState,
        to: JobState,
    },
    /// The attempt the report belongs to has ended.
    #[error("attempt {attempt} of job {id} has ended")]
    Ended { id: JobId, attempt: u32 },
}

/// A change in the queue. Subscribers receive events in the order the changes happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QueueEvent {
    /// A job was enqueued: `Queued`, attempt 1.
    JobAdded { id: JobId, url: String },
    /// An attempt was handed to the executor. The state stays `Queued` until the executor
    /// reports one.
    Started { id: JobId, attempt: u32 },
    /// The job's state changed.
    StateChanged {
        id: JobId,
        from: JobState,
        to: JobState,
    },
    /// The job's progress changed; `None` when unknown.
    Progress { id: JobId, fraction: Option<f64> },
    /// Extra work within the current lifecycle state changed.
    Phase { id: JobId, phase: Option<JobPhase> },
    /// Metadata discovered while probing changed.
    Details { id: JobId, details: JobDetails },
    /// The current transfer speed and remaining time changed.
    Transfer {
        id: JobId,
        speed_bytes_per_s: Option<f64>,
        eta_s: Option<f64>,
    },
    /// An executor reported a state that would move the job back; the job stays in `from`.
    TransitionRejected {
        id: JobId,
        from: JobState,
        to: JobState,
    },
    /// An attempt ended in `state`: `Done` with the outcome, `Failed` with the failure, or
    /// `Canceled` with `None`.
    Finished {
        id: JobId,
        state: JobState,
        outcome_or_error: Option<Result<JobOutcome, JobFailure>>,
    },
    /// A `Done` job's saved file was renamed; its outcome now names `output_path`.
    OutputPathChanged { id: JobId, output_path: String },
    /// A finished job was removed from the queue ([`Queue::forget`]).
    Removed { id: JobId },
}

/// One job as [`Queue::snapshot`] shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobView {
    /// The job.
    pub id: JobId,
    /// The URL it was enqueued with.
    pub url: String,
    /// Its current state.
    pub state: JobState,
    /// The current attempt: 1, plus one per retry.
    pub attempt: u32,
    /// The fraction of the job done in this attempt, 0..=1; `None` when unknown.
    pub progress: Option<f64>,
    /// Extra work within the current lifecycle state.
    pub phase: Option<JobPhase>,
    /// Metadata discovered by the latest probe.
    pub details: Option<JobDetails>,
    /// Current download speed in bytes per second.
    pub speed_bytes_per_s: Option<f64>,
    /// Current estimated seconds remaining.
    pub eta_s: Option<f64>,
    /// What a successful finished job produced.
    pub outcome: Option<JobOutcome>,
    /// Why the job failed, while it is `Failed`.
    pub error: Option<JobFailure>,
}

/// The job queue. Clones are handles to the same queue.
pub struct Queue<E> {
    core: Arc<Core>,
    executor: Arc<E>,
    runtime: Handle,
}

/// State shared by a queue's handles, its running attempts and their sinks. Events are
/// sent while the lock is held, so their order is the order of the changes.
struct Core {
    board: Mutex<Board>,
    events: broadcast::Sender<QueueEvent>,
    store: Option<Arc<Mutex<Store>>>,
}

struct Board {
    jobs: BTreeMap<JobId, Job>,
    /// Snapshot order. Stored queues use database position order, which need not
    /// match id order after a row has been moved.
    order: Vec<JobId>,
    /// Jobs waiting to start, in start order.
    waiting: VecDeque<JobId>,
    /// Attempts handed to the executor that have not returned yet.
    running: usize,
    concurrency: usize,
    next_id: u64,
    /// In-memory ids used only after a persistent insert fails (see FALLBACK_FIRST_ID).
    fallback_next_id: u64,
}

struct Job {
    url: String,
    options: serde_json::Value,
    state: JobState,
    attempt: u32,
    progress: Option<f64>,
    phase: Option<JobPhase>,
    details: Option<JobDetails>,
    speed_bytes_per_s: Option<f64>,
    eta_s: Option<f64>,
    outcome: Option<JobOutcome>,
    error: Option<JobFailure>,
    /// Whether this job has a corresponding row in `Core::store`.
    persisted: bool,
    /// Whether this attempt is already included in the stored attempt count.
    attempt_recorded: bool,
    /// The cancel token of the running attempt; `Some` from its start until its executor
    /// returns.
    running: Option<CancelToken>,
}

struct Start {
    context: JobContext,
    record_attempt: bool,
}

/// What `Queue::requeue` hands back once the lock is released.
struct Requeued {
    starts: Vec<Start>,
    persisted: bool,
    options: serde_json::Value,
}

impl<E: JobExecutor> Queue<E> {
    /// A queue that runs at most `concurrency` jobs at once; values below 1 count as 1.
    ///
    /// # Panics
    ///
    /// When called outside a Tokio runtime: executors run as tasks on the runtime that
    /// creates the queue.
    pub fn new(executor: E, concurrency: usize) -> Self {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let board = Board {
            jobs: BTreeMap::new(),
            order: Vec::new(),
            waiting: VecDeque::new(),
            running: 0,
            concurrency: concurrency.max(1),
            next_id: 1,
            fallback_next_id: FALLBACK_FIRST_ID,
        };
        Self {
            core: Arc::new(Core {
                board: Mutex::new(board),
                events,
                store: None,
            }),
            executor: Arc::new(executor),
            runtime: Handle::current(),
        }
    }

    /// A write-through queue restored from `store` before this function returns.
    /// Terminal jobs remain terminal; unfinished jobs return to `Queued` and are
    /// scheduled in stored position order.
    ///
    /// Store failures are reported to stderr and otherwise ignored so database
    /// availability never changes an in-memory job's result.
    ///
    /// # Panics
    ///
    /// When called outside a Tokio runtime: executors run as tasks on the runtime that
    /// creates the queue.
    pub fn with_store(executor: E, concurrency: usize, store: Arc<Mutex<Store>>) -> Self {
        let rows = match store.lock() {
            Ok(store) => {
                if let Err(error) = store.recover_interrupted(unix_now()) {
                    eprintln!("could not recover interrupted queue jobs: {error}");
                }
                match store.list_jobs(JobFilter::All) {
                    Ok(rows) => rows,
                    Err(error) => {
                        eprintln!("could not restore queue jobs: {error}");
                        Vec::new()
                    }
                }
            }
            Err(_) => {
                eprintln!("could not restore queue jobs: store lock is poisoned");
                Vec::new()
            }
        };

        let mut jobs = BTreeMap::new();
        let mut order = Vec::new();
        let mut waiting = VecDeque::new();
        let mut largest_id = 0_u64;
        for row in rows {
            let Ok(raw_id) = u64::try_from(row.id) else {
                eprintln!("could not restore queue job {}: id is negative", row.id);
                continue;
            };
            let id = JobId(raw_id);
            largest_id = largest_id.max(raw_id);
            let details = row.details_json.and_then(|details| {
                serde_json::from_value(details)
                    .map_err(|error| {
                        eprintln!("could not restore details for queue job {id}: {error}");
                        error
                    })
                    .ok()
            });
            let state = if row.state.is_terminal() {
                row.state
            } else {
                JobState::Queued
            };
            let already_saved = row.already_saved_json.and_then(|saved| {
                serde_json::from_value(saved)
                    .map_err(|error| {
                        eprintln!(
                            "could not restore already-saved info for queue job {id}: {error}"
                        );
                        error
                    })
                    .ok()
            });
            let outcome = (state == JobState::Done).then_some(JobOutcome {
                output_path: row.output_path,
                skipped: row.skipped,
                already_saved,
                ..JobOutcome::default()
            });
            let error = (state == JobState::Failed)
                .then(|| {
                    row.error
                        .map(|(category, message)| JobFailure { category, message })
                })
                .flatten();
            let attempt = u32::try_from(row.attempts.max(1)).unwrap_or(u32::MAX);
            let attempt_recorded = row.attempts > 0;
            jobs.insert(
                id,
                Job {
                    url: row.url,
                    options: row.options_json,
                    state,
                    attempt,
                    progress: None,
                    phase: None,
                    details,
                    speed_bytes_per_s: None,
                    eta_s: None,
                    outcome,
                    error,
                    persisted: true,
                    attempt_recorded,
                    running: None,
                },
            );
            order.push(id);
            if state == JobState::Queued {
                waiting.push_back(id);
            }
        }

        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let core = Arc::new(Core {
            board: Mutex::new(Board {
                jobs,
                order,
                waiting,
                running: 0,
                concurrency: concurrency.max(1),
                next_id: largest_id.saturating_add(1).max(1),
                fallback_next_id: FALLBACK_FIRST_ID,
            }),
            events,
            store: Some(store),
        });
        let queue = Self {
            core: Arc::clone(&core),
            executor: Arc::new(executor),
            runtime: Handle::current(),
        };
        let starts = {
            let mut board = core.lock();
            take_startable(&core, &mut board)
        };
        queue.launch(starts);
        queue
    }

    /// Adds a job at the end of the queue, and starts it if a slot is free.
    pub fn enqueue(&self, url: impl Into<String>, options: serde_json::Value) -> JobId {
        let url = url.into();
        let stored_id = self.core.insert_job(&url, &options);
        let (id, starts) = {
            let mut guard = self.core.lock();
            let board = &mut *guard;
            let (id, persisted) = if let Some(id) = stored_id {
                board.next_id = board.next_id.max(id.0.saturating_add(1));
                (id, true)
            } else if self.core.store.is_some() {
                let id = JobId(board.fallback_next_id);
                board.fallback_next_id = board.fallback_next_id.saturating_add(1);
                (id, false)
            } else {
                let id = JobId(board.next_id);
                board.next_id += 1;
                (id, false)
            };
            board.jobs.insert(
                id,
                Job {
                    url: url.clone(),
                    options,
                    state: JobState::Queued,
                    attempt: 1,
                    progress: None,
                    phase: None,
                    details: None,
                    speed_bytes_per_s: None,
                    eta_s: None,
                    outcome: None,
                    error: None,
                    persisted,
                    attempt_recorded: false,
                    running: None,
                },
            );
            board.order.push(id);
            board.waiting.push_back(id);
            self.core.emit(QueueEvent::JobAdded { id, url });
            (id, take_startable(&self.core, board))
        };
        self.launch(starts);
        id
    }

    /// Cancels a job. A waiting job ends `Canceled` at once without running. A running
    /// job's cancel token is cancelled, and the job ends `Canceled` once its executor
    /// returns, whatever it returns; canceling it again does nothing.
    pub fn cancel(&self, id: JobId) -> Result<(), QueueError> {
        let persist_finished = {
            let mut guard = self.core.lock();
            let board = &mut *guard;
            let job = board.jobs.get_mut(&id).ok_or(QueueError::UnknownJob(id))?;
            if let Some(token) = &job.running {
                token.cancel();
                return Ok(());
            }
            if job.state != JobState::Queued {
                return Err(QueueError::AlreadyEnded {
                    id,
                    state: job.state,
                });
            }
            board.waiting.retain(|waiting| *waiting != id);
            job.state = JobState::Canceled;
            self.core.emit(QueueEvent::StateChanged {
                id,
                from: JobState::Queued,
                to: JobState::Canceled,
            });
            self.core.emit(QueueEvent::Finished {
                id,
                state: JobState::Canceled,
                outcome_or_error: None,
            });
            job.persisted
        };
        if persist_finished {
            self.core.finish_attempt(id, JobState::Canceled, None, None);
        }
        Ok(())
    }

    /// Atomically decides whether one row is eligible for Downloads-list removal.
    /// Waiting (`Queued`) and prepared (`Ready`) rows are canceled with the same state and
    /// persistence behavior as [`Queue::cancel`]. Other active states are left untouched.
    pub fn prepare_for_removal(&self, id: JobId) -> Result<RemovalPreparation, QueueError> {
        let (preparation, persist_finished) = {
            let mut guard = self.core.lock();
            let board = &mut *guard;
            let job = board.jobs.get(&id).ok_or(QueueError::UnknownJob(id))?;
            match job.state {
                JobState::Probing | JobState::Downloading | JobState::PostProcessing => {
                    (RemovalPreparation::Kept, false)
                }
                JobState::Done | JobState::Failed | JobState::Canceled => {
                    (RemovalPreparation::Removable, false)
                }
                JobState::Queued | JobState::Ready => {
                    if let Some(token) = job.running.clone() {
                        token.cancel();
                        (RemovalPreparation::Waiting, false)
                    } else {
                        let from = job.state;
                        let persisted = job.persisted;
                        board.waiting.retain(|waiting| *waiting != id);
                        let job = board.jobs.get_mut(&id).expect("the job still exists");
                        job.state = JobState::Canceled;
                        self.core.emit(QueueEvent::StateChanged {
                            id,
                            from,
                            to: JobState::Canceled,
                        });
                        self.core.emit(QueueEvent::Finished {
                            id,
                            state: JobState::Canceled,
                            outcome_or_error: None,
                        });
                        (RemovalPreparation::Removable, persisted)
                    }
                }
            }
        };
        if persist_finished {
            self.core.finish_attempt(id, JobState::Canceled, None, None);
        }
        Ok(preparation)
    }

    /// Queues a `Failed` or `Canceled` job again, at the end, as its next attempt. Jobs in
    /// any other state are left as they are.
    pub fn retry(&self, id: JobId) -> Result<(), QueueError> {
        let requeued = self.requeue(id, |job| {
            if matches!(job.state, JobState::Failed | JobState::Canceled) && job.running.is_none() {
                Ok(())
            } else {
                Err(QueueError::NotRetryable {
                    id,
                    state: job.state,
                })
            }
        })?;
        if requeued.persisted {
            self.core.persist("retry queue job", |store| {
                store.retry_job(job_id_i64(id), unix_now())
            });
        }
        self.launch(requeued.starts);
        Ok(())
    }

    /// Queues a failed or canceled job with replacement options. Used when the action that
    /// resolves a failure also supplies data needed by the next attempt.
    pub fn retry_with_options(
        &self,
        id: JobId,
        options: serde_json::Value,
    ) -> Result<(), QueueError> {
        let requeued = self.requeue(id, |job| {
            if matches!(job.state, JobState::Failed | JobState::Canceled) && job.running.is_none() {
                job.options = options;
                Ok(())
            } else {
                Err(QueueError::NotRetryable {
                    id,
                    state: job.state,
                })
            }
        })?;
        if requeued.persisted {
            self.core.persist("retry queue job with options", |store| {
                store.retry_job_with_options(job_id_i64(id), &requeued.options, unix_now())
            });
        }
        self.launch(requeued.starts);
        Ok(())
    }

    /// Queues a `Done` job that was skipped because its video is already saved once more,
    /// at the end, as its next attempt, with `force_save` set in its options so the
    /// download goes ahead. Jobs in any other state are left as they are.
    pub fn save_again(&self, id: JobId) -> Result<(), QueueError> {
        let requeued = self.requeue(id, |job| {
            let savable = job.state == JobState::Done
                && job.running.is_none()
                && job
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome.already_saved.is_some());
            match job.options.as_object_mut() {
                Some(options) if savable => {
                    options.insert("force_save".to_owned(), serde_json::Value::Bool(true));
                    Ok(())
                }
                _ => Err(QueueError::NotSavable {
                    id,
                    state: job.state,
                }),
            }
        })?;
        if requeued.persisted {
            self.core.persist("save queue job again", |store| {
                store.save_again_job(job_id_i64(id), &requeued.options, unix_now())
            });
        }
        self.launch(requeued.starts);
        Ok(())
    }

    /// Puts a finished job back at the end of the waiting list as its next attempt, if
    /// `check` accepts it (it may also edit the job). Clears the previous result.
    fn requeue(
        &self,
        id: JobId,
        check: impl FnOnce(&mut Job) -> Result<(), QueueError>,
    ) -> Result<Requeued, QueueError> {
        let mut guard = self.core.lock();
        let board = &mut *guard;
        let job = board.jobs.get_mut(&id).ok_or(QueueError::UnknownJob(id))?;
        check(job)?;
        let from = job.state;
        job.state = JobState::Queued;
        job.attempt += 1;
        job.progress = None;
        job.phase = None;
        job.speed_bytes_per_s = None;
        job.eta_s = None;
        job.outcome = None;
        job.error = None;
        job.attempt_recorded = job.persisted;
        let (persisted, options) = (job.persisted, job.options.clone());
        board.waiting.push_back(id);
        self.core.emit(QueueEvent::StateChanged {
            id,
            from,
            to: JobState::Queued,
        });
        Ok(Requeued {
            starts: take_startable(&self.core, board),
            persisted,
            options,
        })
    }

    /// Removes finished jobs from the queue and publishes [`QueueEvent::Removed`] for each.
    /// When any of `ids` has not ended (`Done`, `Failed` or `Canceled`), nothing is removed
    /// and [`QueueError::NotEnded`] is returned. Ids the queue does not know are skipped.
    ///
    /// The store is never written: the caller deletes the rows first
    /// ([`Store::erase_library_item`]), so a restored queue does not bring them back.
    pub fn forget(&self, ids: &[JobId]) -> Result<(), QueueError> {
        let mut guard = self.core.lock();
        let board = &mut *guard;
        for id in ids {
            if let Some(job) = board.jobs.get(id)
                && (!job.state.is_terminal() || job.running.is_some())
            {
                return Err(QueueError::NotEnded {
                    id: *id,
                    state: job.state,
                });
            }
        }
        for id in ids {
            if board.jobs.remove(id).is_some() {
                board.order.retain(|kept| kept != id);
                board.waiting.retain(|kept| kept != id);
                self.core.emit(QueueEvent::Removed { id: *id });
            }
        }
        Ok(())
    }

    /// Deletes terminal rows from the persistent store and the in-memory queue as one
    /// operation with respect to queue changes, then emits [`QueueEvent::Removed`] for
    /// each row. Unknown ids are ignored. The caller performs the secure-delete WAL
    /// checkpoint and removes work folders after this returns.
    pub fn remove_finished(&self, ids: &[JobId]) -> Result<Vec<JobId>, RemoveJobsError> {
        let mut guard = self.core.lock();
        let board = &mut *guard;
        let mut existing = Vec::new();
        for id in ids {
            let Some(job) = board.jobs.get(id) else {
                continue;
            };
            if !job.state.is_terminal() || job.running.is_some() {
                return Err(QueueError::NotEnded {
                    id: *id,
                    state: job.state,
                }
                .into());
            }
            if !existing.contains(id) {
                existing.push(*id);
            }
        }

        if let Some(store) = &self.core.store {
            let store = store.lock().unwrap_or_else(PoisonError::into_inner);
            let stored_ids: Vec<i64> = existing.iter().copied().map(job_id_i64).collect();
            store.delete_jobs(&stored_ids)?;
        }
        for id in &existing {
            board.jobs.remove(id);
            board.order.retain(|kept| kept != id);
            board.waiting.retain(|kept| kept != id);
            self.core.emit(QueueEvent::Removed { id: *id });
        }
        Ok(existing)
    }

    /// Points a `Done` job's outcome at `output_path` after its file was renamed, and
    /// publishes [`QueueEvent::OutputPathChanged`]. Only the queue in memory changes: the
    /// caller records the new path in the store ([`Store::rename_library_item`]).
    pub fn set_output_path(
        &self,
        id: JobId,
        output_path: impl Into<String>,
    ) -> Result<(), QueueError> {
        let output_path = output_path.into();
        let mut board = self.core.lock();
        let job = board.jobs.get_mut(&id).ok_or(QueueError::UnknownJob(id))?;
        if job.state != JobState::Done || job.running.is_some() {
            return Err(QueueError::NotDone {
                id,
                state: job.state,
            });
        }
        job.outcome
            .get_or_insert_with(JobOutcome::default)
            .output_path = Some(output_path.clone());
        self.core
            .emit(QueueEvent::OutputPathChanged { id, output_path });
        Ok(())
    }

    /// Changes how many jobs may run at once; values below 1 count as 1. Raising it starts
    /// waiting jobs at once. Lowering it never interrupts running jobs: new starts wait
    /// until fewer than `concurrency` run.
    pub fn set_concurrency(&self, concurrency: usize) {
        let starts = {
            let mut guard = self.core.lock();
            guard.concurrency = concurrency.max(1);
            take_startable(&self.core, &mut guard)
        };
        self.launch(starts);
    }

    /// How many jobs may run at once.
    pub fn concurrency(&self) -> usize {
        self.core.lock().concurrency
    }

    /// Every job, in enqueue order.
    pub fn snapshot(&self) -> Vec<JobView> {
        let board = self.core.lock();
        board
            .order
            .iter()
            .filter_map(|&id| board.jobs.get(&id).map(|job| (id, job)))
            .map(|(id, job)| JobView {
                id,
                url: job.url.clone(),
                state: job.state,
                attempt: job.attempt,
                progress: job.progress,
                phase: job.phase,
                details: job.details.clone(),
                speed_bytes_per_s: job.speed_bytes_per_s,
                eta_s: job.eta_s,
                outcome: job.outcome.clone(),
                error: job.error.clone(),
            })
            .collect()
    }

    /// Receives every event from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<QueueEvent> {
        self.core.events.subscribe()
    }

    /// Runs each attempt on a task of its own.
    fn launch(&self, starts: Vec<Start>) {
        for start in starts {
            if start.record_attempt {
                self.core.persist("record queue job attempt", |store| {
                    store.increment_attempts(job_id_i64(start.context.id), unix_now())
                });
            }
            self.runtime.spawn(self.clone().run(start.context));
        }
    }

    /// Runs one attempt, records how it ended, and starts what may start next. The
    /// executor runs on a task of its own so that a panic stays inside it.
    async fn run(self, job: JobContext) {
        let (id, attempt) = (job.id, job.attempt);
        let executor = Arc::clone(&self.executor);
        let result = self
            .runtime
            .spawn(async move { executor.run(job).await })
            .await;
        let starts = finish(&self.core, id, attempt, result);
        self.launch(starts);
    }
}

impl<E> Clone for Queue<E> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            executor: Arc::clone(&self.executor),
            runtime: self.runtime.clone(),
        }
    }
}

impl<E> fmt::Debug for Queue<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queue").finish_non_exhaustive()
    }
}

impl Core {
    fn lock(&self) -> MutexGuard<'_, Board> {
        // No user code runs while the lock is held, so a poisoned lock still holds
        // consistent data.
        self.board.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn emit(&self, event: QueueEvent) {
        // Fails only when nobody is subscribed.
        let _ = self.events.send(event);
    }

    fn persist(
        &self,
        action: &str,
        write: impl FnOnce(&Store) -> Result<(), crate::store::StoreError>,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        match store.lock() {
            Ok(store) => {
                if let Err(error) = write(&store) {
                    eprintln!("could not {action}: {error}");
                }
            }
            Err(_) => eprintln!("could not {action}: store lock is poisoned"),
        }
    }

    fn insert_job(&self, url: &str, options: &serde_json::Value) -> Option<JobId> {
        let store = self.store.as_ref()?;
        match store.lock() {
            Ok(store) => match store.insert_job(
                NewJob {
                    url: url.to_owned(),
                    site: None,
                    title: None,
                    options_json: options.clone(),
                    source_id: None,
                },
                unix_now(),
            ) {
                Ok(id) => match u64::try_from(id) {
                    Ok(id) => Some(JobId(id)),
                    Err(_) => {
                        eprintln!(
                            "could not enqueue persistent job: database returned negative id"
                        );
                        None
                    }
                },
                Err(error) => {
                    eprintln!("could not enqueue persistent job: {error}");
                    None
                }
            },
            Err(_) => {
                eprintln!("could not enqueue persistent job: store lock is poisoned");
                None
            }
        }
    }

    fn finish_attempt(
        &self,
        id: JobId,
        state: JobState,
        outcome: Option<&JobOutcome>,
        error: Option<&JobFailure>,
    ) {
        let already_saved = outcome
            .and_then(|outcome| outcome.already_saved.as_ref())
            .and_then(|saved| {
                serde_json::to_value(saved)
                    .map_err(|error| {
                        eprintln!(
                            "could not serialize already-saved info for queue job {id}: {error}"
                        );
                    })
                    .ok()
            });
        self.persist("record finished queue job attempt", |store| {
            store.finish_job_attempt(
                job_id_i64(id),
                state,
                outcome.and_then(|outcome| outcome.output_path.as_deref()),
                outcome.and_then(|outcome| outcome.skipped.as_deref()),
                already_saved.as_ref(),
                error.map(|error| (error.category, error.message.as_str())),
                unix_now(),
            )
        });
    }

    fn report_state(&self, id: JobId, attempt: u32, to: JobState) -> Result<(), StateError> {
        if !matches!(
            to,
            JobState::Probing | JobState::Ready | JobState::Downloading | JobState::PostProcessing
        ) {
            return Err(StateError::NotReportable(to));
        }
        let persisted = {
            let mut board = self.lock();
            let job = board
                .jobs
                .get_mut(&id)
                .filter(|job| job.attempt == attempt && job.running.is_some())
                .ok_or(StateError::Ended { id, attempt })?;
            let from = job.state;
            if to == from {
                return Ok(());
            }
            if rank(to) < rank(from) {
                self.emit(QueueEvent::TransitionRejected { id, from, to });
                return Err(StateError::Backward { id, from, to });
            }
            job.state = to;
            self.emit(QueueEvent::StateChanged { id, from, to });
            job.persisted
        };
        if persisted {
            self.persist("update queue job state", |store| {
                store.update_job_state(job_id_i64(id), to, None, unix_now())
            });
        }
        Ok(())
    }

    fn report_progress(&self, id: JobId, attempt: u32, fraction: Option<f64>) {
        let fraction = fraction
            .filter(|value| !value.is_nan())
            .map(|value| value.clamp(0.0, 1.0));
        let mut board = self.lock();
        if let Some(job) = board.jobs.get_mut(&id)
            && job.attempt == attempt
            && job.running.is_some()
        {
            job.progress = fraction;
            self.emit(QueueEvent::Progress { id, fraction });
        }
    }

    fn report_phase(&self, id: JobId, attempt: u32, phase: Option<JobPhase>) {
        let mut board = self.lock();
        if let Some(job) = board.jobs.get_mut(&id)
            && job.attempt == attempt
            && job.running.is_some()
            && job.phase != phase
        {
            job.phase = phase;
            self.emit(QueueEvent::Phase { id, phase });
        }
    }

    fn report_options(&self, id: JobId, attempt: u32, options: serde_json::Value) {
        let persisted = {
            let mut board = self.lock();
            if let Some(job) = board.jobs.get_mut(&id)
                && job.attempt == attempt
                && job.running.is_some()
            {
                job.options = options.clone();
                job.persisted
            } else {
                false
            }
        };
        if persisted {
            self.persist("update queue job options", |store| {
                store.set_job_options(job_id_i64(id), &options, unix_now())
            });
        }
    }

    fn report_details(&self, id: JobId, attempt: u32, details: JobDetails) {
        let persisted = {
            let mut board = self.lock();
            if let Some(job) = board.jobs.get_mut(&id)
                && job.attempt == attempt
                && job.running.is_some()
            {
                job.details = Some(details.clone());
                self.emit(QueueEvent::Details {
                    id,
                    details: details.clone(),
                });
                job.persisted
            } else {
                false
            }
        };
        if persisted {
            match serde_json::to_value(&details) {
                Ok(details) => self.persist("record queue job details", |store| {
                    store.set_job_details(job_id_i64(id), &details, unix_now())
                }),
                Err(error) => eprintln!("could not serialize queue job details: {error}"),
            }
        }
    }

    fn report_transfer(
        &self,
        id: JobId,
        attempt: u32,
        speed_bytes_per_s: Option<f64>,
        eta_s: Option<f64>,
    ) {
        let speed_bytes_per_s = valid_transfer_value(speed_bytes_per_s);
        let eta_s = valid_transfer_value(eta_s);
        let mut board = self.lock();
        if let Some(job) = board.jobs.get_mut(&id)
            && job.attempt == attempt
            && job.running.is_some()
        {
            job.speed_bytes_per_s = speed_bytes_per_s;
            job.eta_s = eta_s;
            self.emit(QueueEvent::Transfer {
                id,
                speed_bytes_per_s,
                eta_s,
            });
        }
    }
}

fn valid_transfer_value(value: Option<f64>) -> Option<f64> {
    value.filter(|value| !value.is_nan() && *value >= 0.0)
}

/// Hands waiting jobs to the executor, in order, while fewer than `concurrency` run.
/// Called with the lock held; the caller launches the returned attempts after releasing
/// it.
fn take_startable(core: &Arc<Core>, board: &mut Board) -> Vec<Start> {
    let mut starts = Vec::new();
    while board.running < board.concurrency {
        let Some(id) = board.waiting.pop_front() else {
            break;
        };
        let Some(job) = board.jobs.get_mut(&id) else {
            continue;
        };
        if job.state != JobState::Queued || job.running.is_some() {
            // Not expected: only waiting jobs are in `waiting`.
            continue;
        }
        let cancel = CancelToken::new();
        job.running = Some(cancel.clone());
        board.running += 1;
        core.emit(QueueEvent::Started {
            id,
            attempt: job.attempt,
        });
        let record_attempt = job.persisted && !job.attempt_recorded;
        job.attempt_recorded = true;
        starts.push(Start {
            context: JobContext {
                id,
                url: job.url.clone(),
                options: job.options.clone(),
                attempt: job.attempt,
                sink: JobSink {
                    core: Arc::clone(core),
                    id,
                    attempt: job.attempt,
                },
                cancel,
            },
            record_attempt,
        });
    }
    starts
}

/// Records how an attempt ended and frees its slot; returns the attempts that may start
/// now.
fn finish(
    core: &Arc<Core>,
    id: JobId,
    attempt: u32,
    result: Result<Result<JobOutcome, JobFailure>, JoinError>,
) -> Vec<Start> {
    let (starts, finished) = {
        let mut guard = core.lock();
        let board = &mut *guard;
        let mut finished = None;
        if let Some(job) = board.jobs.get_mut(&id)
            && job.attempt == attempt
            && let Some(token) = job.running.take()
        {
            board.running -= 1;
            let outcome_or_error = if token.is_cancelled() {
                None
            } else {
                Some(result.unwrap_or_else(|error| Err(executor_crashed(error))))
            };
            let to = match &outcome_or_error {
                None => JobState::Canceled,
                Some(Ok(_)) => JobState::Done,
                Some(Err(_)) => JobState::Failed,
            };
            job.error = match &outcome_or_error {
                Some(Err(failure)) => Some(failure.clone()),
                _ => None,
            };
            job.outcome = match &outcome_or_error {
                Some(Ok(outcome)) => Some(outcome.clone()),
                _ => None,
            };
            job.speed_bytes_per_s = None;
            job.eta_s = None;
            job.phase = None;
            let from = job.state;
            job.state = to;
            core.emit(QueueEvent::StateChanged { id, from, to });
            core.emit(QueueEvent::Finished {
                id,
                state: to,
                outcome_or_error,
            });
            finished = Some((job.persisted, to, job.outcome.clone(), job.error.clone()));
        }
        (take_startable(core, board), finished)
    };
    if let Some((true, state, outcome, error)) = finished {
        core.finish_attempt(id, state, outcome.as_ref(), error.as_ref());
    }
    starts
}

fn job_id_i64(id: JobId) -> i64 {
    i64::try_from(id.0).expect("persisted queue job ids come from SQLite INTEGER ids")
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

/// The failure recorded when an executor panicked, or its task was dropped by a runtime
/// shutting down.
fn executor_crashed(error: JoinError) -> JobFailure {
    let message = if error.is_panic() {
        let payload = error.into_panic();
        let detail = payload
            .downcast_ref::<&str>()
            .map(|detail| (*detail).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned());
        match detail {
            Some(detail) => format!("the executor panicked: {detail}"),
            None => "the executor panicked".to_owned(),
        }
    } else {
        "the executor was stopped before it finished".to_owned()
    };
    JobFailure {
        category: ErrorCategory::Unknown,
        message,
    }
}

/// Place in the forward order of states; the terminal states share the last place.
fn rank(state: JobState) -> u8 {
    match state {
        JobState::Queued => 0,
        JobState::Probing => 1,
        JobState::Ready => 2,
        JobState::Downloading => 3,
        JobState::PostProcessing => 4,
        JobState::Done | JobState::Failed | JobState::Canceled => 5,
    }
}

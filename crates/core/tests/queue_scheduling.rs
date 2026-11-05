//! `vids_core::queue` with a fake executor that the test drives through channels: each
//! executor call is handed to the test, which decides when and how it ends. Nothing waits
//! on wall-clock time; timeouts only guard against hangs.
//!
//! Started events are sent under the queue's lock, in the same critical section as the
//! change that freed the slot, and `snapshot` takes that lock. So after the Finished event
//! of a job has arrived, `settle` sees every start that its end caused (SPEC Q1).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use tokio::sync::broadcast::error::TryRecvError;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::timeout;
use vids_core::queue::{
    CancelToken, JobContext, JobDetails, JobExecutor, JobFailure, JobId, JobOutcome, JobSink,
    JobView, Queue, QueueError, QueueEvent, StateError,
};
use vids_core::types::{ErrorCategory, JobPhase, JobState};

/// Guards against hangs only; no test relies on timing.
const HANG: Duration = Duration::from_secs(10);

/// How the fake executor ends a run, once the test decides.
#[derive(Debug)]
enum Plan {
    /// Reports Ready, Downloading and PostProcessing with progress, then succeeds.
    Succeed,
    /// Fails at once.
    Fail(ErrorCategory, &'static str),
    /// Panics.
    Panic,
    /// Waits for the cancel token, then fails.
    AwaitCancel,
}

/// One executor call, handed to the test while the executor waits for its plan.
struct Run {
    id: JobId,
    attempt: u32,
    /// Executors running when this one started, itself included.
    running_at_start: usize,
    sink: JobSink,
    cancel: CancelToken,
    plan: oneshot::Sender<Plan>,
}

impl Run {
    fn finish(self, plan: Plan) {
        self.plan
            .send(plan)
            .expect("the executor waits for its plan");
    }
}

struct Fake {
    runs: mpsc::UnboundedSender<Run>,
    running: Arc<AtomicUsize>,
    max_running: Arc<AtomicUsize>,
}

/// Counts an executor as running until it returns, panics included.
struct Running(Arc<AtomicUsize>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl JobExecutor for Fake {
    async fn run(&self, job: JobContext) -> Result<JobOutcome, JobFailure> {
        let running_at_start = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        let _running = Running(Arc::clone(&self.running));
        self.max_running
            .fetch_max(running_at_start, Ordering::SeqCst);
        job.sink
            .set_state(JobState::Probing)
            .expect("Probing follows Queued");

        let (plan, decided) = oneshot::channel();
        self.runs
            .send(Run {
                id: job.id,
                attempt: job.attempt,
                running_at_start,
                sink: job.sink.clone(),
                cancel: job.cancel.clone(),
                plan,
            })
            .expect("the test receives runs");
        match decided.await.expect("the test decides every run") {
            Plan::Succeed => {
                job.sink.set_state(JobState::Ready).expect("forward");
                job.sink.set_progress(Some(0.25));
                job.sink.set_state(JobState::Downloading).expect("forward");
                job.sink.set_progress(Some(1.0));
                job.sink
                    .set_state(JobState::PostProcessing)
                    .expect("forward");
                Ok(JobOutcome {
                    output_path: Some(format!("{}.mp4", job.url)),
                    skipped: None,
                    ..JobOutcome::default()
                })
            }
            Plan::Fail(category, message) => Err(JobFailure {
                category,
                message: message.to_owned(),
            }),
            Plan::Panic => panic!("the fake executor panics on purpose"),
            Plan::AwaitCancel => {
                job.cancel.cancelled().await;
                Err(JobFailure {
                    category: ErrorCategory::Unknown,
                    message: "stopped".to_owned(),
                })
            }
        }
    }
}

struct Harness {
    queue: Queue<Fake>,
    runs: mpsc::UnboundedReceiver<Run>,
    events: broadcast::Receiver<QueueEvent>,
    /// Every event received so far, in order.
    log: Vec<QueueEvent>,
    /// Finished events already returned by `next_finished`, per job.
    finishes_seen: HashMap<JobId, usize>,
    running: Arc<AtomicUsize>,
    max_running: Arc<AtomicUsize>,
}

impl Harness {
    fn new(concurrency: usize) -> Self {
        let (sender, runs) = mpsc::unbounded_channel();
        let running = Arc::new(AtomicUsize::new(0));
        let max_running = Arc::new(AtomicUsize::new(0));
        let queue = Queue::new(
            Fake {
                runs: sender,
                running: Arc::clone(&running),
                max_running: Arc::clone(&max_running),
            },
            concurrency,
        );
        let events = queue.subscribe();
        Self {
            queue,
            runs,
            events,
            log: Vec::new(),
            finishes_seen: HashMap::new(),
            running,
            max_running,
        }
    }

    fn enqueue(&self, count: usize) -> Vec<JobId> {
        (0..count)
            .map(|n| {
                self.queue
                    .enqueue(format!("https://example.test/{n}"), json!({ "n": n }))
            })
            .collect()
    }

    /// The next executor call.
    async fn next_run(&mut self) -> Run {
        timeout(HANG, self.runs.recv())
            .await
            .expect("an executor call arrives")
            .expect("the queue is alive")
    }

    /// The next `count` executor calls, by job.
    async fn next_runs(&mut self, count: usize) -> HashMap<JobId, Run> {
        let mut runs = HashMap::new();
        for _ in 0..count {
            let run = self.next_run().await;
            runs.insert(run.id, run);
        }
        runs
    }

    async fn next_event(&mut self) -> QueueEvent {
        let event = timeout(HANG, self.events.recv())
            .await
            .expect("an event arrives")
            .expect("the subscriber keeps up");
        self.log.push(event.clone());
        event
    }

    /// Waits for the next attempt of `id` to end; returns its Finished event's contents.
    async fn next_finished(
        &mut self,
        id: JobId,
    ) -> (JobState, Option<Result<JobOutcome, JobFailure>>) {
        let seen = self.finishes_seen.entry(id).or_default();
        let wanted = *seen;
        *seen += 1;
        let mut index = 0;
        loop {
            let found = self
                .log
                .iter()
                .filter_map(|event| match event {
                    QueueEvent::Finished {
                        id: finished,
                        state,
                        outcome_or_error,
                    } if *finished == id => Some((*state, outcome_or_error.clone())),
                    _ => None,
                })
                .nth(wanted);
            if let Some(found) = found {
                return found;
            }
            index += 1;
            assert!(index < 10_000, "job {id} never finished");
            self.next_event().await;
        }
    }

    /// Records every event sent so far. `snapshot` takes the queue's lock, so every change
    /// that has begun is complete, its events included, once it returns.
    fn settle(&mut self) -> Vec<JobView> {
        let view = self.queue.snapshot();
        loop {
            match self.events.try_recv() {
                Ok(event) => self.log.push(event),
                Err(TryRecvError::Empty) => return view,
                Err(error) => panic!("event stream broken: {error}"),
            }
        }
    }

    /// Jobs in the order the queue started them.
    fn started(&self) -> Vec<JobId> {
        self.log
            .iter()
            .filter_map(|event| match event {
                QueueEvent::Started { id, .. } => Some(*id),
                _ => None,
            })
            .collect()
    }

    fn states(&mut self) -> Vec<JobState> {
        self.settle().iter().map(|job| job.state).collect()
    }
}

/// Place in the forward order of states.
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

/// Every job's StateChanged events chain (`from` is the previous `to`) and move forward;
/// only a retry goes from a terminal state back to Queued.
fn assert_forward(log: &[QueueEvent]) {
    let mut current: HashMap<JobId, JobState> = HashMap::new();
    for event in log {
        if let QueueEvent::StateChanged { id, from, to } = event {
            let previous = current.get(id).copied().unwrap_or(JobState::Queued);
            assert_eq!(*from, previous, "job {id}: events chain");
            let retry = from.is_terminal() && *to == JobState::Queued;
            assert!(
                retry || rank(*to) > rank(*from),
                "job {id}: {from:?} -> {to:?} does not move forward"
            );
            current.insert(*id, *to);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn q1_with_concurrency_two_at_most_two_of_six_jobs_run_and_all_finish() {
    let mut h = Harness::new(2);
    let ids = h.enqueue(6);

    let mut open = VecDeque::from([h.next_run().await, h.next_run().await]);
    assert_eq!(h.running.load(Ordering::SeqCst), 2);
    let mut calls = open.len();
    while let Some(run) = open.pop_front() {
        assert!(
            run.running_at_start <= 2,
            "{} running",
            run.running_at_start
        );
        run.finish(Plan::Succeed);
        if calls < ids.len() {
            open.push_back(h.next_run().await);
            calls += 1;
        }
    }
    for &id in &ids {
        let (state, outcome) = h.next_finished(id).await;
        assert_eq!(state, JobState::Done);
        assert!(matches!(
            outcome,
            Some(Ok(JobOutcome {
                output_path: Some(_),
                ..
            }))
        ));
    }

    assert_eq!(h.max_running.load(Ordering::SeqCst), 2);
    assert_eq!(h.states(), [JobState::Done; 6]);
    assert_eq!(h.started(), ids, "jobs start in enqueue order");
    assert_forward(&h.log);
}

#[tokio::test]
async fn a_failure_in_job_two_leaves_jobs_one_and_three_done() {
    let mut h = Harness::new(2);
    let ids = h.enqueue(3);
    let mut runs = h.next_runs(2).await;

    runs.remove(&ids[1])
        .expect("job 2 runs")
        .finish(Plan::Fail(ErrorCategory::Network, "connection reset"));
    let failure = JobFailure {
        category: ErrorCategory::Network,
        message: "connection reset".to_owned(),
    };
    assert_eq!(
        h.next_finished(ids[1]).await,
        (JobState::Failed, Some(Err(failure.clone())))
    );
    runs.remove(&ids[0])
        .expect("job 1 runs")
        .finish(Plan::Succeed);
    let third = h.next_run().await;
    assert_eq!(third.id, ids[2]);
    third.finish(Plan::Succeed);
    assert_eq!(h.next_finished(ids[0]).await.0, JobState::Done);
    assert_eq!(h.next_finished(ids[2]).await.0, JobState::Done);

    let view = h.settle();
    assert_eq!(
        view.iter().map(|job| job.state).collect::<Vec<_>>(),
        [JobState::Done, JobState::Failed, JobState::Done]
    );
    assert_eq!(view[1].error, Some(failure));
    assert_eq!(view[0].error, None);
    assert_eq!(view[2].error, None);
    assert_eq!(view[0].progress, Some(1.0));
    assert_forward(&h.log);
}

#[tokio::test]
async fn canceling_a_queued_job_never_calls_the_executor() {
    let mut h = Harness::new(1);
    let ids = h.enqueue(2);
    let first = h.next_run().await;
    assert_eq!(first.id, ids[0]);

    h.queue
        .cancel(ids[1])
        .expect("a waiting job can be canceled");
    assert_eq!(h.next_finished(ids[1]).await, (JobState::Canceled, None));
    assert_eq!(h.settle()[1].state, JobState::Canceled);

    // FIFO: had the canceled job been kept, it would start before this one.
    let after = h.queue.enqueue("https://example.test/after", json!({}));
    first.finish(Plan::Succeed);
    let next = h.next_run().await;
    assert_eq!(next.id, after, "the canceled job is skipped");
    next.finish(Plan::Succeed);
    assert_eq!(h.next_finished(ids[0]).await.0, JobState::Done);
    assert_eq!(h.next_finished(after).await.0, JobState::Done);

    h.settle();
    assert_eq!(h.started(), [ids[0], after]);
    assert_eq!(
        h.queue.cancel(ids[1]),
        Err(QueueError::AlreadyEnded {
            id: ids[1],
            state: JobState::Canceled
        })
    );
    assert_eq!(
        h.queue.cancel(JobId(999)),
        Err(QueueError::UnknownJob(JobId(999)))
    );
    assert_forward(&h.log);
}

#[tokio::test]
async fn canceling_a_running_job_ends_it_canceled_whatever_the_executor_returns() {
    let mut h = Harness::new(2);
    let ids = h.enqueue(2);
    let mut runs = h.next_runs(2).await;
    let ignores_token = runs.remove(&ids[0]).expect("job 1 runs");
    let honors_token = runs.remove(&ids[1]).expect("job 2 runs");
    honors_token.finish(Plan::AwaitCancel);

    h.queue.cancel(ids[0]).expect("cancel job 1");
    h.queue
        .cancel(ids[0])
        .expect("canceling again does nothing");
    h.queue.cancel(ids[1]).expect("cancel job 2");
    assert!(ignores_token.cancel.is_cancelled());
    assert_eq!(
        h.settle()[0].state,
        JobState::Probing,
        "a running job stays in its state until its executor returns"
    );

    // Job 1's executor ignores the token and even succeeds; job 2's stops with a failure.
    ignores_token.finish(Plan::Succeed);
    assert_eq!(h.next_finished(ids[0]).await, (JobState::Canceled, None));
    assert_eq!(h.next_finished(ids[1]).await, (JobState::Canceled, None));

    let view = h.settle();
    assert_eq!(view[0].error, None);
    assert_eq!(view[1].error, None);
    assert_forward(&h.log);

    // A canceled job can be retried.
    h.queue.retry(ids[1]).expect("retry a canceled job");
    let again = h.next_run().await;
    assert_eq!((again.id, again.attempt), (ids[1], 2));
    again.finish(Plan::Succeed);
    assert_eq!(h.next_finished(ids[1]).await.0, JobState::Done);
}

#[tokio::test]
async fn retry_runs_a_failed_job_again_at_the_end_as_attempt_two() {
    let mut h = Harness::new(1);
    let ids = h.enqueue(3);
    let first = h.next_run().await;
    assert_eq!((first.id, first.attempt), (ids[0], 1));
    let details = JobDetails {
        title: Some("First video".to_owned()),
        uploader: Some("Uploader".to_owned()),
        site: Some("Example".to_owned()),
        published_at: Some(1_700_000_000),
        duration_s: Some(42.0),
        thumbnail_url: Some("https://example.test/thumbnail.jpg".to_owned()),
    };
    first.sink.set_details(details.clone());
    first.sink.set_progress(Some(0.4));
    first.sink.set_transfer(Some(2048.0), Some(12.0));

    // Running and waiting jobs are left as they are.
    assert_eq!(
        h.queue.retry(ids[0]),
        Err(QueueError::NotRetryable {
            id: ids[0],
            state: JobState::Probing
        })
    );
    assert_eq!(
        h.queue.retry(ids[1]),
        Err(QueueError::NotRetryable {
            id: ids[1],
            state: JobState::Queued
        })
    );
    assert_eq!(
        h.queue.retry(JobId(999)),
        Err(QueueError::UnknownJob(JobId(999)))
    );

    first.finish(Plan::Fail(ErrorCategory::Network, "timed out"));
    assert_eq!(h.next_finished(ids[0]).await.0, JobState::Failed);
    let second = h.next_run().await;
    assert_eq!(second.id, ids[1]);

    h.queue.retry(ids[0]).expect("a failed job can be retried");
    let view = h.settle();
    assert_eq!(
        (
            view[0].state,
            view[0].attempt,
            view[0].error.clone(),
            view[0].progress,
            view[0].speed_bytes_per_s,
            view[0].eta_s,
            view[0].outcome.clone(),
            view[0].details.clone()
        ),
        (
            JobState::Queued,
            2,
            None,
            None,
            None,
            None,
            None,
            Some(details)
        )
    );

    // "At the end": job 3, waiting since before the retry, starts first.
    second.finish(Plan::Succeed);
    let third = h.next_run().await;
    assert_eq!(third.id, ids[2]);
    third.finish(Plan::Succeed);
    let again = h.next_run().await;
    assert_eq!((again.id, again.attempt), (ids[0], 2));
    again.finish(Plan::Succeed);

    assert_eq!(h.next_finished(ids[0]).await.0, JobState::Done);
    let view = h.settle();
    assert_eq!((view[0].state, view[0].attempt), (JobState::Done, 2));
    assert_eq!(
        h.queue.retry(ids[1]),
        Err(QueueError::NotRetryable {
            id: ids[1],
            state: JobState::Done
        })
    );
    assert_eq!(h.started(), [ids[0], ids[1], ids[2], ids[0]]);
    assert_forward(&h.log);
}

#[tokio::test]
async fn raising_concurrency_from_one_to_three_starts_two_waiting_jobs_at_once() {
    let mut h = Harness::new(1);
    let ids = h.enqueue(4);
    let first = h.next_run().await;
    h.settle();
    assert_eq!(h.started(), [ids[0]]);

    h.queue.set_concurrency(3);
    assert_eq!(h.queue.concurrency(), 3);
    h.settle();
    assert_eq!(
        h.started(),
        [ids[0], ids[1], ids[2]],
        "two more jobs start while the first still runs"
    );
    let mut more = h.next_runs(2).await;
    assert_eq!(h.running.load(Ordering::SeqCst), 3);

    first.finish(Plan::Succeed);
    let fourth = h.next_run().await;
    assert_eq!(fourth.id, ids[3]);
    for run in more.drain().map(|(_, run)| run).chain([fourth]) {
        run.finish(Plan::Succeed);
    }
    for &id in &ids {
        assert_eq!(h.next_finished(id).await.0, JobState::Done);
    }
    assert_eq!(h.max_running.load(Ordering::SeqCst), 3);
    assert_forward(&h.log);
}

#[tokio::test]
async fn lowering_concurrency_from_three_to_one_lets_running_jobs_finish_then_runs_one_at_a_time() {
    let mut h = Harness::new(3);
    let ids = h.enqueue(5);
    let mut runs = h.next_runs(3).await;

    h.queue.set_concurrency(1);
    h.settle();
    assert!(
        runs.values().all(|run| !run.cancel.is_cancelled()),
        "running jobs are not interrupted"
    );

    for (index, &id) in ids[..2].iter().enumerate() {
        runs.remove(&id).expect("running").finish(Plan::Succeed);
        assert_eq!(h.next_finished(id).await.0, JobState::Done);
        h.settle();
        assert_eq!(
            h.started(),
            ids[..3],
            "no start while {} jobs still run",
            2 - index
        );
    }
    runs.remove(&ids[2]).expect("running").finish(Plan::Succeed);
    assert_eq!(h.next_finished(ids[2]).await.0, JobState::Done);

    let fourth = h.next_run().await;
    assert_eq!((fourth.id, fourth.running_at_start), (ids[3], 1));
    h.settle();
    assert_eq!(h.started(), ids[..4], "the fifth job waits for the fourth");
    fourth.finish(Plan::Succeed);
    assert_eq!(h.next_finished(ids[3]).await.0, JobState::Done);

    let fifth = h.next_run().await;
    assert_eq!((fifth.id, fifth.running_at_start), (ids[4], 1));
    fifth.finish(Plan::Succeed);
    assert_eq!(h.next_finished(ids[4]).await.0, JobState::Done);
    assert_eq!(h.states(), [JobState::Done; 5]);
    assert_forward(&h.log);
}

#[tokio::test]
async fn a_panicking_executor_fails_its_job_while_the_others_continue() {
    let mut h = Harness::new(2);
    let ids = h.enqueue(3);
    let mut runs = h.next_runs(2).await;

    runs.remove(&ids[0])
        .expect("job 1 runs")
        .finish(Plan::Panic);
    let (state, outcome) = h.next_finished(ids[0]).await;
    assert_eq!(state, JobState::Failed);
    let Some(Err(failure)) = outcome else {
        panic!("a panic is a failure: {outcome:?}");
    };
    assert_eq!(failure.category, ErrorCategory::Unknown);
    assert!(failure.message.contains("panics on purpose"), "{failure}");

    runs.remove(&ids[1])
        .expect("job 2 runs")
        .finish(Plan::Succeed);
    let third = h.next_run().await;
    assert_eq!(third.id, ids[2]);
    third.finish(Plan::Succeed);
    assert_eq!(h.next_finished(ids[1]).await.0, JobState::Done);
    assert_eq!(h.next_finished(ids[2]).await.0, JobState::Done);

    let view = h.settle();
    assert_eq!(view[0].error, Some(failure));
    assert_eq!(
        view.iter().map(|job| job.state).collect::<Vec<_>>(),
        [JobState::Failed, JobState::Done, JobState::Done]
    );
    assert_forward(&h.log);
}

#[tokio::test]
async fn states_only_move_forward_and_reports_after_the_end_are_refused() {
    let mut h = Harness::new(1);
    let ids = h.enqueue(1);
    let id = ids[0];
    let run = h.next_run().await;
    let sink = run.sink.clone();

    assert_eq!(
        sink.set_state(JobState::Downloading),
        Ok(()),
        "skipping Ready is allowed"
    );
    assert_eq!(
        sink.set_state(JobState::Downloading),
        Ok(()),
        "repeating does nothing"
    );
    assert_eq!(
        sink.set_state(JobState::Ready),
        Err(StateError::Backward {
            id,
            from: JobState::Downloading,
            to: JobState::Ready
        })
    );
    for owned_by_the_queue in [
        JobState::Queued,
        JobState::Done,
        JobState::Failed,
        JobState::Canceled,
    ] {
        assert_eq!(
            sink.set_state(owned_by_the_queue),
            Err(StateError::NotReportable(owned_by_the_queue))
        );
    }
    sink.set_progress(Some(1.5));
    assert_eq!(h.settle()[0].progress, Some(1.0), "progress is clamped");
    sink.set_progress(Some(f64::NAN));
    assert_eq!(h.settle()[0].progress, None, "NaN means unknown");
    assert_eq!(h.settle()[0].state, JobState::Downloading);

    run.finish(Plan::Fail(
        ErrorCategory::Postprocess,
        "ffmpeg exited with 1",
    ));
    assert_eq!(h.next_finished(id).await.0, JobState::Failed);
    assert_eq!(
        sink.set_state(JobState::PostProcessing),
        Err(StateError::Ended { id, attempt: 1 })
    );
    sink.set_progress(Some(0.5));
    let view = h.settle();
    assert_eq!(view[0].state, JobState::Failed, "terminal states stay");
    assert_eq!(view[0].progress, None, "progress after the end is ignored");

    let changes: Vec<(JobState, JobState)> = h
        .log
        .iter()
        .filter_map(|event| match event {
            QueueEvent::StateChanged { from, to, .. } => Some((*from, *to)),
            _ => None,
        })
        .collect();
    assert_eq!(
        changes,
        [
            (JobState::Queued, JobState::Probing),
            (JobState::Probing, JobState::Downloading),
            (JobState::Downloading, JobState::Failed),
        ]
    );
    assert!(h.log.contains(&QueueEvent::TransitionRejected {
        id,
        from: JobState::Downloading,
        to: JobState::Ready
    }));
    let progress: Vec<Option<f64>> = h
        .log
        .iter()
        .filter_map(|event| match event {
            QueueEvent::Progress { fraction, .. } => Some(*fraction),
            _ => None,
        })
        .collect();
    assert_eq!(progress, [Some(1.0), None]);
}

#[tokio::test]
async fn details_transfer_and_outcome_reach_events_and_snapshot() {
    let mut h = Harness::new(1);
    let id = h.enqueue(1)[0];
    let run = h.next_run().await;
    let sink = run.sink.clone();
    let details = JobDetails {
        title: Some("Video title".to_owned()),
        uploader: Some("Uploader".to_owned()),
        site: Some("Example".to_owned()),
        published_at: Some(1_700_000_000),
        duration_s: Some(90.5),
        thumbnail_url: Some("https://example.test/thumbnail.jpg".to_owned()),
    };

    sink.set_details(details.clone());
    sink.set_transfer(Some(f64::NAN), Some(-1.0));
    let view = h.settle();
    assert_eq!(view[0].details, Some(details.clone()));
    assert_eq!(view[0].speed_bytes_per_s, None);
    assert_eq!(view[0].eta_s, None);
    assert!(h.log.contains(&QueueEvent::Details {
        id,
        details: details.clone(),
    }));
    assert!(h.log.contains(&QueueEvent::Transfer {
        id,
        speed_bytes_per_s: None,
        eta_s: None,
    }));

    sink.set_transfer(Some(4096.0), Some(8.0));
    let view = h.settle();
    assert_eq!(view[0].speed_bytes_per_s, Some(4096.0));
    assert_eq!(view[0].eta_s, Some(8.0));
    assert!(h.log.contains(&QueueEvent::Transfer {
        id,
        speed_bytes_per_s: Some(4096.0),
        eta_s: Some(8.0),
    }));

    run.finish(Plan::Succeed);
    let expected_outcome = JobOutcome {
        output_path: Some("https://example.test/0.mp4".to_owned()),
        skipped: None,
        ..JobOutcome::default()
    };
    assert_eq!(
        h.next_finished(id).await,
        (JobState::Done, Some(Ok(expected_outcome.clone())))
    );
    let view = h.settle();
    assert_eq!(view[0].outcome, Some(expected_outcome.clone()));
    assert_eq!(view[0].speed_bytes_per_s, None);
    assert_eq!(view[0].eta_s, None);

    let event_count = h.log.len();
    sink.set_details(JobDetails::default());
    sink.set_transfer(Some(1.0), Some(1.0));
    let view = h.settle();
    assert_eq!(view[0].details, Some(details));
    assert_eq!(view[0].outcome, Some(expected_outcome));
    assert_eq!(view[0].speed_bytes_per_s, None);
    assert_eq!(view[0].eta_s, None);
    assert_eq!(h.log.len(), event_count, "ended reports emit no events");
}

#[tokio::test]
async fn phase_reaches_events_and_snapshot_and_clears_when_the_attempt_ends() {
    let mut h = Harness::new(1);
    let id = h.enqueue(1)[0];
    let run = h.next_run().await;

    run.sink.set_phase(Some(JobPhase::PageSearch));
    assert_eq!(h.settle()[0].phase, Some(JobPhase::PageSearch));
    assert!(h.log.contains(&QueueEvent::Phase {
        id,
        phase: Some(JobPhase::PageSearch),
    }));

    run.finish(Plan::Fail(ErrorCategory::PageMediaNotFound, "not found"));
    assert_eq!(h.next_finished(id).await.0, JobState::Failed);
    assert_eq!(h.settle()[0].phase, None);
}

#[tokio::test]
async fn events_announce_each_job_and_snapshot_keeps_enqueue_order() {
    let mut h = Harness::new(0);
    assert_eq!(h.queue.concurrency(), 1, "zero counts as one");
    let ids = h.enqueue(2);
    assert!(ids[0] < ids[1]);
    let first = h.next_run().await;
    first.finish(Plan::Succeed);
    let second = h.next_run().await;
    second.finish(Plan::Succeed);
    assert_eq!(h.next_finished(ids[1]).await.0, JobState::Done);

    let view = h.settle();
    assert_eq!(view.iter().map(|job| job.id).collect::<Vec<_>>(), ids);
    assert_eq!(view[0].url, "https://example.test/0");
    assert_eq!(view[1].attempt, 1);
    assert_eq!(
        h.log[0],
        QueueEvent::JobAdded {
            id: ids[0],
            url: "https://example.test/0".to_owned()
        }
    );
    assert_eq!(
        h.log[1],
        QueueEvent::Started {
            id: ids[0],
            attempt: 1
        }
    );
}

#[tokio::test]
async fn cancel_token_clones_share_one_flag_and_wake_waiters() {
    let token = CancelToken::new();
    let clone = token.clone();
    assert!(!clone.is_cancelled());

    let waiter = tokio::spawn(async move {
        clone.cancelled().await;
        clone.is_cancelled()
    });
    tokio::task::yield_now().await;
    token.cancel();
    assert!(
        timeout(HANG, waiter)
            .await
            .expect("woken")
            .expect("no panic")
    );

    token.cancel();
    assert!(token.is_cancelled());
    timeout(HANG, token.cancelled())
        .await
        .expect("an already cancelled token completes at once");
}

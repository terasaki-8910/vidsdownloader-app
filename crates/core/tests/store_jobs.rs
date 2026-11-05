//! Job lifecycle, error set/clear, crash recovery, and queue reordering.

use std::collections::HashMap;

use vids_core::store::{Job, JobFilter, NewJob, Store};
use vids_core::types::{ErrorCategory, JobState};

fn sample_job(url: &str) -> NewJob {
    NewJob {
        url: url.to_owned(),
        site: None,
        title: None,
        options_json: serde_json::json!({}),
        source_id: None,
    }
}

#[test]
fn insert_job_starts_queued_at_the_end_of_the_queue() {
    let store = Store::open_in_memory().unwrap();
    let id1 = store
        .insert_job(sample_job("https://example.com/1"), 100)
        .unwrap();
    let id2 = store
        .insert_job(sample_job("https://example.com/2"), 200)
        .unwrap();
    let id3 = store
        .insert_job(sample_job("https://example.com/3"), 300)
        .unwrap();

    let job1 = store.get_job(id1).unwrap();
    assert_eq!(job1.state, JobState::Queued);
    assert_eq!(job1.position, 1);
    assert_eq!(job1.attempts, 0);
    assert_eq!(job1.created_at, 100);
    assert_eq!(job1.updated_at, 100);
    assert_eq!(job1.error, None);

    assert_eq!(store.get_job(id2).unwrap().position, 2);
    assert_eq!(store.get_job(id3).unwrap().position, 3);
}

#[test]
fn job_lifecycle_through_info_json_and_output() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .insert_job(sample_job("https://example.com/v"), 100)
        .unwrap();

    store
        .update_job_state(id, JobState::Probing, None, 110)
        .unwrap();
    store
        .set_job_info_json(id, "/tmp/job1.info.json", 120)
        .unwrap();
    store
        .update_job_state(id, JobState::Downloading, None, 130)
        .unwrap();
    store.increment_attempts(id, 140).unwrap();
    store
        .update_job_state(id, JobState::Done, None, 150)
        .unwrap();
    store
        .set_job_output(id, "/downloads/video.mp4", 160)
        .unwrap();

    let job = store.get_job(id).unwrap();
    assert_eq!(job.state, JobState::Done);
    assert_eq!(job.info_json_path, Some("/tmp/job1.info.json".to_owned()));
    assert_eq!(job.output_path, Some("/downloads/video.mp4".to_owned()));
    assert_eq!(job.attempts, 1);
    assert_eq!(job.created_at, 100);
    assert_eq!(job.updated_at, 160);
}

#[test]
fn details_and_finished_attempt_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .insert_job(sample_job("https://example.com/v"), 100)
        .unwrap();
    let details = serde_json::json!({
        "title": "Video title",
        "uploader": "Uploader",
        "duration_s": 42.5
    });

    store.set_job_details(id, &details, 110).unwrap();
    store.increment_attempts(id, 120).unwrap();
    store
        .finish_job_attempt(
            id,
            JobState::Done,
            Some("/downloads/video.mp4"),
            Some("excluded by filter"),
            None,
            None,
            130,
        )
        .unwrap();

    let job = store.get_job(id).unwrap();
    assert_eq!(job.details_json, Some(details));
    assert_eq!(job.output_path.as_deref(), Some("/downloads/video.mp4"));
    assert_eq!(job.skipped.as_deref(), Some("excluded by filter"));
    assert_eq!(job.state, JobState::Done);
    assert_eq!(job.attempts, 1);
    assert_eq!(job.updated_at, 130);

    store.retry_job(id, 140).unwrap();
    let retried = store.get_job(id).unwrap();
    assert_eq!(retried.state, JobState::Queued);
    assert_eq!(retried.attempts, 2);
    assert_eq!(retried.output_path, None);
    assert_eq!(retried.skipped, None);
    assert_eq!(retried.error, None);
    assert_eq!(retried.details_json, job.details_json);
}

#[test]
fn already_saved_round_trips_and_is_cleared_by_retry_and_save_again() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .insert_job(sample_job("https://example.com/v"), 100)
        .unwrap();
    let saved = serde_json::json!({"savedAt": 90, "fileMissing": false, "path": "/a/v.mp4"});
    let finish = |saved: Option<&serde_json::Value>, now: i64| {
        store
            .finish_job_attempt(
                id,
                JobState::Done,
                None,
                Some("already saved"),
                saved,
                None,
                now,
            )
            .unwrap();
    };

    finish(Some(&saved), 110);
    assert_eq!(
        store.get_job(id).unwrap().already_saved_json,
        Some(saved.clone())
    );
    finish(None, 120);
    assert_eq!(store.get_job(id).unwrap().already_saved_json, None);

    finish(Some(&saved), 130);
    store.retry_job(id, 140).unwrap();
    assert_eq!(store.get_job(id).unwrap().already_saved_json, None);

    finish(Some(&saved), 150);
    let options = serde_json::json!({"force_save": true});
    store.save_again_job(id, &options, 160).unwrap();
    let again = store.get_job(id).unwrap();
    assert_eq!(again.state, JobState::Queued);
    assert_eq!(
        again.attempts, 2,
        "the earlier retry counted one, this one more"
    );
    assert_eq!(again.skipped, None);
    assert_eq!(again.already_saved_json, None);
    assert_eq!(again.options_json, options);
    assert_eq!(again.updated_at, 160);
    assert!(store.save_again_job(id + 1, &options, 170).is_err());
}

#[test]
fn finished_failure_round_trips_and_clears_success_fields() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .insert_job(sample_job("https://example.com/v"), 100)
        .unwrap();
    store
        .finish_job_attempt(
            id,
            JobState::Done,
            Some("/downloads/old.mp4"),
            Some("old skip"),
            None,
            None,
            110,
        )
        .unwrap();
    store
        .finish_job_attempt(
            id,
            JobState::Failed,
            None,
            None,
            None,
            Some((ErrorCategory::Network, "timed out")),
            120,
        )
        .unwrap();

    let job = store.get_job(id).unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.output_path, None);
    assert_eq!(job.skipped, None);
    assert_eq!(
        job.error,
        Some((ErrorCategory::Network, "timed out".to_owned()))
    );
}

#[test]
fn update_job_state_sets_and_then_clears_the_error() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .insert_job(sample_job("https://example.com/v"), 100)
        .unwrap();

    store
        .update_job_state(
            id,
            JobState::Failed,
            Some((ErrorCategory::Network, "timed out".to_owned())),
            200,
        )
        .unwrap();
    let job = store.get_job(id).unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(
        job.error,
        Some((ErrorCategory::Network, "timed out".to_owned()))
    );
    assert_eq!(job.updated_at, 200);
    assert_eq!(job.created_at, 100);

    store
        .update_job_state(id, JobState::Queued, None, 300)
        .unwrap();
    let job = store.get_job(id).unwrap();
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.error, None);
    assert_eq!(job.updated_at, 300);
}

#[test]
fn update_job_state_on_missing_id_errors() {
    let store = Store::open_in_memory().unwrap();
    assert!(
        store
            .update_job_state(999, JobState::Queued, None, 1)
            .is_err()
    );
}

#[test]
fn get_job_on_missing_id_errors() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.get_job(999).is_err());
}

#[test]
fn delete_job_removes_the_row() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .insert_job(sample_job("https://example.com/v"), 1)
        .unwrap();
    store.delete_job(id).unwrap();
    assert!(store.get_job(id).is_err());
    // Deleting again is a no-op, not an error.
    assert!(store.delete_job(id).is_ok());
}

#[test]
fn list_jobs_active_excludes_done_failed_and_canceled() {
    let store = Store::open_in_memory().unwrap();
    let queued = store
        .insert_job(sample_job("https://example.com/queued"), 1)
        .unwrap();
    let downloading = store
        .insert_job(sample_job("https://example.com/downloading"), 1)
        .unwrap();
    let done = store
        .insert_job(sample_job("https://example.com/done"), 1)
        .unwrap();
    let failed = store
        .insert_job(sample_job("https://example.com/failed"), 1)
        .unwrap();
    let canceled = store
        .insert_job(sample_job("https://example.com/canceled"), 1)
        .unwrap();

    store
        .update_job_state(downloading, JobState::Downloading, None, 2)
        .unwrap();
    store
        .update_job_state(done, JobState::Done, None, 2)
        .unwrap();
    store
        .update_job_state(failed, JobState::Failed, None, 2)
        .unwrap();
    store
        .update_job_state(canceled, JobState::Canceled, None, 2)
        .unwrap();

    let active: Vec<i64> = store
        .list_jobs(JobFilter::Active)
        .unwrap()
        .iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(active, vec![queued, downloading]);

    let all: Vec<i64> = store
        .list_jobs(JobFilter::All)
        .unwrap()
        .iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(all, vec![queued, downloading, done, failed, canceled]);
}

#[test]
fn recover_interrupted_only_affects_the_four_in_progress_states() {
    let store = Store::open_in_memory().unwrap();
    let states = [
        JobState::Queued,
        JobState::Probing,
        JobState::Ready,
        JobState::Downloading,
        JobState::PostProcessing,
        JobState::Done,
        JobState::Failed,
        JobState::Canceled,
    ];
    let mut ids = Vec::new();
    for (index, &state) in states.iter().enumerate() {
        let id = store
            .insert_job(sample_job(&format!("https://example.com/{index}")), 1)
            .unwrap();
        store.update_job_state(id, state, None, 1).unwrap();
        ids.push(id);
    }

    let changed = store.recover_interrupted(999).unwrap();
    assert_eq!(changed, 4);

    let jobs: HashMap<i64, Job> = store
        .list_jobs(JobFilter::All)
        .unwrap()
        .into_iter()
        .map(|job| (job.id, job))
        .collect();
    for (index, &state) in states.iter().enumerate() {
        let job = &jobs[&ids[index]];
        let expected = match state {
            JobState::Probing
            | JobState::Ready
            | JobState::Downloading
            | JobState::PostProcessing => JobState::Queued,
            other => other,
        };
        assert_eq!(
            job.state, expected,
            "unexpected state for original {state:?}"
        );
        if expected != state {
            assert_eq!(job.updated_at, 999);
        }
    }
}

#[test]
fn move_job_renumbers_so_positions_stay_unique_and_contiguous() {
    let store = Store::open_in_memory().unwrap();
    let ids: Vec<i64> = (0..5)
        .map(|index| {
            store
                .insert_job(sample_job(&format!("https://example.com/{index}")), 1)
                .unwrap()
        })
        .collect();

    // Move the first job (position 1) to position 4.
    store.move_job(ids[0], 4, 2000).unwrap();

    let jobs = store.list_jobs(JobFilter::All).unwrap();
    let positions: Vec<i64> = jobs.iter().map(|job| job.position).collect();
    assert_eq!(positions, vec![1, 2, 3, 4, 5]);

    let order: Vec<i64> = jobs.iter().map(|job| job.id).collect();
    assert_eq!(order, vec![ids[1], ids[2], ids[3], ids[0], ids[4]]);

    // Only the moved job's updated_at changes.
    assert_eq!(store.get_job(ids[0]).unwrap().updated_at, 2000);
    assert_eq!(store.get_job(ids[1]).unwrap().updated_at, 1);
}

#[test]
fn move_job_clamps_out_of_range_positions() {
    let store = Store::open_in_memory().unwrap();
    let ids: Vec<i64> = (0..3)
        .map(|index| {
            store
                .insert_job(sample_job(&format!("https://example.com/{index}")), 1)
                .unwrap()
        })
        .collect();

    store.move_job(ids[2], 0, 10).unwrap(); // below range -> front
    let order: Vec<i64> = store
        .list_jobs(JobFilter::All)
        .unwrap()
        .iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(order, vec![ids[2], ids[0], ids[1]]);

    store.move_job(ids[2], 1000, 20).unwrap(); // above range -> back
    let order: Vec<i64> = store
        .list_jobs(JobFilter::All)
        .unwrap()
        .iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(order, vec![ids[0], ids[1], ids[2]]);
}

#[test]
fn move_job_on_missing_id_errors() {
    let store = Store::open_in_memory().unwrap();
    store
        .insert_job(sample_job("https://example.com/v"), 1)
        .unwrap();
    assert!(store.move_job(999, 1, 2).is_err());
}

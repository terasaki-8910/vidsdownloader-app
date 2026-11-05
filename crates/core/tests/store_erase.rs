//! Erasing a saved item's records without traces (R8-7 A): which jobs go with the library
//! row, refusal while one is still running, the raw database and WAL bytes afterwards, and
//! recording a rename.

use std::fs;
use std::path::Path;

use serde_json::json;
use vids_core::store::{NewJob, NewLibraryItem, Store, StoreError};
use vids_core::types::JobState;

fn new_job(url: &str) -> NewJob {
    NewJob {
        url: url.to_owned(),
        site: None,
        title: None,
        options_json: json!({}),
        source_id: None,
    }
}

fn new_item(path: &str, url: Option<&str>, job_id: Option<i64>) -> NewLibraryItem {
    NewLibraryItem {
        job_id,
        site: Some("Youtube".to_owned()),
        video_id: Some("abc123".to_owned()),
        url: url.map(str::to_owned),
        title: Some("A video".to_owned()),
        uploader: Some("Someone".to_owned()),
        published_at: Some(1_700_000_000),
        duration_s: Some(12.0),
        path: path.to_owned(),
        comments_path: None,
        size_bytes: Some(1024),
    }
}

/// A job that ended `state` with `output_path`.
fn finished_job(store: &Store, url: &str, state: JobState, output_path: Option<&str>) -> i64 {
    let id = store.insert_job(new_job(url), 10).unwrap();
    store
        .finish_job_attempt(id, state, output_path, None, None, None, 20)
        .unwrap();
    id
}

/// A Done job skipped because the video was already saved at `path`.
fn already_saved_job(store: &Store, url: &str, path: &str) -> i64 {
    let id = store.insert_job(new_job(url), 10).unwrap();
    let saved = json!({ "savedAt": 1_700_000_000, "fileMissing": false, "path": path });
    store
        .finish_job_attempt(
            id,
            JobState::Done,
            None,
            Some("already saved"),
            Some(&saved),
            None,
            20,
        )
        .unwrap();
    id
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Raw bytes of the database file and, when present, its write-ahead log.
fn raw_files(db: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = vec![(db.display().to_string(), fs::read(db).unwrap())];
    let wal = db.with_file_name(format!("{}-wal", db.file_name().unwrap().to_string_lossy()));
    if wal.exists() {
        files.push((wal.display().to_string(), fs::read(&wal).unwrap()));
    }
    files
}

#[test]
fn erasing_leaves_no_trace_in_the_database_or_the_wal() {
    const MARKER: &str = "zq7TraceMarkerVx41k";
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("vidsdownloader.db");
    let store = Store::open(&db).unwrap();

    // Unrelated rows share the pages with the erased ones.
    for index in 0..20 {
        let other = finished_job(
            &store,
            &format!("https://example.test/other/{index}"),
            JobState::Done,
            Some(&format!("/videos/other {index}.mp4")),
        );
        store
            .add_library_item(
                new_item(
                    &format!("/videos/other {index}.mp4"),
                    Some(&format!("https://example.test/other/{index}")),
                    Some(other),
                ),
                30,
            )
            .unwrap();
    }

    let url = format!("https://example.test/watch?v={MARKER}");
    let path = format!("/videos/{MARKER} title.mp4");
    let job = store
        .insert_job(
            NewJob {
                url: url.clone(),
                site: Some("Youtube".to_owned()),
                title: Some(format!("{MARKER} title")),
                options_json: json!({ "dest_dir": format!("/videos/{MARKER}") }),
                source_id: None,
            },
            100,
        )
        .unwrap();
    store
        .update_job_state(job, JobState::Probing, None, 101)
        .unwrap();
    store
        .set_job_details(job, &json!({ "title": format!("{MARKER} title") }), 102)
        .unwrap();
    store
        .update_job_state(job, JobState::Downloading, None, 103)
        .unwrap();
    store
        .finish_job_attempt(job, JobState::Done, Some(&path), None, None, None, 104)
        .unwrap();
    let mut item = new_item(&path, Some(&url), Some(job));
    item.title = Some(format!("{MARKER} title"));
    item.uploader = Some(format!("{MARKER} uploader"));
    item.comments_path = Some(format!("/videos/{MARKER} title.comments.ass"));
    let item_id = store.add_library_item(item, 105).unwrap();
    let skipped = already_saved_job(&store, "https://example.test/short-link", &path);

    // The check below is meaningful only if the marker reached the files.
    assert!(store.checkpoint_truncate().unwrap());
    assert!(
        raw_files(&db)
            .iter()
            .any(|(_, bytes)| contains(bytes, MARKER.as_bytes())),
        "the marker was never written"
    );

    let erased = store.erase_library_item(item_id).unwrap();
    assert_eq!(erased.job_ids, vec![job, skipped]);
    assert_eq!(erased.path, path);
    assert!(store.checkpoint_truncate().unwrap());

    for (name, bytes) in raw_files(&db) {
        assert!(
            !contains(&bytes, MARKER.as_bytes()),
            "{name} still holds the marker"
        );
    }
    // The other rows are intact.
    assert!(store.get_library_item(item_id).is_err());
    assert_eq!(store.library_paths_except(item_id).unwrap().len(), 20);
}

#[test]
fn compacting_clears_content_deleted_before_secure_delete() {
    const MARKER: &str = "oldDeleteMarker93Kq";
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("vidsdownloader.db");
    drop(Store::open(&db).unwrap());
    // What an older build left: rows deleted with secure delete off.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(None, "secure_delete", false).unwrap();
        for index in 0..50 {
            conn.execute(
                "INSERT INTO settings (key, value_json) VALUES (?1, ?2)",
                rusqlite::params![format!("old{index}"), format!("\"{MARKER} {index}\"")],
            )
            .unwrap();
        }
        conn.execute("DELETE FROM settings WHERE key LIKE 'old%'", [])
            .unwrap();
        let busy: i64 = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy, 0);
    }
    assert!(
        raw_files(&db)
            .iter()
            .any(|(_, bytes)| contains(bytes, MARKER.as_bytes())),
        "the old delete left nothing to clear"
    );

    let store = Store::open(&db).unwrap();
    store.compact().unwrap();
    assert!(store.checkpoint_truncate().unwrap());

    for (name, bytes) in raw_files(&db) {
        assert!(
            !contains(&bytes, MARKER.as_bytes()),
            "{name} still holds the marker"
        );
    }
}

#[test]
fn erase_takes_the_jobs_that_point_to_the_item_only() {
    let store = Store::open_in_memory().unwrap();
    let url = "https://example.test/post/1";
    let path = "/videos/post 1.mp4";
    let linked = finished_job(&store, url, JobState::Done, Some(path));
    let item = store
        .add_library_item(new_item(path, Some(url), Some(linked)), 30)
        .unwrap();
    // Saved again to the same file: its own library insert failed (unique path).
    let same_path = finished_job(&store, "https://other.test/1", JobState::Done, Some(path));
    let skipped = already_saved_job(&store, "https://example.test/short/1", path);
    // The list job of the same post, which has no library row of its own.
    let list = finished_job(&store, url, JobState::Done, None);
    let failed = finished_job(&store, url, JobState::Failed, None);
    // Another saved video of the same multi-video post keeps its job and row.
    let sibling = finished_job(&store, url, JobState::Done, Some("/videos/post 1 (2).mp4"));
    let sibling_item = store
        .add_library_item(
            new_item("/videos/post 1 (2).mp4", Some(url), Some(sibling)),
            31,
        )
        .unwrap();
    let unrelated = finished_job(
        &store,
        "https://example.test/post/2",
        JobState::Done,
        Some("/videos/post 2.mp4"),
    );

    let expected = vec![linked, same_path, skipped, list, failed];
    assert_eq!(store.library_item_jobs_to_erase(item).unwrap(), expected);
    assert!(
        store.get_library_item(item).is_ok(),
        "the check changes nothing"
    );

    let erased = store.erase_library_item(item).unwrap();
    assert_eq!(erased.job_ids, expected);
    assert_eq!(erased.path, path);
    assert_eq!(erased.comments_path, None);
    assert!(matches!(
        store.get_library_item(item),
        Err(StoreError::NotFound { .. })
    ));
    for id in &expected {
        assert!(matches!(
            store.get_job(*id),
            Err(StoreError::NotFound { .. })
        ));
    }
    assert!(store.get_job(sibling).is_ok());
    assert!(store.get_job(unrelated).is_ok());
    assert_eq!(
        store.get_library_item(sibling_item).unwrap().job_id,
        Some(sibling)
    );
}

#[test]
fn erase_without_a_url_or_linked_job_deletes_only_the_row() {
    let store = Store::open_in_memory().unwrap();
    let other = finished_job(
        &store,
        "https://example.test/x",
        JobState::Done,
        Some("/b.mp4"),
    );
    let item = store
        .add_library_item(new_item("/a.mp4", None, None), 30)
        .unwrap();

    let erased = store.erase_library_item(item).unwrap();
    assert!(erased.job_ids.is_empty());
    assert!(store.get_job(other).is_ok());
    assert!(matches!(
        store.erase_library_item(item),
        Err(StoreError::NotFound { .. })
    ));
}

#[test]
fn an_unfinished_job_refuses_the_erase_and_nothing_is_deleted() {
    for state in [
        JobState::Queued,
        JobState::Probing,
        JobState::Ready,
        JobState::Downloading,
        JobState::PostProcessing,
    ] {
        let store = Store::open_in_memory().unwrap();
        let url = "https://example.test/v";
        let path = "/videos/v.mp4";
        let linked = finished_job(&store, url, JobState::Done, Some(path));
        let item = store
            .add_library_item(new_item(path, Some(url), Some(linked)), 30)
            .unwrap();
        // "Save again" queued for the same video.
        let again = store.insert_job(new_job(url), 40).unwrap();
        store.update_job_state(again, state, None, 41).unwrap();

        let refused = store.erase_library_item(item);
        assert!(
            matches!(refused, Err(StoreError::JobNotFinished { id, state: found })
                if id == again && found == state),
            "{state:?}: {refused:?}"
        );
        assert!(matches!(
            store.library_item_jobs_to_erase(item),
            Err(StoreError::JobNotFinished { .. })
        ));
        assert!(store.get_library_item(item).is_ok());
        assert!(store.get_job(linked).is_ok());
        assert_eq!(store.get_job(again).unwrap().state, state);
    }
}

#[test]
fn checkpoint_of_an_in_memory_store_is_a_no_op() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.checkpoint_truncate().unwrap());
}

#[test]
fn rename_updates_the_row_and_the_linked_job() {
    let store = Store::open_in_memory().unwrap();
    let job = finished_job(
        &store,
        "https://example.test/v",
        JobState::Done,
        Some("/videos/old.mp4"),
    );
    let mut item = new_item("/videos/old.mp4", Some("https://example.test/v"), Some(job));
    item.comments_path = Some("/videos/old.comments.ass".to_owned());
    let id = store.add_library_item(item, 30).unwrap();

    let updated = store
        .rename_library_item(id, "/videos/new.mp4", Some("/videos/new.comments.ass"), 50)
        .unwrap();
    assert_eq!(updated, Some(job));
    let row = store.get_library_item(id).unwrap();
    assert_eq!(row.path, "/videos/new.mp4");
    assert_eq!(
        row.comments_path.as_deref(),
        Some("/videos/new.comments.ass")
    );
    let stored = store.get_job(job).unwrap();
    assert_eq!(stored.output_path.as_deref(), Some("/videos/new.mp4"));
    assert_eq!(stored.updated_at, 50);
}

#[test]
fn rename_of_a_second_image_keeps_the_job_output() {
    // gallery-dl: one job, one library row per image; the job names the first image.
    let store = Store::open_in_memory().unwrap();
    let job = finished_job(
        &store,
        "https://example.test/post",
        JobState::Done,
        Some("/images/post 1.jpg"),
    );
    store
        .add_library_item(
            new_item(
                "/images/post 1.jpg",
                Some("https://example.test/post"),
                Some(job),
            ),
            30,
        )
        .unwrap();
    let second = store
        .add_library_item(
            new_item(
                "/images/post 2.jpg",
                Some("https://example.test/post"),
                Some(job),
            ),
            31,
        )
        .unwrap();

    let updated = store
        .rename_library_item(second, "/images/renamed.jpg", None, 50)
        .unwrap();
    assert_eq!(updated, None);
    assert_eq!(
        store.get_job(job).unwrap().output_path.as_deref(),
        Some("/images/post 1.jpg")
    );
    assert_eq!(
        store.get_library_item(second).unwrap().path,
        "/images/renamed.jpg"
    );
}

#[test]
fn rename_of_a_missing_row_or_onto_another_rows_path_fails_unchanged() {
    let store = Store::open_in_memory().unwrap();
    let first = store
        .add_library_item(new_item("/a.mp4", None, None), 30)
        .unwrap();
    store
        .add_library_item(new_item("/b.mp4", None, None), 31)
        .unwrap();

    assert!(matches!(
        store.rename_library_item(999, "/c.mp4", None, 50),
        Err(StoreError::NotFound { .. })
    ));
    assert!(matches!(
        store.rename_library_item(first, "/b.mp4", None, 50),
        Err(StoreError::Sqlite(_))
    ));
    assert_eq!(store.get_library_item(first).unwrap().path, "/a.mp4");
}

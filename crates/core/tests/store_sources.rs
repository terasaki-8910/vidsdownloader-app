//! Sources CRUD, known-items new/mark semantics (including duplicate input keys),
//! cascading delete, and foreign key enforcement.

use vids_core::store::{NewJob, NewSource, SourceKind, Store};

fn sample_source(name: &str, url: &str) -> NewSource {
    NewSource {
        name: name.to_owned(),
        url: url.to_owned(),
        kind: SourceKind::YtDlp,
        rule_id: None,
        overrides: serde_json::json!({}),
    }
}

#[test]
fn add_source_and_get_roundtrip() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .add_source(sample_source("TED talks", "https://www.ted.com/talks"), 100)
        .unwrap();

    let source = store.get_source(id).unwrap();
    assert_eq!(source.name, "TED talks");
    assert_eq!(source.url, "https://www.ted.com/talks");
    assert_eq!(source.kind, SourceKind::YtDlp);
    assert_eq!(source.rule_id, None);
    assert_eq!(source.overrides, serde_json::json!({}));
    assert_eq!(source.last_checked_at, None);
    assert_eq!(source.created_at, 100);
}

#[test]
fn set_source_checked_records_the_time_and_result_and_overrides_are_replaced() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .add_source(sample_source("s", "https://example.com/s"), 1)
        .unwrap();
    assert_eq!(store.get_source(id).unwrap().last_result, None);

    store
        .set_source_checked(id, 500, &serde_json::json!({"kind": "new", "count": 3}))
        .unwrap();
    let source = store.get_source(id).unwrap();
    assert_eq!(source.last_checked_at, Some(500));
    assert_eq!(
        source.last_result,
        Some(serde_json::json!({"kind": "new", "count": 3}))
    );

    // A later check replaces both.
    store
        .set_source_checked(id, 900, &serde_json::json!({"kind": "none"}))
        .unwrap();
    let source = store.get_source(id).unwrap();
    assert_eq!(source.last_checked_at, Some(900));
    assert_eq!(
        source.last_result,
        Some(serde_json::json!({"kind": "none"}))
    );

    store
        .set_source_overrides(id, serde_json::json!({"destDir": "/videos"}))
        .unwrap();
    store
        .set_source_overrides(id, serde_json::json!({"preset": "upTo720"}))
        .unwrap();
    assert_eq!(
        store.get_source(id).unwrap().overrides,
        serde_json::json!({"preset": "upTo720"})
    );
}

#[test]
fn setters_on_a_missing_source_are_not_found_errors() {
    let store = Store::open_in_memory().unwrap();
    assert!(
        store
            .set_source_checked(42, 1, &serde_json::json!({"kind": "none"}))
            .is_err()
    );
    assert!(store.rename_source(42, "x").is_err());
    assert!(
        store
            .set_source_overrides(42, serde_json::json!({}))
            .is_err()
    );
}

#[test]
fn rename_source_changes_only_the_name() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .add_source(sample_source("old", "https://example.com/s"), 7)
        .unwrap();
    store.rename_source(id, "新しい名前").unwrap();
    let source = store.get_source(id).unwrap();
    assert_eq!(source.name, "新しい名前");
    assert_eq!(source.url, "https://example.com/s");
    assert_eq!(source.created_at, 7);
}

#[test]
fn source_by_url_finds_exactly_that_url() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .add_source(sample_source("a", "https://example.com/a"), 1)
        .unwrap();
    store
        .add_source(sample_source("b", "https://example.com/b"), 1)
        .unwrap();

    assert_eq!(
        store
            .source_by_url("https://example.com/a")
            .unwrap()
            .map(|source| source.id),
        Some(id)
    );
    assert_eq!(store.source_by_url("https://example.com/a/").unwrap(), None);
    assert_eq!(store.source_by_url("https://example.com/c").unwrap(), None);
}

#[test]
fn delete_source_drops_its_known_items_only() {
    let store = Store::open_in_memory().unwrap();
    let gone = store
        .add_source(sample_source("gone", "https://example.com/gone"), 1)
        .unwrap();
    let kept = store
        .add_source(sample_source("kept", "https://example.com/kept"), 1)
        .unwrap();
    store.mark_known(gone, &["a".to_owned()], 2).unwrap();
    store.mark_known(kept, &["a".to_owned()], 2).unwrap();

    store.delete_source(gone).unwrap();

    assert!(store.get_source(gone).is_err());
    assert!(
        store
            .filter_new(kept, &["a".to_owned()])
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.list_sources().unwrap().len(), 1);
}

#[test]
fn list_sources_returns_every_source() {
    let store = Store::open_in_memory().unwrap();
    store
        .add_source(sample_source("a", "https://example.com/a"), 1)
        .unwrap();
    store
        .add_source(sample_source("b", "https://example.com/b"), 1)
        .unwrap();
    assert_eq!(store.list_sources().unwrap().len(), 2);
}

#[test]
fn mark_known_ignores_already_known_and_duplicate_input_keys() {
    let store = Store::open_in_memory().unwrap();
    let source_id = store
        .add_source(sample_source("s", "https://example.com/s"), 1)
        .unwrap();

    store
        .mark_known(
            source_id,
            &["a".to_owned(), "b".to_owned(), "a".to_owned()],
            10,
        )
        .unwrap();

    let new_keys = store
        .filter_new(
            source_id,
            &[
                "a".to_owned(),
                "c".to_owned(),
                "c".to_owned(),
                "b".to_owned(),
            ],
        )
        .unwrap();
    assert_eq!(new_keys, vec!["c".to_owned()]);
}

#[test]
fn filter_new_preserves_input_order_when_nothing_is_known_yet() {
    let store = Store::open_in_memory().unwrap();
    let source_id = store
        .add_source(sample_source("s", "https://example.com/s"), 1)
        .unwrap();

    let order = store
        .filter_new(source_id, &["z".to_owned(), "y".to_owned(), "x".to_owned()])
        .unwrap();
    assert_eq!(order, vec!["z".to_owned(), "y".to_owned(), "x".to_owned()]);
}

#[test]
fn deleting_a_source_cascades_known_items_and_nulls_job_source_id() {
    let store = Store::open_in_memory().unwrap();
    let source_id = store
        .add_source(sample_source("s", "https://example.com/s"), 1)
        .unwrap();
    store.mark_known(source_id, &["a".to_owned()], 10).unwrap();
    let job_id = store
        .insert_job(
            NewJob {
                url: "https://example.com/v".to_owned(),
                site: None,
                title: None,
                options_json: serde_json::json!({}),
                source_id: Some(source_id),
            },
            20,
        )
        .unwrap();

    store.delete_source(source_id).unwrap();

    assert!(store.get_source(source_id).is_err());

    // The known_items row was cascaded away: "a" reads as new again.
    let still_new = store.filter_new(source_id, &["a".to_owned()]).unwrap();
    assert_eq!(still_new, vec!["a".to_owned()]);

    assert_eq!(store.get_job(job_id).unwrap().source_id, None);
}

#[test]
fn delete_source_on_missing_id_is_idempotent() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.delete_source(42).is_ok());
}

#[test]
fn foreign_keys_are_enforced_for_known_items_with_a_missing_source() {
    let store = Store::open_in_memory().unwrap();
    let result = store.mark_known(999_999, &["x".to_owned()], 1);
    assert!(result.is_err());
}

#[test]
fn a_file_database_cascades_known_items_on_every_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sources.sqlite3");
    let id = {
        let store = Store::open(&path).unwrap();
        let id = store
            .add_source(sample_source("s", "https://example.com/s"), 1)
            .unwrap();
        store.mark_known(id, &["a".to_owned()], 2).unwrap();
        id
    };

    // A second connection (as after an app restart) enforces the cascade too.
    let store = Store::open(&path).unwrap();
    store.delete_source(id).unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM known_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(left, 0);
}

//! Migration idempotency and basic open/apply behavior.

use vids_core::store::{NewJob, Store};

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
fn reopening_an_up_to_date_database_changes_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("vids.sqlite3");

    {
        let store = Store::open(&path).expect("first open applies migrations");
        let id = store
            .insert_job(sample_job("https://example.com/v1"), 1)
            .expect("insert job");
        assert_eq!(id, 1);
    }

    // Reopening an already-migrated database must not fail (e.g. by re-running the
    // `CREATE TABLE` statements) and must keep the data written through the first
    // connection.
    let store = Store::open(&path).expect("second open is a no-op migration");
    let job = store.get_job(1).expect("job survives reopen");
    assert_eq!(job.url, "https://example.com/v1");

    let id2 = store
        .insert_job(sample_job("https://example.com/v2"), 2)
        .expect("insert after reopen");
    assert_eq!(id2, 2);

    // A third open, for good measure: still a no-op.
    drop(store);
    let store = Store::open(&path).expect("third open is also a no-op migration");
    assert_eq!(
        store
            .list_jobs(vids_core::store::JobFilter::All)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn open_in_memory_is_ready_to_use_immediately() {
    let store = Store::open_in_memory().expect("in-memory open applies migrations");
    let id = store
        .insert_job(sample_job("https://example.com"), 1)
        .expect("insert job");
    assert_eq!(id, 1);
}

#[test]
fn version_one_database_gains_queue_persistence_columns() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v1.sqlite3");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        // Every real database has `sources` since v1; v4 adds a column to it.
        conn.execute_batch(
            "CREATE TABLE rules (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                match_pattern TEXT NOT NULL,
                rule_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE sources (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                url TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL,
                rule_id INTEGER,
                overrides_json TEXT NOT NULL DEFAULT '{}',
                last_checked_at INTEGER,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE jobs (
                id INTEGER PRIMARY KEY,
                url TEXT NOT NULL,
                site TEXT,
                state TEXT NOT NULL,
                title TEXT,
                info_json_path TEXT,
                output_path TEXT,
                error_category TEXT,
                error_message TEXT,
                options_json TEXT NOT NULL,
                source_id INTEGER,
                position INTEGER NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            INSERT INTO jobs
                (id, url, state, options_json, position, attempts, created_at, updated_at)
            VALUES (7, 'https://example.com/old', 'done', '{}', 1, 1, 10, 20);
            PRAGMA user_version = 1;",
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("v1 migrates to the latest schema");
    let old = store.get_job(7).expect("v1 row remains readable");
    assert_eq!(old.details_json, None);
    assert_eq!(old.skipped, None);
    let details = serde_json::json!({"title": "restored"});
    store.set_job_details(7, &details, 30).unwrap();
    store
        .finish_job_attempt(
            7,
            vids_core::types::JobState::Done,
            None,
            Some("skip"),
            None,
            None,
            40,
        )
        .unwrap();
    assert_eq!(store.get_job(7).unwrap().details_json, Some(details));
    assert_eq!(store.get_job(7).unwrap().skipped.as_deref(), Some("skip"));
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[test]
fn version_two_database_gains_the_already_saved_column() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v2.sqlite3");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        // Every real database has `sources` since v1; v4 adds a column to it.
        conn.execute_batch(
            "CREATE TABLE rules (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                match_pattern TEXT NOT NULL,
                rule_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE sources (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                url TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL,
                rule_id INTEGER,
                overrides_json TEXT NOT NULL DEFAULT '{}',
                last_checked_at INTEGER,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE jobs (
                id INTEGER PRIMARY KEY,
                url TEXT NOT NULL,
                site TEXT,
                state TEXT NOT NULL,
                title TEXT,
                info_json_path TEXT,
                output_path TEXT,
                error_category TEXT,
                error_message TEXT,
                options_json TEXT NOT NULL,
                source_id INTEGER,
                position INTEGER NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                details_json TEXT,
                skipped TEXT
            );
            INSERT INTO jobs
                (id, url, state, skipped, options_json, position, attempts, created_at,
                 updated_at)
            VALUES (3, 'https://example.com/v2', 'done', 'old skip', '{}', 1, 1, 10, 20);
            PRAGMA user_version = 2;",
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("v2 migrates to the latest schema");
    let old = store.get_job(3).expect("v2 row remains readable");
    assert_eq!(old.skipped.as_deref(), Some("old skip"));
    assert_eq!(old.already_saved_json, None);

    let saved = serde_json::json!({"savedAt": 5, "fileMissing": false, "path": "/a.mp4"});
    store
        .finish_job_attempt(
            3,
            vids_core::types::JobState::Done,
            None,
            Some("already saved"),
            Some(&saved),
            None,
            30,
        )
        .unwrap();
    assert_eq!(store.get_job(3).unwrap().already_saved_json, Some(saved));
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[test]
fn version_three_database_gains_the_source_result_column() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v3.sqlite3");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE rules (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                match_pattern TEXT NOT NULL,
                rule_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE sources (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                url TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL CHECK(kind IN ('ytdlp','rule')),
                rule_id INTEGER REFERENCES rules(id) ON DELETE SET NULL,
                overrides_json TEXT NOT NULL DEFAULT '{}',
                last_checked_at INTEGER,
                created_at INTEGER NOT NULL
            );
            INSERT INTO sources (id, name, url, kind, overrides_json, last_checked_at, created_at)
            VALUES (5, 'Old', 'https://example.com/old', 'ytdlp', '{}', 50, 10);
            PRAGMA user_version = 3;",
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("v3 migrates to the latest schema");
    let old = store.get_source(5).expect("v3 row remains readable");
    assert_eq!(old.name, "Old");
    assert_eq!(old.last_checked_at, Some(50));
    assert_eq!(old.last_result, None);

    let result = serde_json::json!({"kind": "error", "category": "Network", "message": "x"});
    store.set_source_checked(5, 60, &result).unwrap();
    assert_eq!(store.get_source(5).unwrap().last_result, Some(result));
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[test]
fn version_five_methods_gain_empty_options_and_no_last_use() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v5.sqlite3");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE rules (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                match_pattern TEXT NOT NULL,
                rule_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE flows (
                id INTEGER PRIMARY KEY,
                site_id INTEGER,
                host TEXT NOT NULL,
                name TEXT NOT NULL,
                start_url TEXT NOT NULL,
                steps_json TEXT NOT NULL,
                scopes_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            INSERT INTO rules VALUES
                (2, 'Old rule', 'https://example.com/*', '{\"name\":\"Old rule\"}', 10, 10);
            INSERT INTO flows VALUES
                (3, NULL, 'example.com', 'Old flow', 'https://example.com/', '[]', '[]', 20, 20);
            PRAGMA user_version = 5;",
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("v5 migrates to v6");
    let rule = store.get_rule(2).unwrap();
    assert_eq!(rule.options, serde_json::json!({}));
    assert_eq!(rule.last_used_at, None);
    let flow = store.list_all_flows().unwrap().remove(0);
    assert_eq!(flow.options, serde_json::json!({}));
    assert_eq!(flow.last_used_at, None);
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[test]
fn version_six_database_gains_saved_pages() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("v6.sqlite3");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE rules (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                match_pattern TEXT NOT NULL,
                rule_json TEXT NOT NULL,
                options_json TEXT NOT NULL DEFAULT '{}',
                last_used_at INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            INSERT INTO rules (id, name, match_pattern, rule_json, created_at, updated_at) VALUES
                (2, 'Old rule', 'https://example.com/*', '{\"name\":\"Old rule\"}', 10, 10);
            PRAGMA user_version = 6;",
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("v6 migrates to v7");
    assert_eq!(store.get_rule(2).unwrap().name, "Old rule");
    assert!(store.list_saved_pages().unwrap().is_empty());
    let page = store
        .add_saved_page("https://example.com/list?page=2#top", "List", 50)
        .unwrap();
    assert_eq!(page.url, "https://example.com/list?page=2");
    drop(store);

    // The saved page survives a reopen, and the version stays at 7.
    let store = Store::open(&path).expect("v7 reopens");
    assert_eq!(store.list_saved_pages().unwrap(), vec![page]);
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'saved_pages'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(sql.contains("url TEXT NOT NULL UNIQUE"), "{sql}");
}

#[test]
fn a_database_from_a_newer_build_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("newer.sqlite3");
    {
        let store = Store::open(&path).expect("a new database is created at the latest version");
        drop(store);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 8).unwrap();
    }

    match Store::open(&path) {
        Err(vids_core::store::StoreError::UnsupportedSchemaVersion(8)) => {}
        other => panic!(
            "expected the newer database to be refused, got {:?}",
            other.err()
        ),
    }
}

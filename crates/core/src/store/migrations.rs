//! Schema creation and version migrations, tracked via `PRAGMA user_version`.

use rusqlite::Connection;

use super::StoreError;

/// Schema version 1. Tables are created in dependency order so every `REFERENCES`
/// target already exists; column names and types are the on-disk contract used by
/// every other file in this module.
const SCHEMA_V1: &str = "
    CREATE TABLE rules (
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
        source_id INTEGER REFERENCES sources(id) ON DELETE SET NULL,
        position INTEGER NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );

    CREATE TABLE library_items (
        id INTEGER PRIMARY KEY,
        job_id INTEGER REFERENCES jobs(id) ON DELETE SET NULL,
        site TEXT,
        video_id TEXT,
        url TEXT,
        title TEXT,
        uploader TEXT,
        published_at INTEGER,
        duration_s REAL,
        path TEXT NOT NULL UNIQUE,
        comments_path TEXT,
        size_bytes INTEGER,
        added_at INTEGER NOT NULL,
        missing INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE known_items (
        source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
        item_key TEXT NOT NULL,
        first_seen_at INTEGER NOT NULL,
        PRIMARY KEY(source_id, item_key)
    );

    CREATE TABLE site_profiles (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        profile_json TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    );

    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value_json TEXT NOT NULL
    );

    CREATE INDEX idx_jobs_position ON jobs(position);
    CREATE INDEX idx_jobs_source_id ON jobs(source_id);
    CREATE INDEX idx_library_items_added_at ON library_items(added_at, id);
    CREATE INDEX idx_library_items_site ON library_items(site);
";

/// Queue metadata and successful-skip outcomes needed to restore the exact row
/// shown before an app restart.
const SCHEMA_V2: &str = "
    ALTER TABLE jobs ADD COLUMN details_json TEXT;
    ALTER TABLE jobs ADD COLUMN skipped TEXT;
";

/// Where a job that was skipped because its video is already saved found it, so the
/// row can be restored after an app restart.
const SCHEMA_V3: &str = "
    ALTER TABLE jobs ADD COLUMN already_saved_json TEXT;
";

/// What a source's last check found (new items queued, none, or the error), shown with
/// the source until its next check.
const SCHEMA_V4: &str = "
    ALTER TABLE sources ADD COLUMN last_result_json TEXT;
";

/// Browser click flows are looked up by the exact host they were recorded on.
/// `site_id` is only for grouping them under registered sites in Settings; a
/// flow remains available by host if that site is removed.
const SCHEMA_V5: &str = "
    CREATE TABLE flows (
        id INTEGER PRIMARY KEY,
        site_id INTEGER REFERENCES site_profiles(id) ON DELETE SET NULL,
        host TEXT NOT NULL,
        name TEXT NOT NULL,
        start_url TEXT NOT NULL,
        steps_json TEXT NOT NULL,
        scopes_json TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX idx_flows_host ON flows(host);
    CREATE INDEX idx_flows_site_id ON flows(site_id);
";

/// Per-method queue overrides and the time each method was last used explicitly.
const SCHEMA_V6: &str = "
    ALTER TABLE rules ADD COLUMN options_json TEXT NOT NULL DEFAULT '{}';
    ALTER TABLE rules ADD COLUMN last_used_at INTEGER;
    ALTER TABLE flows ADD COLUMN options_json TEXT NOT NULL DEFAULT '{}';
    ALTER TABLE flows ADD COLUMN last_used_at INTEGER;
";

/// Every schema version in order, paired with the SQL that creates it from the
/// previous one. Add a new `(version, sql)` entry here for the next migration;
/// never change an entry that has already shipped.
const MIGRATIONS: &[(i64, &str)] = &[
    (1, SCHEMA_V1),
    (2, SCHEMA_V2),
    (3, SCHEMA_V3),
    (4, SCHEMA_V4),
    (5, SCHEMA_V5),
    (6, SCHEMA_V6),
];

/// Apply any migrations this connection is missing, each inside its own
/// transaction, advancing `PRAGMA user_version` as it goes. A database already at
/// the latest version is left untouched.
pub(super) fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let mut current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for &(version, sql) in MIGRATIONS {
        if version <= current {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        current = version;
    }
    let latest = MIGRATIONS.last().map_or(0, |&(version, _)| version);
    if current > latest {
        return Err(StoreError::UnsupportedSchemaVersion(current));
    }
    Ok(())
}

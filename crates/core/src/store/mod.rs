//! SQLite storage: jobs, library, sources, known items, rules, site profiles and
//! settings.
//!
//! [`Store`] wraps one `rusqlite` connection. Opening a database (file or
//! in-memory) applies any pending migrations tracked by `PRAGMA user_version`;
//! opening a database already at the latest version changes nothing. Every write
//! method takes the current unix time as `now: i64` so callers (and tests) control
//! timestamps instead of the store reading the clock itself.
//!
//! Every connection runs with `PRAGMA secure_delete = ON`: deleted rows are overwritten
//! with zeros instead of lingering in free pages, so erasing a saved item leaves no trace
//! once [`Store::checkpoint_truncate`] has also emptied the write-ahead log.
//!
//! Enums shared with the rest of the crate ([`crate::types::JobState`],
//! [`crate::types::ErrorCategory`]) are stored as snake_case TEXT; reading a value
//! that is not one of the known variants is a [`StoreError`], never a panic.
//! Free-form data ([`crate::naming::NameTemplate`], [`crate::filters::Filter`], and
//! site/rule-specific settings) is stored as opaque JSON (`serde_json::Value`): this
//! module only persists it, it does not interpret it.

mod error;
mod flows;
mod jobs;
mod known_items;
mod library;
mod migrations;
mod rules;
mod settings;
mod site_profiles;
mod sources;

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

pub use error::StoreError;
pub use flows::{Flow, NewFlow};
pub use jobs::{Job, JobFilter, NewJob};
pub use library::{ErasedItem, LibraryItem, LibraryQuery, NewLibraryItem};
pub use rules::Rule;
pub use site_profiles::SiteProfile;
pub use sources::{NewSource, Source, SourceKind};

/// A SQLite-backed store for jobs, the library, sources, rules, site profiles and
/// settings.
///
/// Methods take `&self`: a `rusqlite::Connection` does not need exclusive access to
/// run statements, so the store does not need interior mutability. `Store` is
/// `Send` but not `Sync`; share one across threads behind a `Mutex` (or give each
/// thread its own `Store` pointed at the same file, relying on `busy_timeout`).
pub struct Store {
    conn: Connection,
    /// `:memory:` databases have no write-ahead log to checkpoint.
    in_memory: bool,
}

/// How many times [`Store::checkpoint_truncate`] tries again while readers block it.
const CHECKPOINT_RETRIES: u32 = 5;
/// Wait between two checkpoint attempts.
const CHECKPOINT_RETRY_DELAY: Duration = Duration::from_millis(100);

impl Store {
    /// Open (or create) a file-backed database at `path`: WAL journaling, foreign
    /// keys enforced, secure delete on, and a 5 second busy timeout so a second
    /// connection waits out brief write contention instead of failing immediately.
    /// Applies any pending migrations.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        Self::from_connection(conn, false)
    }

    /// Open a private, in-memory database (for tests and other short-lived use).
    /// Foreign keys are enforced and secure delete is on; WAL journaling does not
    /// apply to `:memory:` databases and is not requested.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn, true)
    }

    fn from_connection(conn: Connection, in_memory: bool) -> Result<Self, StoreError> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "secure_delete", true)?;
        migrations::migrate(&conn)?;
        Ok(Self { conn, in_memory })
    }

    /// Copy the write-ahead log into the database file and truncate the log to zero
    /// bytes (`PRAGMA wal_checkpoint(TRUNCATE)`), so content deleted earlier no longer
    /// survives in the log. While readers keep it from completing it is tried again, up
    /// to 5 more times 100 ms apart. Returns whether it completed; an in-memory store has
    /// no log and returns `Ok(true)` at once.
    ///
    /// Blocks the calling thread while it waits.
    pub fn checkpoint_truncate(&self) -> Result<bool, StoreError> {
        if self.in_memory {
            return Ok(true);
        }
        for attempt in 0..=CHECKPOINT_RETRIES {
            if attempt > 0 {
                std::thread::sleep(CHECKPOINT_RETRY_DELAY);
            }
            let busy: i64 = self
                .conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
            if busy == 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Rebuild the database file (`VACUUM`), so no free page keeps content deleted before
    /// secure delete was turned on (databases written by builds older than it). Its pages
    /// pass through the write-ahead log: call [`Store::checkpoint_truncate`] afterwards.
    pub fn compact(&self) -> Result<(), StoreError> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }
}

/// Turn "0 rows changed" from an `UPDATE ... WHERE id = ?` into a not-found error.
/// Shared by every submodule whose setters address one existing row by id.
fn require_row(changed: usize, entity: &'static str, id: i64) -> Result<(), StoreError> {
    if changed == 0 {
        Err(StoreError::NotFound { entity, id })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secure_delete(store: &Store) -> i64 {
        store
            .conn
            .query_row("PRAGMA secure_delete", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn secure_delete_is_on_for_file_and_memory_stores() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            secure_delete(&Store::open(dir.path().join("test.db")).unwrap()),
            1
        );
        assert_eq!(secure_delete(&Store::open_in_memory().unwrap()), 1);
    }
}

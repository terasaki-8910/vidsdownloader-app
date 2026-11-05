//! Items already seen for a source, used to find new items on the next check.

use std::collections::HashSet;

use rusqlite::params;

use super::{Store, StoreError};

impl Store {
    /// Of `keys`, return the ones not yet known for `source_id`, in input order,
    /// without duplicates.
    pub fn filter_new(&self, source_id: i64, keys: &[String]) -> Result<Vec<String>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM known_items WHERE source_id = ?1 AND item_key = ?2")?;
        let mut seen = HashSet::with_capacity(keys.len());
        let mut new_keys = Vec::new();
        for key in keys {
            if !seen.insert(key.as_str()) {
                continue;
            }
            if !stmt.exists(params![source_id, key])? {
                new_keys.push(key.clone());
            }
        }
        Ok(new_keys)
    }

    /// Record `keys` as known for `source_id`. Keys already known, or repeated
    /// within `keys`, are left as they were (first-seen time is not updated).
    pub fn mark_known(&self, source_id: i64, keys: &[String], now: i64) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO known_items (source_id, item_key, first_seen_at) \
                 VALUES (?1, ?2, ?3)",
            )?;
            for key in keys {
                stmt.execute(params![source_id, key, now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

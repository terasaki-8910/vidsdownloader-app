//! Arbitrary app settings, keyed by name, each stored as a JSON value.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{Store, StoreError};

impl Store {
    /// Read and deserialize a setting, or `None` if `key` has never been set.
    pub fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, StoreError> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(StoreError::from)
    }

    /// Serialize and store a setting, replacing any previous value.
    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<(), StoreError> {
        let value_json = serde_json::to_string(value)?;
        self.conn.execute(
            "INSERT INTO settings (key, value_json) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![key, value_json],
        )?;
        Ok(())
    }

    /// Delete a setting. A no-op if it was never set.
    pub fn delete_setting(&self, key: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
    }
}

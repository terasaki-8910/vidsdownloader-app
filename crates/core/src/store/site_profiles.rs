//! Per-site profiles (URL patterns, login, naming/filter overrides, comment
//! handling), addressed by name and exportable as JSON.

use rusqlite::{OptionalExtension, Row, params};
use serde_json::Value;

use super::{Store, StoreError};

/// A row from `site_profiles`.
#[derive(Debug, Clone, PartialEq)]
pub struct SiteProfile {
    pub id: i64,
    pub name: String,
    pub profile_json: Value,
    pub updated_at: i64,
}

const SITE_PROFILE_COLUMNS: &str = "id, name, profile_json, updated_at";

struct RawProfile {
    id: i64,
    name: String,
    profile_json: String,
    updated_at: i64,
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<RawProfile> {
    Ok(RawProfile {
        id: row.get("id")?,
        name: row.get("name")?,
        profile_json: row.get("profile_json")?,
        updated_at: row.get("updated_at")?,
    })
}

impl RawProfile {
    fn into_profile(self) -> Result<SiteProfile, StoreError> {
        Ok(SiteProfile {
            id: self.id,
            name: self.name,
            profile_json: serde_json::from_str(&self.profile_json)?,
            updated_at: self.updated_at,
        })
    }
}

impl Store {
    /// Insert a new site profile, or replace an existing one with the same
    /// `name`. Returns its id either way.
    pub fn upsert_site_profile(
        &self,
        name: &str,
        profile_json: Value,
        now: i64,
    ) -> Result<i64, StoreError> {
        let profile_json = serde_json::to_string(&profile_json)?;
        let id = self.conn.query_row(
            "INSERT INTO site_profiles (name, profile_json, updated_at) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(name) DO UPDATE SET \
                profile_json = excluded.profile_json, \
                updated_at = excluded.updated_at \
             RETURNING id",
            params![name, profile_json, now],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// Fetch a site profile by name, or `None` if it has no saved profile.
    pub fn get_site_profile(&self, name: &str) -> Result<Option<SiteProfile>, StoreError> {
        let sql = format!("SELECT {SITE_PROFILE_COLUMNS} FROM site_profiles WHERE name = ?1");
        let raw = self
            .conn
            .query_row(&sql, params![name], map_row)
            .optional()?;
        raw.map(RawProfile::into_profile).transpose()
    }

    /// List every saved site profile, ordered by name.
    pub fn list_site_profiles(&self) -> Result<Vec<SiteProfile>, StoreError> {
        let sql = format!("SELECT {SITE_PROFILE_COLUMNS} FROM site_profiles ORDER BY name ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], map_row)?;
        let mut profiles = Vec::new();
        for row in rows {
            profiles.push(row?.into_profile()?);
        }
        Ok(profiles)
    }

    /// Delete a site profile by name. A no-op if it does not exist.
    pub fn delete_site_profile(&self, name: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM site_profiles WHERE name = ?1", params![name])?;
        Ok(())
    }
}

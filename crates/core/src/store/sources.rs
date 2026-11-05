//! Registered listings (a yt-dlp list or a saved rule) with optional overrides.

use rusqlite::{OptionalExtension, Row, params};
use serde_json::Value;

use super::{Store, StoreError, require_row};

/// Where a source's items come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A list yt-dlp already understands (`--flat-playlist`).
    YtDlp,
    /// A saved collecting rule (see `rules`).
    Rule,
}

fn source_kind_str(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::YtDlp => "ytdlp",
        SourceKind::Rule => "rule",
    }
}

fn parse_source_kind(value: &str) -> Result<SourceKind, StoreError> {
    match value {
        "ytdlp" => Ok(SourceKind::YtDlp),
        "rule" => Ok(SourceKind::Rule),
        other => Err(StoreError::InvalidEnumValue {
            type_name: "SourceKind",
            value: other.to_owned(),
        }),
    }
}

/// Input for [`Store::add_source`].
#[derive(Debug, Clone)]
pub struct NewSource {
    pub name: String,
    pub url: String,
    pub kind: SourceKind,
    pub rule_id: Option<i64>,
    /// Opaque per-source naming/filter overrides, interpreted elsewhere.
    pub overrides: Value,
}

/// A row from `sources`.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub kind: SourceKind,
    pub rule_id: Option<i64>,
    pub overrides: Value,
    pub last_checked_at: Option<i64>,
    /// What the last check found, as opaque JSON interpreted by the app; `None` before
    /// the first check (and for rows written before schema v4).
    pub last_result: Option<Value>,
    pub created_at: i64,
}

const SOURCE_COLUMNS: &str = "id, name, url, kind, rule_id, overrides_json, last_checked_at, \
                              last_result_json, created_at";

struct RawSource {
    id: i64,
    name: String,
    url: String,
    kind: String,
    rule_id: Option<i64>,
    overrides_json: String,
    last_checked_at: Option<i64>,
    last_result_json: Option<String>,
    created_at: i64,
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<RawSource> {
    Ok(RawSource {
        id: row.get("id")?,
        name: row.get("name")?,
        url: row.get("url")?,
        kind: row.get("kind")?,
        rule_id: row.get("rule_id")?,
        overrides_json: row.get("overrides_json")?,
        last_checked_at: row.get("last_checked_at")?,
        last_result_json: row.get("last_result_json")?,
        created_at: row.get("created_at")?,
    })
}

impl RawSource {
    fn into_source(self) -> Result<Source, StoreError> {
        Ok(Source {
            id: self.id,
            name: self.name,
            url: self.url,
            kind: parse_source_kind(&self.kind)?,
            rule_id: self.rule_id,
            overrides: serde_json::from_str(&self.overrides_json)?,
            last_checked_at: self.last_checked_at,
            last_result: self
                .last_result_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            created_at: self.created_at,
        })
    }
}

impl Store {
    /// Register a new source.
    pub fn add_source(&self, new_source: NewSource, now: i64) -> Result<i64, StoreError> {
        let overrides_json = serde_json::to_string(&new_source.overrides)?;
        self.conn.execute(
            "INSERT INTO sources \
                (name, url, kind, rule_id, overrides_json, last_checked_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
            params![
                new_source.name,
                new_source.url,
                source_kind_str(new_source.kind),
                new_source.rule_id,
                overrides_json,
                now,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List every source, ordered by id.
    pub fn list_sources(&self) -> Result<Vec<Source>, StoreError> {
        let sql = format!("SELECT {SOURCE_COLUMNS} FROM sources ORDER BY id ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], map_row)?;
        let mut sources = Vec::new();
        for row in rows {
            sources.push(row?.into_source()?);
        }
        Ok(sources)
    }

    /// Fetch one source by id.
    pub fn get_source(&self, id: i64) -> Result<Source, StoreError> {
        let sql = format!("SELECT {SOURCE_COLUMNS} FROM sources WHERE id = ?1");
        let raw = self
            .conn
            .query_row(&sql, params![id], map_row)
            .optional()?
            .ok_or(StoreError::NotFound {
                entity: "source",
                id,
            })?;
        raw.into_source()
    }

    /// Fetch the source registered for exactly this URL, if any.
    pub fn source_by_url(&self, url: &str) -> Result<Option<Source>, StoreError> {
        let sql = format!("SELECT {SOURCE_COLUMNS} FROM sources WHERE url = ?1");
        self.conn
            .query_row(&sql, params![url], map_row)
            .optional()?
            .map(RawSource::into_source)
            .transpose()
    }

    /// Record that a source was just checked for new items, and what the check found.
    pub fn set_source_checked(&self, id: i64, now: i64, result: &Value) -> Result<(), StoreError> {
        let result_json = serde_json::to_string(result)?;
        let changed = self.conn.execute(
            "UPDATE sources SET last_checked_at = ?1, last_result_json = ?2 WHERE id = ?3",
            params![now, result_json, id],
        )?;
        require_row(changed, "source", id)
    }

    /// Rename a source.
    pub fn rename_source(&self, id: i64, name: &str) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE sources SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        require_row(changed, "source", id)
    }

    /// Replace a source's overrides (destination, quality, naming, filters).
    pub fn set_source_overrides(&self, id: i64, overrides: Value) -> Result<(), StoreError> {
        let overrides_json = serde_json::to_string(&overrides)?;
        let changed = self.conn.execute(
            "UPDATE sources SET overrides_json = ?1 WHERE id = ?2",
            params![overrides_json, id],
        )?;
        require_row(changed, "source", id)
    }

    /// Delete a source. Cascades to its `known_items`; jobs that referenced it
    /// keep their row with `source_id` set to `NULL`. A no-op if it does not
    /// exist.
    pub fn delete_source(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM sources WHERE id = ?1", params![id])?;
        Ok(())
    }
}

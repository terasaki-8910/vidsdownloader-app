//! Pages saved from the in-app browser, listed on its start page.

use rusqlite::{Row, params};
use url::Url;

use super::{Store, StoreError, require_row};

/// The longest URL a saved page may have, in characters (the `#fragment` is not part of it).
pub const MAX_SAVED_URL_CHARS: usize = 2048;
/// The longest name a saved page may have, in characters, after trimming.
pub const MAX_SAVED_NAME_CHARS: usize = 200;

/// A row from `saved_pages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPage {
    pub id: i64,
    /// The page's URL as it was shown, query included, without a `#fragment`.
    pub url: String,
    pub name: String,
    pub created_at: i64,
}

const SAVED_PAGE_COLUMNS: &str = "id, url, name, created_at";

fn map_row(row: &Row<'_>) -> rusqlite::Result<SavedPage> {
    Ok(SavedPage {
        id: row.get("id")?,
        url: row.get("url")?,
        name: row.get("name")?,
        created_at: row.get("created_at")?,
    })
}

/// The URL as it is stored: trimmed, cut at the first `#`, and otherwise left exactly as it was
/// (no re-encoding). It must be an http(s) URL with a host, without whitespace or control
/// characters, and at most [`MAX_SAVED_URL_CHARS`] characters long.
fn clean_url(url: &str) -> Result<String, StoreError> {
    let invalid = |message: &str| Err(StoreError::Invalid(message.to_owned()));
    let kept = url.trim().split('#').next().unwrap_or_default();
    if kept.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return invalid("the URL contains a space or a control character");
    }
    match Url::parse(kept) {
        Ok(parsed)
            if matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some() => {}
        _ => return invalid("the URL must be an http(s) URL with a host"),
    }
    if kept.chars().count() > MAX_SAVED_URL_CHARS {
        return Err(StoreError::Invalid(format!(
            "the URL is longer than {MAX_SAVED_URL_CHARS} characters"
        )));
    }
    Ok(kept.to_owned())
}

/// A name: trimmed, not empty, at most [`MAX_SAVED_NAME_CHARS`] characters.
fn clean_name(name: &str) -> Result<String, StoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StoreError::Invalid("the name is empty".to_owned()));
    }
    if name.chars().count() > MAX_SAVED_NAME_CHARS {
        return Err(StoreError::Invalid(format!(
            "the name is longer than {MAX_SAVED_NAME_CHARS} characters"
        )));
    }
    Ok(name.to_owned())
}

impl Store {
    /// Save a page. The URL is stored as [`clean_url`] makes it, the name trimmed. A URL that
    /// is saved already returns its existing row unchanged (the name given now is ignored).
    pub fn add_saved_page(&self, url: &str, name: &str, now: i64) -> Result<SavedPage, StoreError> {
        let url = clean_url(url)?;
        let name = clean_name(name)?;
        self.conn.execute(
            "INSERT INTO saved_pages (url, name, created_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(url) DO NOTHING",
            params![url, name, now],
        )?;
        Ok(self.conn.query_row(
            &format!("SELECT {SAVED_PAGE_COLUMNS} FROM saved_pages WHERE url = ?1"),
            params![url],
            map_row,
        )?)
    }

    /// Every saved page, the first saved first.
    pub fn list_saved_pages(&self) -> Result<Vec<SavedPage>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SAVED_PAGE_COLUMNS} FROM saved_pages ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], map_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Give a saved page another name (trimmed, 1 to [`MAX_SAVED_NAME_CHARS`] characters) and
    /// return the updated row.
    pub fn rename_saved_page(&self, id: i64, name: &str) -> Result<SavedPage, StoreError> {
        let name = clean_name(name)?;
        let changed = self.conn.execute(
            "UPDATE saved_pages SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        require_row(changed, "saved page", id)?;
        Ok(self.conn.query_row(
            &format!("SELECT {SAVED_PAGE_COLUMNS} FROM saved_pages WHERE id = ?1"),
            params![id],
            map_row,
        )?)
    }

    /// Remove a saved page. A no-op if it does not exist.
    pub fn delete_saved_page(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM saved_pages WHERE id = ?1", params![id])?;
        Ok(())
    }
}

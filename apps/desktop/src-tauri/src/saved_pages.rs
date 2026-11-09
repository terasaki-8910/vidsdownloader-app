//! Pages saved from the in-app browser: the commands behind the toolbar's star and the list on
//! the browser's start page. Validation and storage live in `vids_core::store`; this module
//! maps rows to the webview's shape and errors to messages.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::State;
use vids_core::store::{SavedPage, Store, StoreError};

use crate::state::{AppState, lock_recover};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPageRow {
    id: i64,
    url: String,
    name: String,
    created_at: i64,
}

impl From<SavedPage> for SavedPageRow {
    fn from(page: SavedPage) -> Self {
        Self {
            id: page.id,
            url: page.url,
            name: page.name,
            created_at: page.created_at,
        }
    }
}

fn message(error: StoreError) -> String {
    match error {
        StoreError::NotFound { .. } => "saved page not found".to_owned(),
        error => error.to_string(),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| duration.as_secs() as i64)
}

fn list(store: &Store) -> Result<Vec<SavedPageRow>, String> {
    store
        .list_saved_pages()
        .map(|pages| pages.into_iter().map(Into::into).collect())
        .map_err(message)
}

fn add(store: &Store, url: &str, name: &str, now: i64) -> Result<SavedPageRow, String> {
    store
        .add_saved_page(url, name, now)
        .map(Into::into)
        .map_err(message)
}

fn rename(store: &Store, id: i64, name: &str) -> Result<SavedPageRow, String> {
    store
        .rename_saved_page(id, name)
        .map(Into::into)
        .map_err(message)
}

fn remove(store: &Store, id: i64) -> Result<(), String> {
    store.delete_saved_page(id).map_err(message)
}

/// Every saved page, the first saved first.
#[tauri::command]
pub async fn saved_page_list(state: State<'_, AppState>) -> Result<Vec<SavedPageRow>, String> {
    list(&lock_recover(&state.store))
}

/// Saves a page (http(s) only, at most 2048 characters; the `#fragment` is dropped, the rest
/// kept as given). Saving a URL that is saved already returns the existing row.
#[tauri::command]
pub async fn saved_page_add(
    url: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<SavedPageRow, String> {
    add(&lock_recover(&state.store), &url, &name, unix_now())
}

/// Changes a saved page's name (trimmed, 1 to 200 characters) and returns the updated row.
#[tauri::command]
pub async fn saved_page_rename(
    id: i64,
    name: String,
    state: State<'_, AppState>,
) -> Result<SavedPageRow, String> {
    rename(&lock_recover(&state.store), id, &name)
}

/// Removes a saved page; one that is gone already is not an error.
#[tauri::command]
pub async fn saved_page_remove(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    remove(&lock_recover(&state.store), id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://www.nicovideo.jp/ranking/genre/all?term=hour";

    #[test]
    fn rows_reach_the_webview_in_camel_case() {
        let store = Store::open_in_memory().unwrap();
        let row = add(&store, &format!("{URL}#top"), " ランキング ", 1_800_000_000).unwrap();

        assert_eq!(
            serde_json::to_value(&row).unwrap(),
            serde_json::json!({
                "id": row.id,
                "url": URL,
                "name": "ランキング",
                "createdAt": 1_800_000_000,
            })
        );
        assert_eq!(list(&store).unwrap(), vec![row]);
    }

    #[test]
    fn adding_a_saved_url_again_returns_the_first_row() {
        let store = Store::open_in_memory().unwrap();
        let first = add(&store, URL, "First", 1).unwrap();
        let again = add(&store, URL, "Second", 2).unwrap();

        assert_eq!(again, first);
        assert_eq!(list(&store).unwrap().len(), 1);
    }

    #[test]
    fn rename_and_remove_report_what_went_wrong() {
        let store = Store::open_in_memory().unwrap();
        let row = add(&store, URL, "Old", 1).unwrap();

        assert_eq!(rename(&store, row.id, " New ").unwrap().name, "New");
        assert_eq!(
            rename(&store, row.id, "  ").unwrap_err(),
            "the name is empty"
        );
        assert_eq!(
            rename(&store, row.id + 1, "x").unwrap_err(),
            "saved page not found"
        );
        assert_eq!(
            add(&store, "ftp://example.com/", "x", 2).unwrap_err(),
            "the URL must be an http(s) URL with a host"
        );

        remove(&store, row.id).unwrap();
        remove(&store, row.id).unwrap();
        assert!(list(&store).unwrap().is_empty());
    }
}

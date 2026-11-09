//! The library: records of saved files, searchable and independent of the job that
//! created them.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Row, ToSql, params, params_from_iter};

use super::jobs::parse_job_state;
use super::{Store, StoreError, require_row};

/// Input for [`Store::add_library_item`]. New items start as not missing.
#[derive(Debug, Clone)]
pub struct NewLibraryItem {
    pub job_id: Option<i64>,
    pub site: Option<String>,
    pub video_id: Option<String>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub published_at: Option<i64>,
    pub duration_s: Option<f64>,
    /// Absolute path to the saved file; unique across the library.
    pub path: String,
    pub comments_path: Option<String>,
    pub size_bytes: Option<i64>,
}

/// A row from `library_items`.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryItem {
    pub id: i64,
    pub job_id: Option<i64>,
    pub site: Option<String>,
    pub video_id: Option<String>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub published_at: Option<i64>,
    pub duration_s: Option<f64>,
    pub path: String,
    pub comments_path: Option<String>,
    pub size_bytes: Option<i64>,
    pub added_at: i64,
    /// Set by [`Store::set_missing`] when the file could not be found on disk.
    pub missing: bool,
}

/// What [`Store::erase_library_item`] deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErasedItem {
    /// The deleted job rows, ascending.
    pub job_ids: Vec<i64>,
    /// The erased item's file.
    pub path: String,
    /// The erased item's comments file, if it recorded one.
    pub comments_path: Option<String>,
}

/// A library search. Every present field narrows the result; an absent one does
/// not filter.
#[derive(Debug, Clone)]
pub struct LibraryQuery {
    /// Substring match against `title` or `uploader`; ASCII case-insensitive
    /// (SQLite `LIKE`'s native behavior), and correct for non-ASCII text such as
    /// Japanese, which has no case to fold.
    pub text: Option<String>,
    pub site: Option<String>,
    /// Inclusive lower bound on `published_at`.
    pub published_from: Option<i64>,
    /// Inclusive upper bound on `published_at`.
    pub published_to: Option<i64>,
    /// When `false`, items with `missing = true` are excluded.
    pub include_missing: bool,
    pub limit: u32,
    pub offset: u32,
}

const LIBRARY_COLUMNS: &str = "id, job_id, site, video_id, url, title, uploader, \
    published_at, duration_s, path, comments_path, size_bytes, added_at, missing";

struct RawItem {
    id: i64,
    job_id: Option<i64>,
    site: Option<String>,
    video_id: Option<String>,
    url: Option<String>,
    title: Option<String>,
    uploader: Option<String>,
    published_at: Option<i64>,
    duration_s: Option<f64>,
    path: String,
    comments_path: Option<String>,
    size_bytes: Option<i64>,
    added_at: i64,
    missing: bool,
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<RawItem> {
    Ok(RawItem {
        id: row.get("id")?,
        job_id: row.get("job_id")?,
        site: row.get("site")?,
        video_id: row.get("video_id")?,
        url: row.get("url")?,
        title: row.get("title")?,
        uploader: row.get("uploader")?,
        published_at: row.get("published_at")?,
        duration_s: row.get("duration_s")?,
        path: row.get("path")?,
        comments_path: row.get("comments_path")?,
        size_bytes: row.get("size_bytes")?,
        added_at: row.get("added_at")?,
        missing: row.get("missing")?,
    })
}

impl From<RawItem> for LibraryItem {
    fn from(raw: RawItem) -> Self {
        Self {
            id: raw.id,
            job_id: raw.job_id,
            site: raw.site,
            video_id: raw.video_id,
            url: raw.url,
            title: raw.title,
            uploader: raw.uploader,
            published_at: raw.published_at,
            duration_s: raw.duration_s,
            path: raw.path,
            comments_path: raw.comments_path,
            size_bytes: raw.size_bytes,
            added_at: raw.added_at,
            missing: raw.missing,
        }
    }
}

/// Escape `%`, `_` and the escape character itself so `text` can be matched
/// literally inside a `LIKE ... ESCAPE '\'` pattern.
fn escape_like(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

impl Store {
    /// Whether a present library item already records this site and video id.
    pub fn has_library_item(&self, site: &str, video_id: &str) -> Result<bool, StoreError> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM library_items \
                 WHERE site = ?1 AND video_id = ?2 AND missing = 0)",
                params![site, video_id],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }

    /// The most recently added item for this site and video id, including one marked
    /// missing: the library remembers the video even when its file has moved.
    pub fn latest_library_item(
        &self,
        site: &str,
        video_id: &str,
    ) -> Result<Option<LibraryItem>, StoreError> {
        let sql = format!(
            "SELECT {LIBRARY_COLUMNS} FROM library_items \
             WHERE site = ?1 AND video_id = ?2 ORDER BY added_at DESC, id DESC LIMIT 1"
        );
        Ok(self
            .conn
            .query_row(&sql, params![site, video_id], map_row)
            .optional()?
            .map(LibraryItem::from))
    }

    /// The newest library row linked to each job. "Newest" follows library search order:
    /// latest `added_at`, then greatest row id. This is one query for a whole queue
    /// snapshot, rather than one query per job.
    pub fn latest_library_ids_by_job(&self) -> Result<HashMap<i64, i64>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT own.job_id, own.id FROM library_items AS own \
             WHERE own.job_id IS NOT NULL AND NOT EXISTS ( \
                 SELECT 1 FROM library_items AS newer \
                 WHERE newer.job_id = own.job_id AND ( \
                     newer.added_at > own.added_at OR ( \
                         newer.added_at = own.added_at AND newer.id > own.id)))",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        let mut ids = HashMap::new();
        for row in rows {
            let (job_id, library_id) = row?;
            ids.insert(job_id, library_id);
        }
        Ok(ids)
    }

    /// The newest library row linked to one job.
    pub fn latest_library_id_for_job(&self, job_id: i64) -> Result<Option<i64>, StoreError> {
        self.conn
            .query_row(
                "SELECT id FROM library_items WHERE job_id = ?1 \
                 ORDER BY added_at DESC, id DESC LIMIT 1",
                params![job_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// Every library row linked to one of `job_ids`, as `(job_id, library_id)` pairs in
    /// ascending library order. A job has several rows when it saved several files
    /// (page images, a gallery).
    pub fn library_ids_for_jobs(&self, job_ids: &[i64]) -> Result<Vec<(i64, i64)>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT job_id, id FROM library_items WHERE job_id IS NOT NULL ORDER BY id")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        let wanted: HashSet<i64> = job_ids.iter().copied().collect();
        let mut pairs = Vec::new();
        for row in rows {
            let (job_id, library_id) = row?;
            if wanted.contains(&job_id) {
                pairs.push((job_id, library_id));
            }
        }
        Ok(pairs)
    }

    /// Add a file to the library.
    pub fn add_library_item(&self, item: NewLibraryItem, now: i64) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO library_items \
                (job_id, site, video_id, url, title, uploader, published_at, \
                 duration_s, path, comments_path, size_bytes, added_at, missing) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0)",
            params![
                item.job_id,
                item.site,
                item.video_id,
                item.url,
                item.title,
                item.uploader,
                item.published_at,
                item.duration_s,
                item.path,
                item.comments_path,
                item.size_bytes,
                now,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Fetch one library item by id.
    pub fn get_library_item(&self, id: i64) -> Result<LibraryItem, StoreError> {
        let sql = format!("SELECT {LIBRARY_COLUMNS} FROM library_items WHERE id = ?1");
        self.conn
            .query_row(&sql, params![id], map_row)
            .optional()?
            .map(LibraryItem::from)
            .ok_or(StoreError::NotFound {
                entity: "library item",
                id,
            })
    }

    /// Search the library. Results are ordered by `added_at` descending, then
    /// `id` descending (most recently added first).
    pub fn search_library(&self, query: &LibraryQuery) -> Result<Vec<LibraryItem>, StoreError> {
        let mut sql = format!("SELECT {LIBRARY_COLUMNS} FROM library_items WHERE 1 = 1");
        let mut binds: Vec<Box<dyn ToSql>> = Vec::new();

        if !query.include_missing {
            sql.push_str(" AND missing = 0");
        }
        if let Some(site) = &query.site {
            sql.push_str(" AND site = ?");
            binds.push(Box::new(site.clone()));
        }
        if let Some(from) = query.published_from {
            sql.push_str(" AND published_at >= ?");
            binds.push(Box::new(from));
        }
        if let Some(to) = query.published_to {
            sql.push_str(" AND published_at <= ?");
            binds.push(Box::new(to));
        }
        if let Some(text) = &query.text {
            sql.push_str(" AND (title LIKE ? ESCAPE '\\' OR uploader LIKE ? ESCAPE '\\')");
            let pattern = format!("%{}%", escape_like(text));
            binds.push(Box::new(pattern.clone()));
            binds.push(Box::new(pattern));
        }
        sql.push_str(" ORDER BY added_at DESC, id DESC LIMIT ? OFFSET ?");
        binds.push(Box::new(i64::from(query.limit)));
        binds.push(Box::new(i64::from(query.offset)));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(binds), map_row)?;
        let mut items = Vec::new();
        for row in rows {
            items.push(LibraryItem::from(row?));
        }
        Ok(items)
    }

    /// Mark a library item as missing (its file could not be found) or present.
    pub fn set_missing(&self, id: i64, missing: bool) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE library_items SET missing = ?1 WHERE id = ?2",
            params![missing, id],
        )?;
        require_row(changed, "library item", id)
    }

    /// Set or clear (`None`) a library item's comments file path.
    pub fn set_comments_path(
        &self,
        id: i64,
        comments_path: Option<&str>,
    ) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE library_items SET comments_path = ?1 WHERE id = ?2",
            params![comments_path, id],
        )?;
        require_row(changed, "library item", id)
    }

    /// Delete a library item. A no-op if it does not exist.
    pub fn delete_library_item(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM library_items WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Every library item's path except item `id`'s, for telling another saved item's
    /// file apart from a side file.
    pub fn library_paths_except(&self, id: i64) -> Result<Vec<String>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM library_items WHERE id <> ?1 ORDER BY id")?;
        let paths = stmt
            .query_map(params![id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(paths)
    }

    /// The jobs [`Store::erase_library_item`] would delete with item `id`, ascending, or
    /// [`StoreError::JobNotFinished`] when one of them has not finished. Changes nothing.
    pub fn library_item_jobs_to_erase(&self, id: i64) -> Result<Vec<i64>, StoreError> {
        let item = self.get_library_item(id)?;
        let jobs = jobs_of_item(&self.conn, &item)?;
        refuse_unfinished(&jobs)?;
        Ok(jobs.into_iter().map(|(job_id, _)| job_id).collect())
    }

    /// Erase a saved item's records in one transaction: its library row, the job it links
    /// (`job_id`), and every other job pointing to it: one whose `output_path` is the
    /// item's path, one skipped as already saved at the item's path, and one with the
    /// item's URL that has no library row of its own (so another saved video of the same
    /// multi-video post keeps its job). When any of those jobs has not finished (Done,
    /// Failed or Canceled), nothing is deleted and [`StoreError::JobNotFinished`] is
    /// returned.
    ///
    /// With secure delete on, the deleted content is overwritten in the database; call
    /// [`Store::checkpoint_truncate`] afterwards to clear it from the write-ahead log too.
    pub fn erase_library_item(&self, id: i64) -> Result<ErasedItem, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let item = self.get_library_item(id)?;
        let jobs = jobs_of_item(&tx, &item)?;
        refuse_unfinished(&jobs)?;
        tx.execute("DELETE FROM library_items WHERE id = ?1", params![id])?;
        for (job_id, _) in &jobs {
            tx.execute("DELETE FROM jobs WHERE id = ?1", params![job_id])?;
        }
        tx.commit()?;
        Ok(ErasedItem {
            job_ids: jobs.into_iter().map(|(job_id, _)| job_id).collect(),
            path: item.path,
            comments_path: item.comments_path,
        })
    }

    /// Record that a library item's file (and comments file) were renamed: updates the
    /// item's `path` and `comments_path`, and the linked job's `output_path` when it named
    /// the old path (a multi-file job keeps the file it names), in one transaction.
    /// Returns the linked job's id when its `output_path` was updated.
    pub fn rename_library_item(
        &self,
        id: i64,
        new_path: &str,
        new_comments_path: Option<&str>,
        now: i64,
    ) -> Result<Option<i64>, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let item = self.get_library_item(id)?;
        tx.execute(
            "UPDATE library_items SET path = ?1, comments_path = ?2 WHERE id = ?3",
            params![new_path, new_comments_path, id],
        )?;
        let updated_job = match item.job_id {
            Some(job_id) => {
                let changed = tx.execute(
                    "UPDATE jobs SET output_path = ?1, updated_at = ?2 \
                     WHERE id = ?3 AND output_path = ?4",
                    params![new_path, now, job_id, item.path],
                )?;
                (changed > 0).then_some(job_id)
            }
            None => None,
        };
        tx.commit()?;
        Ok(updated_job)
    }
}

/// The jobs that belong to `item` (see [`Store::erase_library_item`]) with their states,
/// ascending by id.
fn jobs_of_item(
    conn: &Connection,
    item: &LibraryItem,
) -> Result<Vec<(i64, crate::types::JobState)>, StoreError> {
    // CASE keeps json_extract away from a column that is not valid JSON.
    let mut stmt = conn.prepare(
        "SELECT id, state FROM jobs \
         WHERE id = ?1 \
            OR output_path = ?2 \
            OR CASE WHEN json_valid(already_saved_json) \
                    THEN json_extract(already_saved_json, '$.path') END = ?2 \
            OR (?3 IS NOT NULL AND url = ?3 AND NOT EXISTS ( \
                    SELECT 1 FROM library_items AS own \
                    WHERE own.job_id = jobs.id AND own.id <> ?4)) \
         ORDER BY id",
    )?;
    let rows = stmt.query_map(params![item.job_id, item.path, item.url, item.id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut jobs = Vec::new();
    for row in rows {
        let (job_id, state) = row?;
        jobs.push((job_id, parse_job_state(&state)?));
    }
    Ok(jobs)
}

fn refuse_unfinished(jobs: &[(i64, crate::types::JobState)]) -> Result<(), StoreError> {
    match jobs.iter().find(|(_, state)| !state.is_terminal()) {
        Some(&(id, state)) => Err(StoreError::JobNotFinished { id, state }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewJob;

    fn item(path: &str, site: &str, video_id: &str) -> NewLibraryItem {
        NewLibraryItem {
            job_id: None,
            site: Some(site.to_owned()),
            video_id: Some(video_id.to_owned()),
            url: None,
            title: None,
            uploader: None,
            published_at: None,
            duration_s: None,
            path: path.to_owned(),
            comments_path: None,
            size_bytes: None,
        }
    }

    fn job(store: &Store, suffix: &str) -> i64 {
        store
            .insert_job(
                NewJob {
                    url: format!("https://example.test/{suffix}"),
                    site: None,
                    title: None,
                    options_json: serde_json::json!({}),
                    source_id: None,
                },
                1,
            )
            .unwrap()
    }

    #[test]
    fn has_library_item_matches_only_present_site_and_video_id() {
        let store = Store::open_in_memory().unwrap();
        let missing = store
            .add_library_item(item("/missing.mp4", "Youtube", "same"), 1)
            .unwrap();
        store.set_missing(missing, true).unwrap();
        store
            .add_library_item(item("/present.mp4", "Youtube", "present"), 2)
            .unwrap();

        assert!(!store.has_library_item("Youtube", "same").unwrap());
        assert!(store.has_library_item("Youtube", "present").unwrap());
        assert!(!store.has_library_item("Niconico", "present").unwrap());
    }

    #[test]
    fn library_ids_for_jobs_lists_every_row_of_the_given_jobs_only() {
        let store = Store::open_in_memory().unwrap();
        let empty_job = job(&store, "empty");
        let one_job = job(&store, "one");
        let two_job = job(&store, "two");
        let one = store
            .add_library_item(
                NewLibraryItem {
                    job_id: Some(one_job),
                    ..item("/one.mp4", "Example", "one")
                },
                10,
            )
            .unwrap();
        let first = store
            .add_library_item(
                NewLibraryItem {
                    job_id: Some(two_job),
                    ..item("/two-a.jpg", "Example", "a")
                },
                10,
            )
            .unwrap();
        let second = store
            .add_library_item(
                NewLibraryItem {
                    job_id: Some(two_job),
                    ..item("/two-b.jpg", "Example", "b")
                },
                10,
            )
            .unwrap();
        store
            .add_library_item(item("/unlinked.mp4", "Example", "unlinked"), 10)
            .unwrap();

        assert_eq!(
            store
                .library_ids_for_jobs(&[empty_job, one_job, two_job])
                .unwrap(),
            [(one_job, one), (two_job, first), (two_job, second)]
        );
        assert_eq!(
            store.library_ids_for_jobs(&[two_job]).unwrap(),
            [(two_job, first), (two_job, second)]
        );
        assert!(store.library_ids_for_jobs(&[empty_job]).unwrap().is_empty());
        assert!(store.library_ids_for_jobs(&[]).unwrap().is_empty());
    }

    #[test]
    fn latest_library_ids_are_none_one_and_the_newest_of_two() {
        let store = Store::open_in_memory().unwrap();
        let empty_job = job(&store, "empty");
        let one_job = job(&store, "one");
        let two_job = job(&store, "two");

        let one = store
            .add_library_item(
                NewLibraryItem {
                    job_id: Some(one_job),
                    ..item("/one.mp4", "Example", "one")
                },
                10,
            )
            .unwrap();
        let older = store
            .add_library_item(
                NewLibraryItem {
                    job_id: Some(two_job),
                    ..item("/older.mp4", "Example", "older")
                },
                10,
            )
            .unwrap();
        let newer = store
            .add_library_item(
                NewLibraryItem {
                    job_id: Some(two_job),
                    ..item("/newer.mp4", "Example", "newer")
                },
                10,
            )
            .unwrap();
        assert!(newer > older);

        let ids = store.latest_library_ids_by_job().unwrap();
        assert_eq!(ids.get(&empty_job), None);
        assert_eq!(ids.get(&one_job), Some(&one));
        assert_eq!(ids.get(&two_job), Some(&newer));
        assert_eq!(store.latest_library_id_for_job(empty_job).unwrap(), None);
        assert_eq!(store.latest_library_id_for_job(one_job).unwrap(), Some(one));
        assert_eq!(
            store.latest_library_id_for_job(two_job).unwrap(),
            Some(newer)
        );
    }
}

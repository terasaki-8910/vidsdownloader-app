//! The download queue: job rows, state transitions and queue position.

use rusqlite::{OptionalExtension, Row, params};
use serde_json::Value;

use super::{Store, StoreError, require_row};
use crate::types::{ErrorCategory, JobState};

/// Input for [`Store::insert_job`]. The new job is queued at the end of the queue.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub url: String,
    pub site: Option<String>,
    pub title: Option<String>,
    /// Opaque per-job options (quality preset, naming template, filters, ...),
    /// interpreted elsewhere; the store only persists it as JSON.
    pub options_json: Value,
    pub source_id: Option<i64>,
}

/// A row from `jobs`.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: i64,
    pub url: String,
    pub site: Option<String>,
    pub state: JobState,
    pub title: Option<String>,
    pub info_json_path: Option<String>,
    pub output_path: Option<String>,
    /// Queue-facing metadata discovered by probing, kept opaque by the store.
    pub details_json: Option<Value>,
    /// Why a successful attempt was skipped instead of saving a file.
    pub skipped: Option<String>,
    /// Where the video was already saved, when the attempt was skipped for that
    /// reason; opaque to the store.
    pub already_saved_json: Option<Value>,
    /// The job's last error, if any; `None` once cleared by
    /// [`Store::update_job_state`].
    pub error: Option<(ErrorCategory, String)>,
    pub options_json: Value,
    pub source_id: Option<i64>,
    /// Position in the queue; unique and, after a [`Store::move_job`] call,
    /// contiguous from 1.
    pub position: i64,
    pub attempts: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Which jobs [`Store::list_jobs`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobFilter {
    /// Every state except `Done`, `Failed` and `Canceled`.
    Active,
    /// Every job, regardless of state.
    All,
}

const JOB_COLUMNS: &str = "id, url, site, state, title, info_json_path, output_path, \
    details_json, skipped, already_saved_json, error_category, error_message, options_json, \
    source_id, position, attempts, created_at, updated_at";

struct RawJob {
    id: i64,
    url: String,
    site: Option<String>,
    state: String,
    title: Option<String>,
    info_json_path: Option<String>,
    output_path: Option<String>,
    details_json: Option<String>,
    skipped: Option<String>,
    already_saved_json: Option<String>,
    error_category: Option<String>,
    error_message: Option<String>,
    options_json: String,
    source_id: Option<i64>,
    position: i64,
    attempts: i64,
    created_at: i64,
    updated_at: i64,
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<RawJob> {
    Ok(RawJob {
        id: row.get("id")?,
        url: row.get("url")?,
        site: row.get("site")?,
        state: row.get("state")?,
        title: row.get("title")?,
        info_json_path: row.get("info_json_path")?,
        output_path: row.get("output_path")?,
        details_json: row.get("details_json")?,
        skipped: row.get("skipped")?,
        already_saved_json: row.get("already_saved_json")?,
        error_category: row.get("error_category")?,
        error_message: row.get("error_message")?,
        options_json: row.get("options_json")?,
        source_id: row.get("source_id")?,
        position: row.get("position")?,
        attempts: row.get("attempts")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl RawJob {
    fn into_job(self) -> Result<Job, StoreError> {
        let error = match self.error_category {
            Some(category) => Some((
                parse_error_category(&category)?,
                self.error_message.unwrap_or_default(),
            )),
            None => None,
        };
        Ok(Job {
            id: self.id,
            url: self.url,
            site: self.site,
            state: parse_job_state(&self.state)?,
            title: self.title,
            info_json_path: self.info_json_path,
            output_path: self.output_path,
            details_json: self
                .details_json
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            skipped: self.skipped,
            already_saved_json: self
                .already_saved_json
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            error,
            options_json: serde_json::from_str(&self.options_json)?,
            source_id: self.source_id,
            position: self.position,
            attempts: self.attempts,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

fn job_state_str(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Probing => "probing",
        JobState::Ready => "ready",
        JobState::Downloading => "downloading",
        JobState::PostProcessing => "post_processing",
        JobState::Done => "done",
        JobState::Failed => "failed",
        JobState::Canceled => "canceled",
    }
}

pub(super) fn parse_job_state(value: &str) -> Result<JobState, StoreError> {
    match value {
        "queued" => Ok(JobState::Queued),
        "probing" => Ok(JobState::Probing),
        "ready" => Ok(JobState::Ready),
        "downloading" => Ok(JobState::Downloading),
        "post_processing" => Ok(JobState::PostProcessing),
        "done" => Ok(JobState::Done),
        "failed" => Ok(JobState::Failed),
        "canceled" => Ok(JobState::Canceled),
        other => Err(StoreError::InvalidEnumValue {
            type_name: "JobState",
            value: other.to_owned(),
        }),
    }
}

fn error_category_str(category: ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::UnsupportedUrl => "unsupported_url",
        ErrorCategory::PageMediaNotFound => "page_media_not_found",
        ErrorCategory::PageMediaChoice => "page_media_choice",
        ErrorCategory::LoginRequired => "login_required",
        ErrorCategory::Unavailable => "unavailable",
        ErrorCategory::GeoBlocked => "geo_blocked",
        ErrorCategory::RateLimited => "rate_limited",
        ErrorCategory::NoVideo => "no_video",
        ErrorCategory::Drm => "drm",
        ErrorCategory::ExtractorBroken => "extractor_broken",
        ErrorCategory::Network => "network",
        ErrorCategory::Postprocess => "postprocess",
        ErrorCategory::Unknown => "unknown",
    }
}

fn parse_error_category(value: &str) -> Result<ErrorCategory, StoreError> {
    match value {
        "unsupported_url" => Ok(ErrorCategory::UnsupportedUrl),
        "page_media_not_found" => Ok(ErrorCategory::PageMediaNotFound),
        "page_media_choice" => Ok(ErrorCategory::PageMediaChoice),
        "login_required" => Ok(ErrorCategory::LoginRequired),
        "unavailable" => Ok(ErrorCategory::Unavailable),
        "geo_blocked" => Ok(ErrorCategory::GeoBlocked),
        "rate_limited" => Ok(ErrorCategory::RateLimited),
        "no_video" => Ok(ErrorCategory::NoVideo),
        "drm" => Ok(ErrorCategory::Drm),
        "extractor_broken" => Ok(ErrorCategory::ExtractorBroken),
        "network" => Ok(ErrorCategory::Network),
        "postprocess" => Ok(ErrorCategory::Postprocess),
        "unknown" => Ok(ErrorCategory::Unknown),
        other => Err(StoreError::InvalidEnumValue {
            type_name: "ErrorCategory",
            value: other.to_owned(),
        }),
    }
}

impl Store {
    /// Queue a new job at the end of the queue (`position` = current max + 1).
    pub fn insert_job(&self, new_job: NewJob, now: i64) -> Result<i64, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let next_position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM jobs",
            [],
            |row| row.get(0),
        )?;
        let options_json = serde_json::to_string(&new_job.options_json)?;
        tx.execute(
            "INSERT INTO jobs \
                (url, site, state, title, options_json, source_id, position, \
                 attempts, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?8)",
            params![
                new_job.url,
                new_job.site,
                job_state_str(JobState::Queued),
                new_job.title,
                options_json,
                new_job.source_id,
                next_position,
                now,
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// Fetch one job by id.
    pub fn get_job(&self, id: i64) -> Result<Job, StoreError> {
        let sql = format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1");
        let raw = self
            .conn
            .query_row(&sql, params![id], map_row)
            .optional()?
            .ok_or(StoreError::NotFound { entity: "job", id })?;
        raw.into_job()
    }

    /// List jobs ordered by `position` ascending.
    pub fn list_jobs(&self, filter: JobFilter) -> Result<Vec<Job>, StoreError> {
        let sql = match filter {
            JobFilter::All => format!("SELECT {JOB_COLUMNS} FROM jobs ORDER BY position ASC"),
            JobFilter::Active => format!(
                "SELECT {JOB_COLUMNS} FROM jobs \
                 WHERE state NOT IN ('done', 'failed', 'canceled') \
                 ORDER BY position ASC"
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], map_row)?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row?.into_job()?);
        }
        Ok(jobs)
    }

    /// Set a job's state and touch `updated_at`. `error` replaces the stored error;
    /// `None` clears it.
    pub fn update_job_state(
        &self,
        id: i64,
        state: JobState,
        error: Option<(ErrorCategory, String)>,
        now: i64,
    ) -> Result<(), StoreError> {
        let (category, message) = match &error {
            Some((category, message)) => {
                (Some(error_category_str(*category)), Some(message.as_str()))
            }
            None => (None, None),
        };
        let changed = self.conn.execute(
            "UPDATE jobs SET state = ?1, error_category = ?2, error_message = ?3, \
             updated_at = ?4 WHERE id = ?5",
            params![job_state_str(state), category, message, now, id],
        )?;
        require_row(changed, "job", id)
    }

    /// Record where a job's probed metadata (`-J` output) was saved.
    pub fn set_job_info_json(&self, id: i64, path: &str, now: i64) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE jobs SET info_json_path = ?1, updated_at = ?2 WHERE id = ?3",
            params![path, now, id],
        )?;
        require_row(changed, "job", id)
    }

    /// Record a job's final output file path.
    pub fn set_job_output(&self, id: i64, path: &str, now: i64) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE jobs SET output_path = ?1, updated_at = ?2 WHERE id = ?3",
            params![path, now, id],
        )?;
        require_row(changed, "job", id)
    }

    /// Record queue-facing metadata discovered while probing.
    pub fn set_job_details(
        &self,
        id: i64,
        details_json: &Value,
        now: i64,
    ) -> Result<(), StoreError> {
        let details_json = serde_json::to_string(details_json)?;
        let changed = self.conn.execute(
            "UPDATE jobs SET details_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![details_json, now, id],
        )?;
        require_row(changed, "job", id)
    }

    /// Replace the opaque options of a running job (for discoveries that a retry must
    /// reuse, such as a media URL found in the page).
    pub fn set_job_options(
        &self,
        id: i64,
        options_json: &Value,
        now: i64,
    ) -> Result<(), StoreError> {
        let options_json = serde_json::to_string(options_json)?;
        let changed = self.conn.execute(
            "UPDATE jobs SET options_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![options_json, now, id],
        )?;
        require_row(changed, "job", id)
    }

    /// Record a terminal attempt in one statement, clearing fields that do not
    /// apply to its terminal state. `already_saved` is stored as JSON; `None` clears it.
    // One argument per column the statement writes, with the same shape as the other setters.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_job_attempt(
        &self,
        id: i64,
        state: JobState,
        output_path: Option<&str>,
        skipped: Option<&str>,
        already_saved: Option<&Value>,
        error: Option<(ErrorCategory, &str)>,
        now: i64,
    ) -> Result<(), StoreError> {
        let (category, message) = error.map_or((None, None), |(category, message)| {
            (Some(error_category_str(category)), Some(message))
        });
        let already_saved = already_saved.map(serde_json::to_string).transpose()?;
        let changed = self.conn.execute(
            "UPDATE jobs SET state = ?1, output_path = ?2, skipped = ?3, \
             already_saved_json = ?4, error_category = ?5, error_message = ?6, \
             updated_at = ?7 WHERE id = ?8",
            params![
                job_state_str(state),
                output_path,
                skipped,
                already_saved,
                category,
                message,
                now,
                id
            ],
        )?;
        require_row(changed, "job", id)
    }

    /// Queue the next attempt of a failed or canceled job and clear its previous
    /// terminal result in one statement.
    pub fn retry_job(&self, id: i64, now: i64) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE jobs SET state = ?1, attempts = attempts + 1, output_path = NULL, \
             skipped = NULL, already_saved_json = NULL, error_category = NULL, \
             error_message = NULL, updated_at = ?2 WHERE id = ?3",
            params![job_state_str(JobState::Queued), now, id],
        )?;
        require_row(changed, "job", id)
    }

    /// Queue the next attempt while replacing options selected after the previous failure.
    pub fn retry_job_with_options(
        &self,
        id: i64,
        options_json: &Value,
        now: i64,
    ) -> Result<(), StoreError> {
        let options_json = serde_json::to_string(options_json)?;
        let changed = self.conn.execute(
            "UPDATE jobs SET state = ?1, attempts = attempts + 1, output_path = NULL, \
             skipped = NULL, already_saved_json = NULL, error_category = NULL, \
             error_message = NULL, options_json = ?2, updated_at = ?3 WHERE id = ?4",
            params![job_state_str(JobState::Queued), options_json, now, id],
        )?;
        require_row(changed, "job", id)
    }

    /// Queue the next attempt of a job that was skipped because its video was already
    /// saved, with `options_json` replacing its options. Otherwise the same as
    /// [`Store::retry_job`].
    pub fn save_again_job(
        &self,
        id: i64,
        options_json: &Value,
        now: i64,
    ) -> Result<(), StoreError> {
        let options_json = serde_json::to_string(options_json)?;
        let changed = self.conn.execute(
            "UPDATE jobs SET state = ?1, attempts = attempts + 1, output_path = NULL, \
             skipped = NULL, already_saved_json = NULL, error_category = NULL, \
             error_message = NULL, options_json = ?2, updated_at = ?3 WHERE id = ?4",
            params![job_state_str(JobState::Queued), options_json, now, id],
        )?;
        require_row(changed, "job", id)
    }

    /// Increment a job's attempt counter by one.
    pub fn increment_attempts(&self, id: i64, now: i64) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE jobs SET attempts = attempts + 1, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        require_row(changed, "job", id)
    }

    /// Move a job to `new_position` (1-based, clamped to the current number of
    /// jobs), renumbering the rest so positions stay unique and contiguous
    /// (`1..=count`). Only the moved job's `updated_at` changes.
    pub fn move_job(&self, id: i64, new_position: i64, now: i64) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut ids: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT id FROM jobs ORDER BY position ASC, id ASC")?;
            stmt.query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?
        };
        let current_index = ids
            .iter()
            .position(|&existing| existing == id)
            .ok_or(StoreError::NotFound { entity: "job", id })?;
        ids.remove(current_index);
        let target_index = (new_position - 1).clamp(0, ids.len() as i64) as usize;
        ids.insert(target_index, id);

        for (index, job_id) in ids.iter().enumerate() {
            let position = index as i64 + 1;
            if *job_id == id {
                tx.execute(
                    "UPDATE jobs SET position = ?1, updated_at = ?2 WHERE id = ?3",
                    params![position, now, job_id],
                )?;
            } else {
                tx.execute(
                    "UPDATE jobs SET position = ?1 WHERE id = ?2",
                    params![position, job_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete a job. A no-op if it does not exist.
    pub fn delete_job(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM jobs WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Delete several queue rows in one transaction. Unknown ids are ignored. Foreign
    /// keys are enabled on every store connection, so linked library rows stay and their
    /// `job_id` becomes NULL (`ON DELETE SET NULL`).
    pub fn delete_jobs(&self, ids: &[i64]) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM jobs WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// After a crash or restart, put every job that was mid-flight (`Probing`,
    /// `Ready`, `Downloading`, `PostProcessing`) back to `Queued`. Returns how many
    /// jobs were changed.
    pub fn recover_interrupted(&self, now: i64) -> Result<usize, StoreError> {
        let changed = self.conn.execute(
            "UPDATE jobs SET state = ?1, updated_at = ?2 \
             WHERE state IN ('probing', 'ready', 'downloading', 'post_processing')",
            params![job_state_str(JobState::Queued), now],
        )?;
        Ok(changed)
    }
}

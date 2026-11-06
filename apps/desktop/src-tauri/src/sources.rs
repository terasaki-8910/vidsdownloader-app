//! Sources: registered listings (a list yt-dlp understands, or a page read with a saved
//! collecting rule) that are checked for new items. One check runs at a time, in the order
//! they were asked for; the new items are queued as ordinary download jobs and remembered
//! as known, so the next check queues only what appeared since.

use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State, Url};
use tokio::sync::Notify;
use vids_core::args::{Cookies, ToolPaths};
use vids_core::executor::{
    ExecutorConfig, JobOptions, ListProbe, SourceItem, YtDlpExecutor, list_items,
};
use vids_core::page_images::site_from_url;
use vids_core::queue::{CancelToken, JobFailure};
use vids_core::store::{NewSource, Rule, Source, SourceKind, Store, StoreError};
use vids_core::tools::Tool;
use vids_core::types::ErrorCategory;

use crate::browser::BrowserProfile;
use crate::collector::{self, CollectItem, CollectSnapshot};
use crate::commands::job_options;
use crate::method_options::{
    overrides_dest_dir, overrides_preset, validate_dest_dir, with_options,
};
use crate::models::{Preset, Settings, SourceAutoCheck};
use crate::notify::NotifyEvent;
use crate::sites::{self, Site};
use crate::state::{self, AppState, TARGET_OS, lock_recover};

/// A preview is dropped this long after it was made.
const PREVIEW_TTL: Duration = Duration::from_secs(15 * 60);
/// A rule check scans the page at most this many times, whatever the rule says.
const MAX_RULE_SCANS: u32 = 10;
/// How long a rule check waits for the page to finish loading.
const PAGE_LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// How often automatic checks look for due sources.
const AUTO_CHECK_TICK: Duration = Duration::from_secs(60);
const MAX_NAME_CHARS: usize = 200;
const MAX_URL_CHARS: usize = 2_048;
/// The collector's reserved shape for a page's own images: not links to other pages.
const PAGE_IMAGES_SHAPE: &str = "{images}";

/// Where a source's items come from, as the UI names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceRowKind {
    /// A list yt-dlp understands.
    List,
    /// A page read with a saved collecting rule.
    Rule,
}

/// Whether a source's check is waiting its turn or running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceStatus {
    Idle,
    Waiting,
    Checking,
}

/// What a source's last check found; stored with the source as this JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SourceResult {
    /// `count` new items were queued.
    New { count: usize },
    /// Nothing new.
    #[serde(rename = "none")]
    Nothing,
    /// The check failed.
    Error {
        category: ErrorCategory,
        message: String,
    },
}

/// One source as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRow {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub kind: SourceRowKind,
    /// The registered site matching the URL, else the URL's host without `www.`.
    pub site_name: String,
    /// The rule's link shape (rule sources only).
    pub rule_shape: Option<String>,
    /// The site's selected account, only when the site has two or more accounts.
    pub account_label: Option<String>,
    /// Where this source's items are saved; `None` uses the download folder.
    pub dest_dir: Option<String>,
    /// This source's quality; `None` uses the default preset.
    pub preset: Option<Preset>,
    pub last_checked_at: Option<i64>,
    pub last_result: Option<SourceResult>,
    pub status: SourceStatus,
}

/// Payload of the `source-removed` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceRemoved {
    pub id: i64,
}

/// What a URL or the collect session would register, with how many items it has now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePreview {
    /// Passed to `source_register`; valid for 15 minutes and until the next preview.
    pub token: String,
    pub kind: SourceRowKind,
    pub name: String,
    pub url: String,
    pub count: usize,
    /// The source already registered for this URL, if any.
    pub existing_id: Option<i64>,
}

/// Why a URL could not be previewed as a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewFailure {
    NotAList,
    LoginRequired,
    Unsupported,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SourcePreviewOutcome {
    Ready {
        preview: SourcePreview,
    },
    Failed {
        reason: PreviewFailure,
        message: String,
    },
}

/// What registering does with the items the list has now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RegisterChoice {
    /// Remember them as known without saving them.
    Record,
    /// Remember them and queue every one.
    SaveAll,
}

/// The newest preview, kept in memory until it is registered, replaced or too old.
#[derive(Debug, Clone, PartialEq)]
struct Preview {
    token: String,
    created: Instant,
    name: String,
    url: String,
    items: Vec<SourceItem>,
    /// Set for a page read with a rule; `None` for a yt-dlp list.
    rule: Option<PreviewRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewRule {
    shape: String,
    max_pages: u32,
    /// The saved rule the collect session used; a new rule is saved when it had none.
    rule_id: Option<i64>,
}

impl Preview {
    fn kind(&self) -> SourceRowKind {
        if self.rule.is_some() {
            SourceRowKind::Rule
        } else {
            SourceRowKind::List
        }
    }

    fn keys(&self) -> Vec<String> {
        self.items.iter().map(|item| item.key.clone()).collect()
    }

    fn public(&self, existing_id: Option<i64>) -> SourcePreview {
        SourcePreview {
            token: self.token.clone(),
            kind: self.kind(),
            name: self.name.clone(),
            url: self.url.clone(),
            count: self.items.len(),
            existing_id,
        }
    }
}

/// Holds only the newest preview: a new one replaces the old, and one older than
/// [`PREVIEW_TTL`] is gone.
#[derive(Debug, Default)]
struct PreviewSlot {
    current: Option<Preview>,
}

impl PreviewSlot {
    fn put(&mut self, preview: Preview) {
        self.current = Some(preview);
    }

    fn expire(&mut self, now: Instant) {
        if self
            .current
            .as_ref()
            .is_some_and(|preview| now.saturating_duration_since(preview.created) >= PREVIEW_TTL)
        {
            self.current = None;
        }
    }

    /// The preview with this token, unless it was replaced or has expired.
    fn get(&mut self, token: &str, now: Instant) -> Option<Preview> {
        self.expire(now);
        self.current
            .as_ref()
            .filter(|preview| preview.token == token)
            .cloned()
    }

    fn remove(&mut self, token: &str) {
        if self
            .current
            .as_ref()
            .is_some_and(|preview| preview.token == token)
        {
            self.current = None;
        }
    }
}

/// The sources waiting for a check, first asked first, and the one being checked.
#[derive(Debug, Clone, Default)]
struct CheckQueue {
    waiting: VecDeque<i64>,
    checking: Option<i64>,
}

impl CheckQueue {
    /// Adds a source unless it is already waiting or being checked; returns whether it was
    /// added.
    fn push(&mut self, id: i64) -> bool {
        if self.status(id) != SourceStatus::Idle {
            return false;
        }
        self.waiting.push_back(id);
        true
    }

    /// The next source to check, unless a check is running.
    fn start_next(&mut self) -> Option<i64> {
        if self.checking.is_some() {
            return None;
        }
        let id = self.waiting.pop_front()?;
        self.checking = Some(id);
        Some(id)
    }

    fn finish(&mut self, id: i64) {
        if self.checking == Some(id) {
            self.checking = None;
        }
    }

    /// Takes a source out of the waiting line; a running check is cancelled by the caller.
    fn remove(&mut self, id: i64) {
        self.waiting.retain(|waiting| *waiting != id);
    }

    fn status(&self, id: i64) -> SourceStatus {
        if self.checking == Some(id) {
            SourceStatus::Checking
        } else if self.waiting.contains(&id) {
            SourceStatus::Waiting
        } else {
            SourceStatus::Idle
        }
    }
}

#[derive(Default)]
struct Inner {
    queue: CheckQueue,
    /// The running check's source and the token that cancels it.
    running: Option<(i64, CancelToken)>,
    preview: PreviewSlot,
}

/// Registers sources, runs their checks one at a time and reports every change as a
/// `source-updated` event (`source-removed` on delete).
pub struct SourceManager {
    app: AppHandle,
    inner: Mutex<Inner>,
    wake: Notify,
    /// The newest local UTC offset a source command passed; automatic checks use it.
    tz_offset_seconds: AtomicI64,
    sequence: AtomicU64,
}

impl SourceManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            inner: Mutex::new(Inner::default()),
            wake: Notify::new(),
            tz_offset_seconds: AtomicI64::new(0),
            sequence: AtomicU64::new(1),
        }
    }

    pub fn remember_tz_offset(&self, tz_offset_seconds: i64) {
        self.tz_offset_seconds
            .store(tz_offset_seconds, Ordering::Relaxed);
    }

    fn tz_offset(&self) -> i64 {
        self.tz_offset_seconds.load(Ordering::Relaxed)
    }

    fn state(&self) -> State<'_, AppState> {
        self.app.state::<AppState>()
    }

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    fn new_token(&self) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        format!("{nanos:x}-{:x}", self.next_sequence())
    }

    /// Every source, in id order.
    pub fn rows(&self) -> Result<Vec<SourceRow>, String> {
        let queue = lock_recover(&self.inner).queue.clone();
        let state = self.state();
        let store = lock_recover(&state.store);
        let sources = store.list_sources().map_err(|error| error.to_string())?;
        let sites = sites::list_sites(&store)?;
        let rules = store.list_rules().map_err(|error| error.to_string())?;
        Ok(sources
            .iter()
            .map(|source| {
                let rule = source
                    .rule_id
                    .and_then(|id| rules.iter().find(|rule| rule.id == id));
                source_row(source, &sites, rule, queue.status(source.id))
            })
            .collect())
    }

    /// One source's row; `None` when it no longer exists.
    fn row(&self, id: i64) -> Result<Option<SourceRow>, String> {
        let status = lock_recover(&self.inner).queue.status(id);
        let state = self.state();
        let store = lock_recover(&state.store);
        let source = match store.get_source(id) {
            Ok(source) => source,
            Err(StoreError::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let sites = sites::list_sites(&store)?;
        let rule = source.rule_id.and_then(|id| store.get_rule(id).ok());
        Ok(Some(source_row(&source, &sites, rule.as_ref(), status)))
    }

    fn emit_row(&self, id: i64) {
        match self.row(id) {
            Ok(Some(row)) => {
                let _ = self.app.emit("source-updated", row);
            }
            Ok(None) => {}
            Err(error) => eprintln!("could not read source {id}: {error}"),
        }
    }

    /// The row after a change, announced as `source-updated`.
    fn updated(&self, id: i64) -> Result<SourceRow, String> {
        let row = self.row(id)?.ok_or_else(|| "source not found".to_owned())?;
        let _ = self.app.emit("source-updated", row.clone());
        Ok(row)
    }

    /// Probes `url` as a yt-dlp list with the account cookies a job would get, and keeps
    /// what it found as the newest preview.
    pub async fn preview_url(&self, url: String) -> Result<SourcePreviewOutcome, String> {
        let url = url.trim().to_owned();
        if url.chars().count() > MAX_URL_CHARS || !collector::is_http_url(&url) {
            return Err("url must be an http(s) URL".to_owned());
        }
        let existing_id = self.existing_id(&url)?;
        // A preview is not cancelled; the token only satisfies the probe's signature.
        let cancel = CancelToken::new();
        let probe = match self.probe_list(&url, &cancel, "preview").await {
            Ok(probe) => probe,
            Err(error) => {
                return Ok(SourcePreviewOutcome::Failed {
                    reason: preview_failure(error.category),
                    message: error.message,
                });
            }
        };
        let ListProbe::List { json, title } = probe else {
            return Ok(SourcePreviewOutcome::Failed {
                reason: PreviewFailure::NotAList,
                message: "this URL shows a single video, not a list".to_owned(),
            });
        };
        let preview = Preview {
            token: self.new_token(),
            created: Instant::now(),
            name: truncate_chars(&title, MAX_NAME_CHARS),
            items: list_items(&url, &json),
            url,
            rule: None,
        };
        let public = preview.public(existing_id);
        lock_recover(&self.inner).preview.put(preview);
        Ok(SourcePreviewOutcome::Ready { preview: public })
    }

    /// A preview of the open collect session's page with the items found so far.
    pub fn preview_collect(&self) -> Result<SourcePreview, String> {
        let snapshot = self.state().collector.snapshot()?;
        let preview = preview_from_snapshot(snapshot, self.new_token(), Instant::now())?;
        let public = preview.public(self.existing_id(&preview.url)?);
        lock_recover(&self.inner).preview.put(preview);
        Ok(public)
    }

    fn existing_id(&self, url: &str) -> Result<Option<i64>, String> {
        let state = self.state();
        let existing = lock_recover(&state.store)
            .source_by_url(url)
            .map_err(|error| error.to_string())?;
        Ok(existing.map(|source| source.id))
    }

    /// Creates the source of a preview (saving a rule first for a collect session without
    /// one), records every item it has now as known, and with `SaveAll` also queues them.
    /// Registering counts as the source's first check.
    pub fn register(&self, token: &str, choice: RegisterChoice) -> Result<SourceRow, String> {
        let preview = lock_recover(&self.inner)
            .preview
            .get(token, Instant::now())
            .ok_or_else(|| "the preview expired".to_owned())?;
        let registered = {
            let state = self.state();
            let store = lock_recover(&state.store);
            register_in_store(&store, &preview, choice, unix_now())?
        };
        lock_recover(&self.inner).preview.remove(token);
        if let Some(rule_id) = registered.created_rule {
            self.state()
                .collector
                .rule_saved(&self.app, &preview.url, rule_id);
        }
        self.enqueue_items(&json!({}), registered.rule_id, &registered.queued)?;
        self.updated(registered.id)
    }

    /// Queues a check of one source.
    pub fn check(&self, id: i64) -> Result<(), String> {
        let state = self.state();
        let found = lock_recover(&state.store).get_source(id);
        match found {
            Ok(_) => {}
            Err(StoreError::NotFound { .. }) => return Err("source not found".to_owned()),
            Err(error) => return Err(error.to_string()),
        }
        self.queue_checks(vec![id]);
        Ok(())
    }

    /// Queues a check of every source, in id order.
    pub fn check_all(&self) -> Result<(), String> {
        let state = self.state();
        let sources = lock_recover(&state.store)
            .list_sources()
            .map_err(|error| error.to_string())?;
        self.queue_checks(sources.into_iter().map(|source| source.id).collect());
        Ok(())
    }

    fn queue_checks(&self, ids: Vec<i64>) {
        let queued: Vec<i64> = {
            let mut inner = lock_recover(&self.inner);
            ids.into_iter().filter(|id| inner.queue.push(*id)).collect()
        };
        for id in &queued {
            self.emit_row(*id);
        }
        if !queued.is_empty() {
            self.wake.notify_one();
        }
    }

    pub fn rename(&self, id: i64, name: &str) -> Result<SourceRow, String> {
        let name = valid_name(name)?;
        let state = self.state();
        lock_recover(&state.store)
            .rename_source(id, &name)
            .map_err(store_message)?;
        self.updated(id)
    }

    pub fn set_options(
        &self,
        id: i64,
        dest_dir: Option<String>,
        preset: Option<Preset>,
    ) -> Result<SourceRow, String> {
        validate_dest_dir(dest_dir.as_deref())?;
        {
            let state = self.state();
            let store = lock_recover(&state.store);
            let source = store.get_source(id).map_err(store_message)?;
            store
                .set_source_overrides(
                    id,
                    with_options(source.overrides, dest_dir.as_deref(), preset),
                )
                .map_err(store_message)?;
        }
        self.updated(id)
    }

    /// Deletes a source and its known items (files, library rows and jobs stay). A waiting
    /// check is dropped and a running one cancelled; nothing is written for it.
    pub fn delete(&self, id: i64) -> Result<(), String> {
        {
            let mut inner = lock_recover(&self.inner);
            inner.queue.remove(id);
            if let Some((running, cancel)) = &inner.running
                && *running == id
            {
                cancel.cancel();
            }
        }
        let state = self.state();
        lock_recover(&state.store)
            .delete_source(id)
            .map_err(|error| error.to_string())?;
        let _ = self.app.emit("source-removed", SourceRemoved { id });
        Ok(())
    }

    /// Runs the queued checks one at a time, for as long as the app runs.
    pub async fn run_checks(self: Arc<Self>) {
        loop {
            let next = {
                let mut inner = lock_recover(&self.inner);
                inner.queue.start_next().map(|id| {
                    let cancel = CancelToken::new();
                    inner.running = Some((id, cancel.clone()));
                    (id, cancel)
                })
            };
            let Some((id, cancel)) = next else {
                self.wake.notified().await;
                continue;
            };
            self.emit_row(id);
            let result = self.check_one(id, &cancel).await;
            // Cancelled only by deleting the source: nothing is written for it.
            let deleted = cancel.is_cancelled();
            if !deleted && let Some(result) = &result {
                self.record(id, result);
                if let SourceResult::New { count } = result {
                    self.notify_found(id, *count);
                }
            }
            {
                let mut inner = lock_recover(&self.inner);
                inner.queue.finish(id);
                inner.running = None;
            }
            if !deleted {
                self.emit_row(id);
            }
        }
    }

    fn record(&self, id: i64, result: &SourceResult) {
        let value = match serde_json::to_value(result) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("could not serialize the result of source {id}: {error}");
                return;
            }
        };
        let state = self.state();
        match lock_recover(&state.store).set_source_checked(id, unix_now(), &value) {
            Ok(()) | Err(StoreError::NotFound { .. }) => {}
            Err(error) => eprintln!("could not record the check of source {id}: {error}"),
        }
    }

    /// Announces a check that queued `count` new items as a notification, under the
    /// source's current name.
    fn notify_found(&self, id: i64, count: usize) {
        if count == 0 {
            return;
        }
        let state = self.state();
        let loaded = lock_recover(&state.store).get_source(id);
        match loaded {
            Ok(source) => state.notifier.send(NotifyEvent::Found {
                source_id: id,
                name: source.name,
                count,
            }),
            Err(StoreError::NotFound { .. }) => {}
            Err(error) => eprintln!("could not read source {id} for a notification: {error}"),
        }
    }

    /// Queues every idle source that is due, once a minute while the app runs.
    pub async fn run_auto_checks(self: Arc<Self>) {
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + AUTO_CHECK_TICK,
            AUTO_CHECK_TICK,
        );
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            lock_recover(&self.inner).preview.expire(Instant::now());
            let setting = lock_recover(&self.state().settings).source_auto_check;
            if setting == SourceAutoCheck::Off {
                continue;
            }
            let sources = {
                let state = self.state();
                let listed = lock_recover(&state.store).list_sources();
                match listed {
                    Ok(sources) => sources,
                    Err(error) => {
                        eprintln!("could not list sources for automatic checks: {error}");
                        continue;
                    }
                }
            };
            let now = unix_now();
            self.queue_checks(
                sources
                    .iter()
                    .filter(|source| auto_check_due(setting, source.last_checked_at, now))
                    .map(|source| source.id)
                    .collect(),
            );
        }
    }

    /// Checks one source; `None` when it no longer exists.
    async fn check_one(&self, id: i64, cancel: &CancelToken) -> Option<SourceResult> {
        let loaded = {
            let state = self.state();
            lock_recover(&state.store).get_source(id)
        };
        let source = match loaded {
            Ok(source) => source,
            Err(StoreError::NotFound { .. }) => return None,
            Err(error) => {
                return Some(SourceResult::Error {
                    category: ErrorCategory::Unknown,
                    message: error.to_string(),
                });
            }
        };
        let outcome = match source.kind {
            SourceKind::YtDlp => self.check_list(&source, cancel).await,
            SourceKind::Rule => self.check_rule(&source, cancel).await,
        };
        Some(match outcome {
            Ok(0) => SourceResult::Nothing,
            Ok(count) => SourceResult::New { count },
            Err(error) => SourceResult::Error {
                category: error.category,
                message: error.message,
            },
        })
    }

    /// The whole flat list, compared with the known items.
    async fn check_list(&self, source: &Source, cancel: &CancelToken) -> Result<usize, JobFailure> {
        let label = format!("source-{}", source.id);
        match self.probe_list(&source.url, cancel, &label).await? {
            ListProbe::List { json, .. } => {
                self.queue_new_items(source, list_items(&source.url, &json), cancel)
            }
            ListProbe::NotAList => Err(JobFailure {
                category: ErrorCategory::NoVideo,
                message: "this URL no longer shows a list".to_owned(),
            }),
        }
    }

    /// One yt-dlp run for a list URL with the cookies of the matching site's selected
    /// account, written (if it needs a file) to `cookies-<label>-<n>.txt` and deleted
    /// afterwards.
    async fn probe_list(
        &self,
        url: &str,
        cancel: &CancelToken,
        label: &str,
    ) -> Result<ListProbe, JobFailure> {
        let state = self.state();
        let tools = managed_tools(&state)?;
        let file_name = format!("cookies-{label}-{}.txt", self.next_sequence());
        let prepared = state::account_cookies(
            &state.store,
            &state.work_dir,
            &file_name,
            label,
            url,
            |account_id, domains| {
                state::in_app_cookie_text(&state.browser, &self.app, account_id, domains)
            },
        )
        .await;
        // `_temporary` keeps the cookie file until yt-dlp has finished.
        let (cookies, _temporary) = match prepared {
            Some(prepared) => (prepared.cookies, prepared.temporary),
            None => (Cookies::None, None),
        };
        let executor = YtDlpExecutor::new(
            ExecutorConfig {
                tools,
                work_dir: state.work_dir.clone(),
                target_os: TARGET_OS,
                gallery_dl: None,
            },
            Arc::clone(&state.store),
        );
        executor.probe_list(url, cookies, cancel).await
    }

    /// The page in a hidden webview of the site's selected in-app account (else the general
    /// browser folder), scrolled per the rule until it shows a known item.
    async fn check_rule(&self, source: &Source, cancel: &CancelToken) -> Result<usize, JobFailure> {
        let state = self.state();
        let (rule, profile) = {
            let store = lock_recover(&state.store);
            let rule = match source.rule_id.map(|id| store.get_rule(id)) {
                Some(Ok(rule)) => rule,
                None | Some(Err(StoreError::NotFound { .. })) => {
                    return Err(failure("the source's rule was deleted"));
                }
                Some(Err(error)) => return Err(failure(error.to_string())),
            };
            let profile = match sites::selected_account_for_url(&store, &source.url) {
                Ok(Some((_, account))) if account.method == sites::LoginMethod::InApp => {
                    BrowserProfile::Account(account.id)
                }
                _ => BrowserProfile::General,
            };
            (rule, profile)
        };
        let shape =
            rule_shape(&rule).ok_or_else(|| failure("the source's rule has no link shape"))?;
        let max_scans = rule_scan_limit(
            rule.rule_json
                .pointer("/stop/maxPages")
                .and_then(Value::as_u64),
        );
        let include_ads = lock_recover(&state.settings).collect_include_ads;
        let collected = self
            .scan_rule_page(source, profile, &shape, include_ads, max_scans, cancel)
            .await?;
        self.queue_new_items(source, rule_check_items(collected)?, cancel)
    }

    /// Opens the page, waits for it to load, then scans with the rule's shape and scrolls
    /// until [`rule_scan_done`]. Returns every item found, in page order. The webview is
    /// closed on every path.
    async fn scan_rule_page(
        &self,
        source: &Source,
        profile: BrowserProfile,
        shape: &str,
        include_ads: bool,
        max_scans: u32,
        cancel: &CancelToken,
    ) -> Result<Vec<CollectItem>, JobFailure> {
        let url = Url::parse(&source.url)
            .map_err(|error| failure(format!("the source's URL is not valid: {error}")))?;
        let scan = format!(
            "{}\n{}",
            collector::COLLECTOR_SCRIPT,
            collector::scan_expression(&Some(shape.to_owned()), include_ads).map_err(failure)?
        );
        let step = format!(
            "{}\nwindow.__vdlCollector.step()",
            collector::COLLECTOR_SCRIPT
        );
        let mut page = {
            let state = self.state();
            state
                .browser
                .open_check_page(&self.app, profile, url)
                .map_err(failure)?
        };
        until_cancelled(cancel, page.loaded(PAGE_LOAD_TIMEOUT))
            .await?
            .map_err(|message| JobFailure {
                category: ErrorCategory::Network,
                message,
            })?;
        // Pages often fill their listing right after the load event.
        until_cancelled(cancel, tokio::time::sleep(collector::SCROLL_WAIT)).await?;

        let mut items = Vec::new();
        let mut ads = HashSet::new();
        let mut scans = 0;
        let mut empty_in_a_row = 0;
        loop {
            let value = until_cancelled(cancel, page.execute(&scan))
                .await?
                .map_err(failure)?;
            let result = collector::parse_scan_result(value).map_err(failure)?;
            scans += 1;
            ads.extend(result.ads);
            items.retain(|item: &CollectItem| !ads.contains(&item.url));
            let scanned: Vec<String> = result
                .items
                .iter()
                .filter(|item| !ads.contains(&item.url))
                .map(|item| item.url.clone())
                .collect();
            let saw_known = self.any_known(source.id, &scanned)?;
            let added = collector::merge_items(&mut items, result.items, &ads);
            empty_in_a_row = if added == 0 { empty_in_a_row + 1 } else { 0 };
            if rule_scan_done(scans, max_scans, saw_known, empty_in_a_row) {
                break;
            }
            until_cancelled(cancel, page.execute(&step))
                .await?
                .map_err(failure)?;
            until_cancelled(cancel, tokio::time::sleep(collector::SCROLL_WAIT)).await?;
        }
        drop(page);
        Ok(items)
    }

    /// Whether any of `keys` is already known for the source.
    fn any_known(&self, source_id: i64, keys: &[String]) -> Result<bool, JobFailure> {
        let unique = keys.iter().collect::<HashSet<_>>().len();
        let state = self.state();
        let new = lock_recover(&state.store)
            .filter_new(source_id, keys)
            .map_err(|error| failure(error.to_string()))?;
        Ok(new.len() < unique)
    }

    /// Queues the items not yet known for the source, in list order, and records them as
    /// known. Returns how many were queued.
    fn queue_new_items(
        &self,
        source: &Source,
        items: Vec<SourceItem>,
        cancel: &CancelToken,
    ) -> Result<usize, JobFailure> {
        let state = self.state();
        let keys: Vec<String> = items.iter().map(|item| item.key.clone()).collect();
        let new_keys = lock_recover(&state.store)
            .filter_new(source.id, &keys)
            .map_err(|error| failure(error.to_string()))?;
        if cancel.is_cancelled() {
            return Err(canceled());
        }
        let fresh = new_items(items, &new_keys);
        self.enqueue_items(&source.overrides, source.rule_id, &fresh)
            .map_err(failure)?;
        lock_recover(&state.store)
            .mark_known(source.id, &new_keys, unix_now())
            .map_err(|error| failure(error.to_string()))?;
        Ok(fresh.len())
    }

    /// Queues items like `enqueue_urls`, with the source's quality and destination (else the
    /// current defaults).
    fn enqueue_items(
        &self,
        overrides: &Value,
        rule_id: Option<i64>,
        items: &[SourceItem],
    ) -> Result<(), String> {
        let state = self.state();
        let settings = lock_recover(&state.settings).clone();
        let rule_options = rule_id
            .and_then(|id| lock_recover(&state.store).get_rule(id).ok())
            .map(|rule| rule.options)
            .unwrap_or_else(|| json!({}));
        let options = source_job_options(&settings, overrides, &rule_options, self.tz_offset());
        for item in items {
            let (url, options) = job_for_item(&options, item)?;
            state.queue.enqueue(url, options);
        }
        Ok(())
    }
}

/// What [`register_in_store`] created.
#[derive(Debug)]
struct Registered {
    id: i64,
    rule_id: Option<i64>,
    /// The rule saved for a collect session that had none.
    created_rule: Option<i64>,
    /// The items to queue now: every item for `SaveAll`, none for `Record`.
    queued: Vec<SourceItem>,
}

/// Creates a preview's source in the store: a rule first when the collect session had none
/// (named after the preview, matching exactly its page URL), then the source; every item
/// it has now is recorded as known, and the registration is recorded as the first check
/// (`none` for `Record`, `new` with the number of items to queue for `SaveAll`). Nothing
/// is left behind when a step fails.
fn register_in_store(
    store: &Store,
    preview: &Preview,
    choice: RegisterChoice,
    now: i64,
) -> Result<Registered, String> {
    if store
        .source_by_url(&preview.url)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("this URL is already a source".to_owned());
    }
    let keys = preview.keys();
    let queued = match choice {
        RegisterChoice::Record => Vec::new(),
        RegisterChoice::SaveAll => new_items(preview.items.clone(), &keys),
    };
    let result = if queued.is_empty() {
        SourceResult::Nothing
    } else {
        SourceResult::New {
            count: queued.len(),
        }
    };
    let result = serde_json::to_value(result).map_err(|error| error.to_string())?;
    let (kind, rule_id, created_rule) = match &preview.rule {
        None => (SourceKind::YtDlp, None, None),
        Some(PreviewRule {
            rule_id: Some(id), ..
        }) => (SourceKind::Rule, Some(*id), None),
        Some(rule) => {
            let rule_json = collector::build_rule_json(
                &preview.name,
                &preview.url,
                &rule.shape,
                rule.max_pages,
            );
            let id = store
                .add_rule(&preview.name, &preview.url, rule_json, now)
                .map_err(|error| error.to_string())?;
            (SourceKind::Rule, Some(id), Some(id))
        }
    };
    let added = store
        .add_source(
            NewSource {
                name: preview.name.clone(),
                url: preview.url.clone(),
                kind,
                rule_id,
                overrides: json!({}),
            },
            now,
        )
        .and_then(|id| {
            let recorded = store
                .mark_known(id, &keys, now)
                .and_then(|()| store.set_source_checked(id, now, &result));
            match recorded {
                Ok(()) => Ok(id),
                Err(error) => {
                    let _ = store.delete_source(id);
                    Err(error)
                }
            }
        });
    match added {
        Ok(id) => Ok(Registered {
            id,
            rule_id,
            created_rule,
            queued,
        }),
        Err(error) => {
            if let Some(rule_id) = created_rule {
                let _ = store.delete_rule(rule_id);
            }
            Err(error.to_string())
        }
    }
}

/// A rule check's items, keyed by URL in page order. A check that found nothing at all for
/// the rule's shape fails instead of reporting nothing new: an error page after a failed
/// load, a logged-out page or a rule that no longer matches looks exactly like that.
fn rule_check_items(collected: Vec<CollectItem>) -> Result<Vec<SourceItem>, JobFailure> {
    if collected.is_empty() {
        return Err(JobFailure {
            category: ErrorCategory::NoVideo,
            message: "the page showed nothing for this source's rule".to_owned(),
        });
    }
    Ok(collected
        .into_iter()
        .map(|item| SourceItem {
            key: item.url.clone(),
            url: item.url,
            playlist_item: None,
            title: (!item.title.is_empty()).then_some(item.title),
        })
        .collect())
}

/// yt-dlp, ffmpeg and Deno as installed by the app (never from PATH).
pub(crate) fn managed_tools(state: &AppState) -> Result<ToolPaths, JobFailure> {
    match (
        state.tools.resolve(Tool::YtDlp, TARGET_OS),
        state.tools.resolve(Tool::Ffmpeg, TARGET_OS),
        state.tools.resolve(Tool::Deno, TARGET_OS),
    ) {
        (Some(yt_dlp), Some(ffmpeg), Some(deno)) => Ok(ToolPaths {
            yt_dlp: yt_dlp.path,
            ffmpeg_dir: ffmpeg.path,
            deno: deno.path,
        }),
        _ => Err(failure("required tools are not installed")),
    }
}

async fn until_cancelled<F: Future>(
    cancel: &CancelToken,
    future: F,
) -> Result<F::Output, JobFailure> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(canceled()),
        output = future => Ok(output),
    }
}

/// The row the UI renders for a source.
fn source_row(
    source: &Source,
    sites: &[Site],
    rule: Option<&Rule>,
    status: SourceStatus,
) -> SourceRow {
    let site = sites::site_for_url_in(sites, &source.url)
        .and_then(|site_id| sites.iter().find(|site| site.id == site_id));
    let site_name = site.map_or_else(|| site_from_url(&source.url), |site| site.name.clone());
    let account_label = site
        .filter(|site| site.accounts.len() >= 2)
        .and_then(|site| {
            site.selected_account_id
                .and_then(|id| site.accounts.iter().find(|account| account.id == id))
        })
        .map(|account| account.label.clone());
    let (kind, rule_shape) = match source.kind {
        SourceKind::YtDlp => (SourceRowKind::List, None),
        SourceKind::Rule => (SourceRowKind::Rule, rule.and_then(rule_shape)),
    };
    SourceRow {
        id: source.id,
        name: source.name.clone(),
        url: source.url.clone(),
        kind,
        site_name,
        rule_shape,
        account_label,
        dest_dir: overrides_dest_dir(&source.overrides),
        preset: overrides_preset(&source.overrides),
        last_checked_at: source.last_checked_at,
        last_result: source
            .last_result
            .clone()
            .and_then(|value| serde_json::from_value(value).ok()),
        status,
    }
}

fn rule_shape(rule: &Rule) -> Option<String> {
    rule.rule_json
        .pointer("/items/href")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// A source item's job options: the source's preset (else the default preset) and
/// destination (else the download folder) on the usual job options.
fn source_job_options(
    settings: &Settings,
    overrides: &Value,
    rule_options: &Value,
    tz_offset_seconds: i64,
) -> JobOptions {
    let preset = overrides_preset(overrides)
        .or_else(|| overrides_preset(rule_options))
        .unwrap_or(settings.default_preset);
    let mut options = job_options(settings, preset, tz_offset_seconds);
    if let Some(dir) = overrides_dest_dir(overrides).or_else(|| overrides_dest_dir(rule_options)) {
        options.dest_dir = PathBuf::from(dir);
    }
    options
}

/// The URL and options of one item's job: an item that is only reachable through the list
/// keeps the list URL and selects its number with `playlist_item`.
fn job_for_item(options: &JobOptions, item: &SourceItem) -> Result<(String, Value), String> {
    let mut options = options.clone();
    options.playlist_item = item.playlist_item;
    let options = serde_json::to_value(options)
        .map_err(|error| format!("could not serialize job options: {error}"))?;
    Ok((item.url.clone(), options))
}

/// The items whose key is in `new_keys`, the first of each key, in list order.
fn new_items(items: Vec<SourceItem>, new_keys: &[String]) -> Vec<SourceItem> {
    let wanted: HashSet<&str> = new_keys.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| wanted.contains(item.key.as_str()) && seen.insert(item.key.clone()))
        .collect()
}

/// A rule source's preview from the collect session: it needs a chosen link shape; named
/// after the page title, else the host; its items are the session's items, keyed by URL.
fn preview_from_snapshot(
    snapshot: CollectSnapshot,
    token: String,
    now: Instant,
) -> Result<Preview, String> {
    let shape = snapshot
        .shape
        .ok_or_else(|| "no link shape is selected".to_owned())?;
    if shape == PAGE_IMAGES_SHAPE {
        return Err("a source needs a link shape, not the page's own images".to_owned());
    }
    let name = snapshot
        .page_title
        .map(|title| truncate_chars(&title, MAX_NAME_CHARS))
        .unwrap_or_else(|| {
            Url::parse(&snapshot.page_url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_else(|| snapshot.page_url.clone())
        });
    let mut seen = HashSet::new();
    let items = snapshot
        .items
        .into_iter()
        .filter(|item| seen.insert(item.url.clone()))
        .map(|item| SourceItem {
            key: item.url.clone(),
            url: item.url,
            playlist_item: None,
            title: (!item.title.is_empty()).then_some(item.title),
        })
        .collect();
    Ok(Preview {
        token,
        created: now,
        name,
        url: snapshot.page_url,
        items,
        rule: Some(PreviewRule {
            shape,
            max_pages: snapshot.max_scrolls.min(MAX_RULE_SCANS),
            rule_id: snapshot.rule_id,
        }),
    })
}

/// How many scans a rule check makes at most: the rule's `maxPages` (10 when absent),
/// from 1 to 10.
fn rule_scan_limit(max_pages: Option<u64>) -> u32 {
    let limit = max_pages
        .unwrap_or(u64::from(MAX_RULE_SCANS))
        .clamp(1, u64::from(MAX_RULE_SCANS));
    u32::try_from(limit).unwrap_or(MAX_RULE_SCANS)
}

/// Whether a rule check stops after its `scans`-th scan: that scan showed an item already
/// known for the source, the last 2 scans added nothing, or the scan limit is reached.
fn rule_scan_done(scans: u32, max_scans: u32, saw_known: bool, empty_scans_in_a_row: u32) -> bool {
    saw_known || empty_scans_in_a_row >= 2 || scans >= max_scans
}

/// Whether an automatic check is due: never when off; else when the source was never
/// checked or its last check is at least the interval old.
fn auto_check_due(setting: SourceAutoCheck, last_checked_at: Option<i64>, now: i64) -> bool {
    let Some(interval) = setting.interval_seconds() else {
        return false;
    };
    last_checked_at.is_none_or(|checked| now.saturating_sub(checked) >= interval)
}

fn preview_failure(category: ErrorCategory) -> PreviewFailure {
    match category {
        ErrorCategory::LoginRequired => PreviewFailure::LoginRequired,
        ErrorCategory::UnsupportedUrl => PreviewFailure::Unsupported,
        _ => PreviewFailure::Other,
    }
}

/// A source name: trimmed, not empty, at most 200 characters.
fn valid_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the name is empty".to_owned());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "the name is longer than {MAX_NAME_CHARS} characters"
        ));
    }
    Ok(name.to_owned())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.trim().chars().take(max_chars).collect()
}

fn store_message(error: StoreError) -> String {
    match error {
        StoreError::NotFound { .. } => "source not found".to_owned(),
        error => error.to_string(),
    }
}

fn failure(message: impl Into<String>) -> JobFailure {
    JobFailure {
        category: ErrorCategory::Unknown,
        message: message.into(),
    }
}

fn canceled() -> JobFailure {
    failure("canceled")
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

#[tauri::command]
pub async fn sources_list(
    tz_offset_seconds: i64,
    state: State<'_, AppState>,
) -> Result<Vec<SourceRow>, String> {
    state.sources.remember_tz_offset(tz_offset_seconds);
    state.sources.rows()
}

#[tauri::command]
pub async fn source_preview(
    url: String,
    state: State<'_, AppState>,
) -> Result<SourcePreviewOutcome, String> {
    state.sources.preview_url(url).await
}

#[tauri::command]
pub async fn source_preview_collect(state: State<'_, AppState>) -> Result<SourcePreview, String> {
    state.sources.preview_collect()
}

#[tauri::command]
pub async fn source_register(
    token: String,
    choice: RegisterChoice,
    tz_offset_seconds: i64,
    state: State<'_, AppState>,
) -> Result<SourceRow, String> {
    state.sources.remember_tz_offset(tz_offset_seconds);
    state.sources.register(&token, choice)
}

#[tauri::command]
pub async fn source_check(
    id: i64,
    tz_offset_seconds: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.sources.remember_tz_offset(tz_offset_seconds);
    state.sources.check(id)
}

#[tauri::command]
pub async fn source_check_all(
    tz_offset_seconds: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.sources.remember_tz_offset(tz_offset_seconds);
    state.sources.check_all()
}

#[tauri::command]
pub async fn source_rename(
    id: i64,
    name: String,
    state: State<'_, AppState>,
) -> Result<SourceRow, String> {
    state.sources.rename(id, &name)
}

#[tauri::command]
pub async fn source_set_options(
    id: i64,
    dest_dir: Option<String>,
    preset: Option<Preset>,
    state: State<'_, AppState>,
) -> Result<SourceRow, String> {
    state.sources.set_options(id, dest_dir, preset)
}

#[tauri::command]
pub async fn source_delete(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state.sources.delete(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sites::{Account, LoginMethod};

    fn item(key: &str, url: &str, playlist_item: Option<u32>) -> SourceItem {
        SourceItem {
            key: key.to_owned(),
            url: url.to_owned(),
            playlist_item,
            title: None,
        }
    }

    fn preview(token: &str, created: Instant) -> Preview {
        Preview {
            token: token.to_owned(),
            created,
            name: "List".to_owned(),
            url: "https://example.com/list".to_owned(),
            items: vec![item("Example:1", "https://example.com/1", None)],
            rule: None,
        }
    }

    #[test]
    fn a_new_preview_replaces_the_old_one() {
        let start = Instant::now();
        let mut slot = PreviewSlot::default();
        slot.put(preview("a", start));
        slot.put(preview("b", start));
        assert_eq!(slot.get("a", start), None);
        assert_eq!(
            slot.get("b", start).map(|preview| preview.token),
            Some("b".to_owned())
        );
    }

    #[test]
    fn a_preview_expires_after_fifteen_minutes() {
        let start = Instant::now();
        let mut slot = PreviewSlot::default();
        slot.put(preview("a", start));
        let almost = start + PREVIEW_TTL - Duration::from_secs(1);
        assert!(slot.get("a", almost).is_some());
        assert_eq!(slot.get("a", start + PREVIEW_TTL), None);
        // Gone for good, not only for that call.
        assert!(slot.current.is_none());

        slot.put(preview("b", start));
        slot.expire(start + PREVIEW_TTL + Duration::from_secs(60));
        assert!(slot.current.is_none());
    }

    #[test]
    fn removing_a_preview_needs_its_token() {
        let start = Instant::now();
        let mut slot = PreviewSlot::default();
        slot.put(preview("b", start));
        slot.remove("a");
        assert!(slot.get("b", start).is_some());
        slot.remove("b");
        assert!(slot.get("b", start).is_none());
    }

    #[test]
    fn checks_run_first_come_first_served_and_a_source_is_queued_once() {
        let mut queue = CheckQueue::default();
        assert!(queue.push(3));
        assert!(queue.push(1));
        assert!(!queue.push(3), "already waiting");
        assert_eq!(queue.status(3), SourceStatus::Waiting);
        assert_eq!(queue.start_next(), Some(3));
        assert_eq!(queue.status(3), SourceStatus::Checking);
        assert!(!queue.push(3), "being checked");
        assert_eq!(queue.start_next(), None, "one check at a time");
        queue.finish(3);
        assert_eq!(queue.status(3), SourceStatus::Idle);
        assert!(queue.push(3), "idle again");
        queue.remove(1);
        assert_eq!(queue.status(1), SourceStatus::Idle);
        assert_eq!(queue.start_next(), Some(3));
        assert_eq!(queue.start_next(), None);
    }

    #[test]
    fn a_rule_check_stops_at_a_known_item_after_two_empty_scans_or_at_the_limit() {
        // Nothing known, new items each time: scans go on until the limit.
        assert!(!rule_scan_done(1, 10, false, 0));
        assert!(!rule_scan_done(9, 10, false, 0));
        assert!(rule_scan_done(10, 10, false, 0));
        // A scan that showed a known item is the last one.
        assert!(rule_scan_done(1, 10, true, 0));
        // One scan that added nothing is not enough; two in a row are.
        assert!(!rule_scan_done(3, 10, false, 1));
        assert!(rule_scan_done(3, 10, false, 2));
        // A limit of one scan never scrolls.
        assert!(rule_scan_done(1, 1, false, 0));
    }

    #[test]
    fn the_rule_scan_limit_is_the_rules_max_pages_from_one_to_ten() {
        assert_eq!(rule_scan_limit(None), 10);
        assert_eq!(rule_scan_limit(Some(0)), 1);
        assert_eq!(rule_scan_limit(Some(4)), 4);
        assert_eq!(rule_scan_limit(Some(10)), 10);
        assert_eq!(rule_scan_limit(Some(50)), 10);
        assert_eq!(rule_scan_limit(Some(u64::MAX)), 10);
    }

    #[test]
    fn automatic_checks_are_due_when_never_checked_or_an_interval_old() {
        let now = 1_800_000_000;
        assert!(!auto_check_due(SourceAutoCheck::Off, None, now));
        assert!(!auto_check_due(SourceAutoCheck::Off, Some(0), now));
        assert!(auto_check_due(SourceAutoCheck::Hourly, None, now));
        assert!(!auto_check_due(
            SourceAutoCheck::Hourly,
            Some(now - 3_599),
            now
        ));
        assert!(auto_check_due(
            SourceAutoCheck::Hourly,
            Some(now - 3_600),
            now
        ));
        assert!(!auto_check_due(
            SourceAutoCheck::SixHours,
            Some(now - 3_600),
            now
        ));
        assert!(auto_check_due(
            SourceAutoCheck::SixHours,
            Some(now - 21_600),
            now
        ));
        assert!(!auto_check_due(
            SourceAutoCheck::Daily,
            Some(now - 86_399),
            now
        ));
        assert!(auto_check_due(
            SourceAutoCheck::Daily,
            Some(now - 86_400),
            now
        ));
        // A clock set back does not make a source due.
        assert!(!auto_check_due(
            SourceAutoCheck::Hourly,
            Some(now + 60),
            now
        ));
    }

    fn account(id: i64, label: &str) -> Account {
        Account {
            id,
            label: label.to_owned(),
            method: LoginMethod::InApp,
            cookie_count: None,
            updated_at: None,
        }
    }

    fn site(id: i64, name: &str, domain: &str, accounts: Vec<Account>) -> Site {
        let selected_account_id = accounts.last().map(|account| account.id);
        Site {
            id,
            name: name.to_owned(),
            domains: vec![domain.to_owned()],
            login_url: format!("https://{domain}/login"),
            builtin: true,
            accounts,
            selected_account_id,
            ad_block: true,
        }
    }

    fn source(kind: SourceKind, url: &str, overrides: Value) -> Source {
        Source {
            id: 7,
            name: "Name".to_owned(),
            url: url.to_owned(),
            kind,
            rule_id: (kind == SourceKind::Rule).then_some(3),
            overrides,
            last_checked_at: Some(1_800_000_000),
            last_result: Some(json!({ "kind": "new", "count": 2 })),
            created_at: 1,
        }
    }

    fn rule() -> Rule {
        Rule {
            id: 3,
            name: "Talks".to_owned(),
            match_pattern: "https://www.ted.com/talks".to_owned(),
            rule_json: collector::build_rule_json(
                "Talks",
                "https://www.ted.com/talks",
                "/talks/{slug}",
                5,
            ),
            options: json!({}),
            last_used_at: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn a_source_row_serializes_in_the_contract_shape() {
        let sites = vec![site(
            1,
            "YouTube",
            "youtube.com",
            vec![account(10, "メイン"), account(11, "サブ")],
        )];
        let dir = std::env::current_dir()
            .unwrap()
            .join("blender")
            .to_string_lossy()
            .into_owned();
        let row = source_row(
            &source(
                SourceKind::YtDlp,
                "https://www.youtube.com/@BlenderOfficial/videos",
                json!({ "destDir": dir, "preset": "upTo720", "filter": {} }),
            ),
            &sites,
            None,
            SourceStatus::Waiting,
        );
        assert_eq!(
            serde_json::to_value(&row).unwrap(),
            json!({
                "id": 7,
                "name": "Name",
                "url": "https://www.youtube.com/@BlenderOfficial/videos",
                "kind": "list",
                "siteName": "YouTube",
                "ruleShape": null,
                "accountLabel": "サブ",
                "destDir": dir,
                "preset": "upTo720",
                "lastCheckedAt": 1_800_000_000,
                "lastResult": { "kind": "new", "count": 2 },
                "status": "waiting"
            })
        );
    }

    #[test]
    fn a_rule_row_has_its_shape_and_an_unregistered_host_without_www() {
        let one_account = vec![site(
            1,
            "YouTube",
            "youtube.com",
            vec![account(10, "メイン")],
        )];
        let row = source_row(
            &source(SourceKind::Rule, "https://www.ted.com/talks", json!({})),
            &one_account,
            Some(&rule()),
            SourceStatus::Idle,
        );
        assert_eq!(row.kind, SourceRowKind::Rule);
        assert_eq!(row.site_name, "ted.com");
        assert_eq!(row.rule_shape.as_deref(), Some("/talks/{slug}"));
        assert_eq!(row.account_label, None);
        assert_eq!(row.dest_dir, None);
        assert_eq!(row.preset, None);

        // A site with one account shows no account label.
        let row = source_row(
            &source(SourceKind::YtDlp, "https://www.youtube.com/@a", json!({})),
            &one_account,
            None,
            SourceStatus::Idle,
        );
        assert_eq!(row.account_label, None);
    }

    #[test]
    fn unreadable_overrides_and_results_count_as_absent() {
        let mut stored = source(
            SourceKind::YtDlp,
            "https://example.com/list",
            json!({ "destDir": "relative/dir", "preset": "8k" }),
        );
        stored.last_result = Some(json!({ "kind": "later" }));
        let row = source_row(&stored, &[], None, SourceStatus::Idle);
        assert_eq!(row.dest_dir, None);
        assert_eq!(row.preset, None);
        assert_eq!(row.last_result, None);
    }

    #[test]
    fn results_and_preview_outcomes_serialize_in_the_contract_shape() {
        assert_eq!(
            serde_json::to_value(SourceResult::New { count: 3 }).unwrap(),
            json!({ "kind": "new", "count": 3 })
        );
        assert_eq!(
            serde_json::to_value(SourceResult::Nothing).unwrap(),
            json!({ "kind": "none" })
        );
        let error = SourceResult::Error {
            category: ErrorCategory::LoginRequired,
            message: "members only".to_owned(),
        };
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(
            value,
            json!({ "kind": "error", "category": "LoginRequired", "message": "members only" })
        );
        assert_eq!(
            serde_json::from_value::<SourceResult>(value).unwrap(),
            error
        );

        let ready = SourcePreviewOutcome::Ready {
            preview: SourcePreview {
                token: "t".to_owned(),
                kind: SourceRowKind::List,
                name: "List".to_owned(),
                url: "https://example.com/list".to_owned(),
                count: 12,
                existing_id: None,
            },
        };
        assert_eq!(
            serde_json::to_value(ready).unwrap(),
            json!({
                "kind": "ready",
                "preview": {
                    "token": "t",
                    "kind": "list",
                    "name": "List",
                    "url": "https://example.com/list",
                    "count": 12,
                    "existingId": null
                }
            })
        );
        assert_eq!(
            serde_json::to_value(SourcePreviewOutcome::Failed {
                reason: PreviewFailure::NotAList,
                message: "single".to_owned(),
            })
            .unwrap(),
            json!({ "kind": "failed", "reason": "notAList", "message": "single" })
        );
        assert_eq!(
            serde_json::from_value::<RegisterChoice>(json!("saveAll")).unwrap(),
            RegisterChoice::SaveAll
        );
        assert_eq!(
            serde_json::to_value(SourceRemoved { id: 4 }).unwrap(),
            json!({ "id": 4 })
        );
    }

    #[test]
    fn preview_failures_follow_the_error_category() {
        assert_eq!(
            preview_failure(ErrorCategory::LoginRequired),
            PreviewFailure::LoginRequired
        );
        assert_eq!(
            preview_failure(ErrorCategory::UnsupportedUrl),
            PreviewFailure::Unsupported
        );
        assert_eq!(
            preview_failure(ErrorCategory::Network),
            PreviewFailure::Other
        );
    }

    #[test]
    fn source_items_get_the_sources_quality_and_folder_else_the_defaults() {
        let mut settings = Settings::defaults("/videos".to_owned());
        settings.default_preset = Preset::UpTo1080;

        let defaults = source_job_options(&settings, &json!({}), &json!({}), 32_400);
        assert_eq!(defaults.dest_dir, PathBuf::from("/videos"));
        assert_eq!(defaults.preset, vids_core::args::Preset::UpTo1080);
        assert_eq!(defaults.tz_offset_seconds, 32_400);

        let dir = std::env::current_dir().unwrap().join("blender");
        let overrides = json!({ "destDir": dir.to_string_lossy(), "preset": "audioM4a" });
        let own = source_job_options(&settings, &overrides, &json!({}), 0);
        assert_eq!(own.dest_dir, dir);
        assert_eq!(own.preset, vids_core::args::Preset::AudioM4a);

        let source_dir = std::env::current_dir().unwrap().join("source");
        let rule_dir = std::env::current_dir().unwrap().join("rule");
        let source = json!({ "destDir": source_dir.to_string_lossy() });
        let rule = json!({ "destDir": rule_dir.to_string_lossy(), "preset": "audioMp3" });
        let layered = source_job_options(&settings, &source, &rule, 0);
        assert_eq!(layered.dest_dir, source_dir);
        assert_eq!(layered.preset, vids_core::args::Preset::AudioMp3);

        let source = json!({ "preset": "upTo720" });
        let layered = source_job_options(&settings, &source, &rule, 0);
        assert_eq!(layered.dest_dir, rule_dir);
        assert_eq!(layered.preset, vids_core::args::Preset::UpTo720);
    }

    #[test]
    fn an_item_reachable_only_through_the_list_keeps_the_list_url_and_its_number() {
        let options = source_job_options(
            &Settings::defaults("/videos".to_owned()),
            &json!({}),
            &json!({}),
            0,
        );
        let (url, value) = job_for_item(
            &options,
            &item("Example:c", "https://example.com/list", Some(9)),
        )
        .unwrap();
        assert_eq!(url, "https://example.com/list");
        let read: JobOptions = serde_json::from_value(value).unwrap();
        assert_eq!(read.playlist_item, Some(9));

        let (url, value) =
            job_for_item(&options, &item("Example:d", "https://example.com/d", None)).unwrap();
        assert_eq!(url, "https://example.com/d");
        let read: JobOptions = serde_json::from_value(value).unwrap();
        assert_eq!(read.playlist_item, None);
    }

    #[test]
    fn new_items_keep_list_order_and_the_first_of_each_key() {
        let items = vec![
            item("a", "https://example.com/a", None),
            item("b", "https://example.com/b", None),
            item("a", "https://example.com/a2", None),
            item("c", "https://example.com/c", None),
        ];
        let fresh = new_items(items, &["c".to_owned(), "a".to_owned()]);
        let urls: Vec<_> = fresh.iter().map(|item| item.url.as_str()).collect();
        assert_eq!(urls, vec!["https://example.com/a", "https://example.com/c"]);
    }

    #[test]
    fn options_are_set_or_cleared_and_other_overrides_stay() {
        let set = with_options(
            json!({ "filter": { "minDurationS": 60 }, "preset": "best" }),
            Some("/videos/a"),
            Some(Preset::UpTo720),
        );
        assert_eq!(
            set,
            json!({ "filter": { "minDurationS": 60 }, "destDir": "/videos/a", "preset": "upTo720" })
        );
        assert_eq!(
            with_options(set, None, None),
            json!({ "filter": { "minDurationS": 60 } })
        );
        assert_eq!(
            with_options(json!(null), Some("/v"), None),
            json!({ "destDir": "/v" })
        );
    }

    fn collect_item(url: &str, title: &str) -> CollectItem {
        CollectItem {
            url: url.to_owned(),
            title: title.to_owned(),
            thumbnail: None,
            index: None,
            played: None,
        }
    }

    fn snapshot() -> CollectSnapshot {
        CollectSnapshot {
            page_url: "https://www.ted.com/talks?sort=newest".to_owned(),
            shape: Some("/talks/{slug}".to_owned()),
            items: vec![
                collect_item("https://www.ted.com/talks/a", "A"),
                collect_item("https://www.ted.com/talks/b", ""),
                collect_item("https://www.ted.com/talks/a", "A again"),
            ],
            max_scrolls: 25,
            page_title: Some("TED Talks".to_owned()),
            rule_id: None,
        }
    }

    #[test]
    fn a_collect_preview_takes_the_sessions_page_items_and_limits() {
        let now = Instant::now();
        let preview = preview_from_snapshot(snapshot(), "t".to_owned(), now).unwrap();
        assert_eq!(preview.name, "TED Talks");
        assert_eq!(preview.url, "https://www.ted.com/talks?sort=newest");
        assert_eq!(
            preview.keys(),
            vec!["https://www.ted.com/talks/a", "https://www.ted.com/talks/b"]
        );
        assert_eq!(preview.items[1].title, None);
        assert_eq!(
            preview.rule,
            Some(PreviewRule {
                shape: "/talks/{slug}".to_owned(),
                max_pages: 10,
                rule_id: None,
            })
        );
        assert_eq!(preview.public(Some(4)).count, 2);
        assert_eq!(preview.public(Some(4)).kind, SourceRowKind::Rule);
        assert_eq!(preview.public(Some(4)).existing_id, Some(4));

        let mut untitled = snapshot();
        untitled.page_title = None;
        untitled.rule_id = Some(8);
        untitled.max_scrolls = 3;
        let preview = preview_from_snapshot(untitled, "t".to_owned(), now).unwrap();
        assert_eq!(preview.name, "www.ted.com");
        assert_eq!(preview.rule.unwrap().max_pages, 3);
    }

    #[test]
    fn a_collect_preview_needs_a_link_shape() {
        let now = Instant::now();
        let mut none = snapshot();
        none.shape = None;
        assert!(preview_from_snapshot(none, "t".to_owned(), now).is_err());
        let mut images = snapshot();
        images.shape = Some(PAGE_IMAGES_SHAPE.to_owned());
        assert!(preview_from_snapshot(images, "t".to_owned(), now).is_err());
    }

    #[test]
    fn source_names_are_trimmed_and_bounded() {
        assert_eq!(valid_name("  Blender  ").unwrap(), "Blender");
        assert!(valid_name("   ").is_err());
        assert_eq!(valid_name(&"名".repeat(200)).unwrap().chars().count(), 200);
        assert!(valid_name(&"名".repeat(201)).is_err());
    }

    fn list_preview(items: Vec<SourceItem>) -> Preview {
        Preview {
            token: "t".to_owned(),
            created: Instant::now(),
            name: "Blender - Videos".to_owned(),
            url: "https://www.youtube.com/@BlenderOfficial/videos".to_owned(),
            items,
            rule: None,
        }
    }

    fn three_items() -> Vec<SourceItem> {
        vec![
            item("Youtube:a", "https://www.youtube.com/watch?v=a", None),
            item("Youtube:b", "https://www.youtube.com/watch?v=b", None),
            item("Youtube:a", "https://www.youtube.com/shorts/a", None),
        ]
    }

    #[test]
    fn registering_to_record_counts_as_a_first_check_with_nothing_new() {
        let store = Store::open_in_memory().unwrap();
        let registered = register_in_store(
            &store,
            &list_preview(three_items()),
            RegisterChoice::Record,
            1_800_000_000,
        )
        .unwrap();
        assert!(registered.queued.is_empty());
        assert_eq!(registered.created_rule, None);

        let source = store.get_source(registered.id).unwrap();
        assert_eq!(source.kind, SourceKind::YtDlp);
        let row = source_row(&source, &[], None, SourceStatus::Idle);
        assert_eq!(row.last_checked_at, Some(1_800_000_000));
        assert_eq!(row.last_result, Some(SourceResult::Nothing));
        // Every item present now is known.
        assert!(
            store
                .filter_new(
                    registered.id,
                    &["Youtube:a".to_owned(), "Youtube:b".to_owned()]
                )
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn registering_to_save_all_counts_the_items_it_queues() {
        let store = Store::open_in_memory().unwrap();
        let registered = register_in_store(
            &store,
            &list_preview(three_items()),
            RegisterChoice::SaveAll,
            1_800_000_100,
        )
        .unwrap();
        let urls: Vec<_> = registered
            .queued
            .iter()
            .map(|item| item.url.as_str())
            .collect();
        assert_eq!(
            urls,
            vec![
                "https://www.youtube.com/watch?v=a",
                "https://www.youtube.com/watch?v=b"
            ]
        );
        let row = source_row(
            &store.get_source(registered.id).unwrap(),
            &[],
            None,
            SourceStatus::Idle,
        );
        assert_eq!(row.last_checked_at, Some(1_800_000_100));
        assert_eq!(row.last_result, Some(SourceResult::New { count: 2 }));

        // An empty list saves nothing: nothing new.
        let mut empty = list_preview(Vec::new());
        empty.url = "https://www.youtube.com/@Empty/videos".to_owned();
        let registered =
            register_in_store(&store, &empty, RegisterChoice::SaveAll, 1_800_000_200).unwrap();
        assert_eq!(
            store.get_source(registered.id).unwrap().last_result,
            Some(json!({ "kind": "none" }))
        );
    }

    #[test]
    fn registering_a_collect_page_without_a_rule_saves_one_and_a_known_url_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let preview = preview_from_snapshot(snapshot(), "t".to_owned(), Instant::now()).unwrap();
        let registered =
            register_in_store(&store, &preview, RegisterChoice::Record, 1_800_000_000).unwrap();
        let rule_id = registered.created_rule.expect("a rule was saved");
        let source = store.get_source(registered.id).unwrap();
        assert_eq!(source.kind, SourceKind::Rule);
        assert_eq!(source.rule_id, Some(rule_id));
        let rule = store.get_rule(rule_id).unwrap();
        assert_eq!(rule.name, "TED Talks");
        assert_eq!(rule.match_pattern, "https://www.ted.com/talks?sort=newest");
        assert_eq!(
            rule.rule_json,
            collector::build_rule_json(
                "TED Talks",
                "https://www.ted.com/talks?sort=newest",
                "/talks/{slug}",
                10
            )
        );

        let again = register_in_store(&store, &preview, RegisterChoice::Record, 1_800_000_001);
        assert_eq!(again.unwrap_err(), "this URL is already a source");
        assert_eq!(store.list_rules().unwrap().len(), 1, "no second rule");
    }

    #[test]
    fn a_rule_check_that_found_nothing_at_all_is_an_error() {
        let error = rule_check_items(Vec::new()).unwrap_err();
        assert_eq!(error.category, ErrorCategory::NoVideo);
        assert_eq!(
            error.message,
            "the page showed nothing for this source's rule"
        );

        // Items were found (whether or not they are known): they are compared as usual.
        let items = rule_check_items(vec![
            collect_item("https://www.ted.com/talks/a", "A"),
            collect_item("https://www.ted.com/talks/b", ""),
        ])
        .unwrap();
        assert_eq!(
            items,
            vec![
                SourceItem {
                    key: "https://www.ted.com/talks/a".to_owned(),
                    url: "https://www.ted.com/talks/a".to_owned(),
                    playlist_item: None,
                    title: Some("A".to_owned()),
                },
                SourceItem {
                    key: "https://www.ted.com/talks/b".to_owned(),
                    url: "https://www.ted.com/talks/b".to_owned(),
                    playlist_item: None,
                    title: None,
                },
            ]
        );
    }

    #[test]
    fn a_rule_check_whose_items_are_all_known_finds_nothing_new() {
        let store = Store::open_in_memory().unwrap();
        let preview = preview_from_snapshot(snapshot(), "t".to_owned(), Instant::now()).unwrap();
        let registered =
            register_in_store(&store, &preview, RegisterChoice::Record, 1_800_000_000).unwrap();
        let items =
            rule_check_items(vec![collect_item("https://www.ted.com/talks/a", "A")]).unwrap();
        let keys: Vec<String> = items.iter().map(|item| item.key.clone()).collect();
        let new_keys = store.filter_new(registered.id, &keys).unwrap();
        assert!(new_items(items, &new_keys).is_empty());
    }
}

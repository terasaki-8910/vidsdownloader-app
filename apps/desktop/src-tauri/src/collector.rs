use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, State, Url};

use crate::browser::BrowserManager;
use crate::state::{AppState, lock_recover};

pub(crate) const COLLECTOR_SCRIPT: &str = include_str!("collector.js");
/// Wait after each scroll before the next scan (the fetching etiquette's 1.5 s).
pub(crate) const SCROLL_WAIT: Duration = Duration::from_millis(1500);
const PLAYED_REFRESH: Duration = Duration::from_secs(2);
const RELOAD_LOAD_LIMIT: Duration = Duration::from_secs(10);
const RELOAD_PLAY_AFTER: Duration = Duration::from_secs(3);
const RELOAD_FIND_LIMIT: Duration = Duration::from_secs(20);
const RELOAD_FIND_POLL: Duration = Duration::from_secs(1);
pub(crate) const PLAYED_SHAPE: &str = "__played__";
const IMAGES_SHAPE: &str = "{images}";
const MAX_SCAN_ITEMS: usize = 2_000;
const MAX_URL_CHARS: usize = 2_048;
const MAX_TITLE_CHARS: usize = 300;
const MAX_SHAPE_CHARS: usize = 2_048;
const MAX_PAGE_ID_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectGroup {
    pub shape: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectItem {
    pub url: String,
    pub title: String,
    pub thumbnail: Option<String>,
    /// A page image's 1-based number among the page's images, kept across scans; `None`
    /// for links.
    #[serde(default)]
    pub index: Option<u32>,
    /// Present only for a stream observed in the visible page's network traffic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub played: Option<CollectPlayed>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectPlayed {
    pub kind: String,
    pub duration_seconds: Option<u64>,
}

/// Facts about the page itself, read the same way on every site (Open Graph, JSON-LD,
/// meta tags, the URL); used to name saved page images.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectPage {
    pub title: String,
    pub uploader: Option<String>,
    /// Unix seconds.
    pub published_at: Option<i64>,
    /// The URL's last path segment without extension, when it has at least 3 characters.
    pub id: Option<String>,
}

/// The best-ranked link group's URLs, sent while the page's images are the chosen group so
/// the tray can list them folded under the images (「ほかのリンク」).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectLinks {
    pub shape: String,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectState {
    pub running: bool,
    pub scrolls: u32,
    pub max_scrolls: u32,
    pub groups: Vec<CollectGroup>,
    pub shape: Option<String>,
    pub items: Vec<CollectItem>,
    pub ads_excluded: usize,
    pub rule_id: Option<i64>,
    pub error: Option<String>,
    /// The page facts from the last scan; `None` until the first scan has returned.
    pub page: Option<CollectPage>,
    /// Set by a scan that chose the page's images; `None` otherwise.
    pub other_links: Option<CollectLinks>,
}

#[derive(Debug)]
struct CollectSession {
    generation: u64,
    page_url: String,
    no_new_scans: u8,
    /// True when the shape was chosen by the user or a saved rule; otherwise every scan
    /// picks the largest group again (the page grows while it scrolls).
    shape_fixed: bool,
    include_ads: bool,
    ads: HashSet<String>,
    state: CollectState,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScanResult {
    groups: Vec<CollectGroup>,
    shape: Option<String>,
    pub(crate) items: Vec<CollectItem>,
    pub(crate) ads: Vec<String>,
    page: Option<CollectPage>,
    other_links: Option<CollectLinks>,
}

/// What a source registered from the open collect session starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectSnapshot {
    /// The page the session collects from.
    pub page_url: String,
    /// The chosen link shape (the user's, a saved rule's or the automatic pick).
    pub shape: Option<String>,
    pub items: Vec<CollectItem>,
    pub max_scrolls: u32,
    /// The page title from the last scan, when it had one.
    pub page_title: Option<String>,
    /// The saved rule the session uses, if any.
    pub rule_id: Option<i64>,
}

pub struct CollectorManager {
    browser: Arc<BrowserManager>,
    store: Arc<Mutex<vids_core::store::Store>>,
    session: Mutex<Option<CollectSession>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadFindDecision {
    Done,
    PlayNow,
    KeepWaiting,
}

fn reload_find_decision(
    elapsed: Duration,
    loaded_for: Option<Duration>,
    loading: bool,
    new_items: usize,
    played_once: bool,
) -> ReloadFindDecision {
    if new_items > 0 || elapsed >= RELOAD_FIND_LIMIT {
        return ReloadFindDecision::Done;
    }
    if loaded_for.is_none() && loading && elapsed < RELOAD_LOAD_LIMIT {
        return ReloadFindDecision::KeepWaiting;
    }
    if !played_once && loaded_for.is_some_and(|loaded_for| loaded_for >= RELOAD_PLAY_AFTER) {
        return ReloadFindDecision::PlayNow;
    }
    ReloadFindDecision::KeepWaiting
}

#[derive(Debug, Serialize)]
pub struct CollectReloadFindResult {
    found: usize,
}

impl CollectorManager {
    pub fn new(browser: Arc<BrowserManager>, store: Arc<Mutex<vids_core::store::Store>>) -> Self {
        Self {
            browser,
            store,
            session: Mutex::new(None),
        }
    }

    pub fn start(
        self: &Arc<Self>,
        app: &AppHandle,
        shape: Option<String>,
        max_scrolls: u32,
        include_ads: bool,
    ) -> Result<(), String> {
        let page_url = self
            .browser
            .current_page_url()
            .ok_or_else(|| "browser page is not open".to_owned())?;
        let mut session = lock_recover(&self.session);
        let generation;
        let new_session;
        if let Some(current) = session
            .as_mut()
            .filter(|current| current.page_url == page_url)
        {
            new_session = false;
            current.generation = current.generation.wrapping_add(1);
            generation = current.generation;
            current.state.running = true;
            current.state.error = None;
            current.state.max_scrolls = max_scrolls;
            if current.include_ads != include_ads {
                current.ads.clear();
                current.state.ads_excluded = 0;
            }
            current.include_ads = include_ads;
            current.no_new_scans = 0;
            if shape.is_some() {
                current.state.shape = shape;
                current.shape_fixed = true;
            }
        } else {
            new_session = true;
            let saved = self.saved_rule(&page_url, max_scrolls)?;
            let (saved_shape, saved_max_scrolls, rule_id) = saved
                .map(|saved| (Some(saved.shape), saved.max_scrolls, Some(saved.id)))
                .unwrap_or((shape, max_scrolls, None));
            generation = 1;
            let shape_fixed = saved_shape.is_some();
            *session = Some(CollectSession {
                generation,
                page_url: page_url.clone(),
                no_new_scans: 0,
                shape_fixed,
                include_ads,
                ads: HashSet::new(),
                state: CollectState {
                    running: true,
                    scrolls: 0,
                    max_scrolls: saved_max_scrolls,
                    groups: Vec::new(),
                    shape: saved_shape,
                    items: Vec::new(),
                    ads_excluded: 0,
                    rule_id,
                    error: None,
                    page: None,
                    other_links: None,
                },
            });
        }
        let state = session
            .as_ref()
            .expect("collector session was set")
            .state
            .clone();
        drop(session);
        emit_state(app, &state);
        self.spawn_loop(app.clone(), generation);
        if new_session {
            self.spawn_played_refresh(app.clone(), page_url);
        }
        Ok(())
    }

    pub fn stop(&self, app: &AppHandle) -> Result<(), String> {
        let mut session = lock_recover(&self.session);
        let current = session
            .as_mut()
            .ok_or_else(|| "no collect session is open".to_owned())?;
        current.generation = current.generation.wrapping_add(1);
        current.state.running = false;
        let state = current.state.clone();
        drop(session);
        emit_state(app, &state);
        Ok(())
    }

    pub async fn reload_find(&self, app: &AppHandle) -> Result<CollectReloadFindResult, String> {
        let page_url = self
            .browser
            .current_page_url()
            .ok_or_else(|| "browser page is not open".to_owned())?;
        let initial_urls = self
            .browser
            .played_media(&page_url)
            .into_iter()
            .map(|media| media.url)
            .collect::<HashSet<_>>();
        let generation = {
            let mut session = lock_recover(&self.session);
            let current = session
                .as_mut()
                .filter(|current| current.page_url == page_url)
                .ok_or_else(|| "no collect session is open on the current page".to_owned())?;
            current.generation = current.generation.wrapping_add(1);
            current.state.running = false;
            let generation = current.generation;
            let state = current.state.clone();
            drop(session);
            emit_state(app, &state);
            generation
        };

        // The reload counts as loaded once a load finishes after it, not when the old page's
        // "not loading" is still showing because the reload's start has not arrived yet.
        let loads_before = self
            .browser
            .page_loads(&page_url)
            .ok_or_else(|| "browser page is not open".to_owned())?;
        self.browser.reload()?;
        let started = Instant::now();
        let mut loaded_at = None;
        let mut played_once = false;

        loop {
            tokio::time::sleep(RELOAD_FIND_POLL).await;
            self.ensure_reload_find(generation, &page_url)?;
            let elapsed = started.elapsed();
            let loading = self
                .browser
                .page_loads(&page_url)
                .ok_or_else(|| "browser page changed while looking for videos".to_owned())?
                == loads_before;
            if loaded_at.is_none() && (!loading || elapsed >= RELOAD_LOAD_LIMIT) {
                loaded_at = Some(Instant::now());
            }
            let played = self.browser.played_media(&page_url);
            let new_items = played
                .iter()
                .filter(|media| !initial_urls.contains(&media.url))
                .count();
            let decision = reload_find_decision(
                elapsed,
                loaded_at.map(|loaded_at| loaded_at.elapsed()),
                loading,
                new_items,
                played_once,
            );
            match decision {
                ReloadFindDecision::Done => break,
                ReloadFindDecision::PlayNow => {
                    self.browser.execute(crate::page_media::PLAY_SCRIPT).await?;
                    self.ensure_reload_find(generation, &page_url)?;
                    played_once = true;
                }
                ReloadFindDecision::KeepWaiting => {}
            }
        }

        if let Some(Some(state)) = self.refresh_played(&page_url) {
            emit_state(app, &state);
        }
        self.ensure_reload_find(generation, &page_url)?;
        let played = self.browser.played_media(&page_url);
        let found = played.len();
        if found > 0 {
            let mut session = lock_recover(&self.session);
            let current = session
                .as_mut()
                .filter(|current| current.generation == generation && current.page_url == page_url)
                .ok_or_else(|| "collect session changed while looking for videos".to_owned())?;
            current.shape_fixed = true;
            current.state.shape = Some(PLAYED_SHAPE.to_owned());
            apply_played(&mut current.state, true, played);
            let state = current.state.clone();
            drop(session);
            emit_state(app, &state);
        }
        Ok(CollectReloadFindResult { found })
    }

    pub fn select(
        self: &Arc<Self>,
        app: &AppHandle,
        shape: String,
        include_ads: bool,
    ) -> Result<(), String> {
        let shape = bounded_nonempty(shape, MAX_SHAPE_CHARS, "shape")?;
        let mut session = lock_recover(&self.session);
        let current = session
            .as_mut()
            .ok_or_else(|| "no collect session is open".to_owned())?;
        current.generation = current.generation.wrapping_add(1);
        current.no_new_scans = 0;
        current.shape_fixed = true;
        current.include_ads = include_ads;
        current.state.shape = Some(shape);
        current.state.items.clear();
        current.ads.clear();
        current.state.ads_excluded = 0;
        current.state.error = None;
        let generation = current.generation;
        let running = current.state.running;
        let state = current.state.clone();
        drop(session);
        emit_state(app, &state);
        if running {
            self.spawn_loop(app.clone(), generation);
        } else {
            self.spawn_single_scan(app.clone(), generation);
        }
        Ok(())
    }

    /// Scrolls the page to an item's card and flashes it; false when the page no longer has it.
    pub async fn reveal(&self, url: String) -> Result<bool, String> {
        let url = bounded_nonempty(url, MAX_URL_CHARS, "url")?;
        if !is_http_url(&url) {
            return Err("url must be an http(s) URL".to_owned());
        }
        if self.browser.current_page_url().is_none() {
            return Err("browser page is not open".to_owned());
        }
        let expression = reveal_expression(&url)?;
        let value = self
            .browser
            .execute(&format!("{COLLECTOR_SCRIPT}\n{expression}"))
            .await?;
        Ok(value.get("found").and_then(Value::as_bool).unwrap_or(false))
    }

    pub async fn close(&self) -> Result<(), String> {
        let had_session = lock_recover(&self.session).take().is_some();
        if had_session && self.browser.current_page_url().is_some() {
            self.browser
                .execute(&format!(
                    "{COLLECTOR_SCRIPT}\nwindow.__vdlCollector.clear()"
                ))
                .await?;
        }
        Ok(())
    }

    pub fn save_rule(
        &self,
        app: &AppHandle,
        name: String,
        match_pattern: String,
    ) -> Result<i64, String> {
        let name = name.trim();
        let match_pattern = match_pattern.trim();
        if name.is_empty() || match_pattern.is_empty() {
            return Err("rule name and match pattern are required".to_owned());
        }
        let (shape, max_scrolls) = {
            let session = lock_recover(&self.session);
            let current = session
                .as_ref()
                .ok_or_else(|| "no collect session is open".to_owned())?;
            let shape = current
                .state
                .shape
                .clone()
                .ok_or_else(|| "no link shape is selected".to_owned())?;
            (shape, current.state.max_scrolls)
        };
        let rule_json = build_rule_json(name, match_pattern, &shape, max_scrolls);
        let id = lock_recover(&self.store)
            .add_rule(name, match_pattern, rule_json, unix_now())
            .map_err(|error| error.to_string())?;
        let mut session = lock_recover(&self.session);
        if let Some(current) = session.as_mut() {
            current.state.rule_id = Some(id);
            let state = current.state.clone();
            drop(session);
            emit_state(app, &state);
        }
        Ok(id)
    }

    /// Tells the session on `page_url` that a rule was saved for it elsewhere (registering
    /// the page as a source), as `save_rule` would.
    pub fn rule_saved(&self, app: &AppHandle, page_url: &str, id: i64) {
        let mut session = lock_recover(&self.session);
        if let Some(current) = session
            .as_mut()
            .filter(|current| current.page_url == page_url && current.state.rule_id.is_none())
        {
            current.state.rule_id = Some(id);
            let state = current.state.clone();
            drop(session);
            emit_state(app, &state);
        }
    }

    /// The open session's page, shape, items and limits, for registering it as a source.
    pub fn snapshot(&self) -> Result<CollectSnapshot, String> {
        let session = lock_recover(&self.session);
        let current = session
            .as_ref()
            .ok_or_else(|| "no collect session is open".to_owned())?;
        Ok(CollectSnapshot {
            page_url: current.page_url.clone(),
            shape: current.state.shape.clone(),
            items: current.state.items.clone(),
            max_scrolls: current.state.max_scrolls,
            page_title: current
                .state
                .page
                .as_ref()
                .map(|page| page.title.trim().to_owned())
                .filter(|title| !title.is_empty()),
            rule_id: current.state.rule_id,
        })
    }

    pub fn page_facts(&self, page_url: &str) -> Option<CollectPage> {
        lock_recover(&self.session)
            .as_ref()
            .filter(|current| current.page_url == page_url)
            .and_then(|current| current.state.page.clone())
    }

    fn saved_rule(&self, url: &str, default_max_scrolls: u32) -> Result<Option<SavedRule>, String> {
        let rules = lock_recover(&self.store)
            .list_rules()
            .map_err(|error| error.to_string())?;
        for rule in rules {
            if !glob_matches(&rule.match_pattern, url) {
                continue;
            }
            let Some(shape) = rule
                .rule_json
                .pointer("/items/href")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                continue;
            };
            let max_scrolls = rule
                .rule_json
                .pointer("/stop/maxPages")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(default_max_scrolls);
            return Ok(Some(SavedRule {
                id: rule.id,
                shape,
                max_scrolls,
            }));
        }
        Ok(None)
    }

    fn spawn_loop(self: &Arc<Self>, app: AppHandle, generation: u64) {
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            manager.run_loop(&app, generation).await;
        });
    }

    fn spawn_single_scan(self: &Arc<Self>, app: AppHandle, generation: u64) {
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            if let Err(error) = manager.scan_and_merge(&app, generation).await {
                manager.fail(&app, generation, error);
            }
        });
    }

    fn spawn_played_refresh(self: &Arc<Self>, app: AppHandle, page_url: String) {
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(PLAYED_REFRESH).await;
                let Some(state) = manager.refresh_played(&page_url) else {
                    return;
                };
                if let Some(state) = state {
                    emit_state(&app, &state);
                }
            }
        });
    }

    /// `None` means this page's tray is gone; `Some(None)` means nothing changed.
    fn refresh_played(&self, page_url: &str) -> Option<Option<CollectState>> {
        if self.browser.current_page_url().as_deref() != Some(page_url) {
            return None;
        }
        let played = self.browser.played_media(page_url);
        let mut session = lock_recover(&self.session);
        let current = session
            .as_mut()
            .filter(|current| current.page_url == page_url)?;
        let before = current.state.clone();
        apply_played(&mut current.state, current.shape_fixed, played);
        (current.state != before)
            .then(|| Some(current.state.clone()))
            .or(Some(None))
    }

    async fn run_loop(&self, app: &AppHandle, generation: u64) {
        loop {
            if !self.is_current_running(generation) {
                return;
            }
            if let Err(error) = self.ensure_page(generation) {
                self.fail(app, generation, error);
                return;
            }
            let new_items = match self.scan_and_merge(app, generation).await {
                Ok(new_items) => new_items,
                Err(error) => {
                    self.fail(app, generation, error);
                    return;
                }
            };
            let should_stop = {
                let mut session = lock_recover(&self.session);
                let Some(current) = session
                    .as_mut()
                    .filter(|current| current.generation == generation && current.state.running)
                else {
                    return;
                };
                if new_items == 0 {
                    current.no_new_scans = current.no_new_scans.saturating_add(1);
                } else {
                    current.no_new_scans = 0;
                }
                let stop =
                    current.no_new_scans >= 2 || current.state.scrolls >= current.state.max_scrolls;
                if stop {
                    current.state.running = false;
                }
                let state = current.state.clone();
                drop(session);
                emit_state(app, &state);
                stop
            };
            if should_stop {
                return;
            }

            if let Err(error) = self
                .browser
                .execute(&format!("{COLLECTOR_SCRIPT}\nwindow.__vdlCollector.step()"))
                .await
            {
                self.fail(app, generation, error);
                return;
            }
            tokio::time::sleep(SCROLL_WAIT).await;
            if self.ensure_page(generation).is_err() {
                self.stop_for_page_change(app, generation);
                return;
            }
            let mut session = lock_recover(&self.session);
            let Some(current) = session
                .as_mut()
                .filter(|current| current.generation == generation && current.state.running)
            else {
                return;
            };
            current.state.scrolls = current.state.scrolls.saturating_add(1);
        }
    }

    async fn scan_and_merge(&self, app: &AppHandle, generation: u64) -> Result<usize, String> {
        let (requested_shape, include_ads) = {
            let session = lock_recover(&self.session);
            let current = session
                .as_ref()
                .filter(|current| current.generation == generation)
                .ok_or_else(|| "collect session changed".to_owned())?;
            (
                current
                    .shape_fixed
                    .then(|| current.state.shape.clone())
                    .flatten()
                    .filter(|shape| shape != PLAYED_SHAPE),
                current.include_ads,
            )
        };
        let expression = scan_expression(&requested_shape, include_ads)?;
        let value = self
            .browser
            .execute(&format!("{COLLECTOR_SCRIPT}\n{expression}"))
            .await?;
        self.ensure_page(generation)?;
        let mut scan = parse_scan_result(value)?;
        let played = self
            .browser
            .played_media(&self.ensure_page_url(generation)?);
        let mut session = lock_recover(&self.session);
        let current = session
            .as_mut()
            .filter(|current| current.generation == generation)
            .ok_or_else(|| "collect session changed".to_owned())?;
        current.state.groups = std::mem::take(&mut scan.groups);
        // An automatic pick that moved to another group starts the list again, so the
        // list never mixes two kinds of links.
        let next_shape = preferred_shape(
            current.state.shape.as_deref(),
            scan.shape.as_deref(),
            &current.state.groups,
            played.len(),
        );
        if !current.shape_fixed && current.state.shape != next_shape {
            current.state.items.clear();
            current.ads.clear();
        }
        if !current.shape_fixed {
            current.state.shape = next_shape;
        }
        if scan.page.is_some() {
            current.state.page = scan.page;
        }
        current.state.other_links = scan.other_links;
        current.ads.extend(scan.ads);
        current
            .state
            .items
            .retain(|item| !current.ads.contains(&item.url));
        current.state.ads_excluded = current.ads.len();
        let added = if current.state.shape.as_deref() == Some(PLAYED_SHAPE) {
            0
        } else {
            merge_items(&mut current.state.items, scan.items, &current.ads)
        };
        apply_played(&mut current.state, current.shape_fixed, played);
        let state = current.state.clone();
        drop(session);
        emit_state(app, &state);
        Ok(added)
    }

    fn is_current_running(&self, generation: u64) -> bool {
        lock_recover(&self.session)
            .as_ref()
            .is_some_and(|current| current.generation == generation && current.state.running)
    }

    fn ensure_page(&self, generation: u64) -> Result<(), String> {
        let expected = self.ensure_page_url(generation)?;
        if self.browser.current_page_url().as_deref() == Some(expected.as_str()) {
            Ok(())
        } else {
            Err("browser page changed while collecting".to_owned())
        }
    }

    fn ensure_reload_find(&self, generation: u64, page_url: &str) -> Result<(), String> {
        if self.browser.current_page_url().as_deref() != Some(page_url) {
            return Err("browser page changed while looking for videos".to_owned());
        }
        let session = lock_recover(&self.session);
        let Some(current) = session.as_ref() else {
            return Err("collect session closed while looking for videos".to_owned());
        };
        if current.page_url != page_url || current.generation != generation {
            return Err("collect session changed while looking for videos".to_owned());
        }
        Ok(())
    }

    fn ensure_page_url(&self, generation: u64) -> Result<String, String> {
        lock_recover(&self.session)
            .as_ref()
            .filter(|current| current.generation == generation)
            .map(|current| current.page_url.clone())
            .ok_or_else(|| "collect session changed".to_owned())
    }

    fn stop_for_page_change(&self, app: &AppHandle, generation: u64) {
        let mut session = lock_recover(&self.session);
        let Some(current) = session
            .as_mut()
            .filter(|current| current.generation == generation)
        else {
            return;
        };
        current.state.running = false;
        let state = current.state.clone();
        drop(session);
        emit_state(app, &state);
    }

    fn fail(&self, app: &AppHandle, generation: u64, error: String) {
        let mut session = lock_recover(&self.session);
        let Some(current) = session
            .as_mut()
            .filter(|current| current.generation == generation)
        else {
            return;
        };
        current.state.running = false;
        current.state.error = Some(error);
        let state = current.state.clone();
        drop(session);
        emit_state(app, &state);
    }
}

fn preferred_shape(
    current: Option<&str>,
    scanned: Option<&str>,
    groups: &[CollectGroup],
    played_count: usize,
) -> Option<String> {
    let best_link_count = groups
        .iter()
        .filter(|group| group.shape != IMAGES_SHAPE && group.shape != PLAYED_SHAPE)
        .map(|group| group.count)
        .max()
        .unwrap_or(0);
    if played_count > 0 && best_link_count < 2 {
        Some(PLAYED_SHAPE.to_owned())
    } else {
        current.or(scanned).map(str::to_owned)
    }
}

fn apply_played(
    state: &mut CollectState,
    shape_fixed: bool,
    played: Vec<crate::browser::PlayedMedia>,
) {
    state.groups.retain(|group| group.shape != PLAYED_SHAPE);
    state.groups.push(CollectGroup {
        shape: PLAYED_SHAPE.to_owned(),
        count: played.len(),
    });
    if !shape_fixed {
        state.shape = preferred_shape(state.shape.as_deref(), None, &state.groups, played.len());
    }
    if state.shape.as_deref() != Some(PLAYED_SHAPE) {
        return;
    }
    let base_title = state
        .page
        .as_ref()
        .map(|page| page.title.trim())
        .filter(|title| !title.is_empty())
        .unwrap_or("Video");
    state.items = played
        .into_iter()
        .enumerate()
        .map(|(index, media)| CollectItem {
            url: media.url,
            title: if index == 0 {
                base_title.to_owned()
            } else {
                format!("{base_title} · {}", index + 1)
            },
            thumbnail: None,
            index: None,
            played: Some(CollectPlayed {
                kind: media.kind,
                duration_seconds: media.duration_seconds,
            }),
        })
        .collect();
    state.other_links = None;
    state.ads_excluded = 0;
}

#[derive(Debug)]
struct SavedRule {
    id: i64,
    shape: String,
    max_scrolls: u32,
}

#[tauri::command]
pub async fn collect_start(
    shape: Option<String>,
    max_scrolls: u32,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let include_ads = lock_recover(&state.settings).collect_include_ads;
    state.collector.start(&app, shape, max_scrolls, include_ads)
}

#[tauri::command]
pub async fn collect_stop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.collector.stop(&app)
}

#[tauri::command]
pub async fn collect_reload_find(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<CollectReloadFindResult, String> {
    state.collector.reload_find(&app).await
}

#[tauri::command]
pub async fn collect_select(
    shape: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let include_ads = lock_recover(&state.settings).collect_include_ads;
    state.collector.select(&app, shape, include_ads)
}

#[tauri::command]
pub async fn collect_close(state: State<'_, AppState>) -> Result<(), String> {
    state.collector.close().await
}

#[tauri::command]
pub async fn collect_reveal(url: String, state: State<'_, AppState>) -> Result<bool, String> {
    state.collector.reveal(url).await
}

#[tauri::command]
pub async fn collect_save_rule(
    name: String,
    match_pattern: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    state.collector.save_rule(&app, name, match_pattern)
}

pub(crate) fn parse_scan_result(value: Value) -> Result<ScanResult, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "collector returned a non-object result".to_owned())?;
    let groups = object
        .get("groups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .take(MAX_SCAN_ITEMS)
                .filter_map(|group| {
                    let shape = bounded_string(group.get("shape")?, MAX_SHAPE_CHARS)?;
                    let count = group
                        .get("count")?
                        .as_u64()
                        .and_then(|count| usize::try_from(count).ok())?
                        .min(MAX_SCAN_ITEMS);
                    Some(CollectGroup { shape, count })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let shape = object
        .get("shape")
        .and_then(|shape| (!shape.is_null()).then_some(shape))
        .and_then(|shape| bounded_string(shape, MAX_SHAPE_CHARS));
    let items = object
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(MAX_SCAN_ITEMS)
                .filter_map(parse_item)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let ads = object
        .get("ads")
        .and_then(Value::as_array)
        .map(|ads| {
            ads.iter()
                .take(MAX_SCAN_ITEMS)
                .filter_map(|url| bounded_string(url, MAX_URL_CHARS))
                .filter(|url| is_http_url(url))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let page = object.get("page").and_then(parse_page);
    let other_links = object.get("otherLinks").and_then(parse_links);
    Ok(ScanResult {
        groups,
        shape,
        items,
        ads,
        page,
        other_links,
    })
}

/// The other link group comes from the page too: its shape is bounded and only http(s)
/// URLs are kept.
fn parse_links(value: &Value) -> Option<CollectLinks> {
    let object = value.as_object()?;
    let shape = bounded_string(object.get("shape")?, MAX_SHAPE_CHARS)?;
    let urls = object
        .get("urls")?
        .as_array()?
        .iter()
        .take(MAX_SCAN_ITEMS)
        .filter_map(|url| bounded_string(url, MAX_URL_CHARS))
        .filter(|url| is_http_url(url))
        .collect();
    Some(CollectLinks { shape, urls })
}

/// Page facts come from the page itself, so every field is checked and bounded; a field of
/// the wrong type counts as absent.
fn parse_page(value: &Value) -> Option<CollectPage> {
    let object = value.as_object()?;
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(|value| truncate_chars(value.trim(), MAX_TITLE_CHARS))
            .filter(|value| !value.is_empty())
    };
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| (3..=MAX_PAGE_ID_CHARS).contains(&id.chars().count()))
        .map(str::to_owned);
    Some(CollectPage {
        title: text("title").unwrap_or_default(),
        uploader: text("uploader"),
        published_at: object.get("publishedAt").and_then(Value::as_i64),
        id,
    })
}

/// Adds the scan's new items and returns how many were added. Pages load images and text
/// lazily, so an item found again fills the thumbnail or title an earlier scan missed.
pub(crate) fn merge_items(
    items: &mut Vec<CollectItem>,
    scanned: Vec<CollectItem>,
    ads: &HashSet<String>,
) -> usize {
    let mut index = items
        .iter()
        .enumerate()
        .map(|(position, item)| (item.url.clone(), position))
        .collect::<HashMap<_, _>>();
    let before = items.len();
    for item in scanned {
        if ads.contains(&item.url) {
            continue;
        }
        if let Some(&position) = index.get(&item.url) {
            let existing = &mut items[position];
            if existing.thumbnail.is_none() {
                existing.thumbnail = item.thumbnail;
            }
            if existing.title.is_empty() {
                existing.title = item.title;
            }
            if existing.index.is_none() {
                existing.index = item.index;
            }
        } else if items.len() < MAX_SCAN_ITEMS {
            index.insert(item.url.clone(), items.len());
            items.push(item);
        }
    }
    items.len() - before
}

pub(crate) fn scan_expression(
    requested_shape: &Option<String>,
    include_ads: bool,
) -> Result<String, String> {
    let shape = serde_json::to_string(requested_shape).map_err(|error| error.to_string())?;
    let options = serde_json::to_string(&json!({ "includeAds": include_ads }))
        .map_err(|error| error.to_string())?;
    Ok(format!("window.__vdlCollector.scan({shape}, {options})"))
}

fn reveal_expression(url: &str) -> Result<String, String> {
    let url = serde_json::to_string(url).map_err(|error| error.to_string())?;
    Ok(format!("window.__vdlCollector.reveal({url})"))
}

fn parse_item(value: &Value) -> Option<CollectItem> {
    let url = bounded_string(value.get("url")?, MAX_URL_CHARS)?;
    if !is_http_url(&url) {
        return None;
    }
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(|title| truncate_chars(title, MAX_TITLE_CHARS))
        .unwrap_or_default();
    let thumbnail = value
        .get("thumbnail")
        .and_then(Value::as_str)
        .map(|thumbnail| truncate_chars(thumbnail, MAX_URL_CHARS))
        .filter(|thumbnail| is_http_url(thumbnail));
    let index = value
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|index| u32::try_from(index).ok())
        .filter(|index| *index >= 1);
    Some(CollectItem {
        url,
        title,
        thumbnail,
        index,
        played: None,
    })
}

fn bounded_string(value: &Value, max_chars: usize) -> Option<String> {
    let value = value.as_str()?;
    (value.chars().count() <= max_chars).then(|| value.to_owned())
}

fn bounded_nonempty(value: String, max_chars: usize, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max_chars {
        return Err(format!("{label} is empty or too long"));
    }
    Ok(value.to_owned())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn is_http_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut p, mut v) = (0, 0);
    let (mut star, mut retry) = (None, 0);
    while v < value.len() {
        if p < pattern.len() && pattern[p] == value[v] {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            retry = v;
        } else if let Some(star_index) = star {
            retry += 1;
            v = retry;
            p = star_index + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

pub(crate) fn build_rule_json(
    name: &str,
    match_pattern: &str,
    shape: &str,
    max_scrolls: u32,
) -> Value {
    json!({
        "name": name,
        "match": match_pattern,
        "fetch": "rendered",
        "items": { "href": shape },
        "next": "scroll",
        "stop": { "maxPages": max_scrolls, "atKnown": true },
        "waitSec": 1.5
    })
}

fn emit_state(app: &AppHandle, state: &CollectState) {
    let _ = app.emit("collect-state", state.clone());
}

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        COLLECTOR_SCRIPT, CollectGroup, CollectItem, CollectLinks, CollectPage, CollectState,
        MAX_SCAN_ITEMS, MAX_URL_CHARS, PLAYED_SHAPE, ReloadFindDecision, build_rule_json,
        glob_matches, merge_items, parse_scan_result, preferred_shape, reload_find_decision,
        reveal_expression, scan_expression,
    };
    use serde_json::json;
    use std::collections::HashSet;
    use std::time::Duration;

    fn item(url: &str, title: &str, thumbnail: Option<&str>) -> CollectItem {
        CollectItem {
            url: url.to_owned(),
            title: title.to_owned(),
            thumbnail: thumbnail.map(str::to_owned),
            index: None,
            played: None,
        }
    }

    fn image(url: &str, index: Option<u32>) -> CollectItem {
        CollectItem {
            url: url.to_owned(),
            title: String::new(),
            thumbnail: Some(url.to_owned()),
            index,
            played: None,
        }
    }

    #[test]
    fn reload_find_plays_after_three_loaded_seconds_without_a_new_item() {
        assert_eq!(
            reload_find_decision(
                Duration::from_secs(5),
                Some(Duration::from_secs(3)),
                false,
                0,
                false,
            ),
            ReloadFindDecision::PlayNow
        );
    }

    #[test]
    fn reload_find_finishes_when_a_new_item_arrives() {
        assert_eq!(
            reload_find_decision(
                Duration::from_secs(2),
                Some(Duration::from_secs(1)),
                false,
                1,
                false,
            ),
            ReloadFindDecision::Done
        );
    }

    #[test]
    fn reload_find_finishes_at_twenty_seconds_without_an_item() {
        assert_eq!(
            reload_find_decision(
                Duration::from_secs(20),
                Some(Duration::from_secs(10)),
                false,
                0,
                true,
            ),
            ReloadFindDecision::Done
        );
    }

    #[test]
    fn reload_find_plays_only_once() {
        assert_eq!(
            reload_find_decision(
                Duration::from_secs(8),
                Some(Duration::from_secs(6)),
                false,
                0,
                true,
            ),
            ReloadFindDecision::KeepWaiting
        );
    }

    #[test]
    fn a_later_scan_fills_missing_thumbnails_and_titles_and_skips_ads() {
        let mut items = vec![item("https://example.com/watch?v=1", "", None)];
        let ads = HashSet::from(["https://example.com/watch?v=ad".to_owned()]);
        let added = merge_items(
            &mut items,
            vec![
                item(
                    "https://example.com/watch?v=1",
                    "One",
                    Some("https://i.example.com/1.jpg"),
                ),
                item("https://example.com/watch?v=ad", "Ad", None),
                item("https://example.com/watch?v=2", "Two", None),
            ],
            &ads,
        );
        assert_eq!(added, 1);
        assert_eq!(
            items,
            vec![
                item(
                    "https://example.com/watch?v=1",
                    "One",
                    Some("https://i.example.com/1.jpg")
                ),
                item("https://example.com/watch?v=2", "Two", None),
            ]
        );
        // What a scan already found is kept.
        merge_items(
            &mut items,
            vec![item(
                "https://example.com/watch?v=1",
                "Other",
                Some("https://i.example.com/x.jpg"),
            )],
            &ads,
        );
        assert_eq!(
            items[0],
            item(
                "https://example.com/watch?v=1",
                "One",
                Some("https://i.example.com/1.jpg")
            )
        );
    }

    #[test]
    fn rule_globs_match_any_number_of_characters() {
        assert!(glob_matches(
            "https://example.com/videos*",
            "https://example.com/videos?page=2"
        ));
        assert!(glob_matches(
            "*://*.example.com/*",
            "https://www.example.com/a"
        ));
        assert!(!glob_matches(
            "https://example.com/videos*",
            "https://example.com/talks"
        ));
    }

    #[test]
    fn saved_rule_uses_the_spec_draft_shape() {
        assert_eq!(
            build_rule_json(
                "Example videos",
                "https://example.com/videos*",
                "/watch?v={id}",
                10
            ),
            json!({
                "name": "Example videos",
                "match": "https://example.com/videos*",
                "fetch": "rendered",
                "items": { "href": "/watch?v={id}" },
                "next": "scroll",
                "stop": { "maxPages": 10, "atKnown": true },
                "waitSec": 1.5
            })
        );
    }

    #[test]
    fn scan_results_are_validated_and_capped() {
        let mut items = (0..=MAX_SCAN_ITEMS)
            .map(|index| {
                json!({
                    "url": format!("https://example.com/watch/{index}"),
                    "title": "x".repeat(301),
                    "thumbnail": "data:image/png;base64,not-accepted"
                })
            })
            .collect::<Vec<_>>();
        items.insert(0, json!({ "url": "javascript:alert(1)", "title": "bad" }));
        let parsed = parse_scan_result(json!({
            "groups": [{ "shape": "/watch/{num}", "count": 9000 }],
            "shape": "/watch/{num}",
            "items": items
        }))
        .expect("valid outer result");
        assert!(parsed.items.len() <= MAX_SCAN_ITEMS);
        assert!(
            parsed
                .items
                .iter()
                .all(|item| item.title.chars().count() == 300)
        );
        assert!(parsed.items.iter().all(|item| item.thumbnail.is_none()));
        assert_eq!(parsed.groups[0].count, MAX_SCAN_ITEMS);
        assert!(parsed.ads.is_empty());
    }

    #[test]
    fn scan_results_keep_only_valid_bounded_ad_urls() {
        let valid = "https://example.com/ads/1";
        let parsed = parse_scan_result(json!({
            "groups": [],
            "shape": null,
            "items": [],
            "ads": [valid, "javascript:alert(1)", format!("https://example.com/{}", "x".repeat(MAX_URL_CHARS))]
        }))
        .expect("valid outer result");

        assert_eq!(parsed.ads, vec![valid]);
    }

    #[test]
    fn scan_expression_serializes_shape_and_options_as_json() {
        assert_eq!(
            scan_expression(&None, false).unwrap(),
            r#"window.__vdlCollector.scan(null, {"includeAds":false})"#
        );
        assert_eq!(
            scan_expression(&Some("a\"b\\c".to_owned()), true).unwrap(),
            r#"window.__vdlCollector.scan("a\"b\\c", {"includeAds":true})"#
        );
    }

    #[test]
    fn reveal_expression_quotes_the_url_as_json() {
        assert_eq!(
            reveal_expression("https://example.com/a?b=\"c\"").unwrap(),
            r#"window.__vdlCollector.reveal("https://example.com/a?b=\"c\"")"#
        );
    }

    #[test]
    fn page_image_items_keep_their_index_and_links_have_none() {
        let parsed = parse_scan_result(json!({
            "groups": [{ "shape": "{images}", "count": 3 }],
            "shape": "{images}",
            "items": [
                { "url": "https://example.com/a.jpg", "title": "", "thumbnail": "https://example.com/a.jpg", "index": 1 },
                { "url": "https://example.com/b.jpg", "title": "", "index": 0 },
                { "url": "https://example.com/c.jpg", "title": "", "index": "3" },
                { "url": "https://example.com/d.jpg", "title": "", "index": 4_294_967_296_u64 },
                { "url": "https://example.com/watch/1", "title": "A link" }
            ]
        }))
        .expect("valid outer result");
        let indexes: Vec<_> = parsed.items.iter().map(|item| item.index).collect();
        assert_eq!(indexes, vec![Some(1), None, None, None, None]);

        // A later scan fills an index an earlier one lacked and never renumbers.
        let mut items = vec![
            image("https://example.com/a.jpg", Some(1)),
            image("https://example.com/b.jpg", None),
        ];
        let added = merge_items(
            &mut items,
            vec![
                image("https://example.com/a.jpg", Some(7)),
                image("https://example.com/b.jpg", Some(2)),
                image("https://example.com/c.jpg", Some(3)),
            ],
            &HashSet::new(),
        );
        assert_eq!(added, 1);
        let indexes: Vec<_> = items.iter().map(|item| item.index).collect();
        assert_eq!(indexes, vec![Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn page_facts_are_checked_and_bounded() {
        let parsed = parse_scan_result(json!({
            "groups": [],
            "shape": null,
            "items": [],
            "page": {
                "title": format!("  {}  ", "t".repeat(400)),
                "uploader": "  Kanade  ",
                "publishedAt": 1_793_318_400,
                "id": "nfe806dde550a"
            }
        }))
        .expect("valid outer result");
        let page = parsed.page.expect("page facts");
        assert_eq!(page.title.chars().count(), 300);
        assert_eq!(page.uploader.as_deref(), Some("Kanade"));
        assert_eq!(page.published_at, Some(1_793_318_400));
        assert_eq!(page.id.as_deref(), Some("nfe806dde550a"));

        let parsed = parse_scan_result(json!({
            "page": { "title": 5, "uploader": "", "publishedAt": "2026-10-30", "id": "ab" }
        }))
        .expect("valid outer result");
        assert_eq!(
            parsed.page,
            Some(CollectPage {
                title: String::new(),
                uploader: None,
                published_at: None,
                id: None,
            })
        );
        let parsed = parse_scan_result(json!({ "page": { "id": "x".repeat(201) } })).unwrap();
        assert_eq!(parsed.page.unwrap().id, None);
        assert_eq!(parse_scan_result(json!({ "page": [] })).unwrap().page, None);
        assert_eq!(parse_scan_result(json!({})).unwrap().page, None);
    }

    #[test]
    fn other_links_keep_http_urls_only() {
        let parsed = parse_scan_result(json!({
            "otherLinks": {
                "shape": "/hashtag/{any}",
                "urls": ["https://note.com/hashtag/a", "javascript:alert(1)", 7, "http://x.test/b"]
            }
        }))
        .unwrap();
        assert_eq!(
            parsed.other_links,
            Some(CollectLinks {
                shape: "/hashtag/{any}".to_owned(),
                urls: vec![
                    "https://note.com/hashtag/a".to_owned(),
                    "http://x.test/b".to_owned()
                ],
            })
        );
        assert_eq!(
            parse_scan_result(json!({ "otherLinks": null }))
                .unwrap()
                .other_links,
            None
        );
        assert_eq!(
            parse_scan_result(json!({ "otherLinks": { "urls": [] } }))
                .unwrap()
                .other_links,
            None
        );
    }

    #[test]
    fn collect_state_serializes_page_facts_and_indexes_in_camel_case() {
        let state = CollectState {
            running: false,
            scrolls: 0,
            max_scrolls: 10,
            groups: Vec::new(),
            shape: Some("{images}".to_owned()),
            items: vec![
                image("https://example.com/a.jpg", Some(1)),
                item("https://example.com/p", "P", None),
            ],
            ads_excluded: 0,
            rule_id: None,
            error: None,
            page: Some(CollectPage {
                title: "T".to_owned(),
                uploader: None,
                published_at: Some(1),
                id: Some("abc".to_owned()),
            }),
            other_links: None,
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["items"][0]["index"], json!(1));
        assert_eq!(value["items"][1]["index"], json!(null));
        assert_eq!(
            value["page"],
            json!({ "title": "T", "uploader": null, "publishedAt": 1, "id": "abc" })
        );
    }

    #[test]
    fn collector_script_keeps_its_entry_points() {
        assert!(COLLECTOR_SCRIPT.contains("function scan("));
        assert!(COLLECTOR_SCRIPT.contains("function step("));
        assert!(COLLECTOR_SCRIPT.contains("function reveal("));
        assert!(COLLECTOR_SCRIPT.contains("function clear("));
    }

    #[test]
    fn played_group_is_preselected_only_when_the_best_link_group_has_fewer_than_two() {
        let groups = vec![CollectGroup {
            shape: "/watch/{id}".to_owned(),
            count: 1,
        }];
        assert_eq!(
            preferred_shape(None, Some("/watch/{id}"), &groups, 2).as_deref(),
            Some(PLAYED_SHAPE)
        );
        let groups = vec![CollectGroup {
            shape: "/watch/{id}".to_owned(),
            count: 2,
        }];
        assert_eq!(
            preferred_shape(None, Some("/watch/{id}"), &groups, 2).as_deref(),
            Some("/watch/{id}")
        );
        assert_eq!(
            preferred_shape(Some("/shorts/{id}"), Some("/watch/{id}"), &groups, 0).as_deref(),
            Some("/shorts/{id}")
        );
    }
}

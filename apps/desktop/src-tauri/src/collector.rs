use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, State, Url};

use crate::browser::BrowserManager;
use crate::state::{AppState, lock_recover};

pub(crate) const COLLECTOR_SCRIPT: &str = include_str!("collector.js");
/// Wait after each scroll before the next scan (the fetching etiquette's 1.5 s): a source
/// check's fixed wait, and the longest the collect loop waits for a page that is not at its
/// end to grow.
pub(crate) const SCROLL_WAIT: Duration = Duration::from_millis(1500);
/// How often the collect loop looks at the page while it waits after a scroll; a stop takes
/// effect within one tick.
const SCROLL_TICK: Duration = Duration::from_millis(250);
/// The soonest the collect loop goes on after a scroll, even when the page has grown already.
const SCROLL_MIN_WAIT: Duration = Duration::from_millis(400);
/// How long the collect loop waits at the end of the page for more to load before it looks
/// one last time and stops.
const SCROLL_END_WAIT: Duration = Duration::from_millis(5000);
const PLAYED_REFRESH: Duration = Duration::from_secs(2);
const RELOAD_LOAD_LIMIT: Duration = Duration::from_secs(10);
const RELOAD_PLAY_AFTER: Duration = Duration::from_secs(3);
const RELOAD_FIND_LIMIT: Duration = Duration::from_secs(20);
const RELOAD_FIND_POLL: Duration = Duration::from_secs(1);
/// How often the open session asks the page what it wants to tell the app: badges clicked in
/// the page and, in pick mode, the element picked.
const PAGE_POLL: Duration = Duration::from_millis(500);
/// How long a pressed video is given to show a new played stream, and how often that is looked at.
const PICK_VIDEO_WAIT: Duration = Duration::from_secs(10);
const PICK_VIDEO_POLL: Duration = Duration::from_millis(250);
/// The most videos a page is believed to hold when the picker names one by its position.
const MAX_VIDEO_INDEX: u64 = 10_000;
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

/// A badge the person clicked in the page: the item and whether it is selected now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectToggle {
    pub url: String,
    pub selected: bool,
}

/// The "collect-toggles" event: the clicks made in the page since the last one, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectTogglesEvent {
    toggles: Vec<CollectToggle>,
}

/// The "collect-pick" event: how the element picker (「ページの中から選ぶ」) is going.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CollectPickEvent {
    /// Pick mode is on in the page.
    Active,
    /// Pick mode is over and nothing was picked (Esc, the stop button, a page change).
    Ended,
    /// A video was pressed; its stream is being looked for.
    Finding,
    /// A link was pressed: its address, the shape it belongs to and how many links have it.
    Link {
        url: String,
        shape: String,
        count: usize,
    },
    /// A page image was pressed.
    Image { url: String },
    /// A video was pressed and these streams are new in the played group, which is chosen.
    Video { urls: Vec<String> },
    /// A video was pressed and no new stream arrived in time.
    VideoNotFound,
}

/// What the page's element picker reports through `takePick()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PagePick {
    /// Esc, or an answer that could not be read: pick mode is over and nothing was picked.
    Cancel,
    /// A press on nothing: the page keeps pick mode on.
    Nothing,
    Link {
        url: String,
        shape: String,
        count: usize,
    },
    Video {
        /// The video's own address when it is an http(s) one (not a blob: stream).
        url: Option<String>,
        /// Its position among the page's `<video>` elements.
        index: Option<usize>,
    },
    Image {
        url: String,
    },
}

/// What one look at the page returned (`poll_expression`).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PagePoll {
    pub(crate) toggles: Vec<CollectToggle>,
    pub(crate) pick: Option<PagePick>,
    /// The page has no collector any more: it was loaded again.
    pub(crate) gone: bool,
}

/// Why a collect run is not running (`CollectState::stop_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectStopReason {
    /// The page's end was reached and nothing more loaded for a while.
    End,
    /// The user stopped it, the page changed, or it failed (see `error`).
    Stopped,
    /// The session holds the most items it keeps.
    Cap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectState {
    pub running: bool,
    /// How many scroll steps this session has made. Informational: nothing stops on it.
    pub scrolls: u32,
    /// Kept for the saved rule's `stop.maxPages`; the collect loop no longer stops on it.
    pub max_scrolls: u32,
    /// Why the run ended; `None` while it runs and before the first run.
    pub stop_reason: Option<CollectStopReason>,
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
    /// Tells this session from an earlier one on the same page, for the tasks that outlive a call.
    id: u64,
    generation: u64,
    page_url: String,
    /// Whether the task asking the page for toggles and picks is running.
    poll_active: bool,
    /// Pick mode is on in the page.
    picking: bool,
    /// Changes whenever picking starts, stops or ends, so a video lookup started by an earlier
    /// pick gives up quietly.
    pick_epoch: u64,
    /// True when the shape was chosen by the user or a saved rule; otherwise every scan
    /// picks the largest group again (the page grows while it scrolls).
    shape_fixed: bool,
    include_ads: bool,
    ads: HashSet<String>,
    state: CollectState,
}

/// One round of the element picker in one session; see `CollectSession::pick_epoch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PickTicket {
    session_id: u64,
    epoch: u64,
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
    next_session_id: AtomicU64,
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

/// What the collect loop does at one tick while it waits after a scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollWait {
    KeepWaiting,
    /// Scroll again.
    GoOn,
    /// The end of the page and nothing loaded for the whole wait: scan once more.
    LookAgain,
}

/// The wait after a scroll, decided at each tick from the time `waited` since the scroll,
/// whether the scroll ended at the end of the page (`at_end`, also true when it did not
/// move) and whether the page is taller than the scroll left it (`grew`).
///
/// A page that grew goes on at the first tick from 0.4 s. Not at the end, a page that
/// does not grow goes on after 1.5 s anyway (a feed that fills in later is scrolled on
/// to). At the end a page that does not grow is given 5 s to load more, then looked at
/// once more.
fn scroll_wait_decision(waited: Duration, at_end: bool, grew: bool) -> ScrollWait {
    if grew && waited >= SCROLL_MIN_WAIT {
        ScrollWait::GoOn
    } else if at_end {
        if waited >= SCROLL_END_WAIT {
            ScrollWait::LookAgain
        } else {
            ScrollWait::KeepWaiting
        }
    } else if waited >= SCROLL_WAIT {
        ScrollWait::GoOn
    } else {
        ScrollWait::KeepWaiting
    }
}

/// Where the content scrolls, as `step()` and `scrollInfo()` of the collector report it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ScrollReport {
    /// The scroller's full content height.
    height: f64,
    at_end: bool,
    /// Whether the scroll position changed; always false for `scrollInfo()`, which does not scroll.
    moved: bool,
}

impl ScrollReport {
    /// After a scroll step: at the end of the content, or not able to move at all (nothing
    /// scrolls, or the page ignores scrolling), which the loop treats the same way.
    fn ends_the_page(&self) -> bool {
        self.at_end || !self.moved
    }
}

/// The report comes from the page, so each field is checked.
fn parse_scroll_report(value: &Value) -> Result<ScrollReport, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "collector returned a non-object scroll result".to_owned())?;
    let height = object
        .get("height")
        .and_then(Value::as_f64)
        .filter(|height| height.is_finite() && *height >= 0.0)
        .ok_or_else(|| "collector scroll result has no valid height".to_owned())?;
    let at_end = object
        .get("atEnd")
        .and_then(Value::as_bool)
        .ok_or_else(|| "collector scroll result has no atEnd".to_owned())?;
    let moved = object
        .get("moved")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(ScrollReport {
        height,
        at_end,
        moved,
    })
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
            next_session_id: AtomicU64::new(1),
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
        let saved_for_page = self.saved_rule(&page_url, max_scrolls)?;
        let mut session = lock_recover(&self.session);
        let generation;
        let new_session;
        let session_id;
        let spawn_poll;
        if let Some(current) = session
            .as_mut()
            .filter(|current| current.page_url == page_url)
        {
            new_session = false;
            session_id = current.id;
            // The poll ended when the page changed; coming back to the page starts it again.
            spawn_poll = !current.poll_active;
            current.poll_active = true;
            current.generation = current.generation.wrapping_add(1);
            generation = current.generation;
            current.state.running = true;
            current.state.stop_reason = None;
            current.state.error = None;
            current.state.max_scrolls = max_scrolls;
            if current.include_ads != include_ads {
                current.ads.clear();
                current.state.ads_excluded = 0;
            }
            current.include_ads = include_ads;
            if shape.is_some() {
                current.state.shape = shape.clone();
                current.shape_fixed = true;
                current.state.rule_id = saved_for_page
                    .as_ref()
                    .filter(|saved| Some(&saved.shape) == shape.as_ref())
                    .map(|saved| saved.id);
            }
        } else {
            new_session = true;
            spawn_poll = true;
            session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
            let (saved_shape, saved_max_scrolls, rule_id) = saved_for_page
                .map(|saved| (Some(saved.shape), saved.max_scrolls, Some(saved.id)))
                .unwrap_or((shape, max_scrolls, None));
            generation = 1;
            let shape_fixed = saved_shape.is_some();
            *session = Some(CollectSession {
                id: session_id,
                generation,
                page_url: page_url.clone(),
                poll_active: true,
                picking: false,
                pick_epoch: 0,
                shape_fixed,
                include_ads,
                ads: HashSet::new(),
                state: CollectState {
                    running: true,
                    scrolls: 0,
                    max_scrolls: saved_max_scrolls,
                    stop_reason: None,
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
        if spawn_poll {
            self.spawn_page_poll(app.clone(), session_id);
        }
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
        if current.state.running {
            current.state.running = false;
            current.state.stop_reason = Some(CollectStopReason::Stopped);
        }
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
            if current.state.running {
                current.state.running = false;
                current.state.stop_reason = Some(CollectStopReason::Stopped);
            }
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
        current.shape_fixed = true;
        current.include_ads = include_ads;
        current.state.shape = Some(shape);
        current.state.items.clear();
        current.ads.clear();
        current.state.ads_excluded = 0;
        current.state.error = None;
        // The items start again, so "collected the most there can be" no longer holds; the
        // tray offers 続ける instead of nothing.
        if current.state.stop_reason == Some(CollectStopReason::Cap) {
            current.state.stop_reason = Some(CollectStopReason::Stopped);
        }
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

    /// Tells the page which found items the tray has not selected, so its badges show the same
    /// checks, and which ones the tray's filter hides (no outline, no badge). Does nothing when
    /// no session is open on the page that is showing.
    pub async fn set_selection(
        &self,
        unselected: Vec<String>,
        hidden: Vec<String>,
    ) -> Result<(), String> {
        if !self.session_on_current_page() {
            return Ok(());
        }
        let expression = set_selection_expression(&unselected, &hidden)?;
        self.browser
            .execute(&format!("{COLLECTOR_SCRIPT}\n{expression}"))
            .await?;
        Ok(())
    }

    /// Turns pick mode on in the page. Scrolling stops first: a page moving under the pointer
    /// cannot be pointed at.
    pub async fn pick_start(&self, app: &AppHandle, language: &str) -> Result<(), String> {
        let page_url = self
            .browser
            .current_page_url()
            .ok_or_else(|| "browser page is not open".to_owned())?;
        let expression = pick_start_expression(language)?;
        let (ticket, scroll_stopped) = {
            let mut session = lock_recover(&self.session);
            let current = session
                .as_mut()
                .filter(|current| current.page_url == page_url)
                .ok_or_else(|| "no collect session is open on the current page".to_owned())?;
            let was_running = current.state.running;
            if was_running {
                current.generation = current.generation.wrapping_add(1);
                current.state.running = false;
                current.state.stop_reason = Some(CollectStopReason::Stopped);
            }
            current.picking = true;
            current.pick_epoch = current.pick_epoch.wrapping_add(1);
            (
                PickTicket {
                    session_id: current.id,
                    epoch: current.pick_epoch,
                },
                was_running.then(|| current.state.clone()),
            )
        };
        if let Some(state) = scroll_stopped {
            emit_state(app, &state);
        }
        if let Err(error) = self
            .browser
            .execute(&format!("{COLLECTOR_SCRIPT}\n{expression}"))
            .await
        {
            self.end_pick(ticket);
            return Err(error);
        }
        if self.pick_ticket_valid(ticket) {
            emit_pick(app, &CollectPickEvent::Active);
        }
        Ok(())
    }

    /// Turns pick mode off, and drops a video lookup that is still waiting.
    pub async fn pick_stop(&self, app: &AppHandle) -> Result<(), String> {
        {
            let mut session = lock_recover(&self.session);
            let current = session
                .as_mut()
                .ok_or_else(|| "no collect session is open".to_owned())?;
            current.picking = false;
            current.pick_epoch = current.pick_epoch.wrapping_add(1);
        }
        emit_pick(app, &CollectPickEvent::Ended);
        // The page may be gone or loaded again already; then there is nothing to turn off.
        let _ = self.browser.execute(STOP_PICK_SCRIPT).await;
        Ok(())
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

    fn session_on_current_page(&self) -> bool {
        let Some(page_url) = self.browser.current_page_url() else {
            return false;
        };
        lock_recover(&self.session)
            .as_ref()
            .is_some_and(|current| current.page_url == page_url)
    }

    /// Whether `session_id` is the open session and the browser still shows its page.
    fn session_on_page(&self, session_id: u64, page_url: &str) -> bool {
        self.browser.current_page_url().as_deref() == Some(page_url)
            && lock_recover(&self.session)
                .as_ref()
                .is_some_and(|current| current.id == session_id && current.page_url == page_url)
    }

    fn pick_ticket_valid(&self, ticket: PickTicket) -> bool {
        lock_recover(&self.session).as_ref().is_some_and(|current| {
            current.id == ticket.session_id && current.pick_epoch == ticket.epoch
        })
    }

    /// Leaves pick mode in the session if `ticket` is still the latest pick. Returns the ticket
    /// that follows it (for what is done with the answer), or `None` when it was out of date.
    fn end_pick(&self, ticket: PickTicket) -> Option<PickTicket> {
        let mut session = lock_recover(&self.session);
        let current = session.as_mut().filter(|current| {
            current.id == ticket.session_id && current.pick_epoch == ticket.epoch
        })?;
        current.picking = false;
        current.pick_epoch = current.pick_epoch.wrapping_add(1);
        Some(PickTicket {
            session_id: current.id,
            epoch: current.pick_epoch,
        })
    }

    fn poll_ended(&self, session_id: u64) {
        if let Some(current) = lock_recover(&self.session)
            .as_mut()
            .filter(|current| current.id == session_id)
        {
            current.poll_active = false;
        }
    }

    /// Asks the open session's page every half second for the badges clicked in it and, in
    /// pick mode, for the element picked. The page never calls the app; this only pulls.
    fn spawn_page_poll(self: &Arc<Self>, app: AppHandle, session_id: u64) {
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(PAGE_POLL).await;
                if !manager.poll_page(&app, session_id).await {
                    manager.poll_ended(session_id);
                    return;
                }
            }
        });
    }

    /// One look at the page. False once the session is closed or the page changed: the poll ends.
    async fn poll_page(self: &Arc<Self>, app: &AppHandle, session_id: u64) -> bool {
        let (page_url, picking, epoch) = {
            let session = lock_recover(&self.session);
            let Some(current) = session.as_ref().filter(|current| current.id == session_id) else {
                return false;
            };
            (
                current.page_url.clone(),
                current.picking,
                current.pick_epoch,
            )
        };
        let ticket = PickTicket { session_id, epoch };
        if self.browser.current_page_url().as_deref() != Some(page_url.as_str()) {
            // Pick mode lives in the page's script: turn it off if the page is still the same
            // document (a single-page app that changed its address).
            if picking && self.end_pick(ticket).is_some() {
                emit_pick(app, &CollectPickEvent::Ended);
                let _ = self.browser.execute(STOP_PICK_SCRIPT).await;
            }
            return false;
        }
        let Ok(value) = self.browser.execute(poll_expression(picking)).await else {
            // A page that is loading cannot answer; the next look tries again.
            return true;
        };
        let poll = parse_poll(&value);
        // The page may have been left, or the session replaced, while it was asked.
        if !self.session_on_page(session_id, &page_url) {
            return true;
        }
        if !poll.toggles.is_empty() {
            emit_toggles(app, poll.toggles);
        }
        if picking {
            if poll.gone {
                if self.end_pick(ticket).is_some() {
                    emit_pick(app, &CollectPickEvent::Ended);
                }
            } else if let Some(pick) = poll.pick {
                self.handle_pick(app, ticket, &page_url, pick);
            }
        }
        true
    }

    /// What the tray is told about an element the person pressed in the page.
    fn handle_pick(
        self: &Arc<Self>,
        app: &AppHandle,
        ticket: PickTicket,
        page_url: &str,
        pick: PagePick,
    ) {
        let event = match pick {
            // The page stays in pick mode after a press on nothing.
            PagePick::Nothing => return,
            PagePick::Cancel => CollectPickEvent::Ended,
            PagePick::Link { url, shape, count } => CollectPickEvent::Link { url, shape, count },
            PagePick::Image { url } => CollectPickEvent::Image { url },
            PagePick::Video { url, index } => {
                let Some(next) = self.end_pick(ticket) else {
                    return;
                };
                emit_pick(app, &CollectPickEvent::Finding);
                let manager = Arc::clone(self);
                let app = app.clone();
                let page_url = page_url.to_owned();
                tauri::async_runtime::spawn(async move {
                    manager
                        .find_picked_video(&app, next, &page_url, url, index)
                        .await;
                });
                return;
            }
        };
        if self.end_pick(ticket).is_some() {
            emit_pick(app, &event);
        }
    }

    /// Looks for the stream of a video the person pressed. A video with an http(s) address of
    /// its own is added to the played streams; any other is played muted in the page. Either
    /// way, up to ten seconds are given for a new stream to show, and then the played group is
    /// chosen with the new streams (the tray checks them).
    async fn find_picked_video(
        &self,
        app: &AppHandle,
        ticket: PickTicket,
        page_url: &str,
        url: Option<String>,
        index: Option<usize>,
    ) {
        let before = self
            .browser
            .played_media(page_url)
            .into_iter()
            .map(|media| media.url)
            .collect::<HashSet<_>>();
        let added = url
            .as_deref()
            .is_some_and(|url| self.browser.observe_picked_media(page_url, url));
        if !added && let Some(index) = index {
            let _ = self.browser.execute(&play_video_expression(index)).await;
        }
        let started = Instant::now();
        loop {
            tokio::time::sleep(PICK_VIDEO_POLL).await;
            if !self.pick_ticket_valid(ticket) {
                // Another pick, a stop or a closed tray took over; it tells the tray itself.
                return;
            }
            if self.browser.current_page_url().as_deref() != Some(page_url) {
                emit_pick(app, &CollectPickEvent::Ended);
                return;
            }
            let played = self.browser.played_media(page_url);
            let fresh = fresh_streams(&played, &before, url.as_deref());
            if !fresh.is_empty() {
                let state = {
                    let mut session = lock_recover(&self.session);
                    let Some(current) = session.as_mut().filter(|current| {
                        current.id == ticket.session_id && current.pick_epoch == ticket.epoch
                    }) else {
                        return;
                    };
                    current.shape_fixed = true;
                    current.state.shape = Some(PLAYED_SHAPE.to_owned());
                    apply_played(&mut current.state, true, played);
                    current.state.clone()
                };
                emit_state(app, &state);
                emit_pick(app, &CollectPickEvent::Video { urls: fresh });
                return;
            }
            if started.elapsed() >= PICK_VIDEO_WAIT {
                emit_pick(app, &CollectPickEvent::VideoNotFound);
                return;
            }
        }
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

    /// Scrolls the page by steps until its end, the item cap or a stop: scan, scroll, then
    /// wait (in ticks) for the page to load what the scroll revealed. See
    /// [`scroll_wait_decision`] for how long that takes.
    async fn run_loop(&self, app: &AppHandle, generation: u64) {
        let step_script = format!("{COLLECTOR_SCRIPT}\nwindow.__vdlCollector.step()");
        let info_script = format!("{COLLECTOR_SCRIPT}\nwindow.__vdlCollector.scrollInfo()");
        loop {
            if !self.is_current_running(generation) {
                return;
            }
            if let Err(error) = self.ensure_page(generation) {
                self.fail(app, generation, error);
                return;
            }
            if let Err(error) = self.scan_and_merge(app, generation).await {
                self.fail(app, generation, error);
                return;
            }
            match self.running_item_count(generation) {
                None => return,
                Some(count) if count >= MAX_SCAN_ITEMS => {
                    self.finish(app, generation, CollectStopReason::Cap);
                    return;
                }
                Some(_) => {}
            }

            let stepped = match self.scroll_report(&step_script).await {
                Ok(report) => report,
                Err(error) => {
                    self.fail(app, generation, error);
                    return;
                }
            };
            self.count_scroll(generation);
            let at_end = stepped.ends_the_page();
            let started = Instant::now();
            loop {
                tokio::time::sleep(SCROLL_TICK).await;
                if !self.is_current_running(generation) {
                    return;
                }
                if self.ensure_page(generation).is_err() {
                    self.stop_for_page_change(app, generation);
                    return;
                }
                let info = match self.scroll_report(&info_script).await {
                    Ok(report) => report,
                    Err(error) => {
                        self.fail(app, generation, error);
                        return;
                    }
                };
                let grew = info.height > stepped.height;
                match scroll_wait_decision(started.elapsed(), at_end, grew) {
                    ScrollWait::KeepWaiting => {}
                    ScrollWait::GoOn => break,
                    ScrollWait::LookAgain => {
                        // Nothing loaded for the whole wait at the end: one more scan decides.
                        match self.scan_and_merge(app, generation).await {
                            Ok(0) => {
                                self.finish(app, generation, CollectStopReason::End);
                                return;
                            }
                            Ok(_) => break,
                            Err(error) => {
                                self.fail(app, generation, error);
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn scroll_report(&self, script: &str) -> Result<ScrollReport, String> {
        parse_scroll_report(&self.browser.execute(script).await?)
    }

    /// The number of items the session holds, while `generation` is still the one running.
    fn running_item_count(&self, generation: u64) -> Option<usize> {
        lock_recover(&self.session)
            .as_ref()
            .filter(|current| current.generation == generation && current.state.running)
            .map(|current| current.state.items.len())
    }

    fn count_scroll(&self, generation: u64) {
        if let Some(current) = lock_recover(&self.session)
            .as_mut()
            .filter(|current| current.generation == generation && current.state.running)
        {
            current.state.scrolls = current.state.scrolls.saturating_add(1);
        }
    }

    /// Ends the run `generation` with `reason`, unless it was stopped or replaced meanwhile.
    fn finish(&self, app: &AppHandle, generation: u64, reason: CollectStopReason) {
        let mut session = lock_recover(&self.session);
        let Some(current) = session
            .as_mut()
            .filter(|current| current.generation == generation && current.state.running)
        else {
            return;
        };
        current.state.running = false;
        current.state.stop_reason = Some(reason);
        let state = current.state.clone();
        drop(session);
        emit_state(app, &state);
    }

    async fn scan_and_merge(&self, app: &AppHandle, generation: u64) -> Result<usize, String> {
        let (requested_shape, include_ads, mark_cards) = {
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
                // The played streams have no cards in the page: nothing is marked there.
                current.state.shape.as_deref() != Some(PLAYED_SHAPE),
            )
        };
        let expression = scan_expression_marking(&requested_shape, include_ads, mark_cards)?;
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
        current.state.stop_reason = Some(CollectStopReason::Stopped);
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
        current.state.stop_reason = Some(CollectStopReason::Stopped);
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

/// The tray's selection changed: the page's badges follow (the item URLs that are not selected,
/// which include the hidden ones) and the items the tray's filter hides (`hidden`, none when
/// left out) lose their outline and badge until they are shown again.
#[tauri::command]
pub async fn collect_set_selection(
    unselected: Vec<String>,
    hidden: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .collector
        .set_selection(unselected, hidden.unwrap_or_default())
        .await
}

#[tauri::command]
pub async fn collect_pick_start(
    language: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.collector.pick_start(&app, &language).await
}

#[tauri::command]
pub async fn collect_pick_stop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.collector.pick_stop(&app).await
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
    scan_expression_marking(requested_shape, include_ads, true)
}

/// A scan that leaves the page unmarked when `mark_cards` is false (the option is only sent then,
/// so the default stays the scan every other caller makes).
fn scan_expression_marking(
    requested_shape: &Option<String>,
    include_ads: bool,
    mark_cards: bool,
) -> Result<String, String> {
    let shape = serde_json::to_string(requested_shape).map_err(|error| error.to_string())?;
    let mut options = json!({ "includeAds": include_ads });
    if !mark_cards {
        options["mark"] = json!(false);
    }
    let options = serde_json::to_string(&options).map_err(|error| error.to_string())?;
    Ok(format!("window.__vdlCollector.scan({shape}, {options})"))
}

fn reveal_expression(url: &str) -> Result<String, String> {
    let url = serde_json::to_string(url).map_err(|error| error.to_string())?;
    Ok(format!("window.__vdlCollector.reveal({url})"))
}

/// What the app asks the page every `PAGE_POLL`. Without pick mode only the clicked badges are
/// asked for, by a call into the collector the page already has (the script is not evaluated
/// again for a poll); in pick mode the answer also carries the pick, and `gone` when the page
/// has no collector any more (it was loaded again, and pick mode went with it).
const POLL_TOGGLES_SCRIPT: &str =
    "window.__vdlCollector ? window.__vdlCollector.takeToggles() : []";
const POLL_PICKING_SCRIPT: &str = "(() => { const c = window.__vdlCollector; return c ? { toggles: c.takeToggles(), pick: c.takePick() } : { gone: true }; })()";
const STOP_PICK_SCRIPT: &str = "window.__vdlCollector ? window.__vdlCollector.stopPick() : null";

fn poll_expression(picking: bool) -> &'static str {
    if picking {
        POLL_PICKING_SCRIPT
    } else {
        POLL_TOGGLES_SCRIPT
    }
}

/// The unselected and the hidden URLs go into the page as two JSON arrays; only plain bounded
/// strings are sent.
fn set_selection_expression(unselected: &[String], hidden: &[String]) -> Result<String, String> {
    let unselected = bounded_url_list(unselected)?;
    let hidden = bounded_url_list(hidden)?;
    Ok(format!(
        "window.__vdlCollector.setSelection({unselected},{hidden})"
    ))
}

fn bounded_url_list(urls: &[String]) -> Result<String, String> {
    let urls = urls
        .iter()
        .filter(|url| !url.is_empty() && url.chars().count() <= MAX_URL_CHARS)
        .take(MAX_SCAN_ITEMS)
        .collect::<Vec<_>>();
    serde_json::to_string(&urls).map_err(|error| error.to_string())
}

/// The page's labels follow the app's language ("en", else Japanese).
fn pick_start_expression(language: &str) -> Result<String, String> {
    let language = serde_json::to_string(if language == "en" { "en" } else { "ja" })
        .map_err(|error| error.to_string())?;
    Ok(format!("window.__vdlCollector.startPick({language})"))
}

/// Plays the `index`-th `<video>` of the page muted, the way `PLAY_SCRIPT` plays them all.
fn play_video_expression(index: usize) -> String {
    format!(
        r#"(() => {{
  const video = document.querySelectorAll("video")[{index}];
  if (!video) return false;
  video.muted = true;
  video.play().catch(() => {{}});
  return true;
}})()"#
    )
}

/// The played streams a press on a video made new: those not played before it, and the video's
/// own address when it is among them.
fn fresh_streams(
    played: &[crate::browser::PlayedMedia],
    before: &HashSet<String>,
    picked: Option<&str>,
) -> Vec<String> {
    played
        .iter()
        .filter(|media| !before.contains(&media.url) || Some(media.url.as_str()) == picked)
        .map(|media| media.url.clone())
        .collect()
}

/// What the page returned for `poll_expression`: a list of toggles, or the object of pick mode.
/// Everything is checked and bounded, as the scan's result is.
pub(crate) fn parse_poll(value: &Value) -> PagePoll {
    match value {
        Value::Array(_) => PagePoll {
            toggles: parse_toggles(value),
            ..PagePoll::default()
        },
        Value::Object(object) => PagePoll {
            toggles: object.get("toggles").map(parse_toggles).unwrap_or_default(),
            pick: object.get("pick").and_then(parse_pick),
            gone: object.get("gone").and_then(Value::as_bool).unwrap_or(false),
        },
        _ => PagePoll::default(),
    }
}

/// The badge clicks: at most `MAX_SCAN_ITEMS`, each an http(s) URL and a selected flag.
pub(crate) fn parse_toggles(value: &Value) -> Vec<CollectToggle> {
    value
        .as_array()
        .map(|toggles| {
            toggles
                .iter()
                .take(MAX_SCAN_ITEMS)
                .filter_map(|toggle| {
                    let url = bounded_string(toggle.get("url")?, MAX_URL_CHARS)?;
                    if !is_http_url(&url) {
                        return None;
                    }
                    let selected = toggle.get("selected")?.as_bool()?;
                    Some(CollectToggle { url, selected })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The picker's answer. `None` when nothing was picked yet (null); an answer that cannot be
/// read counts as a cancel, so pick mode ends rather than hangs.
pub(crate) fn parse_pick(value: &Value) -> Option<PagePick> {
    if value.is_null() {
        return None;
    }
    Some(read_pick(value).unwrap_or(PagePick::Cancel))
}

fn read_pick(value: &Value) -> Option<PagePick> {
    let object = value.as_object()?;
    let http_url = |key: &str| {
        object
            .get(key)
            .and_then(|url| bounded_string(url, MAX_URL_CHARS))
            .filter(|url| is_http_url(url))
    };
    match object.get("kind")?.as_str()? {
        "cancel" => Some(PagePick::Cancel),
        "none" => Some(PagePick::Nothing),
        "link" => {
            let shape = bounded_string(object.get("shape")?, MAX_SHAPE_CHARS)
                .filter(|shape| !shape.is_empty())?;
            let count = usize::try_from(object.get("count")?.as_u64()?)
                .ok()?
                .min(MAX_SCAN_ITEMS);
            Some(PagePick::Link {
                url: http_url("url")?,
                shape,
                count,
            })
        }
        "image" => Some(PagePick::Image {
            url: http_url("url")?,
        }),
        "video" => Some(PagePick::Video {
            url: http_url("url"),
            index: object
                .get("videoIndex")
                .and_then(Value::as_u64)
                .filter(|index| *index < MAX_VIDEO_INDEX)
                .and_then(|index| usize::try_from(index).ok()),
        }),
        _ => None,
    }
}

fn emit_pick(app: &AppHandle, event: &CollectPickEvent) {
    let _ = app.emit("collect-pick", event.clone());
}

fn emit_toggles(app: &AppHandle, toggles: Vec<CollectToggle>) {
    let _ = app.emit("collect-toggles", CollectTogglesEvent { toggles });
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
        COLLECTOR_SCRIPT, CollectGroup, CollectItem, CollectLinks, CollectPage, CollectPickEvent,
        CollectState, CollectStopReason, CollectToggle, CollectTogglesEvent, MAX_SCAN_ITEMS,
        MAX_URL_CHARS, PLAYED_SHAPE, PagePick, PagePoll, ReloadFindDecision, ScrollReport,
        ScrollWait, build_rule_json, fresh_streams, glob_matches, merge_items, parse_pick,
        parse_poll, parse_scan_result, parse_scroll_report, parse_toggles, pick_start_expression,
        play_video_expression, poll_expression, preferred_shape, reload_find_decision,
        reveal_expression, scan_expression, scan_expression_marking, scroll_wait_decision,
        set_selection_expression,
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

    fn wait_at(millis: u64, at_end: bool, grew: bool) -> ScrollWait {
        scroll_wait_decision(Duration::from_millis(millis), at_end, grew)
    }

    #[test]
    fn a_page_that_grew_goes_on_at_the_first_tick_from_400_ms() {
        for at_end in [false, true] {
            assert_eq!(wait_at(250, at_end, true), ScrollWait::KeepWaiting);
            assert_eq!(wait_at(399, at_end, true), ScrollWait::KeepWaiting);
            assert_eq!(wait_at(500, at_end, true), ScrollWait::GoOn);
            assert_eq!(wait_at(400, at_end, true), ScrollWait::GoOn);
        }
    }

    #[test]
    fn a_page_that_is_not_at_the_end_goes_on_after_1500_ms_without_growing() {
        assert_eq!(wait_at(0, false, false), ScrollWait::KeepWaiting);
        assert_eq!(wait_at(500, false, false), ScrollWait::KeepWaiting);
        assert_eq!(wait_at(1250, false, false), ScrollWait::KeepWaiting);
        assert_eq!(wait_at(1500, false, false), ScrollWait::GoOn);
        assert_eq!(wait_at(1750, false, false), ScrollWait::GoOn);
    }

    #[test]
    fn at_the_end_a_page_that_does_not_grow_is_looked_at_once_more_after_5000_ms() {
        assert_eq!(wait_at(500, true, false), ScrollWait::KeepWaiting);
        assert_eq!(wait_at(1500, true, false), ScrollWait::KeepWaiting);
        assert_eq!(wait_at(4750, true, false), ScrollWait::KeepWaiting);
        assert_eq!(wait_at(5000, true, false), ScrollWait::LookAgain);
        assert_eq!(wait_at(5250, true, false), ScrollWait::LookAgain);
    }

    #[test]
    fn at_the_end_growth_beats_the_final_look() {
        assert_eq!(wait_at(5000, true, true), ScrollWait::GoOn);
        assert_eq!(wait_at(3000, true, true), ScrollWait::GoOn);
    }

    #[test]
    fn scroll_reports_are_checked() {
        assert_eq!(
            parse_scroll_report(&json!({
                "target": "window", "top": 900, "height": 2400.0, "client": 800,
                "atEnd": false, "moved": true
            }))
            .unwrap(),
            ScrollReport {
                height: 2400.0,
                at_end: false,
                moved: true
            }
        );
        // scrollInfo() does not scroll, so it carries no `moved`.
        assert_eq!(
            parse_scroll_report(&json!({ "height": 2400, "atEnd": true })).unwrap(),
            ScrollReport {
                height: 2400.0,
                at_end: true,
                moved: false
            }
        );
        for bad in [
            json!(null),
            json!([]),
            json!({ "atEnd": true }),
            json!({ "height": "2400", "atEnd": true }),
            json!({ "height": -1, "atEnd": true }),
            json!({ "height": 100 }),
            json!({ "height": 100, "atEnd": "yes" }),
        ] {
            assert!(
                parse_scroll_report(&bad).is_err(),
                "{bad} should be refused"
            );
        }
    }

    #[test]
    fn a_step_that_reached_the_end_or_could_not_move_ends_the_page() {
        let after = |at_end, moved| {
            parse_scroll_report(&json!({ "height": 900, "atEnd": at_end, "moved": moved }))
                .unwrap()
                .ends_the_page()
        };
        assert!(!after(false, true));
        assert!(after(true, true));
        assert!(after(true, false));
        assert!(after(false, false));
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
    fn a_scan_of_the_played_group_asks_the_page_to_mark_nothing() {
        assert_eq!(
            scan_expression_marking(&None, false, false).unwrap(),
            r#"window.__vdlCollector.scan(null, {"includeAds":false,"mark":false})"#
        );
        // Marking is the default and is not sent.
        assert_eq!(
            scan_expression_marking(&None, true, true).unwrap(),
            scan_expression(&None, true).unwrap()
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
            stop_reason: Some(CollectStopReason::Cap),
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
        assert_eq!(value["stopReason"], json!("cap"));
        assert_eq!(value["maxScrolls"], json!(10));
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
        assert!(COLLECTOR_SCRIPT.contains("function scrollInfo("));
        assert!(COLLECTOR_SCRIPT.contains("function reveal("));
        assert!(COLLECTOR_SCRIPT.contains("function clear("));
    }

    #[test]
    fn stop_reasons_serialize_in_camel_case_and_none_as_null() {
        let reasons = [
            (CollectStopReason::End, "end"),
            (CollectStopReason::Stopped, "stopped"),
            (CollectStopReason::Cap, "cap"),
        ];
        for (reason, text) in reasons {
            assert_eq!(serde_json::to_value(reason).unwrap(), json!(text));
        }
        let mut state = CollectState {
            running: true,
            scrolls: 0,
            max_scrolls: 10,
            stop_reason: None,
            groups: Vec::new(),
            shape: None,
            items: Vec::new(),
            ads_excluded: 0,
            rule_id: None,
            error: None,
            page: None,
            other_links: None,
        };
        assert_eq!(
            serde_json::to_value(&state).unwrap()["stopReason"],
            json!(null)
        );
        state.stop_reason = Some(CollectStopReason::End);
        assert_eq!(
            serde_json::to_value(&state).unwrap()["stopReason"],
            json!("end")
        );
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

    fn toggle(url: &str, selected: bool) -> CollectToggle {
        CollectToggle {
            url: url.to_owned(),
            selected,
        }
    }

    #[test]
    fn collector_script_keeps_the_selection_and_picker_entry_points() {
        for entry in [
            "function setSelection(",
            "function takeToggles(",
            "function startPick(",
            "function stopPick(",
            "function takePick(",
        ] {
            assert!(COLLECTOR_SCRIPT.contains(entry), "{entry} is missing");
        }
        // The poll calls these on the collector the page already has.
        assert!(COLLECTOR_SCRIPT.contains("setSelection,"));
        assert!(COLLECTOR_SCRIPT.contains("takeToggles,"));
        assert!(COLLECTOR_SCRIPT.contains("takePick,"));
    }

    #[test]
    fn toggles_keep_valid_http_urls_with_a_selected_flag_in_order() {
        let toggles = parse_toggles(&json!([
            { "url": "https://example.com/watch/1", "selected": false },
            { "url": "https://example.com/watch/2", "selected": true },
            { "url": "https://example.com/watch/1", "selected": true },
        ]));
        assert_eq!(
            toggles,
            vec![
                toggle("https://example.com/watch/1", false),
                toggle("https://example.com/watch/2", true),
                toggle("https://example.com/watch/1", true),
            ]
        );
    }

    #[test]
    fn toggles_drop_what_is_not_a_bounded_http_url_with_a_boolean() {
        let too_long = format!("https://example.com/{}", "x".repeat(MAX_URL_CHARS));
        let toggles = parse_toggles(&json!([
            { "url": "javascript:alert(1)", "selected": true },
            { "url": "file:///etc/passwd", "selected": true },
            { "url": too_long, "selected": true },
            { "url": 5, "selected": true },
            { "url": "https://example.com/a" },
            { "url": "https://example.com/b", "selected": "yes" },
            { "selected": true },
            "https://example.com/c",
            null,
            { "url": "https://example.com/ok", "selected": false },
        ]));
        assert_eq!(toggles, vec![toggle("https://example.com/ok", false)]);
        // Not a list at all.
        for bad in [json!(null), json!({}), json!("x"), json!(3)] {
            assert!(parse_toggles(&bad).is_empty(), "{bad}");
        }
    }

    #[test]
    fn toggles_are_capped_at_the_most_items_a_session_holds() {
        let many = (0..MAX_SCAN_ITEMS + 500)
            .map(|index| json!({ "url": format!("https://example.com/watch/{index}"), "selected": false }))
            .collect::<Vec<_>>();
        let toggles = parse_toggles(&json!(many));
        assert_eq!(toggles.len(), MAX_SCAN_ITEMS);
        assert_eq!(toggles[0].url, "https://example.com/watch/0");
    }

    #[test]
    fn a_poll_answer_is_a_list_of_toggles_or_the_picking_object() {
        let list = parse_poll(&json!([{ "url": "https://example.com/a", "selected": true }]));
        assert_eq!(
            list,
            PagePoll {
                toggles: vec![toggle("https://example.com/a", true)],
                pick: None,
                gone: false,
            }
        );
        let picking = parse_poll(&json!({
            "toggles": [{ "url": "https://example.com/a", "selected": false }],
            "pick": { "kind": "image", "url": "https://example.com/a.jpg" }
        }));
        assert_eq!(
            picking.toggles,
            vec![toggle("https://example.com/a", false)]
        );
        assert_eq!(
            picking.pick,
            Some(PagePick::Image {
                url: "https://example.com/a.jpg".to_owned()
            })
        );
        assert!(!picking.gone);
        // A page without the collector (loaded again) says so.
        assert!(parse_poll(&json!({ "gone": true })).gone);
        // Nothing picked yet is null, not a pick.
        assert_eq!(
            parse_poll(&json!({ "toggles": [], "pick": null })),
            PagePoll::default()
        );
        // An empty answer, a missing result or nonsense changes nothing.
        for other in [json!([]), json!(null), json!("x"), json!(7), json!({})] {
            assert_eq!(parse_poll(&other), PagePoll::default(), "{other}");
        }
    }

    #[test]
    fn picks_of_each_kind_are_read() {
        assert_eq!(parse_pick(&json!(null)), None);
        assert_eq!(
            parse_pick(&json!({ "kind": "cancel" })),
            Some(PagePick::Cancel)
        );
        assert_eq!(
            parse_pick(&json!({ "kind": "none" })),
            Some(PagePick::Nothing)
        );
        assert_eq!(
            parse_pick(&json!({
                "kind": "link",
                "url": "https://example.com/watch/3",
                "shape": "/watch/{num}",
                "count": 6,
                "videoIndex": null,
                "rect": { "x": 1, "y": 2, "width": 3, "height": 4 }
            })),
            Some(PagePick::Link {
                url: "https://example.com/watch/3".to_owned(),
                shape: "/watch/{num}".to_owned(),
                count: 6,
            })
        );
        assert_eq!(
            parse_pick(&json!({ "kind": "image", "url": "https://example.com/large.png" })),
            Some(PagePick::Image {
                url: "https://example.com/large.png".to_owned()
            })
        );
        assert_eq!(
            parse_pick(&json!({
                "kind": "video",
                "url": "https://example.com/clip.mp4",
                "videoIndex": 2
            })),
            Some(PagePick::Video {
                url: Some("https://example.com/clip.mp4".to_owned()),
                index: Some(2),
            })
        );
    }

    #[test]
    fn a_video_without_an_http_address_is_still_a_video_to_play() {
        for url in [
            json!(null),
            json!("blob:https://example.com/1234"),
            json!("data:video/mp4;base64,AAAA"),
        ] {
            assert_eq!(
                parse_pick(&json!({ "kind": "video", "url": url, "videoIndex": 0 })),
                Some(PagePick::Video {
                    url: None,
                    index: Some(0)
                })
            );
        }
        // The position is bounded; without one the video can only be waited for.
        assert_eq!(
            parse_pick(&json!({ "kind": "video", "url": null, "videoIndex": 10_000 })),
            Some(PagePick::Video {
                url: None,
                index: None
            })
        );
        assert_eq!(
            parse_pick(&json!({ "kind": "video", "url": null, "videoIndex": -1 })),
            Some(PagePick::Video {
                url: None,
                index: None
            })
        );
    }

    #[test]
    fn picks_are_bounded_and_a_pick_that_cannot_be_read_is_a_cancel() {
        assert_eq!(
            parse_pick(&json!({
                "kind": "link", "url": "https://example.com/a", "shape": "/a/{num}", "count": 90_000
            })),
            Some(PagePick::Link {
                url: "https://example.com/a".to_owned(),
                shape: "/a/{num}".to_owned(),
                count: MAX_SCAN_ITEMS,
            })
        );
        let long_url = format!("https://example.com/{}", "x".repeat(MAX_URL_CHARS));
        let long_shape = format!("/{}", "x".repeat(2_048));
        for bad in [
            json!("link"),
            json!(5),
            json!([]),
            json!({}),
            json!({ "kind": 3 }),
            json!({ "kind": "teleport" }),
            json!({ "kind": "link", "shape": "/a/{num}", "count": 3 }),
            json!({ "kind": "link", "url": "https://example.com/a", "count": 3 }),
            json!({ "kind": "link", "url": "https://example.com/a", "shape": "", "count": 3 }),
            json!({ "kind": "link", "url": "https://example.com/a", "shape": "/a", "count": "3" }),
            json!({ "kind": "link", "url": "https://example.com/a", "shape": "/a", "count": -3 }),
            json!({ "kind": "link", "url": "javascript:alert(1)", "shape": "/a", "count": 3 }),
            json!({ "kind": "link", "url": long_url, "shape": "/a", "count": 3 }),
            json!({ "kind": "link", "url": "https://example.com/a", "shape": long_shape, "count": 3 }),
            json!({ "kind": "image", "url": "data:image/png;base64,AAAA" }),
            json!({ "kind": "image" }),
        ] {
            assert_eq!(parse_pick(&bad), Some(PagePick::Cancel), "{bad}");
        }
    }

    #[test]
    fn the_poll_asks_the_collector_the_page_already_has() {
        assert_eq!(
            poll_expression(false),
            "window.__vdlCollector ? window.__vdlCollector.takeToggles() : []"
        );
        let picking = poll_expression(true);
        assert!(picking.contains("takeToggles()"));
        assert!(picking.contains("takePick()"));
        // The page script is never evaluated again for a poll.
        assert!(!picking.contains("function"));
        assert!(!poll_expression(false).contains("function"));
    }

    #[test]
    fn selection_goes_to_the_page_as_a_json_list_of_bounded_urls() {
        assert_eq!(
            set_selection_expression(&[], &[]).unwrap(),
            "window.__vdlCollector.setSelection([],[])"
        );
        let long = "x".repeat(MAX_URL_CHARS + 1);
        assert_eq!(
            set_selection_expression(
                &[
                    "https://example.com/a?b=\"c\"".to_owned(),
                    String::new(),
                    long,
                    "https://example.com/d".to_owned(),
                ],
                &[]
            )
            .unwrap(),
            r#"window.__vdlCollector.setSelection(["https://example.com/a?b=\"c\"","https://example.com/d"],[])"#
        );
        let many = (0..MAX_SCAN_ITEMS + 10)
            .map(|index| format!("https://example.com/{index}"))
            .collect::<Vec<_>>();
        let expression = set_selection_expression(&many, &[]).unwrap();
        assert_eq!(expression.matches("https://").count(), MAX_SCAN_ITEMS);
    }

    #[test]
    fn hidden_urls_go_into_the_page_as_a_second_bounded_list() {
        let long = "x".repeat(MAX_URL_CHARS + 1);
        assert_eq!(
            set_selection_expression(
                &["https://example.com/a".to_owned()],
                &[
                    "https://example.com/b".to_owned(),
                    String::new(),
                    long,
                    "https://example.com/c\"d".to_owned(),
                ]
            )
            .unwrap(),
            r#"window.__vdlCollector.setSelection(["https://example.com/a"],["https://example.com/b","https://example.com/c\"d"])"#
        );
        let many = (0..MAX_SCAN_ITEMS + 10)
            .map(|index| format!("https://example.com/{index}"))
            .collect::<Vec<_>>();
        let expression = set_selection_expression(&[], &many).unwrap();
        assert_eq!(expression.matches("https://").count(), MAX_SCAN_ITEMS);
    }

    #[test]
    fn pick_mode_starts_with_the_app_language() {
        assert_eq!(
            pick_start_expression("en").unwrap(),
            r#"window.__vdlCollector.startPick("en")"#
        );
        for other in ["ja", "fr", "", "en\");alert(1);(\""] {
            assert_eq!(
                pick_start_expression(other).unwrap(),
                r#"window.__vdlCollector.startPick("ja")"#
            );
        }
    }

    #[test]
    fn a_pressed_video_is_played_muted_by_its_position() {
        let script = play_video_expression(3);
        assert!(script.contains(r#"document.querySelectorAll("video")[3]"#));
        assert!(script.contains("video.muted = true"));
        assert!(script.contains("video.play()"));
    }

    fn played(url: &str) -> crate::browser::PlayedMedia {
        crate::browser::PlayedMedia {
            url: url.to_owned(),
            kind: "HLS".to_owned(),
            duration_seconds: None,
        }
    }

    #[test]
    fn a_pressed_video_gets_the_streams_that_were_not_played_before() {
        let before = HashSet::from(["https://cdn.example.com/old.m3u8".to_owned()]);
        let now = [
            played("https://cdn.example.com/old.m3u8"),
            played("https://cdn.example.com/new.m3u8"),
        ];
        assert_eq!(
            fresh_streams(&now, &before, None),
            vec!["https://cdn.example.com/new.m3u8"]
        );
        // Nothing new: nothing arrived.
        assert!(fresh_streams(&now[..1], &before, None).is_empty());
        // The video's own address counts even when it was played before.
        assert_eq!(
            fresh_streams(&now[..1], &before, Some("https://cdn.example.com/old.m3u8")),
            vec!["https://cdn.example.com/old.m3u8"]
        );
        assert!(
            fresh_streams(
                &now[..1],
                &before,
                Some("https://cdn.example.com/other.mp4")
            )
            .is_empty()
        );
    }

    #[test]
    fn toggle_and_pick_events_serialize_for_the_tray() {
        assert_eq!(
            serde_json::to_value(CollectTogglesEvent {
                toggles: vec![toggle("https://example.com/a", false)]
            })
            .unwrap(),
            json!({ "toggles": [{ "url": "https://example.com/a", "selected": false }] })
        );
        let events = [
            (CollectPickEvent::Active, json!({ "kind": "active" })),
            (CollectPickEvent::Ended, json!({ "kind": "ended" })),
            (CollectPickEvent::Finding, json!({ "kind": "finding" })),
            (
                CollectPickEvent::Link {
                    url: "https://example.com/watch/3".to_owned(),
                    shape: "/watch/{num}".to_owned(),
                    count: 6,
                },
                json!({ "kind": "link", "url": "https://example.com/watch/3", "shape": "/watch/{num}", "count": 6 }),
            ),
            (
                CollectPickEvent::Image {
                    url: "https://example.com/a.png".to_owned(),
                },
                json!({ "kind": "image", "url": "https://example.com/a.png" }),
            ),
            (
                CollectPickEvent::Video {
                    urls: vec!["https://example.com/a.m3u8".to_owned()],
                },
                json!({ "kind": "video", "urls": ["https://example.com/a.m3u8"] }),
            ),
            (
                CollectPickEvent::VideoNotFound,
                json!({ "kind": "videoNotFound" }),
            ),
        ];
        for (event, expected) in events {
            assert_eq!(serde_json::to_value(event).unwrap(), expected);
        }
    }
}

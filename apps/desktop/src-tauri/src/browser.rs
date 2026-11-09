use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewUrl};
use vids_core::page_media::{
    ManifestCache, MediaCandidate, ReducedMedia, add_candidate, reduce_remote_cached,
};
#[cfg(target_os = "windows")]
use vids_core::tools::Tool;
use vids_core::tools::ToolManager;

use crate::sites::{self, Site};
use crate::state::lock_recover;

const URL_POLL_INTERVAL: Duration = Duration::from_millis(500);
const MEDIA_REDUCE_SETTLE: Duration = Duration::from_millis(300);
/// The logical size of a source check's hidden page.
const CHECK_PAGE_WIDTH: f64 = 1280.0;
const CHECK_PAGE_HEIGHT: f64 = 900.0;

pub(crate) const MEDIA_SCRIPT: &str = r#"(() => {
  if (!window.__vdlPageMediaTiming) {
    performance.setResourceTimingBufferSize(5000);
    window.__vdlPageMediaTiming = true;
  }
  const urls = performance.getEntriesByType("resource").map((entry) => entry.name);
  for (const element of document.querySelectorAll("video[src], video source[src]")) {
    urls.push(element.src || element.getAttribute("src"));
  }
  const unique = [];
  const seen = new Set();
  for (const value of urls) {
    try {
      const url = new URL(value, location.href);
      if ((url.protocol === "http:" || url.protocol === "https:") && !seen.has(url.href)) {
        seen.add(url.href);
        unique.push(url.href);
      }
    } catch {}
  }
  return unique;
})()"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserMode {
    Browse,
    Login,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserEventState {
    pub url: Option<String>,
    pub loading: bool,
    pub mode: BrowserMode,
    pub site_id: Option<i64>,
    pub account_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for BrowserBounds {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}

impl BrowserBounds {
    pub fn validated(self) -> Result<Self, String> {
        if ![self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            || self.width <= 0.0
            || self.height <= 0.0
        {
            return Err("browser bounds must be finite with positive width and height".to_owned());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserProfile {
    General,
    Account(i64),
}

impl BrowserProfile {
    fn directory_name(self) -> String {
        match self {
            Self::General => "general".to_owned(),
            Self::Account(id) => id.to_string(),
        }
    }

    #[cfg(target_os = "macos")]
    fn data_store_identifier(self) -> [u8; 16] {
        let mut identifier = *b"vids-browser-000";
        let number = match self {
            Self::General => 0,
            Self::Account(id) => id,
        };
        identifier[8..].copy_from_slice(&number.to_be_bytes());
        identifier
    }
}

struct BrowserView {
    webview: Webview,
    profile: BrowserProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayedMedia {
    pub url: String,
    pub kind: String,
    pub duration_seconds: Option<u64>,
}

#[derive(Default)]
struct PlayedPage {
    page_url: Option<String>,
    generation: u64,
    revision: u64,
    candidates: Vec<MediaCandidate>,
    media: Vec<PlayedMedia>,
    manifests: ManifestCache,
    reducing: bool,
}

struct Session {
    view: Option<BrowserView>,
    bounds: BrowserBounds,
    destination_visible: bool,
    page_open: bool,
    generation: u64,
    event: BrowserEventState,
    played: PlayedPage,
    /// Finished page loads in this view, so a caller can wait for a reload it started.
    loads_finished: u64,
    /// Script installed again whenever the visible page finishes loading. Used by the
    /// prototype flow recorder; it only writes to a page global and exposes no IPC.
    page_load_script: Option<&'static str>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            view: None,
            bounds: BrowserBounds::default(),
            destination_visible: false,
            page_open: false,
            generation: 0,
            loads_finished: 0,
            page_load_script: None,
            event: BrowserEventState {
                url: None,
                loading: false,
                mode: BrowserMode::Browse,
                site_id: None,
                account_id: None,
            },
            played: PlayedPage::default(),
        }
    }
}

pub struct BrowserManager {
    root: PathBuf,
    store: Arc<Mutex<vids_core::store::Store>>,
    /// Read only for the ad blocker, which exists on Windows only.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    tools: Arc<ToolManager>,
    session: Mutex<Session>,
    operations: Mutex<()>,
    helper_sequence: AtomicU64,
    last_refresh: Mutex<std::collections::HashMap<i64, std::time::Instant>>,
}

impl BrowserManager {
    pub fn new(
        root: PathBuf,
        store: Arc<Mutex<vids_core::store::Store>>,
        tools: Arc<ToolManager>,
    ) -> Self {
        Self {
            root,
            store,
            tools,
            session: Mutex::new(Session::default()),
            operations: Mutex::new(()),
            helper_sequence: AtomicU64::new(1),
            last_refresh: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn open(
        self: &Arc<Self>,
        app: &AppHandle,
        url: Option<Url>,
        profile: BrowserProfile,
        mode: BrowserMode,
        site_id: Option<i64>,
        account_id: Option<i64>,
    ) -> Result<(), String> {
        let _operation = lock_recover(&self.operations);
        let page_open = url.is_some();
        let target = url.unwrap_or_else(about_blank);
        let (old, create, generation, bounds) = {
            let mut session = lock_recover(&self.session);
            let create = session
                .view
                .as_ref()
                .is_none_or(|view| view.profile != profile);
            let old = create.then(|| session.view.take()).flatten();
            if create {
                session.generation = session.generation.wrapping_add(1);
            }
            session.destination_visible = page_open;
            session.page_open = page_open;
            session.event = BrowserEventState {
                url: page_open.then(|| target.to_string()),
                loading: page_open,
                mode,
                site_id,
                account_id,
            };
            reset_played_page(&mut session.played, page_open.then(|| target.to_string()));
            (old, create, session.generation, session.bounds)
        };

        if let Some(old) = old {
            old.webview.close().map_err(|error| error.to_string())?;
            self.emit(app);
        }

        let webview = if create {
            let webview = self.create_browser_view(app, profile, generation, bounds)?;
            let mut session = lock_recover(&self.session);
            if session.generation != generation {
                let _ = webview.close();
                return Err("browser view changed while it was opening".to_owned());
            }
            session.view = Some(BrowserView {
                webview: webview.clone(),
                profile,
            });
            // Bounds reported while the view was being created (a login banner appearing,
            // say) only reached the session; apply the latest ones to the new view.
            let latest = session.bounds;
            drop(session);
            self.start_url_polling(app, webview.clone(), generation);
            if latest != bounds {
                webview
                    .set_bounds(tauri::Rect {
                        position: LogicalPosition::new(latest.x, latest.y).into(),
                        size: LogicalSize::new(latest.width, latest.height).into(),
                    })
                    .map_err(|error| error.to_string())?;
            }
            self.apply_ad_block_for_url(&webview, mode, &target, Some(target.to_string()))?;
            webview
        } else {
            let session = lock_recover(&self.session);
            let webview = session
                .view
                .as_ref()
                .expect("existing profile has a browser view")
                .webview
                .clone();
            drop(session);
            self.apply_ad_block_for_url(&webview, mode, &target, Some(target.to_string()))?;
            webview
        };

        if page_open {
            webview.show().map_err(|error| error.to_string())?;
        } else {
            webview.hide().map_err(|error| error.to_string())?;
        }
        self.emit(app);
        Ok(())
    }

    pub fn set_bounds(&self, bounds: BrowserBounds) -> Result<(), String> {
        let bounds = bounds.validated()?;
        let webview = {
            let mut session = lock_recover(&self.session);
            session.bounds = bounds;
            session.view.as_ref().map(|view| view.webview.clone())
        };
        if let Some(webview) = webview {
            webview
                .set_bounds(tauri::Rect {
                    position: LogicalPosition::new(bounds.x, bounds.y).into(),
                    size: LogicalSize::new(bounds.width, bounds.height).into(),
                })
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn set_visible(&self, app: &AppHandle, visible: bool) -> Result<(), String> {
        let (webview, should_show) = {
            let mut session = lock_recover(&self.session);
            session.destination_visible = visible;
            (
                session.view.as_ref().map(|view| view.webview.clone()),
                visible && session.page_open,
            )
        };
        if let Some(webview) = webview {
            if should_show {
                webview.show()
            } else {
                webview.hide()
            }
            .map_err(|error| error.to_string())?;
        }
        self.emit(app);
        Ok(())
    }

    pub fn eval(&self, script: &str) -> Result<(), String> {
        self.webview()?
            .eval(script)
            .map_err(|error| error.to_string())
    }

    /// Execute JavaScript in the untrusted child browser and return WebView2's
    /// JSON-encoded result without blocking the main thread.
    pub async fn execute(&self, script: &str) -> Result<serde_json::Value, String> {
        let webview = self.webview()?;
        execute_in(&webview, script).await
    }

    pub fn set_page_load_script(&self, script: Option<&'static str>) {
        lock_recover(&self.session).page_load_script = script;
    }

    pub fn reload(&self) -> Result<(), String> {
        self.webview()?.reload().map_err(|error| error.to_string())
    }

    /// Re-evaluate the open page after a per-site setting changes. The extension is
    /// toggled in place; the page is not reloaded.
    pub fn reapply_ad_block(&self) -> Result<(), String> {
        let current = {
            let session = lock_recover(&self.session);
            session.view.as_ref().and_then(|view| {
                session
                    .event
                    .url
                    .as_deref()
                    .and_then(|url| Url::parse(url).ok())
                    .map(|url| (view.webview.clone(), session.event.mode, url))
            })
        };
        if let Some((webview, mode, url)) = current {
            self.apply_ad_block_for_url(&webview, mode, &url, None)?;
        }
        Ok(())
    }

    fn apply_ad_block_for_url(
        &self,
        webview: &Webview,
        mode: BrowserMode,
        url: &Url,
        navigate_after: Option<String>,
    ) -> Result<(), String> {
        let installed = self.ublock_path().is_some();
        let site_setting = self.site_ad_block_for_url(url);
        let desired = ad_block_desired(installed, mode, site_setting);
        apply_ublock_state(webview, desired, url.as_str().to_owned(), navigate_after)
    }

    fn site_ad_block_for_url(&self, url: &Url) -> Option<bool> {
        let profiles = lock_recover(&self.store).list_site_profiles().ok()?;
        let sites = profiles
            .into_iter()
            .filter_map(|profile| Site::from_stored(profile).ok())
            .collect::<Vec<_>>();
        let site_id = sites::site_for_url_in(&sites, url.as_str())?;
        sites
            .into_iter()
            .find(|site| site.id == site_id)
            .map(|site| site.ad_block)
    }

    #[cfg(target_os = "windows")]
    fn ublock_path(&self) -> Option<PathBuf> {
        self.tools
            .resolve(Tool::UblockLite, vids_core::naming::TargetOs::Windows)
            .map(|resolved| resolved.path)
    }

    #[cfg(not(target_os = "windows"))]
    fn ublock_path(&self) -> Option<PathBuf> {
        None
    }

    pub fn login_context(&self) -> Result<(i64, i64), String> {
        let session = lock_recover(&self.session);
        if session.event.mode != BrowserMode::Login {
            return Err("no login is in progress".to_owned());
        }
        match (session.event.site_id, session.event.account_id) {
            (Some(site_id), Some(account_id)) => Ok((site_id, account_id)),
            _ => Err("login context is incomplete".to_owned()),
        }
    }

    pub fn start_page(&self, app: &AppHandle) -> Result<(), String> {
        let webview = {
            let mut session = lock_recover(&self.session);
            session.page_open = false;
            session.destination_visible = false;
            session.event = BrowserEventState {
                url: None,
                loading: false,
                mode: BrowserMode::Browse,
                site_id: None,
                account_id: None,
            };
            reset_played_page(&mut session.played, None);
            session.view.as_ref().map(|view| view.webview.clone())
        };
        if let Some(webview) = webview {
            webview
                .navigate(about_blank())
                .map_err(|error| error.to_string())?;
            webview.hide().map_err(|error| error.to_string())?;
        }
        self.emit(app);
        Ok(())
    }

    pub fn close_account_view(&self, app: &AppHandle, account_id: i64) -> Result<(), String> {
        let view = {
            let mut session = lock_recover(&self.session);
            if session
                .view
                .as_ref()
                .is_some_and(|view| view.profile == BrowserProfile::Account(account_id))
            {
                session.generation = session.generation.wrapping_add(1);
                session.page_open = false;
                session.destination_visible = false;
                session.event.url = None;
                session.event.loading = false;
                session.event.mode = BrowserMode::Browse;
                session.event.site_id = None;
                session.event.account_id = None;
                reset_played_page(&mut session.played, None);
                session.view.take()
            } else {
                None
            }
        };
        if let Some(view) = view {
            view.webview.close().map_err(|error| error.to_string())?;
            self.emit(app);
        }
        Ok(())
    }

    pub fn delete_account_data(&self, app: &AppHandle, account_id: i64) -> Result<(), String> {
        self.close_account_view(app, account_id)?;
        let path = self.profile_path(BrowserProfile::Account(account_id));
        for attempt in 0..3 {
            match fs::remove_dir_all(&path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(_) if attempt < 2 => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(error) => {
                    return Err(format!("could not delete {}: {error}", path.display()));
                }
            }
        }
        unreachable!("the deletion loop returns on its last attempt")
    }

    pub async fn cookies_for_site(
        &self,
        app: &AppHandle,
        account_id: i64,
        domains: &[String],
    ) -> Result<Vec<CookieRecord>, String> {
        let urls = site_urls(domains)?;
        let (webview, helper) = self.account_webview(app, BrowserProfile::Account(account_id))?;
        let cookies = read_view_cookies(webview.clone(), urls).await;
        if helper && let Err(error) = webview.close() {
            eprintln!("could not close cookie helper webview: {error}");
        }
        cookies
    }

    /// Adds cookies imported from another browser to an in-app account's browser folder,
    /// unless the account already has cookies for the site (an in-app login is never
    /// mixed with an imported one). Uses the browser view when it shows that account,
    /// else a hidden helper webview that is closed afterwards.
    pub async fn import_cookies(
        &self,
        app: &AppHandle,
        account_id: i64,
        domains: &[String],
        cookies: Vec<vids_core::firefox::Cookie>,
    ) -> Result<CookieImport, String> {
        let urls = site_urls(domains)?;
        let (webview, helper) = self.account_webview(app, BrowserProfile::Account(account_id))?;
        let outcome = async {
            let existing = read_view_cookies(webview.clone(), urls.clone()).await?;
            if !existing.is_empty() {
                return Ok(CookieImport::AlreadyHasCookies {
                    total: existing.len(),
                });
            }
            let added = add_view_cookies(&webview, cookies).await?;
            let total = read_view_cookies(webview.clone(), urls).await?.len();
            Ok(CookieImport::Added { added, total })
        }
        .await;
        if helper && let Err(error) = webview.close() {
            eprintln!("could not close cookie import helper webview: {error}");
        }
        outcome
    }

    /// The webview to reach an account's browser data through: the browser view when it
    /// shows that account (`false`), else a new hidden helper (`true`) the caller closes.
    fn account_webview(
        &self,
        app: &AppHandle,
        profile: BrowserProfile,
    ) -> Result<(Webview, bool), String> {
        let current = {
            let session = lock_recover(&self.session);
            session
                .view
                .as_ref()
                .filter(|view| view.profile == profile)
                .map(|view| view.webview.clone())
        };
        match current {
            Some(webview) => Ok((webview, false)),
            None => Ok((self.create_helper_view(app, profile)?, true)),
        }
    }

    pub async fn clear_account(&self, app: &AppHandle, account_id: i64) -> Result<(), String> {
        let (webview, helper) = self.account_webview(app, BrowserProfile::Account(account_id))?;
        let worker_view = webview.clone();
        let worker =
            tauri::async_runtime::spawn_blocking(move || worker_view.clear_all_browsing_data())
                .await;
        if helper && let Err(error) = webview.close() {
            eprintln!("could not close browser data helper webview: {error}");
        }
        worker
            .map_err(|error| format!("browser data task failed: {error}"))?
            .map_err(|error| error.to_string())?;
        if !helper {
            webview
                .navigate(about_blank())
                .map_err(|error| error.to_string())?;
            webview.hide().map_err(|error| error.to_string())?;
            let mut session = lock_recover(&self.session);
            session.page_open = false;
            session.destination_visible = false;
            session.event = BrowserEventState {
                url: None,
                loading: false,
                mode: BrowserMode::Browse,
                site_id: None,
                account_id: None,
            };
            reset_played_page(&mut session.played, None);
            drop(session);
            self.emit(app);
        }
        Ok(())
    }

    fn create_browser_view(
        self: &Arc<Self>,
        app: &AppHandle,
        profile: BrowserProfile,
        generation: u64,
        bounds: BrowserBounds,
    ) -> Result<Webview, String> {
        fs::create_dir_all(self.profile_path(profile)).map_err(|error| error.to_string())?;
        let manager = Arc::clone(self);
        let event_app = app.clone();
        let builder = WebviewBuilder::new("browser", WebviewUrl::External(about_blank()))
            .data_directory(self.profile_path(profile))
            .on_new_window(|url, _features| {
                if is_extension_url(&url) {
                    return NewWindowResponse::Deny;
                }
                // This app already denied page-created windows by not supplying a
                // handler. Preserve that behavior for ordinary pages.
                NewWindowResponse::Deny
            })
            .on_page_load(move |webview, payload| {
                if payload.event() == PageLoadEvent::Started {
                    let mode = lock_recover(&manager.session).event.mode;
                    if let Err(error) =
                        manager.apply_ad_block_for_url(&webview, mode, payload.url(), None)
                    {
                        eprintln!("could not apply ad blocking for page load: {error}");
                    }
                }
                BrowserManager::page_load(
                    &manager,
                    &event_app,
                    &webview,
                    generation,
                    payload.url().clone(),
                    payload.event(),
                );
            });
        #[cfg(target_os = "windows")]
        let builder = {
            let builder = builder.browser_extensions_enabled(true);
            match self.ublock_path() {
                Some(path) => builder.extensions_path(path),
                None => builder,
            }
        };
        #[cfg(target_os = "macos")]
        let builder = builder.data_store_identifier(profile.data_store_identifier());
        let webview = app
            .get_window("main")
            .ok_or_else(|| "main window was not found".to_owned())?
            .add_child(
                builder,
                LogicalPosition::new(bounds.x, bounds.y),
                LogicalSize::new(bounds.width, bounds.height),
            )
            .map_err(|error| format!("could not create browser webview: {error}"))?;
        crate::app_like::apply(&webview, crate::app_like::Role::BrowserPage);
        self.start_resource_capture(webview.clone(), generation)?;
        Ok(webview)
    }

    fn create_helper_view(
        &self,
        app: &AppHandle,
        profile: BrowserProfile,
    ) -> Result<Webview, String> {
        self.build_helper_view(app, profile, |builder| builder)
    }

    /// Opens `url` for a source check in a new hidden helper webview on `profile`'s browser
    /// folder (an account's, or the general one): muted, never opening other windows, with
    /// the size of a common desktop window so the page lays out and scrolls as it would for
    /// the user. The webview is closed when the returned page is dropped.
    pub fn open_check_page(
        &self,
        app: &AppHandle,
        profile: BrowserProfile,
        url: Url,
    ) -> Result<CheckPage, String> {
        self.open_helper_page(app, profile, url, false)
    }

    /// A check page that also reports WebView2 responses on Windows. Other platforms use
    /// the same page and collect through performance entries.
    pub fn open_media_page(
        &self,
        app: &AppHandle,
        profile: BrowserProfile,
        url: Url,
    ) -> Result<CheckPage, String> {
        self.open_helper_page(app, profile, url, true)
    }

    fn open_helper_page(
        &self,
        app: &AppHandle,
        profile: BrowserProfile,
        url: Url,
        capture_responses: bool,
    ) -> Result<CheckPage, String> {
        let (sender, loaded) = tokio::sync::mpsc::unbounded_channel();
        let webview = self.build_helper_view(app, profile, move |builder| {
            builder
                .on_new_window(|_url, _features| NewWindowResponse::Deny)
                .on_page_load(move |_webview, payload| {
                    if payload.event() == PageLoadEvent::Finished
                        && payload.url().as_str() != "about:blank"
                    {
                        let _ = sender.send(());
                    }
                })
        })?;
        let responses = capture_responses
            .then(|| resource_responses(&webview))
            .transpose()?
            .flatten();
        // From here on the page closes its webview on every path.
        let page = CheckPage {
            webview,
            loaded,
            responses,
        };
        mute(&page.webview);
        page.webview
            .set_bounds(tauri::Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: LogicalSize::new(CHECK_PAGE_WIDTH, CHECK_PAGE_HEIGHT).into(),
            })
            .map_err(|error| error.to_string())?;
        self.apply_ad_block_for_url(
            &page.webview,
            BrowserMode::Browse,
            &url,
            Some(url.to_string()),
        )?;
        Ok(page)
    }

    fn build_helper_view(
        &self,
        app: &AppHandle,
        profile: BrowserProfile,
        customize: impl FnOnce(WebviewBuilder<tauri::Wry>) -> WebviewBuilder<tauri::Wry>,
    ) -> Result<Webview, String> {
        fs::create_dir_all(self.profile_path(profile)).map_err(|error| error.to_string())?;
        let sequence = self.helper_sequence.fetch_add(1, Ordering::Relaxed);
        let label = format!("browser-helper-{sequence}");
        let builder = WebviewBuilder::new(&label, WebviewUrl::External(about_blank()))
            .data_directory(self.profile_path(profile));
        let builder = customize(builder);
        #[cfg(target_os = "windows")]
        let builder = {
            let builder = builder.browser_extensions_enabled(true);
            match self.ublock_path() {
                Some(path) => builder.extensions_path(path),
                None => builder,
            }
        };
        #[cfg(target_os = "macos")]
        let builder = builder.data_store_identifier(profile.data_store_identifier());
        let webview = app
            .get_window("main")
            .ok_or_else(|| "main window was not found".to_owned())?
            .add_child(
                builder,
                LogicalPosition::new(0.0, 0.0),
                LogicalSize::new(1.0, 1.0),
            )
            .map_err(|error| format!("could not create cookie helper webview: {error}"))?;
        crate::app_like::apply(&webview, crate::app_like::Role::Helper);
        webview.hide().map_err(|error| error.to_string())?;
        Ok(webview)
    }

    fn start_url_polling(self: &Arc<Self>, app: &AppHandle, webview: Webview, generation: u64) {
        let manager = Arc::clone(self);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            #[cfg(target_os = "macos")]
            let mut media_ticks = 0_u8;
            loop {
                tokio::time::sleep(URL_POLL_INTERVAL).await;
                let page_open = {
                    let session = lock_recover(&manager.session);
                    if session.generation != generation {
                        return;
                    }
                    session.page_open
                };
                if !page_open {
                    continue;
                }
                let Ok(url) = webview.url() else {
                    return;
                };
                if let Some(state) = manager.update_polled_url(generation, &url) {
                    let _ = app.emit("browser-state", state);
                }
                #[cfg(target_os = "macos")]
                {
                    media_ticks = media_ticks.wrapping_add(1);
                }
                #[cfg(target_os = "macos")]
                if media_ticks.is_multiple_of(4)
                    && let Ok(value) = execute_in(&webview, MEDIA_SCRIPT).await
                {
                    manager.observe_media_urls(generation, &value);
                }
            }
        });
    }

    fn start_resource_capture(
        self: &Arc<Self>,
        webview: Webview,
        generation: u64,
    ) -> Result<(), String> {
        let Some(mut responses) = resource_responses(&webview)? else {
            return Ok(());
        };
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            while let Some(response) = responses.recv().await {
                manager.observe_media_response(generation, response);
            }
        });
        Ok(())
    }

    fn observe_media_response(self: &Arc<Self>, generation: u64, response: ResourceResponse) {
        self.observe_media_candidate(
            generation,
            MediaCandidate {
                url: response.url,
                status: response.status,
                content_type: response.content_type,
                content_length: response.content_length,
            },
        );
    }

    #[cfg(target_os = "macos")]
    fn observe_media_urls(self: &Arc<Self>, generation: u64, value: &serde_json::Value) {
        let Some(urls) = value.as_array() else {
            return;
        };
        for url in urls.iter().filter_map(serde_json::Value::as_str) {
            self.observe_media_candidate(
                generation,
                MediaCandidate {
                    url: url.to_owned(),
                    status: 200,
                    content_type: None,
                    content_length: None,
                },
            );
        }
    }

    fn observe_media_candidate(self: &Arc<Self>, generation: u64, candidate: MediaCandidate) {
        let should_reduce = {
            let mut session = lock_recover(&self.session);
            if session.generation != generation
                || !session.page_open
                || session.played.page_url.is_none()
            {
                return;
            }
            let before = session.played.candidates.clone();
            add_candidate(&mut session.played.candidates, candidate);
            if session.played.candidates == before {
                return;
            }
            session.played.revision = session.played.revision.wrapping_add(1);
            if session.played.reducing {
                false
            } else {
                session.played.reducing = true;
                true
            }
        };
        if should_reduce {
            let manager = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                manager.reduce_played_media(generation).await;
            });
        }
    }

    async fn reduce_played_media(self: Arc<Self>, browser_generation: u64) {
        tokio::time::sleep(MEDIA_REDUCE_SETTLE).await;
        loop {
            let (page_generation, page_url, revision, candidates, mut manifests) = {
                let mut session = lock_recover(&self.session);
                if session.generation != browser_generation || !session.page_open {
                    return;
                }
                (
                    session.played.generation,
                    session.played.page_url.clone().unwrap_or_default(),
                    session.played.revision,
                    session.played.candidates.clone(),
                    std::mem::take(&mut session.played.manifests),
                )
            };
            let referer = page_url.clone();
            let reduced = tauri::async_runtime::spawn_blocking(move || {
                let media = reduce_remote_cached(&candidates, &referer, &mut manifests);
                (media, manifests)
            })
            .await;
            let Ok((media, manifests)) = reduced else {
                let mut session = lock_recover(&self.session);
                if session.generation == browser_generation
                    && session.played.generation == page_generation
                {
                    session.played.reducing = false;
                }
                return;
            };
            let repeat = {
                let mut session = lock_recover(&self.session);
                if session.generation != browser_generation
                    || session.played.generation != page_generation
                    || session.played.page_url.as_deref() != Some(page_url.as_str())
                {
                    return;
                }
                session.played.manifests = manifests;
                session.played.media = media.into_iter().map(played_media).collect();
                let repeat = session.played.revision != revision;
                if !repeat {
                    session.played.reducing = false;
                }
                repeat
            };
            if !repeat {
                return;
            }
            tokio::time::sleep(MEDIA_REDUCE_SETTLE).await;
        }
    }

    /// Adds the media file a pressed `<video>` points at to the visible page's played streams, as
    /// if its request had been seen, so it is reduced and listed like any other. Returns
    /// whether the address is one the played list can hold (an HLS or DASH manifest, an MP4,
    /// WebM or MOV file) on the page that is showing.
    pub fn observe_picked_media(self: &Arc<Self>, page_url: &str, url: &str) -> bool {
        let generation = {
            let session = lock_recover(&self.session);
            if !session.page_open
                || session.event.url.as_deref() != Some(page_url)
                || session.played.page_url.as_deref() != Some(page_url)
            {
                return false;
            }
            session.generation
        };
        self.observe_media_candidate(
            generation,
            MediaCandidate {
                url: url.to_owned(),
                status: 200,
                content_type: None,
                content_length: None,
            },
        );
        lock_recover(&self.session)
            .played
            .candidates
            .iter()
            .any(|candidate| candidate.url == url)
    }

    pub fn played_media(&self, page_url: &str) -> Vec<PlayedMedia> {
        let session = lock_recover(&self.session);
        if session.page_open && session.played.page_url.as_deref() == Some(page_url) {
            session.played.media.clone()
        } else {
            Vec::new()
        }
    }

    /// How many loads of `page_url` have finished, or None once another page is shown.
    pub fn page_loads(&self, page_url: &str) -> Option<u64> {
        let session = lock_recover(&self.session);
        (session.page_open && session.event.url.as_deref() == Some(page_url))
            .then_some(session.loads_finished)
    }

    fn update_polled_url(&self, generation: u64, url: &Url) -> Option<BrowserEventState> {
        if url.as_str() == "about:blank" {
            return None;
        }
        let url_string = url.to_string();
        let mut session = lock_recover(&self.session);
        if session.generation != generation
            || !session.page_open
            || session.event.url.as_deref() == Some(url_string.as_str())
        {
            return None;
        }
        session.event.url = Some(url_string);
        reset_played_page(&mut session.played, Some(url.to_string()));
        if session.event.mode == BrowserMode::Browse {
            session.event.site_id = lock_recover(&self.store)
                .list_site_profiles()
                .ok()
                .and_then(|profiles| {
                    let sites = profiles
                        .into_iter()
                        .filter_map(|profile| Site::from_stored(profile).ok())
                        .collect::<Vec<_>>();
                    sites::site_for_url_in(&sites, url.as_str())
                });
        }
        Some(session.event.clone())
    }

    fn page_load(
        self: &Arc<Self>,
        app: &AppHandle,
        webview: &Webview,
        generation: u64,
        url: Url,
        event: PageLoadEvent,
    ) {
        let mut session = lock_recover(&self.session);
        if session.generation != generation {
            return;
        }
        let is_blank = url.as_str() == "about:blank";
        if event == PageLoadEvent::Started
            && !is_blank
            && session.played.page_url.as_deref() != Some(url.as_str())
        {
            reset_played_page(&mut session.played, Some(url.to_string()));
        }
        session.event.url = (!is_blank).then(|| url.to_string());
        session.event.loading = event == PageLoadEvent::Started && !is_blank;
        if event == PageLoadEvent::Finished && !is_blank {
            session.loads_finished = session.loads_finished.wrapping_add(1);
        }
        if session.event.mode == BrowserMode::Browse {
            session.event.site_id = lock_recover(&self.store)
                .list_site_profiles()
                .ok()
                .and_then(|profiles| {
                    let sites = profiles
                        .into_iter()
                        .filter_map(|profile| Site::from_stored(profile).ok())
                        .collect::<Vec<_>>();
                    sites::site_for_url_in(&sites, url.as_str())
                });
        }
        let state = session.event.clone();
        // A page of a registered site has finished loading in that site's account folder:
        // read the account's cookies again, so a login made by browsing (without the
        // login banner's 完了) is recorded too.
        let refresh = match (event, state.mode, state.site_id, session.view.as_ref()) {
            (PageLoadEvent::Finished, BrowserMode::Browse, Some(site_id), Some(view)) => {
                match view.profile {
                    BrowserProfile::Account(account_id) => Some((site_id, account_id)),
                    BrowserProfile::General => None,
                }
            }
            _ => None,
        };
        let page_load_script = (event == PageLoadEvent::Finished && !is_blank)
            .then_some(session.page_load_script)
            .flatten();
        drop(session);
        if let Some(script) = page_load_script
            && let Err(error) = webview.eval(script)
        {
            eprintln!("could not inject visible-page script after load: {error}");
        }
        let _ = app.emit("browser-state", state);
        if let Some((site_id, account_id)) = refresh {
            let manager = Arc::clone(self);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                manager
                    .refresh_account_cookies(&app, site_id, account_id)
                    .await;
            });
        }
    }

    /// Updates an account's recorded cookie count after browsing, at most every few
    /// seconds per account; only a change is saved and announced.
    async fn refresh_account_cookies(&self, app: &AppHandle, site_id: i64, account_id: i64) {
        {
            let mut last = lock_recover(&self.last_refresh);
            let now = std::time::Instant::now();
            if last
                .get(&account_id)
                .is_some_and(|at| now.duration_since(*at) < std::time::Duration::from_secs(3))
            {
                return;
            }
            last.insert(account_id, now);
        }
        let site = {
            let store = lock_recover(&self.store);
            match sites::site_and_account(&store, account_id) {
                Ok((site, _)) if site.id == site_id => site,
                _ => return,
            }
        };
        let cookies = match self.cookies_for_site(app, account_id, &site.domains).await {
            Ok(cookies) => cookies,
            Err(error) => {
                eprintln!("could not read cookies for account {account_id}: {error}");
                return;
            }
        };
        let store = lock_recover(&self.store);
        match record_cookie_count(&store, account_id, cookies.len(), unix_now()) {
            Ok(false) => {}
            Ok(true) => {
                if let Ok(sites) = sites::list_sites(&store) {
                    let _ = app.emit("sites-updated", sites);
                }
            }
            Err(error) => {
                eprintln!("could not record cookies for account {account_id}: {error}");
            }
        }
    }

    /// Whether an in-app account has a browser data folder yet (it is created the first
    /// time that account's page is opened).
    pub fn has_profile(&self, account_id: i64) -> bool {
        self.profile_path(BrowserProfile::Account(account_id))
            .is_dir()
    }

    pub fn current_page_url(&self) -> Option<String> {
        let session = lock_recover(&self.session);
        session
            .page_open
            .then(|| session.event.url.clone())
            .flatten()
    }

    fn webview(&self) -> Result<Webview, String> {
        lock_recover(&self.session)
            .view
            .as_ref()
            .map(|view| view.webview.clone())
            .ok_or_else(|| "browser is not open".to_owned())
    }

    fn profile_path(&self, profile: BrowserProfile) -> PathBuf {
        self.root.join(profile.directory_name())
    }

    fn emit(&self, app: &AppHandle) {
        let state = lock_recover(&self.session).event.clone();
        let _ = app.emit("browser-state", state);
    }
}

fn reset_played_page(played: &mut PlayedPage, page_url: Option<String>) {
    played.page_url = page_url;
    played.generation = played.generation.wrapping_add(1);
    played.revision = 0;
    played.candidates.clear();
    played.media.clear();
    played.manifests = ManifestCache::default();
    played.reducing = false;
}

fn played_media(media: ReducedMedia) -> PlayedMedia {
    PlayedMedia {
        url: media.url,
        kind: media.kind.label().to_owned(),
        duration_seconds: media.duration_seconds,
    }
}

/// A page opened by [`BrowserManager::open_check_page`]; closes its webview when dropped.
pub struct CheckPage {
    webview: Webview,
    loaded: tokio::sync::mpsc::UnboundedReceiver<()>,
    responses: Option<tokio::sync::mpsc::UnboundedReceiver<ResourceResponse>>,
}

impl CheckPage {
    /// Waits until the page has finished loading (its first load other than about:blank),
    /// at most `timeout`.
    pub async fn loaded(&mut self, timeout: Duration) -> Result<(), String> {
        match tokio::time::timeout(timeout, self.loaded.recv()).await {
            Ok(Some(())) => Ok(()),
            Ok(None) => Err("the check page closed while it was loading".to_owned()),
            Err(_) => Err(format!(
                "the page did not finish loading within {} seconds",
                timeout.as_secs()
            )),
        }
    }

    /// Runs `script` in the page and returns its JSON result (see [`execute_in`]).
    pub async fn execute(&self, script: &str) -> Result<serde_json::Value, String> {
        execute_in(&self.webview, script).await
    }

    pub fn is_loaded(&mut self) -> bool {
        self.loaded.try_recv().is_ok()
    }

    pub fn take_responses(
        &mut self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<ResourceResponse>> {
        self.responses.take()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceResponse {
    pub url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
}

#[cfg(target_os = "windows")]
fn resource_responses(
    webview: &Webview,
) -> Result<Option<tokio::sync::mpsc::UnboundedReceiver<ResourceResponse>>, String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_2, ICoreWebView2WebResourceResponseReceivedEventArgs,
    };
    use webview2_com::{CoTaskMemPWSTR, WebResourceResponseReceivedEventHandler};
    use windows_core::{HSTRING, Interface, PWSTR};

    unsafe fn header(
        headers: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2HttpResponseHeaders,
        name: &str,
    ) -> Option<String> {
        let mut value = PWSTR::null();
        unsafe {
            headers.GetHeader(&HSTRING::from(name), &mut value).ok()?;
        }
        Some(CoTaskMemPWSTR::from(value).to_string())
    }

    unsafe fn read(
        args: ICoreWebView2WebResourceResponseReceivedEventArgs,
    ) -> windows_core::Result<ResourceResponse> {
        let request = unsafe { args.Request()? };
        let mut uri = PWSTR::null();
        unsafe { request.Uri(&mut uri)? };
        let response = unsafe { args.Response()? };
        let mut status = 0_i32;
        unsafe { response.StatusCode(&mut status)? };
        let headers = unsafe { response.Headers()? };
        let content_type = unsafe { header(&headers, "Content-Type") };
        let content_length = unsafe { header(&headers, "Content-Length") }
            .and_then(|value| value.trim().parse().ok());
        Ok(ResourceResponse {
            url: CoTaskMemPWSTR::from(uri).to_string(),
            status: u16::try_from(status).unwrap_or_default(),
            content_type,
            content_length,
        })
    }

    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    webview
        .with_webview(move |platform| {
            let result = unsafe {
                (|| -> windows_core::Result<()> {
                    let core = platform
                        .controller()
                        .CoreWebView2()?
                        .cast::<ICoreWebView2_2>()?;
                    let handler = WebResourceResponseReceivedEventHandler::create(Box::new(
                        move |_sender, args| {
                            if let Some(args) = args
                                && let Ok(response) = read(args)
                            {
                                let _ = sender.send(response);
                            }
                            Ok(())
                        },
                    ));
                    let mut token = 0_i64;
                    core.add_WebResourceResponseReceived(&handler, &mut token)?;
                    Ok(())
                })()
            };
            if let Err(error) = result {
                eprintln!("could not watch helper webview responses: {error}");
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(Some(receiver))
}

#[cfg(not(target_os = "windows"))]
fn resource_responses(
    _webview: &Webview,
) -> Result<Option<tokio::sync::mpsc::UnboundedReceiver<ResourceResponse>>, String> {
    Ok(None)
}

impl Drop for CheckPage {
    fn drop(&mut self) {
        if let Err(error) = self.webview.close() {
            eprintln!("could not close the check page's webview: {error}");
        }
    }
}

/// Execute JavaScript in an untrusted webview (the in-app browser's page or a check page)
/// and return WebView2's JSON-encoded result without blocking the main thread.
#[cfg(target_os = "windows")]
pub async fn execute_in(webview: &Webview, script: &str) -> Result<serde_json::Value, String> {
    use webview2_com::{CoTaskMemPWSTR, ExecuteScriptCompletedHandler};

    const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;

    let script = script.to_owned();
    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<String, String>>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let callback_sender = Arc::clone(&sender);
    webview
        .with_webview(move |platform| {
            let outcome = unsafe {
                (|| -> windows_core::Result<()> {
                    let core = platform.controller().CoreWebView2()?;
                    let handler =
                        ExecuteScriptCompletedHandler::create(Box::new(move |error, result| {
                            let result = error.map(|()| result).map_err(|error| error.to_string());
                            if let Some(sender) = lock_recover(&callback_sender).take() {
                                let _ = sender.send(result);
                            }
                            Ok(())
                        }));
                    let script = CoTaskMemPWSTR::from(script.as_str());
                    core.ExecuteScript(*script.as_ref().as_pcwstr(), &handler)
                })()
            };
            if let Err(error) = outcome
                && let Some(sender) = lock_recover(&sender).take()
            {
                let _ = sender.send(Err(error.to_string()));
            }
        })
        .map_err(|error| error.to_string())?;
    let result = tokio::time::timeout(Duration::from_secs(10), receiver)
        .await
        .map_err(|_| "browser script timed out after 10 seconds".to_owned())?
        .map_err(|_| "browser script result channel closed".to_owned())??;
    if result.len() > MAX_RESULT_BYTES {
        return Err("browser script result was too large".to_owned());
    }
    serde_json::from_str(&result)
        .map_err(|error| format!("browser script returned invalid JSON: {error}"))
}

#[cfg(target_os = "macos")]
pub async fn execute_in(webview: &Webview, script: &str) -> Result<serde_json::Value, String> {
    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{
        NSError, NSJSONSerialization, NSJSONWritingOptions, NSString, NSUTF8StringEncoding,
    };
    use objc2_web_kit::WKWebView;

    const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;

    let script = script.to_owned();
    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<String, String>>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let callback_sender = Arc::clone(&sender);
    webview
        .with_webview(move |platform| unsafe {
            // SAFETY: Tauri supplies its live WKWebView on the main thread for this closure.
            let view = &*(platform.inner() as *const WKWebView);
            // The completion handler sits inside this closure's `unsafe` block, which also covers
            // dereferencing the pointers WebKit hands it (valid for the call).
            let handler = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
                let result = if !error.is_null() {
                    Err((*error).localizedDescription().to_string())
                } else if value.is_null() {
                    Ok("null".to_owned())
                } else {
                    NSJSONSerialization::dataWithJSONObject_options_error(
                        &*value,
                        NSJSONWritingOptions::FragmentsAllowed,
                    )
                    .ok()
                    .and_then(|data| {
                        NSString::initWithData_encoding(
                            NSString::alloc(),
                            &data,
                            NSUTF8StringEncoding,
                        )
                    })
                    .map(|json| json.to_string())
                    .ok_or_else(|| "browser script result could not be encoded".to_owned())
                };
                if let Some(sender) = lock_recover(&callback_sender).take() {
                    let _ = sender.send(result);
                }
            });
            view.evaluateJavaScript_completionHandler(&NSString::from_str(&script), Some(&handler));
        })
        .map_err(|error| error.to_string())?;
    let result = tokio::time::timeout(Duration::from_secs(10), receiver)
        .await
        .map_err(|_| "browser script timed out after 10 seconds".to_owned())?
        .map_err(|_| "browser script result channel closed".to_owned())??;
    if result.len() > MAX_RESULT_BYTES {
        return Err("browser script result was too large".to_owned());
    }
    serde_json::from_str(&result)
        .map_err(|error| format!("browser script returned invalid JSON: {error}"))
}

/// Silences a hidden page that would play sound (a channel's autoplaying trailer).
#[cfg(target_os = "windows")]
fn mute(webview: &Webview) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use windows_core::Interface;

    let result = webview.with_webview(|platform| {
        // SAFETY: the WebView2 COM objects are used on the webview's own thread, which
        // with_webview runs this closure on; the call only changes a setting.
        let outcome = unsafe {
            (|| -> windows_core::Result<()> {
                platform
                    .controller()
                    .CoreWebView2()?
                    .cast::<ICoreWebView2_8>()?
                    .SetIsMuted(true)
            })()
        };
        if let Err(error) = outcome {
            eprintln!("could not mute the check page: {error}");
        }
    });
    if let Err(error) = result {
        eprintln!("could not reach the check page to mute it: {error}");
    }
}

#[cfg(not(target_os = "windows"))]
fn mute(_webview: &Webview) {}

fn ad_block_desired(
    installed: bool,
    mode: BrowserMode,
    matched_site_setting: Option<bool>,
) -> bool {
    installed && mode != BrowserMode::Login && matched_site_setting.unwrap_or(true)
}

fn is_extension_url(url: &Url) -> bool {
    matches!(url.scheme(), "chrome-extension" | "extension")
}

#[cfg(target_os = "windows")]
fn apply_ublock_state(
    webview: &Webview,
    desired: bool,
    page_url: String,
    navigate_after: Option<String>,
) -> Result<(), String> {
    use std::sync::atomic::AtomicBool;

    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2_13, ICoreWebView2BrowserExtension,
        ICoreWebView2BrowserExtensionList, ICoreWebView2Profile7,
    };
    use webview2_com::{
        BrowserExtensionEnableCompletedHandler, CoTaskMemPWSTR,
        ProfileGetBrowserExtensionsCompletedHandler,
    };
    use windows_core::{BOOL, Interface};

    fn navigate_once(
        navigated: &AtomicBool,
        webview: &Webview,
        core: Option<&ICoreWebView2>,
        url: Option<&str>,
    ) {
        let Some(url) = url else {
            return;
        };
        if navigated.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(core) = core {
            let com_url = CoTaskMemPWSTR::from(url);
            match unsafe { core.Navigate(*com_url.as_ref().as_pcwstr()) } {
                Ok(()) => return,
                Err(error) => {
                    eprintln!(
                        "could not navigate after applying uBlock Origin Lite state through WebView2: {error}"
                    );
                }
            }
        }
        match Url::parse(url) {
            Ok(url) => {
                if let Err(error) = webview.navigate(url) {
                    eprintln!(
                        "could not navigate after applying uBlock Origin Lite state through Tauri: {error}"
                    );
                }
            }
            Err(error) => {
                eprintln!(
                    "could not parse navigation URL after applying uBlock Origin Lite state: {error}"
                );
            }
        }
    }

    fn extension_name(extension: &ICoreWebView2BrowserExtension) -> windows_core::Result<String> {
        let mut output = windows_core::PWSTR::null();
        unsafe { extension.Name(&mut output)? };
        let name = CoTaskMemPWSTR::from(output);
        Ok(name.to_string())
    }

    struct ExtensionSearch {
        extension: Option<ICoreWebView2BrowserExtension>,
        count: u32,
        names: Vec<String>,
    }

    fn find_ublock(list: &ICoreWebView2BrowserExtensionList) -> Result<ExtensionSearch, String> {
        let mut count = 0;
        unsafe { list.Count(&mut count) }.map_err(|error| format!("Count failed: {error}"))?;
        let mut names = Vec::new();
        for index in 0..count {
            let extension = match unsafe { list.GetValueAtIndex(index) } {
                Ok(extension) => extension,
                Err(error) => {
                    eprintln!(
                        "could not read browser extension at index {index}: GetValueAtIndex failed: {error}"
                    );
                    continue;
                }
            };
            let name = match extension_name(&extension) {
                Ok(name) => name,
                Err(error) => {
                    eprintln!(
                        "could not read browser extension name at index {index}: Name failed: {error}"
                    );
                    continue;
                }
            };
            let is_ublock = name.to_ascii_lowercase().contains("ublock");
            names.push(name);
            if is_ublock {
                return Ok(ExtensionSearch {
                    extension: Some(extension),
                    count,
                    names,
                });
            }
        }
        Ok(ExtensionSearch {
            extension: None,
            count,
            names,
        })
    }

    let navigated = Arc::new(AtomicBool::new(false));
    if let Some(fallback_url) = navigate_after.clone() {
        let fallback_navigated = Arc::clone(&navigated);
        let fallback_webview = webview.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            if fallback_navigated.swap(true, Ordering::SeqCst) {
                return;
            }
            eprintln!("uBlock Origin Lite state not confirmed before navigation: {fallback_url}");
            match Url::parse(&fallback_url) {
                Ok(url) => {
                    if let Err(error) = fallback_webview.navigate(url) {
                        eprintln!(
                            "could not navigate through the uBlock Origin Lite fallback: {error}"
                        );
                    }
                }
                Err(error) => {
                    eprintln!("could not parse the uBlock Origin Lite fallback URL: {error}");
                }
            }
        });
    }

    let callback_navigated = Arc::clone(&navigated);
    let callback_webview = webview.clone();
    let failed_callback_navigated = Arc::clone(&navigated);
    let failed_callback_webview = webview.clone();
    let failed_callback_navigation = navigate_after.clone();
    let result = webview.with_webview(move |platform| {
        let core_result = (|| -> windows_core::Result<()> {
            let core = unsafe { platform.controller().CoreWebView2()? };
            let setup_result = (|| -> windows_core::Result<()> {
                let profile = unsafe { core.cast::<ICoreWebView2_13>()?.Profile()? };
                let profile = profile.cast::<ICoreWebView2Profile7>()?;
                let callback_core = core.clone();
                let callback_navigation = navigate_after.clone();
                let callback_page_url = page_url.clone();
                let handler_navigated = Arc::clone(&callback_navigated);
                let handler_webview = callback_webview.clone();
                let handler = ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(
                    move |result, list| {
                        let work = (|| -> Result<bool, String> {
                            result.map_err(|error| {
                                format!("GetBrowserExtensions completion failed: {error}")
                            })?;
                            let list = list.ok_or_else(|| {
                                "GetBrowserExtensions returned no extension list".to_owned()
                            })?;
                            let search = find_ublock(&list)?;
                            let Some(extension) = search.extension else {
                                let names = if search.names.is_empty() {
                                    "<none>".to_owned()
                                } else {
                                    search.names.join(", ")
                                };
                                eprintln!(
                                    "uBlock Origin Lite not found among {} browser extensions; names: {names}",
                                    search.count
                                );
                                return Ok(false);
                            };
                            let mut enabled = BOOL(0);
                            unsafe { extension.IsEnabled(&mut enabled) }
                                .map_err(|error| format!("IsEnabled failed: {error}"))?;
                            if enabled.as_bool() == desired {
                                return Ok(false);
                            }

                            let enable_core = callback_core.clone();
                            let enable_navigation = callback_navigation.clone();
                            let enable_page_url = callback_page_url.clone();
                            let enable_navigated = Arc::clone(&handler_navigated);
                            let enable_webview = handler_webview.clone();
                            let enable_handler = BrowserExtensionEnableCompletedHandler::create(
                                Box::new(move |result| {
                                    let work = (|| -> Result<(), String> {
                                        result.map_err(|error| {
                                            format!("Enable completion failed: {error}")
                                        })?;
                                        let state = if desired { "enabled" } else { "disabled" };
                                        eprintln!(
                                            "uBlock Origin Lite {state} for {enable_page_url}"
                                        );
                                        Ok(())
                                    })();
                                    if let Err(error) = work {
                                        eprintln!(
                                            "could not apply uBlock Origin Lite state in Enable handler: {error}"
                                        );
                                    }
                                    navigate_once(
                                        &enable_navigated,
                                        &enable_webview,
                                        Some(&enable_core),
                                        enable_navigation.as_deref(),
                                    );
                                    Ok(())
                                }),
                            );
                            unsafe { extension.Enable(desired, &enable_handler) }
                                .map_err(|error| format!("Enable request failed: {error}"))?;
                            Ok(true)
                        })();
                        match work {
                            Ok(true) => {}
                            Ok(false) => navigate_once(
                                &handler_navigated,
                                &handler_webview,
                                Some(&callback_core),
                                callback_navigation.as_deref(),
                            ),
                            Err(error) => {
                                eprintln!(
                                    "could not apply uBlock Origin Lite state in GetBrowserExtensions handler: {error}"
                                );
                                navigate_once(
                                    &handler_navigated,
                                    &handler_webview,
                                    Some(&callback_core),
                                    callback_navigation.as_deref(),
                                );
                            }
                        }
                        Ok(())
                    },
                ));
                unsafe { profile.GetBrowserExtensions(&handler) }
            })();
            if let Err(error) = setup_result {
                eprintln!("could not access browser extensions: {error}");
                navigate_once(
                    &callback_navigated,
                    &callback_webview,
                    Some(&core),
                    navigate_after.as_deref(),
                );
            }
            Ok(())
        })();
        if let Err(error) = core_result {
            eprintln!("could not access browser extensions: {error}");
            navigate_once(
                &callback_navigated,
                &callback_webview,
                None,
                navigate_after.as_deref(),
            );
        }
    });
    if let Err(error) = result {
        eprintln!("could not schedule browser extension access: {error}");
        navigate_once(
            &failed_callback_navigated,
            &failed_callback_webview,
            None,
            failed_callback_navigation.as_deref(),
        );
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn apply_ublock_state(
    webview: &Webview,
    _desired: bool,
    _page_url: String,
    navigate_after: Option<String>,
) -> Result<(), String> {
    if let Some(url) = navigate_after {
        let url = Url::parse(&url).map_err(|error| error.to_string())?;
        webview.navigate(url).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// What [`BrowserManager::import_cookies`] did for one account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieImport {
    /// The account already had `total` cookies for the site; nothing was added.
    AlreadyHasCookies { total: usize },
    /// `added` cookies were added; the account now has `total` cookies for the site.
    Added { added: usize, total: usize },
}

/// Records how many cookies an in-app account has for its site, the way a page load
/// does: no cookies is "not logged in". Returns whether the stored count changed.
pub fn record_cookie_count(
    store: &vids_core::store::Store,
    account_id: i64,
    cookies: usize,
    now: i64,
) -> Result<bool, String> {
    let count = (cookies > 0).then_some(cookies as u64);
    let unchanged = sites::site_and_account(store, account_id)
        .is_ok_and(|(_, account)| account.cookie_count == count);
    if unchanged {
        return Ok(false);
    }
    sites::set_account_cookie_status(store, account_id, count, count.map(|_| now), now)?;
    Ok(true)
}

fn site_urls(domains: &[String]) -> Result<Vec<(String, Url)>, String> {
    domains
        .iter()
        .map(|domain| {
            Url::parse(&format!("https://{domain}/"))
                .map(|url| (domain.clone(), url))
                .map_err(|error| format!("invalid site domain {domain}: {error}"))
        })
        .collect()
}

/// Every cookie the webview's profile sends to the site's domains (HttpOnly included),
/// read off the main thread as WebView2 requires.
async fn read_view_cookies(
    webview: Webview,
    urls: Vec<(String, Url)>,
) -> Result<Vec<CookieRecord>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut unique = BTreeMap::new();
        for (requested_domain, url) in urls {
            let cookies = webview
                .cookies_for_url(url)
                .map_err(|error| error.to_string())?;
            for cookie in cookies {
                let record = CookieRecord::from_cookie(cookie, &requested_domain);
                unique.insert(
                    (
                        record.name.clone(),
                        record.domain.clone(),
                        record.path.clone(),
                    ),
                    record,
                );
            }
        }
        Ok::<_, String>(unique.into_values().collect())
    })
    .await
    .map_err(|error| format!("cookie reader task failed: {error}"))?
}

/// The SameSite an imported cookie gets in WebView2. Chromium refuses SameSite=None
/// without Secure, and Firefox can store None for a cookie that stated nothing (its
/// default is not Lax), so such a cookie gets Lax: what Chromium gives a cookie that
/// states nothing.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn chromium_same_site(
    same_site: vids_core::firefox::SameSite,
    secure: bool,
) -> vids_core::firefox::SameSite {
    use vids_core::firefox::SameSite;
    match same_site {
        SameSite::None if !secure => SameSite::Lax,
        other => other,
    }
}

/// Adds cookies to the webview's profile through WebView2's cookie manager; returns how
/// many were accepted. A cookie WebView2 refuses is left out (and logged without its
/// value).
#[cfg(target_os = "windows")]
async fn add_view_cookies(
    webview: &Webview,
    cookies: Vec<vids_core::firefox::Cookie>,
) -> Result<usize, String> {
    use vids_core::firefox::SameSite;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX, COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE,
        COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT, ICoreWebView2_2,
    };
    use windows_core::{HSTRING, Interface};

    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<usize, String>>();
    webview
        .with_webview(move |platform| {
            // SAFETY: the WebView2 COM objects are used on the webview's own thread, which
            // with_webview runs this closure on.
            let outcome = unsafe {
                (|| -> windows_core::Result<usize> {
                    let core = platform.controller().CoreWebView2()?;
                    let manager = core.cast::<ICoreWebView2_2>()?.CookieManager()?;
                    let mut added = 0;
                    for cookie in &cookies {
                        let same_site = match chromium_same_site(cookie.same_site, cookie.secure) {
                            SameSite::None => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE,
                            SameSite::Lax => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX,
                            SameSite::Strict => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT,
                        };
                        let result = (|| -> windows_core::Result<()> {
                            let created = manager.CreateCookie(
                                &HSTRING::from(cookie.name.as_str()),
                                &HSTRING::from(cookie.value.as_str()),
                                &HSTRING::from(cookie.host.as_str()),
                                &HSTRING::from(cookie.path.as_str()),
                            )?;
                            // A session cookie keeps WebView2's default (no expiry).
                            if let Some(expires) = cookie.expires {
                                created.SetExpires(expires as f64)?;
                            }
                            created.SetIsHttpOnly(cookie.http_only)?;
                            created.SetIsSecure(cookie.secure)?;
                            created.SetSameSite(same_site)?;
                            manager.AddOrUpdateCookie(&created)
                        })();
                        match result {
                            Ok(()) => added += 1,
                            Err(error) => eprintln!(
                                "could not import cookie {} for {}: {error}",
                                cookie.name, cookie.host
                            ),
                        }
                    }
                    Ok(added)
                })()
            };
            let _ = sender.send(outcome.map_err(|error| error.to_string()));
        })
        .map_err(|error| error.to_string())?;
    tokio::time::timeout(Duration::from_secs(30), receiver)
        .await
        .map_err(|_| "adding cookies timed out after 30 seconds".to_owned())?
        .map_err(|_| "cookie import result channel closed".to_owned())?
}

#[cfg(not(target_os = "windows"))]
async fn add_view_cookies(
    _webview: &Webview,
    _cookies: Vec<vids_core::firefox::Cookie>,
) -> Result<usize, String> {
    Err("not available on this system yet".to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieRecord {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    pub http_only: bool,
    pub secure: bool,
    pub expires: Option<i64>,
}

impl CookieRecord {
    fn from_cookie(cookie: tauri::webview::Cookie<'static>, fallback_domain: &str) -> Self {
        Self {
            domain: cookie.domain().unwrap_or(fallback_domain).to_owned(),
            path: cookie.path().unwrap_or("/").to_owned(),
            name: cookie.name().to_owned(),
            value: cookie.value().to_owned(),
            http_only: cookie.http_only().unwrap_or(false),
            secure: cookie.secure().unwrap_or(false),
            expires: cookie.expires_datetime().map(|date| date.unix_timestamp()),
        }
    }
}

pub fn netscape_cookie_file(cookies: &[CookieRecord]) -> String {
    let mut output = "# Netscape HTTP Cookie File\n".to_owned();
    for cookie in cookies {
        let domain = cookie.domain.trim_start_matches('.');
        if cookie.http_only {
            output.push_str("#HttpOnly_");
        }
        output.push('.');
        output.push_str(domain);
        output.push_str("\tTRUE\t");
        output.push_str(if cookie.path.is_empty() {
            "/"
        } else {
            &cookie.path
        });
        output.push('\t');
        output.push_str(if cookie.secure { "TRUE" } else { "FALSE" });
        output.push('\t');
        output.push_str(&cookie.expires.unwrap_or(0).to_string());
        output.push('\t');
        output.push_str(&cookie.name);
        output.push('\t');
        output.push_str(&cookie.value);
        output.push('\n');
    }
    output
}

pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Err(error) = fs::write(path, bytes) {
        let _ = fs::remove_file(path);
        return Err(format!("could not write {}: {error}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(path);
            return Err(format!("could not protect {}: {error}", path.display()));
        }
    }
    Ok(())
}

fn about_blank() -> Url {
    Url::parse("about:blank").expect("about:blank is a valid URL")
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
    use super::*;

    #[test]
    fn netscape_writer_formats_domain_http_only_session_and_tabs() {
        let text = netscape_cookie_file(&[
            CookieRecord {
                domain: ".example.com".to_owned(),
                path: "/account".to_owned(),
                name: "sid".to_owned(),
                value: "secret".to_owned(),
                http_only: true,
                secure: true,
                expires: None,
            },
            CookieRecord {
                domain: "sub.example.com".to_owned(),
                path: "/".to_owned(),
                name: "theme".to_owned(),
                value: "dark".to_owned(),
                http_only: false,
                secure: false,
                expires: Some(123),
            },
        ]);
        assert!(text.starts_with("# Netscape HTTP Cookie File\n"));
        assert!(text.contains("#HttpOnly_.example.com\tTRUE\t/account\tTRUE\t0\tsid\tsecret\n"));
        assert!(text.contains(".sub.example.com\tTRUE\t/\tFALSE\t123\ttheme\tdark\n"));
    }

    #[test]
    fn imported_same_site_none_needs_secure() {
        use vids_core::firefox::SameSite;
        assert_eq!(chromium_same_site(SameSite::None, true), SameSite::None);
        assert_eq!(chromium_same_site(SameSite::None, false), SameSite::Lax);
        assert_eq!(chromium_same_site(SameSite::Lax, false), SameSite::Lax);
        assert_eq!(
            chromium_same_site(SameSite::Strict, false),
            SameSite::Strict
        );
        assert_eq!(chromium_same_site(SameSite::Strict, true), SameSite::Strict);
    }

    #[test]
    fn cookie_count_is_recorded_only_when_it_changes() {
        let store = vids_core::store::Store::open_in_memory().unwrap();
        let site = sites::add_site(&store, "https://example.com/login", 1).unwrap();
        let account = site.accounts[0].id;

        assert!(!record_cookie_count(&store, account, 0, 5).unwrap());
        assert!(record_cookie_count(&store, account, 3, 6).unwrap());
        let (_, stored) = sites::site_and_account(&store, account).unwrap();
        assert_eq!(stored.cookie_count, Some(3));
        assert_eq!(stored.updated_at, Some(6));
        assert!(!record_cookie_count(&store, account, 3, 7).unwrap());
        assert!(record_cookie_count(&store, account, 0, 8).unwrap());
        let (_, stored) = sites::site_and_account(&store, account).unwrap();
        assert_eq!(stored.cookie_count, None);
        assert_eq!(stored.updated_at, None);
    }

    #[test]
    fn browser_bounds_reject_invalid_geometry() {
        assert!(
            BrowserBounds {
                width: 0.0,
                ..BrowserBounds::default()
            }
            .validated()
            .is_err()
        );
        assert!(
            BrowserBounds {
                x: f64::NAN,
                ..BrowserBounds::default()
            }
            .validated()
            .is_err()
        );
    }

    #[test]
    fn polled_url_updates_only_the_open_current_generation() {
        let manager = BrowserManager::new(
            PathBuf::from("browser-test"),
            Arc::new(Mutex::new(
                vids_core::store::Store::open_in_memory().unwrap(),
            )),
            Arc::new(ToolManager::new(PathBuf::from("tools-test"))),
        );
        {
            let mut session = lock_recover(&manager.session);
            session.generation = 4;
            session.page_open = true;
            session.event.url = Some("https://example.test/old".to_owned());
        }
        let next = Url::parse("https://example.test/new").unwrap();

        assert!(manager.update_polled_url(3, &next).is_none());
        let state = manager.update_polled_url(4, &next).unwrap();
        assert_eq!(state.url.as_deref(), Some("https://example.test/new"));
        assert!(manager.update_polled_url(4, &next).is_none());

        lock_recover(&manager.session).page_open = false;
        let hidden = Url::parse("https://example.test/hidden").unwrap();
        assert!(manager.update_polled_url(4, &hidden).is_none());
    }

    #[test]
    fn desired_ad_block_state_covers_install_mode_and_site_setting() {
        assert!(!ad_block_desired(false, BrowserMode::Browse, None));
        assert!(!ad_block_desired(false, BrowserMode::Browse, Some(true)));
        assert!(!ad_block_desired(true, BrowserMode::Login, None));
        assert!(!ad_block_desired(true, BrowserMode::Login, Some(true)));
        assert!(ad_block_desired(true, BrowserMode::Browse, None));
        assert!(ad_block_desired(true, BrowserMode::Browse, Some(true)));
        assert!(!ad_block_desired(true, BrowserMode::Browse, Some(false)));
    }

    #[test]
    fn extension_pages_are_recognized_for_new_window_refusal() {
        assert!(is_extension_url(
            &Url::parse("chrome-extension://abc/dashboard.html").unwrap()
        ));
        assert!(is_extension_url(
            &Url::parse("extension://abc/dashboard.html").unwrap()
        ));
        assert!(!is_extension_url(
            &Url::parse("https://example.com/").unwrap()
        ));
    }
}

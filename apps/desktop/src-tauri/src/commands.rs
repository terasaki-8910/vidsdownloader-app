use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::broadcast;
use vids_core::args::{Cookies, Embed};
use vids_core::comments::AssSettings;
use vids_core::executor::{JobOptions, PageMedia, PageMediaChoice};
use vids_core::naming::{NameTemplate, OptionalPart, Part, Token};
use vids_core::page_images::{PageImage, PageImages, site_from_url};
use vids_core::queue::{JobExecutor, JobId, Queue, QueueError, QueueEvent, RemovalPreparation};
use vids_core::store::{LibraryQuery, Store};
use vids_core::tools::{
    Arch, Channel, FFMPEG_MACOS_VERSION, FfmpegBuild, InstallProgress, ToolManager, ToolSource,
    deno_source, ffmpeg_macos_source, ffmpeg_source, gallery_dl_source, ublock_lite_source,
    yt_dlp_source,
};

use crate::library_commands::finish_erasing_job_records;
use crate::media::job_file_path;
use crate::models::{
    JobRemoved, JobRow, LibraryRow, Preset, Settings, ToolId, ToolPhase, ToolProgress, ToolStatus,
};
use crate::sites;
use crate::state::{AppState, ShellExecutor, TARGET_OS, lock_recover};

const TOOL_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const JOB_PROGRESS_INTERVAL: Duration = Duration::from_millis(150);
const YT_DLP_VERSION: &str = "2026.10.26";
const FFMPEG_BUILD: FfmpegBuild = FfmpegBuild {
    release_tag: "autobuild-2026-10-31-18-18",
    asset: "ffmpeg-N-128251-g6d1bfd7609-win64-gpl-shared.zip",
    version: "2026-10-31",
};
const DENO_VERSION: &str = "2.9.8";
const UBLOCK_LITE_VERSION: &str = "2026.1026.1812";
const GALLERY_DL_VERSION: &str = "2026.11.03";
/// Bounds on what a page-images request may carry (the collector's own limits).
const MAX_PAGE_IMAGES: usize = 2_000;
const MAX_URL_CHARS: usize = 2_048;
const MAX_PAGE_TEXT_CHARS: usize = 300;
const MAX_PAGE_ID_CHARS: usize = 200;
const MAX_PAGE_MEDIA: usize = 20;

#[cfg(target_os = "windows")]
const MANAGED_TOOLS: &[ToolId] = &[
    ToolId::YtDlp,
    ToolId::Ffmpeg,
    ToolId::Deno,
    ToolId::GalleryDl,
    ToolId::UblockLite,
];
#[cfg(target_os = "macos")]
const MANAGED_TOOLS: &[ToolId] = &[
    ToolId::YtDlp,
    ToolId::Ffmpeg,
    ToolId::Deno,
    ToolId::GalleryDl,
];

const fn optional_tool(tool: ToolId) -> bool {
    matches!(tool, ToolId::GalleryDl | ToolId::UblockLite)
}

struct InstallationGuard(Arc<AtomicBool>);

impl Drop for InstallationGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn queue_snapshot(state: State<'_, AppState>) -> Result<Vec<JobRow>, String> {
    queue_rows(&state.queue, &state.store)
}

fn queue_rows<E: JobExecutor>(
    queue: &Queue<E>,
    store: &Mutex<Store>,
) -> Result<Vec<JobRow>, String> {
    let views = queue.snapshot();
    let (library_ids, media_choices) = {
        let store = lock_recover(store);
        let library_ids = store
            .latest_library_ids_by_job()
            .map_err(|error| error.to_string())?;
        let media_choices = store
            .list_jobs(vids_core::store::JobFilter::All)
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter_map(|job| {
                let choices = job.options_json.get("page_media_choices")?.clone();
                let choices = serde_json::from_value::<Vec<PageMediaChoice>>(choices).ok()?;
                (!choices.is_empty()).then_some((job.id, choices))
            })
            .collect::<HashMap<_, _>>();
        (library_ids, media_choices)
    };
    Ok(views
        .into_iter()
        .map(|view| {
            let library_id = i64::try_from(view.id.0)
                .ok()
                .and_then(|job_id| library_ids.get(&job_id).copied());
            let mut row = JobRow::from_view(view, library_id);
            row.page_media_choices = i64::try_from(row.id)
                .ok()
                .and_then(|id| media_choices.get(&id).cloned())
                .unwrap_or_default();
            row
        })
        .collect())
}

#[tauri::command]
pub async fn enqueue_urls(
    urls: Vec<String>,
    preset: Preset,
    tz_offset_seconds: i64,
    state: State<'_, AppState>,
) -> Result<Vec<u64>, String> {
    let settings = lock_recover(&state.settings).clone();
    let options = job_options(&settings, preset, tz_offset_seconds);
    let options = serde_json::to_value(options)
        .map_err(|error| format!("could not serialize job options: {error}"))?;

    Ok(filter_enqueue_urls(&urls)
        .into_iter()
        .map(|url| state.queue.enqueue(url, options.clone()).0)
        .collect())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageMediaArg {
    pub media_url: String,
    pub page_url: String,
    pub title: String,
}

/// Queues streams already found in the visible browser. The job URL remains the page URL,
/// while `page_media` sends yt-dlp straight to the stream with that page as Referer.
#[tauri::command]
pub async fn enqueue_page_media(
    items: Vec<PageMediaArg>,
    preset: Preset,
    tz_offset_seconds: i64,
    state: State<'_, AppState>,
) -> Result<Vec<u64>, String> {
    if items.is_empty() {
        return Err("no page media were chosen".to_owned());
    }
    if items.len() > MAX_PAGE_MEDIA {
        return Err(format!(
            "at most {MAX_PAGE_MEDIA} page media can be queued at once"
        ));
    }
    let settings = lock_recover(&state.settings).clone();
    let mut queued = Vec::with_capacity(items.len());
    let mut seen = HashSet::new();
    for item in items {
        let facts = state.collector.page_facts(item.page_url.trim());
        let (page_url, options) =
            page_media_options(&settings, preset, tz_offset_seconds, item, facts.as_ref())?;
        let media_url = options
            .page_media
            .as_ref()
            .expect("page media options contain media")
            .url
            .clone();
        if !seen.insert((page_url.clone(), media_url)) {
            continue;
        }
        let options = serde_json::to_value(options)
            .map_err(|error| format!("could not serialize job options: {error}"))?;
        queued.push(state.queue.enqueue(page_url, options).0);
    }
    if queued.is_empty() {
        return Err("no distinct page media were chosen".to_owned());
    }
    Ok(queued)
}

fn page_media_options(
    settings: &Settings,
    preset: Preset,
    tz_offset_seconds: i64,
    item: PageMediaArg,
    facts: Option<&crate::collector::CollectPage>,
) -> Result<(String, JobOptions), String> {
    let page_url = item.page_url.trim().to_owned();
    let media_url = item.media_url.trim().to_owned();
    if !is_http_url(&page_url) {
        return Err("pageUrl must be an http(s) URL".to_owned());
    }
    if media_url.chars().count() > MAX_URL_CHARS || !is_http_url(&media_url) {
        return Err("mediaUrl must be a bounded http(s) URL".to_owned());
    }
    let title = item
        .title
        .trim()
        .chars()
        .take(MAX_PAGE_TEXT_CHARS)
        .collect::<String>();
    let mut options = job_options(settings, preset, tz_offset_seconds);
    options.page_media = Some(PageMedia {
        url: media_url,
        referer: page_url.clone(),
        title: (!title.is_empty()).then_some(title),
        uploader: facts.and_then(|page| page.uploader.clone()),
        published_at: facts.and_then(|page| page.published_at),
    });
    Ok((page_url, options))
}

/// One image chosen in the tray: its URL and its number on the page.
#[derive(Debug, Clone, Deserialize)]
pub struct PageImageArg {
    pub url: String,
    pub index: u32,
}

/// Queues one job for the page that saves the chosen images directly (no yt-dlp or
/// gallery-dl), named and filed like any other job from the current settings. The site is
/// the registered site matching the page, else the page's host without `www.`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn enqueue_page_images(
    page_url: String,
    title: Option<String>,
    uploader: Option<String>,
    published_at: Option<i64>,
    id: Option<String>,
    images: Vec<PageImageArg>,
    tz_offset_seconds: Option<i64>,
    state: State<'_, AppState>,
) -> Result<u64, String> {
    let page_url = page_url.trim().to_owned();
    if !is_http_url(&page_url) {
        return Err("pageUrl must be an http(s) URL".to_owned());
    }
    let page = page_images(title, uploader, published_at, id, images)?;
    let site_name = {
        let store = lock_recover(&state.store);
        let sites = sites::list_sites(&store)?;
        sites::site_for_url_in(&sites, &page_url)
            .and_then(|site_id| sites.into_iter().find(|site| site.id == site_id))
            .map(|site| site.name)
    }
    .unwrap_or_else(|| site_from_url(&page_url));
    let settings = lock_recover(&state.settings).clone();
    let mut options = job_options(
        &settings,
        settings.default_preset,
        tz_offset_seconds.unwrap_or(0),
    );
    options.site_name = Some(site_name);
    options.page_images = Some(page);
    let options = serde_json::to_value(options)
        .map_err(|error| format!("could not serialize job options: {error}"))?;
    Ok(state.queue.enqueue(page_url, options).0)
}

/// Checks and bounds a page-images request: http(s) image URLs, numbers from 1, each URL
/// once, at most the collector's item cap; empty texts count as absent.
fn page_images(
    title: Option<String>,
    uploader: Option<String>,
    published_at: Option<i64>,
    id: Option<String>,
    images: Vec<PageImageArg>,
) -> Result<PageImages, String> {
    let mut seen = HashSet::new();
    let mut chosen = Vec::new();
    for image in images {
        let url = image.url.trim();
        if url.chars().count() > MAX_URL_CHARS || !is_http_url(url) || image.index == 0 {
            return Err(format!("not a page image: {url} (#{})", image.index));
        }
        if seen.insert(url.to_owned()) {
            chosen.push(PageImage {
                url: url.to_owned(),
                index: image.index,
            });
        }
    }
    if chosen.is_empty() {
        return Err("no images were chosen".to_owned());
    }
    if chosen.len() > MAX_PAGE_IMAGES {
        return Err(format!(
            "at most {MAX_PAGE_IMAGES} images can be saved at once"
        ));
    }
    let text = |value: Option<String>| {
        value
            .map(|value| {
                value
                    .trim()
                    .chars()
                    .take(MAX_PAGE_TEXT_CHARS)
                    .collect::<String>()
            })
            .filter(|value| !value.is_empty())
    };
    let id = id
        .map(|id| id.trim().to_owned())
        .filter(|id| (1..=MAX_PAGE_ID_CHARS).contains(&id.chars().count()));
    Ok(PageImages {
        title: text(title),
        uploader: text(uploader),
        published_at,
        id,
        images: chosen,
    })
}

/// A job's options from the current settings, shared by URL jobs, page-image jobs and the
/// items sources queue.
pub(crate) fn job_options(
    settings: &Settings,
    preset: Preset,
    tz_offset_seconds: i64,
) -> JobOptions {
    JobOptions {
        dest_dir: PathBuf::from(&settings.download_dir),
        template: default_name_template(),
        tz_offset_seconds: clamp_tz_offset_seconds(tz_offset_seconds),
        site_name: None,
        preset: preset.into(),
        avoid_av1: settings.avoid_av1,
        embed: Embed {
            subtitles: true,
            subtitle_langs: vec!["ja".to_owned(), "en".to_owned()],
            keep_subtitle_files: settings.keep_subtitle_files,
            thumbnail: true,
            metadata: true,
            chapters: true,
        },
        comments: true,
        ass: AssSettings::default(),
        cookies: Cookies::None,
        filter: None,
        playlist_item: None,
        force_save: false,
        page_images: None,
        page_media: None,
        page_media_choices: Vec::new(),
        cookie_domains: Vec::new(),
    }
}

fn is_http_url(value: &str) -> bool {
    tauri::Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

#[tauri::command]
pub async fn cancel_job(id: u64, state: State<'_, AppState>) -> Result<(), String> {
    state
        .queue
        .cancel(JobId(id))
        .map_err(|error| error.to_string())
}

/// Removes eligible rows from Downloads, preserving saved files and library rows.
#[tauri::command]
pub async fn jobs_remove(ids: Vec<u64>, state: State<'_, AppState>) -> Result<(), String> {
    let removed = remove_download_rows(&state.queue, &ids).await?;
    if !removed.is_empty() {
        let removed: Vec<i64> = removed
            .into_iter()
            .filter_map(|id| i64::try_from(id.0).ok())
            .collect();
        finish_erasing_job_records(Arc::clone(&state.store), &state.work_dir, &removed).await;
    }
    Ok(())
}

async fn remove_download_rows<E: JobExecutor>(
    queue: &Queue<E>,
    ids: &[u64],
) -> Result<Vec<JobId>, String> {
    let mut events = queue.subscribe();
    let mut seen = HashSet::new();
    let mut removable = Vec::new();
    let mut waiting = HashSet::new();

    for id in ids.iter().copied().map(JobId) {
        if !seen.insert(id) {
            continue;
        }
        match queue.prepare_for_removal(id) {
            Ok(RemovalPreparation::Kept) => {}
            Ok(RemovalPreparation::Removable) => removable.push(id),
            Ok(RemovalPreparation::Waiting) => {
                waiting.insert(id);
            }
            Err(QueueError::UnknownJob(_)) => {}
            Err(error) => return Err(error.to_string()),
        }
    }

    while !waiting.is_empty() {
        match events.recv().await {
            Ok(QueueEvent::Finished { id, .. }) if waiting.remove(&id) => removable.push(id),
            Ok(QueueEvent::Removed { id }) => {
                waiting.remove(&id);
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let states: HashMap<JobId, _> = queue
                    .snapshot()
                    .into_iter()
                    .map(|job| (job.id, job.state))
                    .collect();
                let resolved: Vec<(JobId, bool)> = waiting
                    .iter()
                    .filter_map(|id| match states.get(id) {
                        Some(state) if state.is_terminal() => Some((*id, true)),
                        None => Some((*id, false)),
                        Some(_) => None,
                    })
                    .collect();
                for (id, still_present) in resolved {
                    waiting.remove(&id);
                    if still_present {
                        removable.push(id);
                    }
                }
            }
            Err(broadcast::error::RecvError::Closed) => {
                return Err("the queue stopped while rows were being canceled".to_owned());
            }
        }
    }

    queue
        .remove_finished(&removable)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn retry_job(id: u64, state: State<'_, AppState>) -> Result<(), String> {
    state
        .queue
        .retry(JobId(id))
        .map_err(|error| error.to_string())
}

/// Resolves a page-media tie with one of the choices persisted for that job, then queues
/// the next attempt exactly like a normal retry.
#[tauri::command]
pub async fn page_media_choose(
    job_id: u64,
    url: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let stored = lock_recover(&state.store)
        .get_job(i64::try_from(job_id).map_err(|_| "jobId is too large".to_owned())?)
        .map_err(|error| error.to_string())?;
    let mut options: JobOptions = serde_json::from_value(stored.options_json)
        .map_err(|error| format!("could not read job options: {error}"))?;
    options = selected_page_media_options(options, &url)?;
    let options = serde_json::to_value(options)
        .map_err(|error| format!("could not store page media choice: {error}"))?;
    state
        .queue
        .retry_with_options(JobId(job_id), options)
        .map_err(|error| error.to_string())
}

fn selected_page_media_options(mut options: JobOptions, url: &str) -> Result<JobOptions, String> {
    if !options
        .page_media_choices
        .iter()
        .any(|choice| choice.url == url)
    {
        return Err("url is not one of this job's page media choices".to_owned());
    }
    let media = options
        .page_media
        .as_mut()
        .ok_or_else(|| "the job has no stored page media facts".to_owned())?;
    media.url = url.to_owned();
    options.page_media_choices.clear();
    options.cookies = Cookies::None;
    options.cookie_domains.clear();
    Ok(options)
}

#[tauri::command]
pub async fn save_again(id: u64, state: State<'_, AppState>) -> Result<(), String> {
    state
        .queue
        .save_again(JobId(id))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn read_clipboard_url(app: AppHandle) -> Result<Option<String>, String> {
    let clipboard = tauri::async_runtime::spawn_blocking(move || app.clipboard().read_text()).await;
    let text = match clipboard {
        Ok(Ok(text)) => text,
        Ok(Err(_)) | Err(_) => return Ok(None),
    };
    Ok(extract_clipboard_url(&text))
}

#[tauri::command]
pub async fn copy_text(text: String, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || app.clipboard().write_text(text))
        .await
        .map_err(|error| format!("clipboard task failed: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn library_search(
    text: Option<String>,
    limit: i64,
    offset: i64,
    state: State<'_, AppState>,
) -> Result<Vec<LibraryRow>, String> {
    let query = LibraryQuery {
        text: text.filter(|value| !value.is_empty()),
        site: None,
        published_from: None,
        published_to: None,
        include_missing: true,
        limit: limit.clamp(1, 500) as u32,
        offset: offset.clamp(0, i64::from(u32::MAX)) as u32,
    };
    lock_recover(&state.store)
        .search_library(&query)
        .map(|items| items.into_iter().map(Into::into).collect())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn reveal_path(path: String, app: AppHandle) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err(format!("path does not exist: {}", path.display()));
    }
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn open_job_file(
    id: u64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let path = job_file_path(&state.store, id).ok_or_else(|| "file not found".to_owned())?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn job_file_exists(id: u64, state: State<'_, AppState>) -> bool {
    job_file_path(&state.store, id).is_some()
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(lock_recover(&state.settings).clone())
}

#[tauri::command]
pub async fn set_settings(settings: Settings, state: State<'_, AppState>) -> Result<(), String> {
    let settings = settings.validated()?;
    lock_recover(&state.store)
        .set_setting("app", &settings)
        .map_err(|error| error.to_string())?;
    *lock_recover(&state.settings) = settings.clone();
    state.queue.set_concurrency(settings.concurrency as usize);
    Ok(())
}

#[tauri::command]
pub async fn pick_download_dir(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let current = PathBuf::from(&lock_recover(&state.settings).download_dir);
    pick_folder_from(app, window, current).await
}

/// The same folder picker as `pick_download_dir` (a source's destination, say); settings
/// are not touched.
#[tauri::command]
pub async fn pick_folder(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let current = PathBuf::from(&lock_recover(&state.settings).download_dir);
    pick_folder_from(app, window, current).await
}

/// The native folder picker, owned by `window`, starting in `current` when it exists.
/// Returns an absolute path, or `None` when the user cancels.
async fn pick_folder_from(
    app: AppHandle,
    window: tauri::Window,
    current: PathBuf,
) -> Result<Option<String>, String> {
    let selected = tauri::async_runtime::spawn_blocking(move || {
        // Owned by the app window, so it opens over it and stays in front of it.
        let mut dialog = app.dialog().file().set_parent(&window);
        if current.exists() {
            dialog = dialog.set_directory(current);
        }
        dialog
            .blocking_pick_folder()
            .map(|path| path.into_path().map_err(|error| error.to_string()))
            .transpose()
    })
    .await
    .map_err(|error| format!("folder picker task failed: {error}"))??;

    selected
        .map(|path| {
            if !path.is_absolute() {
                return Err("folder picker returned a relative path".to_owned());
            }
            Ok(path.to_string_lossy().into_owned())
        })
        .transpose()
}

#[tauri::command]
pub async fn tools_status(state: State<'_, AppState>) -> Result<Vec<ToolStatus>, String> {
    Ok(MANAGED_TOOLS
        .iter()
        .copied()
        .map(|tool| {
            let resolved = state.tools.resolve(tool.core(), TARGET_OS);
            ToolStatus {
                tool,
                installed: resolved.is_some(),
                version: resolved.map(|resolved| resolved.version),
                expected_version: expected_version(tool).to_owned(),
            }
        })
        .collect())
}

#[tauri::command]
pub async fn install_tools(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state
        .installation_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("installation already running".to_owned());
    }

    let tools = Arc::clone(&state.tools);
    let installation_running = Arc::clone(&state.installation_running);
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = InstallationGuard(installation_running);
        install_missing_tools(&tools, &app)
    })
    .await;

    match result {
        Ok(result) => result,
        Err(error) => Err(format!("tool installation task failed: {error}")),
    }
}

/// Existing installs receive newly added optional tools at launch. A first run waits for
/// the user's setup action, which attempts optional tools alongside the required ones.
pub fn install_optional_tools_in_background(app: &AppHandle) {
    let state = app.state::<AppState>();
    let others_ready = MANAGED_TOOLS
        .iter()
        .filter(|tool| !optional_tool(**tool))
        .all(|tool| state.tools.resolve(tool.core(), TARGET_OS).is_some());
    let wanted = MANAGED_TOOLS
        .iter()
        .copied()
        .any(|tool| optional_tool(tool) && state.tools.resolve(tool.core(), TARGET_OS).is_none());
    if !others_ready
        || !wanted
        || state
            .installation_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }
    let tools = Arc::clone(&state.tools);
    let installation_running = Arc::clone(&state.installation_running);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = InstallationGuard(installation_running);
        if let Err(error) = install_missing_tools(&tools, &app) {
            eprintln!("could not install optional tools: {error}");
        }
    });
}

pub async fn forward_queue_events(
    queue: Queue<ShellExecutor>,
    store: Arc<Mutex<Store>>,
    app: AppHandle,
    mut events: broadcast::Receiver<QueueEvent>,
) {
    let mut last_flood_emit = HashMap::<JobId, Instant>::new();
    let mut pending = HashMap::<JobId, Instant>::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(25));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            received = events.recv() => match received {
                Ok(QueueEvent::Removed { id }) => {
                    pending.remove(&id);
                    last_flood_emit.remove(&id);
                    let _ = app.emit("job-removed", JobRemoved { id: id.0 });
                }
                Ok(event) => {
                    let id = event_job_id(&event);
                    if matches!(event, QueueEvent::Progress { .. } | QueueEvent::Transfer { .. }) {
                        let now = Instant::now();
                        let may_emit = last_flood_emit
                            .get(&id)
                            .is_none_or(|last| now.duration_since(*last) >= JOB_PROGRESS_INTERVAL);
                        if may_emit {
                            emit_job_row(&queue, &store, &app, id);
                            last_flood_emit.insert(id, now);
                            pending.remove(&id);
                        } else if let Some(last) = last_flood_emit.get(&id) {
                            pending.insert(id, *last + JOB_PROGRESS_INTERVAL);
                        }
                    } else {
                        emit_job_row(&queue, &store, &app, id);
                        pending.remove(&id);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    match queue_rows(&queue, &store) {
                        Ok(rows) => for row in rows {
                            let _ = app.emit("job-updated", row);
                        },
                        Err(error) => eprintln!("could not refresh queue rows: {error}"),
                    }
                    pending.clear();
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = ticker.tick() => {
                let now = Instant::now();
                let due: Vec<JobId> = pending
                    .iter()
                    .filter_map(|(&id, &deadline)| (deadline <= now).then_some(id))
                    .collect();
                for id in due {
                    pending.remove(&id);
                    emit_job_row(&queue, &store, &app, id);
                    last_flood_emit.insert(id, now);
                }
            }
        }
    }
}

fn emit_job_row(queue: &Queue<ShellExecutor>, store: &Mutex<Store>, app: &AppHandle, id: JobId) {
    let Some(view) = queue.snapshot().into_iter().find(|job| job.id == id) else {
        return;
    };
    let library_id = match i64::try_from(id.0) {
        Ok(job_id) => match lock_recover(store).latest_library_id_for_job(job_id) {
            Ok(library_id) => library_id,
            Err(error) => {
                eprintln!("could not read library id for queue job {id}: {error}");
                return;
            }
        },
        Err(_) => None,
    };
    let _ = app.emit("job-updated", JobRow::from_view(view, library_id));
}

fn event_job_id(event: &QueueEvent) -> JobId {
    match event {
        QueueEvent::JobAdded { id, .. }
        | QueueEvent::Started { id, .. }
        | QueueEvent::StateChanged { id, .. }
        | QueueEvent::Progress { id, .. }
        | QueueEvent::Phase { id, .. }
        | QueueEvent::Details { id, .. }
        | QueueEvent::Transfer { id, .. }
        | QueueEvent::TransitionRejected { id, .. }
        | QueueEvent::Finished { id, .. }
        | QueueEvent::OutputPathChanged { id, .. }
        | QueueEvent::Removed { id } => *id,
    }
}

fn install_missing_tools(tools: &ToolManager, app: &AppHandle) -> Result<(), String> {
    let mut failures = Vec::new();
    for tool in MANAGED_TOOLS.iter().copied() {
        let resolved = tools.resolve(tool.core(), TARGET_OS);
        if !tool_needs_install(
            resolved.as_ref().map(|tool| tool.version.as_str()),
            expected_version(tool),
        ) {
            continue;
        }
        let source = match source_for(tool) {
            Ok(source) => source,
            Err(message) => {
                emit_tool_progress(app, tool, ToolPhase::Failed, 0, None, Some(message.clone()));
                if !optional_tool(tool) {
                    failures.push(format!("{}: {message}", tool.core().id()));
                }
                continue;
            }
        };

        let mut last_emit = None;
        let mut latest = InstallProgress {
            downloaded_bytes: 0,
            total_bytes: None,
        };
        let result = tools.install(&source, &mut |progress| {
            latest = progress;
            let now = Instant::now();
            if last_emit
                .is_none_or(|last: Instant| now.duration_since(last) >= TOOL_PROGRESS_INTERVAL)
            {
                emit_tool_progress(
                    app,
                    tool,
                    ToolPhase::Downloading,
                    progress.downloaded_bytes,
                    progress.total_bytes,
                    None,
                );
                last_emit = Some(now);
            }
        });
        match result {
            Ok(_) => emit_tool_progress(
                app,
                tool,
                ToolPhase::Done,
                latest.downloaded_bytes,
                latest.total_bytes,
                None,
            ),
            Err(error) => {
                let message = error.to_string();
                emit_tool_progress(
                    app,
                    tool,
                    ToolPhase::Failed,
                    latest.downloaded_bytes,
                    latest.total_bytes,
                    Some(message.clone()),
                );
                if !optional_tool(tool) {
                    failures.push(format!("{}: {message}", tool.core().id()));
                }
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn source_for(tool: ToolId) -> Result<ToolSource, String> {
    match tool {
        ToolId::YtDlp => Ok(yt_dlp_source(Channel::Stable, YT_DLP_VERSION, TARGET_OS)),
        ToolId::Deno => Ok(deno_source(DENO_VERSION, TARGET_OS, target_arch())),
        ToolId::GalleryDl => Ok(gallery_dl_source(TARGET_OS)),
        ToolId::Ffmpeg if TARGET_OS == vids_core::naming::TargetOs::MacOs => {
            Ok(ffmpeg_macos_source(target_arch()))
        }
        ToolId::Ffmpeg => {
            ffmpeg_source(&FFMPEG_BUILD, TARGET_OS).map_err(|error| error.to_string())
        }
        ToolId::UblockLite if TARGET_OS == vids_core::naming::TargetOs::MacOs => {
            Err("uBlock Origin Lite is not supported on macOS".to_owned())
        }
        ToolId::UblockLite => Ok(ublock_lite_source()),
    }
}

const fn expected_version(tool: ToolId) -> &'static str {
    match tool {
        ToolId::YtDlp => YT_DLP_VERSION,
        ToolId::Ffmpeg if matches!(TARGET_OS, vids_core::naming::TargetOs::MacOs) => {
            FFMPEG_MACOS_VERSION
        }
        ToolId::Ffmpeg => FFMPEG_BUILD.version,
        ToolId::Deno => DENO_VERSION,
        ToolId::GalleryDl => GALLERY_DL_VERSION,
        ToolId::UblockLite => UBLOCK_LITE_VERSION,
    }
}

fn tool_needs_install(installed_version: Option<&str>, expected_version: &str) -> bool {
    installed_version != Some(expected_version)
}

fn emit_tool_progress(
    app: &AppHandle,
    tool: ToolId,
    phase: ToolPhase,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    message: Option<String>,
) {
    let _ = app.emit(
        "tools-progress",
        ToolProgress {
            tool,
            phase,
            downloaded_bytes,
            total_bytes,
            message,
        },
    );
}

#[cfg(target_arch = "x86_64")]
const fn target_arch() -> Arch {
    Arch::X86_64
}

#[cfg(target_arch = "aarch64")]
const fn target_arch() -> Arch {
    Arch::Aarch64
}

fn default_name_template() -> NameTemplate {
    NameTemplate {
        folders: vec![vec![Part::Field {
            field: Token::Uploader,
        }]],
        file: vec![
            Part::Optional {
                optional: OptionalPart {
                    before: String::new(),
                    field: Token::Date,
                    after: " ".to_owned(),
                },
            },
            Part::Field {
                field: Token::Title,
            },
            Part::Optional {
                optional: OptionalPart {
                    before: " [".to_owned(),
                    field: Token::Id,
                    after: "]".to_owned(),
                },
            },
        ],
    }
}

fn clamp_tz_offset_seconds(value: i64) -> i32 {
    const MAX_OFFSET_SECONDS: i64 = 14 * 60 * 60;
    value.clamp(-MAX_OFFSET_SECONDS, MAX_OFFSET_SECONDS) as i32
}

fn filter_enqueue_urls(urls: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    urls.iter()
        .filter_map(|input| {
            let trimmed = input.trim();
            (is_http_url(trimmed) && seen.insert(trimmed.to_owned())).then(|| trimmed.to_owned())
        })
        .collect()
}

fn extract_clipboard_url(text: &str) -> Option<String> {
    let line = text.lines().find(|line| !line.trim().is_empty())?;
    let token = line.split_whitespace().next()?;
    (token.starts_with("http://") || token.starts_with("https://")).then(|| token.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use serde_json::json;
    use tokio::sync::{mpsc, oneshot};
    use tokio::time::timeout;
    use vids_core::queue::{
        AlreadySaved, CancelToken, JobContext, JobFailure, JobOutcome, JobSink,
    };
    use vids_core::store::NewLibraryItem;
    use vids_core::types::JobState;

    const HANG: Duration = Duration::from_secs(10);

    struct RemovalRun {
        id: JobId,
        sink: JobSink,
        result: oneshot::Sender<Result<JobOutcome, JobFailure>>,
        cancel: CancelToken,
    }

    impl RemovalRun {
        fn finish(self, result: Result<JobOutcome, JobFailure>) {
            self.result
                .send(result)
                .expect("the executor awaits a result");
        }
    }

    struct RemovalFake {
        runs: mpsc::UnboundedSender<RemovalRun>,
    }

    impl JobExecutor for RemovalFake {
        async fn run(&self, job: JobContext) -> Result<JobOutcome, JobFailure> {
            let (result, decided) = oneshot::channel();
            self.runs
                .send(RemovalRun {
                    id: job.id,
                    sink: job.sink,
                    result,
                    cancel: job.cancel.clone(),
                })
                .expect("the test receives the run");
            tokio::select! {
                result = decided => result.expect("the test decides the run"),
                () = job.cancel.cancelled() => Err(JobFailure {
                    category: vids_core::types::ErrorCategory::Unknown,
                    message: "canceled".to_owned(),
                }),
            }
        }
    }

    fn removal_fake() -> (RemovalFake, mpsc::UnboundedReceiver<RemovalRun>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (RemovalFake { runs: sender }, receiver)
    }

    async fn next_removal_run(runs: &mut mpsc::UnboundedReceiver<RemovalRun>) -> RemovalRun {
        timeout(HANG, runs.recv())
            .await
            .expect("a run starts")
            .expect("the executor remains alive")
    }

    async fn wait_finished(events: &mut broadcast::Receiver<QueueEvent>, id: JobId) {
        timeout(HANG, async {
            loop {
                if matches!(
                    events.recv().await.expect("events remain open"),
                    QueueEvent::Finished { id: finished, .. } if finished == id
                ) {
                    return;
                }
            }
        })
        .await
        .expect("the job finishes");
    }

    #[test]
    fn enqueue_url_filter_trims_rejects_and_deduplicates() {
        let urls = vec![
            "".to_owned(),
            "   ".to_owned(),
            "ftp://example.com/video".to_owned(),
            "not a url".to_owned(),
            "https://example.com/a".to_owned(),
            "  https://example.com/a  ".to_owned(),
            "http://example.com/b".to_owned(),
            "https://".to_owned(),
        ];
        assert_eq!(
            filter_enqueue_urls(&urls),
            vec!["https://example.com/a", "http://example.com/b"]
        );
    }

    #[test]
    fn timezone_offset_is_clamped_to_fourteen_hours() {
        assert_eq!(clamp_tz_offset_seconds(-60_000), -50_400);
        assert_eq!(clamp_tz_offset_seconds(-50_400), -50_400);
        assert_eq!(clamp_tz_offset_seconds(1_800), 1_800);
        assert_eq!(clamp_tz_offset_seconds(50_400), 50_400);
        assert_eq!(clamp_tz_offset_seconds(60_000), 50_400);
    }

    #[test]
    fn clipboard_url_uses_first_token_of_first_non_empty_line() {
        assert_eq!(
            extract_clipboard_url("\n  https://example.com/a trailing\nhttps://example.com/b"),
            Some("https://example.com/a".to_owned())
        );
        assert_eq!(extract_clipboard_url("\n text https://example.com"), None);
        assert_eq!(extract_clipboard_url("\n\t\n"), None);
    }

    fn arg(url: &str, index: u32) -> PageImageArg {
        PageImageArg {
            url: url.to_owned(),
            index,
        }
    }

    #[test]
    fn page_images_are_checked_deduplicated_and_trimmed() {
        let page = page_images(
            Some("  A title  ".to_owned()),
            Some("   ".to_owned()),
            Some(1_793_318_400),
            Some(" n1a2b3 ".to_owned()),
            vec![
                arg(" https://example.com/a.jpg ", 1),
                arg("https://example.com/b.jpg", 2),
                arg("https://example.com/a.jpg", 3),
            ],
        )
        .unwrap();
        assert_eq!(page.title.as_deref(), Some("A title"));
        assert_eq!(page.uploader, None);
        assert_eq!(page.published_at, Some(1_793_318_400));
        assert_eq!(page.id.as_deref(), Some("n1a2b3"));
        assert_eq!(
            page.images,
            vec![
                PageImage {
                    url: "https://example.com/a.jpg".to_owned(),
                    index: 1
                },
                PageImage {
                    url: "https://example.com/b.jpg".to_owned(),
                    index: 2
                },
            ]
        );
        let long_title = page_images(
            Some("t".repeat(400)),
            None,
            None,
            Some("x".repeat(201)),
            vec![arg("https://example.com/a.jpg", 1)],
        )
        .unwrap();
        assert_eq!(long_title.title.unwrap().chars().count(), 300);
        assert_eq!(long_title.id, None);
    }

    #[test]
    fn page_images_reject_bad_urls_numbers_and_empty_choices() {
        for images in [
            vec![],
            vec![arg("javascript:alert(1)", 1)],
            vec![arg("data:image/png;base64,AA", 1)],
            vec![arg("https://example.com/a.jpg", 0)],
            vec![arg(&format!("https://example.com/{}", "x".repeat(2048)), 1)],
        ] {
            assert!(page_images(None, None, None, None, images).is_err());
        }
        let too_many = (1..=2001)
            .map(|index| arg(&format!("https://example.com/{index}.jpg"), index))
            .collect();
        assert!(page_images(None, None, None, None, too_many).is_err());
    }

    #[test]
    fn page_media_options_keep_the_page_job_url_and_direct_stream_shape() {
        let settings = Settings::defaults("C:\\Videos".to_owned());
        let facts = crate::collector::CollectPage {
            title: "Page title".to_owned(),
            uploader: Some("Uploader".to_owned()),
            published_at: Some(1_793_318_400),
            id: None,
        };
        let (job_url, options) = page_media_options(
            &settings,
            Preset::UpTo1080,
            32_400,
            PageMediaArg {
                media_url: " https://cdn.example/video/master.m3u8 ".to_owned(),
                page_url: " https://example.com/watch/one ".to_owned(),
                title: " Page title · 2 ".to_owned(),
            },
            Some(&facts),
        )
        .unwrap();
        assert_eq!(job_url, "https://example.com/watch/one");
        assert_eq!(
            options.page_media,
            Some(PageMedia {
                url: "https://cdn.example/video/master.m3u8".to_owned(),
                referer: "https://example.com/watch/one".to_owned(),
                title: Some("Page title · 2".to_owned()),
                uploader: Some("Uploader".to_owned()),
                published_at: Some(1_793_318_400),
            })
        );
        assert!(options.page_images.is_none());
        assert_eq!(options.cookies, Cookies::None);
    }

    #[test]
    fn page_media_choice_accepts_only_a_stored_url_and_keeps_page_facts() {
        let settings = Settings::defaults("C:\\Videos".to_owned());
        let mut options = job_options(&settings, Preset::Best, 0);
        options.page_media = Some(PageMedia {
            url: "https://cdn.example.com/ep01.mp4".to_owned(),
            referer: "https://example.com/watch".to_owned(),
            title: Some("Page title".to_owned()),
            uploader: Some("Uploader".to_owned()),
            published_at: Some(123),
        });
        options.page_media_choices = vec![
            PageMediaChoice {
                url: "https://cdn.example.com/ep01.mp4".to_owned(),
                kind: "MP4".to_owned(),
                duration_seconds: Some(1_420),
            },
            PageMediaChoice {
                url: "https://cdn.example.com/ep02.mp4".to_owned(),
                kind: "MP4".to_owned(),
                duration_seconds: Some(1_420),
            },
        ];

        assert!(
            selected_page_media_options(options.clone(), "https://cdn.example.com/other.mp4")
                .is_err()
        );
        let selected =
            selected_page_media_options(options, "https://cdn.example.com/ep02.mp4").unwrap();
        let media = selected.page_media.unwrap();
        assert_eq!(media.url, "https://cdn.example.com/ep02.mp4");
        assert_eq!(media.referer, "https://example.com/watch");
        assert_eq!(media.title.as_deref(), Some("Page title"));
        assert!(selected.page_media_choices.is_empty());
    }

    #[test]
    fn install_selection_includes_missing_and_outdated_tools() {
        assert!(tool_needs_install(None, "2026-10-31"));
        assert!(tool_needs_install(Some("latest"), "2026-10-31"));
        assert!(!tool_needs_install(Some("2026-10-31"), "2026-10-31"));
    }

    #[tokio::test]
    async fn queue_rows_leave_an_already_saved_skip_without_a_library_id() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (executor, mut runs) = removal_fake();
        let queue = Queue::with_store(executor, 1, Arc::clone(&store));
        let mut events = queue.subscribe();
        let id = queue.enqueue("https://example.test/saved", json!({}));
        next_removal_run(&mut runs).await.finish(Ok(JobOutcome {
            skipped: Some("already saved".to_owned()),
            already_saved: Some(AlreadySaved {
                saved_at: Some(10),
                file_missing: false,
                path: "/videos/existing.mp4".to_owned(),
            }),
            ..JobOutcome::default()
        }));
        wait_finished(&mut events, id).await;

        let rows = queue_rows(&queue, &store).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].already_saved.is_some());
        assert_eq!(rows[0].library_id, None);
    }

    #[tokio::test]
    async fn jobs_remove_keeps_library_and_running_rows_but_erases_the_other_queue_rows() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("queue.db");
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let store = Arc::new(Mutex::new(Store::open(&db).unwrap()));
        let (executor, mut runs) = removal_fake();
        let queue = Queue::with_store(executor, 2, Arc::clone(&store));
        let mut events = queue.subscribe();

        let done = queue.enqueue("https://example.test/done", json!({}));
        let downloading = queue.enqueue("https://example.test/downloading", json!({}));
        let queued = queue.enqueue("https://example.test/queued", json!({}));
        let first = next_removal_run(&mut runs).await;
        let second = next_removal_run(&mut runs).await;
        let (done_run, downloading_run) = if first.id == done {
            (first, second)
        } else {
            (second, first)
        };
        assert_eq!(downloading_run.id, downloading);
        downloading_run
            .sink
            .set_state(JobState::Downloading)
            .unwrap();

        let saved_file = temp.path().join("saved.mp4");
        fs::write(&saved_file, b"saved").unwrap();
        done_run.finish(Ok(JobOutcome {
            output_path: Some(saved_file.to_string_lossy().into_owned()),
            ..JobOutcome::default()
        }));
        wait_finished(&mut events, done).await;
        let queued_run = next_removal_run(&mut runs).await;
        assert_eq!(queued_run.id, queued);
        assert!(!queued_run.cancel.is_cancelled());

        let library_id = lock_recover(&store)
            .add_library_item(
                NewLibraryItem {
                    job_id: Some(done.0 as i64),
                    site: Some("Example".to_owned()),
                    video_id: Some("done".to_owned()),
                    url: Some("https://example.test/done".to_owned()),
                    title: Some("Saved".to_owned()),
                    uploader: None,
                    published_at: None,
                    duration_s: None,
                    path: saved_file.to_string_lossy().into_owned(),
                    comments_path: None,
                    size_bytes: Some(5),
                },
                10,
            )
            .unwrap();
        assert_eq!(
            queue_rows(&queue, &store)
                .unwrap()
                .into_iter()
                .find(|row| row.id == done.0)
                .unwrap()
                .library_id,
            Some(library_id)
        );

        for id in [done, queued, downloading] {
            let folder = work.join(format!("job-{}-1", id.0));
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("info.json"), b"{}").unwrap();
        }

        let removed = remove_download_rows(&queue, &[done.0, queued.0, downloading.0, 999, done.0])
            .await
            .unwrap();
        assert_eq!(removed, [done, queued]);
        assert!(queued_run.cancel.is_cancelled());
        let erased_ids: Vec<i64> = removed.iter().map(|id| id.0 as i64).collect();
        assert!(finish_erasing_job_records(Arc::clone(&store), &work, &erased_ids).await);

        let item = lock_recover(&store).get_library_item(library_id).unwrap();
        assert_eq!(item.job_id, None, "the foreign key uses ON DELETE SET NULL");
        assert!(saved_file.is_file(), "the saved file stays");
        assert!(lock_recover(&store).get_job(done.0 as i64).is_err());
        assert!(lock_recover(&store).get_job(queued.0 as i64).is_err());
        assert_eq!(
            lock_recover(&store)
                .get_job(downloading.0 as i64)
                .unwrap()
                .state,
            JobState::Downloading
        );
        assert!(!work.join(format!("job-{}-1", done.0)).exists());
        assert!(!work.join(format!("job-{}-1", queued.0)).exists());
        assert!(work.join(format!("job-{}-1", downloading.0)).is_dir());
        assert_eq!(
            queue
                .snapshot()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            [downloading]
        );

        let mut removed_events = Vec::new();
        while removed_events.len() < 2 {
            let event = timeout(HANG, events.recv())
                .await
                .expect("a Removed event arrives")
                .expect("events remain open");
            if let QueueEvent::Removed { id } = event {
                removed_events.push(id);
            }
        }
        assert_eq!(removed_events, [done, queued]);

        downloading_run.finish(Ok(JobOutcome::default()));
        wait_finished(&mut events, downloading).await;
        drop(queue);
        drop(store);

        let reopened = Store::open(&db).unwrap();
        let ids: Vec<i64> = reopened
            .list_jobs(vids_core::store::JobFilter::All)
            .unwrap()
            .into_iter()
            .map(|job| job.id)
            .collect();
        assert_eq!(ids, [downloading.0 as i64]);
        assert_eq!(reopened.get_library_item(library_id).unwrap().job_id, None);
    }
}

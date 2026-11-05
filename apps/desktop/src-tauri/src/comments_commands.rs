//! Fetch comments without downloading a video, including staged conflict resolution.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;
use vids_core::args::{CommentsSource, Cookies, ToolPaths};
use vids_core::comment_files::{self, FetchedComments};
use vids_core::comments::{AssSettings, VideoSize};
use vids_core::executor::{ExecutorConfig, JobOptions, YtDlpExecutor};
use vids_core::queue::CancelToken;
use vids_core::store::LibraryItem;
use vids_core::tools::Tool;

use crate::state::{self, AppState, TARGET_OS, lock_recover};

const STAGING_PREFIX: &str = "comments-";
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static PENDING: LazyLock<Mutex<HashMap<String, PendingComments>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct PendingComments {
    fetched: FetchedComments,
    folder: PathBuf,
    video_path: PathBuf,
    source: CommentsSource,
    library_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentsExtractionResult {
    pub id: Option<i64>,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_modified: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl CommentsExtractionResult {
    fn saved(id: Option<i64>, count: usize) -> Self {
        Self {
            id,
            status: "saved",
            count: u64::try_from(count).ok(),
            existing_count: None,
            existing_modified: None,
            new_count: None,
            token: None,
            file_name: None,
            message: None,
        }
    }

    fn failed(id: Option<i64>, message: String) -> Self {
        Self {
            id,
            status: "failed",
            count: None,
            existing_count: None,
            existing_modified: None,
            new_count: None,
            token: None,
            file_name: None,
            message: Some(message),
        }
    }

    fn needs_url() -> Self {
        Self {
            id: None,
            status: "needsUrl",
            count: None,
            existing_count: None,
            existing_modified: None,
            new_count: None,
            token: None,
            file_name: None,
            message: None,
        }
    }
}

#[tauri::command]
pub async fn comments_extract(
    library_ids: Vec<i64>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<CommentsExtractionResult>, String> {
    let mut results = Vec::with_capacity(library_ids.len());
    for id in library_ids {
        let item = match lock_recover(&state.store).get_library_item(id) {
            Ok(item) => item,
            Err(error) => {
                results.push(CommentsExtractionResult::failed(
                    Some(id),
                    error.to_string(),
                ));
                continue;
            }
        };
        let path = PathBuf::from(&item.path);
        if !path.is_file() {
            results.push(CommentsExtractionResult::failed(
                Some(id),
                format!("file not found: {}", path.display()),
            ));
            continue;
        }
        let Some(url) = item.url.as_deref() else {
            results.push(CommentsExtractionResult::failed(
                Some(id),
                "the saved item has no source URL".to_owned(),
            ));
            continue;
        };
        let source = comment_files::source_for_site(item.site.as_deref())
            .or_else(|| comment_files::source_for_url(url));
        let Some(source) = source else {
            results.push(CommentsExtractionResult::failed(
                Some(id),
                "the saved item has no comments extractor".to_owned(),
            ));
            continue;
        };
        let settings = ass_settings(&state, &item);
        let video = local_video_size(&state, &path, item.duration_s).await;
        let result = extract_one(
            &state,
            &app,
            Some(id),
            &path,
            url,
            source,
            (settings, video),
        )
        .await;
        if result.status == "saved" {
            let _ = app.emit("library-updated", ());
        }
        results.push(result);
    }
    Ok(results)
}

#[tauri::command]
pub async fn comments_resolve(
    token: String,
    keep: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<CommentsExtractionResult, String> {
    if keep != "existing" && keep != "new" {
        return Err("keep must be existing or new".to_owned());
    }
    let pending = lock_recover(&PENDING)
        .remove(&token)
        .ok_or_else(|| "comments conflict token was not found".to_owned())?;
    let id = pending.library_id;
    let operation: Result<CommentsExtractionResult, String> = (|| {
        Ok(if keep == "new" {
            let (_, ass) =
                comment_files::install(&pending.fetched, &pending.video_path, pending.source, true)
                    .map_err(|error| format!("could not replace comments: {error}"))?;
            if let Some(id) = id {
                lock_recover(&state.store)
                    .set_comments_path(id, Some(&ass.to_string_lossy()))
                    .map_err(|error| error.to_string())?;
            }
            CommentsExtractionResult::saved(id, pending.fetched.count)
        } else {
            let (raw, ass) = comment_files::destination_paths(&pending.video_path, pending.source)
                .ok_or_else(|| "comments source is not supported".to_owned())?;
            let count = comment_files::count_raw_file(&raw, pending.source);
            comment_files::discard(&pending.fetched);
            if let Some(id) = id {
                let recorded = if ass.is_file() { &ass } else { &raw };
                lock_recover(&state.store)
                    .set_comments_path(id, Some(&recorded.to_string_lossy()))
                    .map_err(|error| error.to_string())?;
            }
            CommentsExtractionResult::saved(id, count)
        })
    })();
    if let Err(error) = fs::remove_dir_all(&pending.folder) {
        eprintln!(
            "could not remove comments staging folder {}: {error}",
            pending.folder.display()
        );
    }
    let result = operation?;
    if id.is_some() {
        let _ = app.emit("library-updated", ());
    }
    Ok(result)
}

#[tauri::command]
pub async fn pick_video_file(
    app: AppHandle,
    window: tauri::Window,
) -> Result<Option<String>, String> {
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .add_filter("Video", &["mp4", "m4v", "webm", "mkv", "mov", "flv"])
            .blocking_pick_file()
            .map(|path| path.into_path().map_err(|error| error.to_string()))
            .transpose()
    })
    .await
    .map_err(|error| format!("video file picker task failed: {error}"))??;
    selected
        .map(|path| {
            if !path.is_absolute() || !comment_files::is_supported_video_path(&path) {
                return Err("file picker did not return a supported absolute video path".to_owned());
            }
            Ok(path.to_string_lossy().into_owned())
        })
        .transpose()
}

#[tauri::command]
pub async fn file_extract_comments(
    path: String,
    url: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<CommentsExtractionResult, String> {
    let video_path = PathBuf::from(path);
    if !video_path.is_file() || !comment_files::is_supported_video_path(&video_path) {
        return Err("the selected video file was not found or is unsupported".to_owned());
    }
    let found_url = url.or_else(|| {
        video_path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(comment_files::video_url_from_file_name)
    });
    let Some(url) = found_url else {
        return Ok(CommentsExtractionResult::needs_url());
    };
    let Some(source) = comment_files::source_for_url(&url) else {
        return Err("Only niconico and Bilibili have comments to extract".to_owned());
    };
    let video = local_video_size(&state, &video_path, None).await;
    Ok(extract_one(
        &state,
        &app,
        None,
        &video_path,
        &url,
        source,
        (AssSettings::default(), video),
    )
    .await)
}

async fn extract_one(
    state: &AppState,
    app: &AppHandle,
    id: Option<i64>,
    video_path: &Path,
    url: &str,
    source: CommentsSource,
    render: (AssSettings, VideoSize),
) -> CommentsExtractionResult {
    let token = format!(
        "{}{}-{}",
        std::process::id(),
        crate::unix_now(),
        NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
    );
    let folder = state.work_dir.join(format!("{STAGING_PREFIX}{token}"));
    if let Err(error) = fs::create_dir(&folder) {
        return CommentsExtractionResult::failed(
            id,
            format!("could not create comments folder: {error}"),
        );
    }
    let result = fetch_into_folder(
        state,
        app,
        url,
        source,
        (&render.0, render.1),
        &folder,
        &token,
    )
    .await;
    let fetched = match result {
        Ok(fetched) => fetched,
        Err(message) => {
            let _ = fs::remove_dir_all(&folder);
            return CommentsExtractionResult::failed(id, message);
        }
    };
    if let Some(conflict) = comment_files::conflict(video_path, source, fetched.count) {
        let (raw, _) = comment_files::destination_paths(video_path, source)
            .expect("a supported source has destination paths");
        let file_name = raw
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        lock_recover(&PENDING).insert(
            token.clone(),
            PendingComments {
                fetched,
                folder,
                video_path: video_path.to_owned(),
                source,
                library_id: id,
            },
        );
        return CommentsExtractionResult {
            id,
            status: "conflict",
            count: None,
            existing_count: u64::try_from(conflict.existing_count).ok(),
            existing_modified: conflict.existing_modified,
            new_count: u64::try_from(conflict.new_count).ok(),
            token: Some(token),
            file_name,
            message: None,
        };
    }
    let installed = comment_files::install(&fetched, video_path, source, false);
    match installed {
        Ok((_, ass)) => {
            if let Some(id) = id
                && let Err(error) =
                    lock_recover(&state.store).set_comments_path(id, Some(&ass.to_string_lossy()))
            {
                let _ = fs::remove_dir_all(&folder);
                return CommentsExtractionResult::failed(id.into(), error.to_string());
            }
            let count = fetched.count;
            let _ = fs::remove_dir_all(&folder);
            CommentsExtractionResult::saved(id, count)
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&folder);
            CommentsExtractionResult::failed(id, format!("could not place comments: {error}"))
        }
    }
}

async fn fetch_into_folder(
    state: &AppState,
    app: &AppHandle,
    url: &str,
    source: CommentsSource,
    render: (&AssSettings, VideoSize),
    folder: &Path,
    token: &str,
) -> Result<FetchedComments, String> {
    let yt_dlp = state.tools.resolve(Tool::YtDlp, TARGET_OS);
    let ffmpeg = state.tools.resolve(Tool::Ffmpeg, TARGET_OS);
    let deno = state.tools.resolve(Tool::Deno, TARGET_OS);
    let (Some(yt_dlp), Some(ffmpeg), Some(deno)) = (yt_dlp, ffmpeg, deno) else {
        return Err("required tools are not installed".to_owned());
    };
    let cookie_name = format!("cookies-comments-{token}.txt");
    let prepared = state::account_cookies(
        &state.store,
        &state.work_dir,
        &cookie_name,
        "comments extraction",
        url,
        |account_id, domains| state::in_app_cookie_text(&state.browser, app, account_id, domains),
    )
    .await;
    let (cookies, _temporary) = prepared
        .map(|prepared| (prepared.cookies, prepared.temporary))
        .unwrap_or((Cookies::None, None));
    let executor = YtDlpExecutor::new(
        ExecutorConfig {
            tools: ToolPaths {
                yt_dlp: yt_dlp.path,
                ffmpeg_dir: ffmpeg.path,
                deno: deno.path,
            },
            work_dir: state.work_dir.clone(),
            target_os: TARGET_OS,
            gallery_dl: None,
        },
        state.store.clone(),
    );
    executor
        .fetch_comments(url, source, &cookies, render, &CancelToken::new(), folder)
        .await
        .map_err(|error| error.message)
}

fn ass_settings(state: &AppState, item: &LibraryItem) -> AssSettings {
    item.job_id
        .and_then(|job_id| lock_recover(&state.store).get_job(job_id).ok())
        .and_then(|job| serde_json::from_value::<JobOptions>(job.options_json).ok())
        .map_or_else(AssSettings::default, |options| options.ass)
}

fn video_size(duration_s: Option<f64>) -> VideoSize {
    let duration_ms = duration_s.and_then(|seconds| {
        let milliseconds = seconds * 1000.0;
        (milliseconds.is_finite() && milliseconds >= 0.0 && milliseconds <= u64::MAX as f64)
            .then_some(milliseconds.trunc() as u64)
    });
    VideoSize {
        width: 1280,
        height: 720,
        duration_ms,
    }
}

async fn local_video_size(
    state: &AppState,
    path: &Path,
    known_duration_s: Option<f64>,
) -> VideoSize {
    let fallback = video_size(known_duration_s);
    let Some(ffmpeg) = state.tools.resolve(Tool::Ffmpeg, TARGET_OS) else {
        return fallback;
    };
    let program = ffmpeg.path.join(if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    });
    let path = path.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let output = std::process::Command::new(program)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,duration:format=duration",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        let stream = value.get("streams")?.as_array()?.first()?;
        let width = stream
            .get("width")?
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())?;
        let height = stream
            .get("height")?
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())?;
        let seconds = stream
            .get("duration")
            .and_then(json_seconds)
            .or_else(|| value.get("format")?.get("duration").and_then(json_seconds));
        Some(VideoSize {
            width,
            height,
            duration_ms: video_size(seconds.or(known_duration_s)).duration_ms,
        })
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(fallback)
}

fn json_seconds(value: &serde_json::Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

/// Remove staged conflicts left when the app closed before the user answered.
pub fn remove_stale_comment_folders(work_dir: &Path) {
    let Ok(entries) = fs::read_dir(work_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(STAGING_PREFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && let Err(error) = fs::remove_dir_all(entry.path())
        {
            eprintln!(
                "could not remove stale comments folder {}: {error}",
                entry.path().display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_removes_only_abandoned_comment_folders() {
        let work = tempfile::tempdir().unwrap();
        let stale = work.path().join("comments-123-1");
        let kept = work.path().join("job-123-1");
        fs::create_dir(&stale).unwrap();
        fs::create_dir(&kept).unwrap();
        fs::write(stale.join("fetch.comments.json"), b"[]").unwrap();

        remove_stale_comment_folders(work.path());

        assert!(!stale.exists());
        assert!(kept.is_dir());
    }
}

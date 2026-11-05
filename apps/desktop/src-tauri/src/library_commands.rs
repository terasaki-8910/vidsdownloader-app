//! Actions on a saved item in the library (R8-6, R8-7 A): open it with another program,
//! rename it, move it to the Recycle Bin / Trash, or remove it from the list.
//!
//! Moving to the Recycle Bin and removing from the list both erase the item's records
//! without leaving traces inside the app: the library row, the queue rows that point to
//! it, their leftover work folders, the deleted content in SQLite (secure delete plus a
//! WAL checkpoint) and the app UI's HTTP disk cache, which holds the thumbnails.
//!
//! Like every app command these are only reachable from the main webview: the invoke
//! handler in `lib.rs` refuses every other webview before a command runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(windows)]
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use vids_core::item_files;
use vids_core::queue::JobId;
use vids_core::store::LibraryItem;

use crate::models::{EraseResult, LibraryRow};
use crate::state::{AppState, TARGET_OS, lock_recover};

/// How long the app UI's HTTP cache may take to clear before it counts as not cleared.
#[cfg(windows)]
const CACHE_CLEAR_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(windows)]
const TRASH_NAME: &str = "the Recycle Bin";
#[cfg(not(windows))]
const TRASH_NAME: &str = "the Trash";

/// Opens the saved item with the operating system's default application.
#[tauri::command]
pub async fn library_open(
    id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let path = library_file(&state.store, id)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|error| error.to_string())
}

/// Lets the user pick a program to open the item's file with: the shell's "Open with"
/// dialog on Windows, an application chosen in /Applications on macOS. Cancelling the
/// choice is not an error.
#[tauri::command]
pub async fn library_open_with(
    id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let path = library_file(&state.store, id)?;
    let window = app
        .get_window("main")
        .ok_or_else(|| "main window was not found".to_owned())?;
    open_with(&app, &window, path).await
}

/// Renames the item's file to `name` (without extension; the extension and folder stay)
/// together with its side files, records the new paths and returns the updated row.
#[tauri::command]
pub async fn library_rename(
    id: i64,
    name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LibraryRow, String> {
    let item = library_item(&state, id)?;
    let main = existing_file(&item)?;
    let sides = side_files_of(&state, &item)?;
    let moves = item_files::plan_rename(&main, &sides, &name, TARGET_OS)
        .map_err(|error| error.to_string())?;
    let renamed = moves.clone();
    tauri::async_runtime::spawn_blocking(move || item_files::apply_renames(&renamed))
        .await
        .map_err(|error| format!("rename task failed: {error}"))?
        .map_err(|error| error.to_string())?;

    let new_path = moves[0].to.to_string_lossy().into_owned();
    let new_comments_path = item.comments_path.as_deref().map(|old| {
        moves
            .iter()
            .find(|planned| item_files::same_path(&planned.from, Path::new(old)))
            .map_or_else(
                || old.to_owned(),
                |planned| planned.to.to_string_lossy().into_owned(),
            )
    });
    let recorded = lock_recover(&state.store).rename_library_item(
        id,
        &new_path,
        new_comments_path.as_deref(),
        crate::unix_now(),
    );
    let updated_job = match recorded {
        Ok(updated_job) => updated_job,
        Err(error) => {
            let mut message = format!("could not record the new name: {error}");
            let undo_failed = item_files::undo_renames(&moves);
            if !undo_failed.is_empty() {
                message.push_str("; could not rename back: ");
                message.push_str(&undo_failed.join("; "));
            }
            return Err(message);
        }
    };
    if let Some(job) = updated_job.and_then(|job| u64::try_from(job).ok())
        && let Err(error) = state.queue.set_output_path(JobId(job), new_path)
    {
        // The store already has the new path; the queue row shows it after a restart.
        eprintln!("could not show the renamed file in queue job {job}: {error}");
    }

    let row = lock_recover(&state.store)
        .get_library_item(id)
        .map(LibraryRow::from)
        .map_err(|error| error.to_string())?;
    let _ = app.emit("library-updated", ());
    Ok(row)
}

/// Moves the item's file and its side files to the Recycle Bin / Trash, then erases its
/// records like [`library_forget`]. When the file itself cannot be moved, nothing else is
/// done; side files that cannot be moved stay where they are and are listed in the result.
#[tauri::command]
pub async fn library_trash(
    id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<EraseResult, String> {
    let item = library_item(&state, id)?;
    let main = existing_file(&item)?;
    ensure_erasable(&state, id)?;
    let sides = side_files_of(&state, &item)?;
    let side_files_failed = run_on_own_thread("library-trash", move || {
        trash::delete(&main).map_err(|error| {
            format!("could not move {} to {TRASH_NAME}: {error}", main.display())
        })?;
        Ok(sides
            .into_iter()
            .filter_map(|side| match trash::delete(&side) {
                Ok(()) => None,
                Err(error) => {
                    eprintln!(
                        "could not move side file {} to {TRASH_NAME}: {error}",
                        side.display()
                    );
                    Some(side.to_string_lossy().into_owned())
                }
            })
            .collect())
    })
    .await?;
    erase_records(&app, &state, id, side_files_failed).await
}

/// Removes the item from the list and erases its records without traces; its files stay
/// where they are. Refused, with nothing changed, while a job tied to the item has not
/// finished.
#[tauri::command]
pub async fn library_forget(
    id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<EraseResult, String> {
    library_item(&state, id)?;
    ensure_erasable(&state, id)?;
    erase_records(&app, &state, id, Vec::new()).await
}

fn library_item(state: &AppState, id: i64) -> Result<LibraryItem, String> {
    lock_recover(&state.store)
        .get_library_item(id)
        .map_err(|error| error.to_string())
}

fn library_file(
    store: &std::sync::Mutex<vids_core::store::Store>,
    id: i64,
) -> Result<PathBuf, String> {
    let item = lock_recover(store)
        .get_library_item(id)
        .map_err(|error| error.to_string())?;
    existing_file(&item)
}

fn existing_file(item: &LibraryItem) -> Result<PathBuf, String> {
    let path = PathBuf::from(&item.path);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("file not found: {}", path.display()))
    }
}

/// The item's side files and its recorded comments file, minus any that is another
/// library item's own file.
fn side_files_of(state: &AppState, item: &LibraryItem) -> Result<Vec<PathBuf>, String> {
    let others = lock_recover(&state.store)
        .library_paths_except(item.id)
        .map_err(|error| error.to_string())?;
    let mut files = item_files::item_side_files(
        Path::new(&item.path),
        item.comments_path.as_deref().map(Path::new),
    );
    files.retain(|file| {
        !others
            .iter()
            .any(|other| item_files::same_path(file, Path::new(other)))
    });
    Ok(files)
}

/// Refuses, before anything is touched, when a job that erasing the item would delete has
/// not finished, in the store or in the queue.
fn ensure_erasable(state: &AppState, id: i64) -> Result<(), String> {
    let job_ids = lock_recover(&state.store)
        .library_item_jobs_to_erase(id)
        .map_err(|error| error.to_string())?;
    if let Some(job) = state.queue.snapshot().into_iter().find(|job| {
        i64::try_from(job.id.0).is_ok_and(|job_id| job_ids.contains(&job_id))
            && !job.state.is_terminal()
    }) {
        return Err(format!(
            "job {} has not finished ({:?}); cancel it or wait until it ends",
            job.id, job.state
        ));
    }
    Ok(())
}

/// Erases the item's records (library row and jobs), drops the jobs from the queue,
/// compacts and checkpoints the database, deletes the jobs' leftover work folders and clears the app
/// UI's HTTP cache. Emits `library-updated`.
async fn erase_records(
    app: &AppHandle,
    state: &AppState,
    id: i64,
    side_files_failed: Vec<String>,
) -> Result<EraseResult, String> {
    let erased = lock_recover(&state.store)
        .erase_library_item(id)
        .map_err(|error| error.to_string())?;
    let job_ids: Vec<JobId> = erased
        .job_ids
        .iter()
        .filter_map(|job| u64::try_from(*job).ok().map(JobId))
        .collect();
    if let Err(error) = state.queue.forget(&job_ids) {
        // Only possible when a job was retried between the check and the erase; its row
        // is already gone from the store, so it does not come back after a restart.
        eprintln!("could not remove erased jobs from the queue: {error}");
    }

    let checkpointed =
        finish_erasing_job_records(Arc::clone(&state.store), &state.work_dir, &erased.job_ids)
            .await;
    let cache_cleared = clear_app_http_cache(app).await;
    let _ = app.emit("library-updated", ());
    Ok(EraseResult {
        checkpointed,
        cache_cleared,
        side_files_failed,
    })
}

/// Completes a queue-row erasure after the database transaction: compact old free pages,
/// checkpoint and truncate the WAL, then remove leftover attempt folders. Both library
/// erasure and Downloads-only removal use this exact path.
pub(crate) async fn finish_erasing_job_records(
    store: Arc<std::sync::Mutex<vids_core::store::Store>>,
    work_dir: &Path,
    job_ids: &[i64],
) -> bool {
    let checkpointed = match tauri::async_runtime::spawn_blocking(move || {
        let store = lock_recover(&store);
        // Free pages from deletes made before secure delete existed would otherwise keep
        // old content; a failed compaction leaves the rows erased, so it only counts as
        // "not checkpointed".
        match store.compact() {
            Ok(()) => store.checkpoint_truncate(),
            Err(error) => {
                eprintln!("could not compact the database: {error}");
                store.checkpoint_truncate().map(|_| false)
            }
        }
    })
    .await
    {
        Ok(Ok(completed)) => completed,
        Ok(Err(error)) => {
            eprintln!("could not checkpoint the database: {error}");
            false
        }
        Err(error) => {
            eprintln!("database checkpoint task failed: {error}");
            false
        }
    };
    remove_work_folders(work_dir, job_ids);
    checkpointed
}

/// Deletes `work/job-<id>-<attempt>` folders of `job_ids` left by an interrupted attempt.
/// Links and junctions are never followed or removed.
fn remove_work_folders(work_dir: &Path, job_ids: &[i64]) {
    let Ok(entries) = fs::read_dir(work_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(job) = name.to_str().and_then(work_folder_job_id) else {
            continue;
        };
        if !job_ids.contains(&job) || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(entry.path()) {
            eprintln!(
                "could not remove work folder {}: {error}",
                entry.path().display()
            );
        }
    }
}

/// The job id of an attempt folder named `job-<id>-<attempt>` (see the core executor).
fn work_folder_job_id(name: &str) -> Option<i64> {
    let (job, attempt) = name.strip_prefix("job-")?.split_once('-')?;
    let digits = |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    if digits(job) && digits(attempt) {
        job.parse().ok()
    } else {
        None
    }
}

/// Runs `work` on a new thread of its own and waits for it without blocking the async
/// runtime. A new thread has no COM apartment yet, so the shell calls inside (`trash`
/// sets one up and panics if the thread already has an incompatible one; the "Open with"
/// dialog needs one) never meet one set up by other code.
async fn run_on_own_thread<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let _ = sender.send(work());
        })
        .map_err(|error| format!("could not start a worker thread: {error}"))?;
    receiver
        .await
        .map_err(|_| "the worker thread stopped before it finished".to_owned())?
}

#[cfg(windows)]
async fn open_with(_app: &AppHandle, window: &tauri::Window, path: PathBuf) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    // A window handle is a plain value; it crosses to the dialog's thread as an integer.
    let owner = window.hwnd().map_err(|error| error.to_string())?.0 as isize;
    run_on_own_thread("library-open-with", move || {
        use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
        };
        use windows::Win32::UI::Shell::{
            OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OAIF_REGISTER_EXT, OPENASINFO, SHOpenWithDialog,
        };
        use windows::core::{HRESULT, PCWSTR};

        let file: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let info = OPENASINFO {
            pcszFile: PCWSTR(file.as_ptr()),
            pcszClass: PCWSTR::null(),
            oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
        };
        // SAFETY: this thread is new, so it gets its own single-threaded COM apartment,
        // released after the dialog only when this call created it. `file` and `info`
        // outlive the modal call, which only reads them, and `owner` is the app's main
        // window, which lives as long as the app.
        let result = unsafe {
            let initialized =
                CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
            let result = SHOpenWithDialog(Some(HWND(owner as *mut std::ffi::c_void)), &info);
            if initialized {
                CoUninitialize();
            }
            result
        };
        match result {
            Ok(()) => Ok(()),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => Ok(()),
            Err(error) => Err(format!("could not show the Open with dialog: {error}")),
        }
    })
    .await
}

/// macOS: pick an application (starting in /Applications), then `open -a <app> <file>`.
/// Compile-checked only; not run on a Mac yet.
#[cfg(target_os = "macos")]
async fn open_with(app: &AppHandle, window: &tauri::Window, path: PathBuf) -> Result<(), String> {
    use tauri_plugin_dialog::DialogExt;

    let app = app.clone();
    let window = window.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        // Owned by the app window, so it opens over it and stays in front of it.
        app.dialog()
            .file()
            .set_parent(&window)
            .set_directory("/Applications")
            .add_filter("Applications", &["app"])
            .blocking_pick_file()
    })
    .await
    .map_err(|error| format!("application picker task failed: {error}"))?;
    let Some(picked) = picked else {
        return Ok(());
    };
    let application = picked.into_path().map_err(|error| error.to_string())?;
    if !application.is_absolute() || !path.is_absolute() {
        return Err("the application or the file has a relative path".to_owned());
    }
    let status = tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new("/usr/bin/open")
            .arg("-a")
            .arg(&application)
            .arg(&path)
            .status()
    })
    .await
    .map_err(|error| format!("open task failed: {error}"))?
    .map_err(|error| format!("could not run /usr/bin/open: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("/usr/bin/open failed: {status}"))
    }
}

/// Clears the HTTP disk cache of the app UI's own webview (thumbnails shown in the queue
/// and the library). The in-app browser's account webviews use other data folders and
/// keep theirs. Waits up to 5 seconds; returns whether WebView2 reported success.
#[cfg(windows)]
async fn clear_app_http_cache(app: &AppHandle) -> bool {
    use std::sync::Mutex;

    use webview2_com::ClearBrowsingDataCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE, ICoreWebView2_13, ICoreWebView2Profile2,
    };
    use windows_core::Interface;

    let Some(webview) = app.get_webview("main") else {
        eprintln!("could not clear the app cache: the main webview was not found");
        return false;
    };
    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let callback_sender = Arc::clone(&sender);
    let dispatched = webview.with_webview(move |platform| {
        // SAFETY: the WebView2 COM objects are used on the webview's own thread, which
        // with_webview runs this closure on; the call only starts clearing the cache.
        let outcome = unsafe {
            (|| -> windows_core::Result<()> {
                let core = platform.controller().CoreWebView2()?;
                let profile = core
                    .cast::<ICoreWebView2_13>()?
                    .Profile()?
                    .cast::<ICoreWebView2Profile2>()?;
                let handler = ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
                    if let Some(sender) = lock_recover(&callback_sender).take() {
                        let _ = sender.send(result.map_err(|error| error.to_string()));
                    }
                    Ok(())
                }));
                profile.ClearBrowsingData(COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE, &handler)
            })()
        };
        if let Err(error) = outcome
            && let Some(sender) = lock_recover(&sender).take()
        {
            let _ = sender.send(Err(error.to_string()));
        }
    });
    if let Err(error) = dispatched {
        eprintln!("could not reach the app webview to clear its cache: {error}");
        return false;
    }
    match tokio::time::timeout(CACHE_CLEAR_TIMEOUT, receiver).await {
        Ok(Ok(Ok(()))) => true,
        Ok(Ok(Err(error))) => {
            eprintln!("could not clear the app cache: {error}");
            false
        }
        Ok(Err(_)) => {
            eprintln!("could not clear the app cache: the result channel closed");
            false
        }
        Err(_) => {
            eprintln!("could not clear the app cache: no answer within 5 seconds");
            false
        }
    }
}

/// macOS: not done yet (WKWebView's `WKWebsiteDataStore` would need native code), so the
/// result tells the UI the cache was not cleared.
#[cfg(not(windows))]
async fn clear_app_http_cache(_app: &AppHandle) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use vids_core::store::{NewLibraryItem, Store};

    fn library_item_at(path: &Path) -> NewLibraryItem {
        NewLibraryItem {
            job_id: None,
            site: None,
            video_id: None,
            url: None,
            title: None,
            uploader: None,
            published_at: None,
            duration_s: None,
            path: path.to_string_lossy().into_owned(),
            comments_path: None,
            size_bytes: None,
        }
    }

    #[test]
    fn library_file_errors_for_unknown_and_missing_items() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        let unknown = library_file(&store, 999).unwrap_err();
        assert!(unknown.contains("library item 999"));

        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.mp4");
        let id = store
            .lock()
            .unwrap()
            .add_library_item(library_item_at(&missing), 1)
            .unwrap();
        assert_eq!(
            library_file(&store, id).unwrap_err(),
            format!("file not found: {}", missing.display())
        );
    }

    #[test]
    fn work_folders_are_recognized_by_job_id() {
        assert_eq!(work_folder_job_id("job-12-1"), Some(12));
        assert_eq!(work_folder_job_id("job-4503599627370496-3"), Some(1 << 52));
        assert_eq!(work_folder_job_id("job-12"), None);
        assert_eq!(work_folder_job_id("job--1"), None);
        assert_eq!(work_folder_job_id("job-1x-1"), None);
        assert_eq!(work_folder_job_id("job-12-1-extra"), None);
        assert_eq!(work_folder_job_id("cookies-12-1.txt"), None);
    }

    #[test]
    fn only_the_erased_jobs_work_folders_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["job-3-1", "job-3-2", "job-30-1", "job-4-1"] {
            fs::create_dir(dir.path().join(name)).unwrap();
            fs::write(dir.path().join(name).join("info.json"), "{}").unwrap();
        }
        fs::write(dir.path().join("job-3-9"), "a file, not a folder").unwrap();

        remove_work_folders(dir.path(), &[3, 4]);

        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["job-3-9", "job-30-1"]);
    }
}

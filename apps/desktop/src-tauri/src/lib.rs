mod account_commands;
mod app_like;
mod browser;
mod collector;
mod commands;
mod comments_commands;
mod flow;
mod library_commands;
mod login_import;
mod media;
mod models;
mod notify;
mod page_media;
mod site_probe;
mod sites;
mod sources;
mod state;

use std::error::Error;
use std::fs;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tauri::Manager;
use vids_core::queue::Queue;
use vids_core::store::Store;
use vids_core::tools::ToolManager;

use crate::account_commands::{
    account_add, account_logout, account_remove, account_rename, account_select,
    account_set_method, browser_back, browser_forward, browser_open, browser_reload,
    browser_set_bounds, browser_set_visible, firefox_profiles, login_cancel, login_finish,
    login_start, pick_cookies_file, site_add, site_for_url, site_remove, site_set_ad_block,
    sites_list,
};
use crate::collector::{
    collect_close, collect_reload_find, collect_reveal, collect_save_rule, collect_select,
    collect_start, collect_stop,
};
use crate::commands::{
    cancel_job, copy_text, enqueue_page_images, enqueue_page_media, enqueue_urls,
    forward_queue_events, get_settings, install_tools, job_file_exists, jobs_remove,
    library_search, open_job_file, page_media_choose, pick_download_dir, pick_folder,
    queue_snapshot, read_clipboard_url, retry_job, reveal_path, save_again, set_settings,
    tools_status,
};
use crate::comments_commands::{
    comments_extract, comments_resolve, file_extract_comments, pick_video_file,
};
use crate::flow::{
    flow_delete, flow_list, flow_list_for_site, flow_record_start, flow_record_stop, flow_rename,
    flow_replay, flow_replay_cancel, flow_save,
};
use crate::library_commands::{
    library_forget, library_open, library_open_with, library_rename, library_trash,
};
use crate::login_import::{firefox_import, firefox_profiles_info};
use crate::models::Settings;
use crate::site_probe::site_probe;
use crate::sources::{
    SourceManager, source_check, source_check_all, source_delete, source_preview,
    source_preview_collect, source_register, source_rename, source_set_options, sources_list,
};
use crate::state::{AppState, ShellExecutor};

fn initialize(app: &mut tauri::App<tauri::Wry>) -> Result<(), Box<dyn Error>> {
    let data_dir = app.path().app_data_dir()?;
    fs::create_dir_all(&data_dir)?;
    let work_dir = data_dir.join("work");
    fs::create_dir_all(&work_dir)?;
    remove_stale_cookie_files(&work_dir);
    comments_commands::remove_stale_comment_folders(&work_dir);

    let store = Store::open(data_dir.join("vidsdownloader.db"))?;
    sites::seed_builtin_sites(&store, unix_now())?;
    let default_download_dir = app
        .path()
        .video_dir()
        .map(|path| path.join("vidsdownloader"))
        .unwrap_or_else(|_| data_dir.join("downloads"));
    // Unreadable or invalid saved settings must not stop the app from opening.
    let stored_settings = store.get_setting::<Settings>("app").ok().flatten();
    let validated_settings = stored_settings
        .clone()
        .and_then(|stored| stored.validated().ok());
    let settings = validated_settings
        .unwrap_or_else(|| Settings::defaults(default_download_dir.to_string_lossy().into_owned()));
    if stored_settings.as_ref() != Some(&settings) {
        store.set_setting("app", &settings)?;
    }

    let store = Arc::new(Mutex::new(store));
    let tools = Arc::new(ToolManager::new(data_dir.join("tools")));
    let browser_root = data_dir.join("browser");
    fs::create_dir_all(&browser_root)?;
    let browser = Arc::new(browser::BrowserManager::new(
        browser_root,
        Arc::clone(&store),
        Arc::clone(&tools),
    ));
    let collector = Arc::new(collector::CollectorManager::new(
        Arc::clone(&browser),
        Arc::clone(&store),
    ));
    let flow = Arc::new(flow::FlowManager::new(
        Arc::clone(&browser),
        Arc::clone(&store),
    ));
    let page_media: Arc<dyn page_media::PageMediaFinder> =
        Arc::new(page_media::BrowserPageMediaFinder::new(
            app.handle().clone(),
            Arc::clone(&browser),
            Arc::clone(&store),
        ));
    // Queue::with_store captures the current Tokio handle, and setup runs outside the runtime.
    let runtime = tauri::async_runtime::handle();
    let _runtime_guard = runtime.inner().enter();
    let queue = Queue::with_store(
        ShellExecutor {
            tools: Arc::clone(&tools),
            work_dir: work_dir.clone(),
            store: Arc::clone(&store),
            app: app.handle().clone(),
            browser: Arc::clone(&browser),
            page_media,
        },
        settings.concurrency as usize,
        Arc::clone(&store),
    );
    let events = queue.subscribe();
    let event_queue = queue.clone();
    let event_store = Arc::clone(&store);
    let event_app = app.handle().clone();
    let notifier = notify::Notifier::start(app.handle().clone());
    let notify_events = queue.subscribe();
    let notify_queue = queue.clone();
    let sources = Arc::new(SourceManager::new(app.handle().clone()));

    app.manage(AppState {
        settings: Mutex::new(settings),
        store,
        work_dir,
        tools,
        queue,
        installation_running: Arc::new(AtomicBool::new(false)),
        browser,
        collector,
        flow,
        sources: Arc::clone(&sources),
        notifier: notifier.clone(),
    });
    tauri::async_runtime::spawn(forward_queue_events(
        event_queue,
        event_store,
        event_app,
        events,
    ));
    tauri::async_runtime::spawn(notify::watch_queue(notify_queue, notifier, notify_events));
    tauri::async_runtime::spawn(Arc::clone(&sources).run_checks());
    tauri::async_runtime::spawn(sources.run_auto_checks());
    commands::install_optional_tools_in_background(app.handle());
    if let Some(main) = app.get_webview("main") {
        app_like::apply(&main, app_like::Role::AppUi);
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before any window exists, so the windows and the toasts carry the app's identity.
    notify::register_app_id();
    let app_commands: Box<tauri::ipc::InvokeHandler<tauri::Wry>> =
        Box::new(tauri::generate_handler![
            queue_snapshot,
            enqueue_urls,
            enqueue_page_images,
            enqueue_page_media,
            cancel_job,
            retry_job,
            page_media_choose,
            save_again,
            jobs_remove,
            read_clipboard_url,
            copy_text,
            library_search,
            library_open,
            library_open_with,
            library_rename,
            library_trash,
            library_forget,
            comments_extract,
            comments_resolve,
            pick_video_file,
            file_extract_comments,
            reveal_path,
            open_job_file,
            job_file_exists,
            get_settings,
            set_settings,
            pick_download_dir,
            pick_folder,
            tools_status,
            install_tools,
            sites_list,
            site_add,
            site_remove,
            site_set_ad_block,
            account_add,
            account_remove,
            account_rename,
            account_select,
            account_set_method,
            account_logout,
            firefox_profiles,
            firefox_profiles_info,
            firefox_import,
            pick_cookies_file,
            site_for_url,
            site_probe,
            browser_open,
            browser_set_bounds,
            browser_set_visible,
            browser_back,
            browser_forward,
            browser_reload,
            login_start,
            login_finish,
            login_cancel,
            collect_start,
            collect_stop,
            collect_reload_find,
            collect_select,
            collect_close,
            collect_reveal,
            collect_save_rule,
            flow_record_start,
            flow_record_stop,
            flow_save,
            flow_list,
            flow_list_for_site,
            flow_rename,
            flow_delete,
            flow_replay,
            flow_replay_cancel,
            sources_list,
            source_preview,
            source_preview_collect,
            source_register,
            source_check,
            source_check_all,
            source_rename,
            source_set_options,
            source_delete,
        ]);
    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("vdlmedia", |context, request, responder| {
            media::handle_request(
                context.app_handle(),
                context.webview_label(),
                request,
                responder,
            );
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        // In-app updates: the webview checks, downloads and installs through the updater's
        // own commands (capabilities/default.json), and restarts through the process plugin.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(initialize)
        .invoke_handler(move |invoke| {
            if is_main_webview(invoke.message.webview_ref().label()) {
                app_commands(invoke)
            } else {
                invoke
                    .resolver
                    .reject("commands are only available to the main webview");
                true
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Per-job cookie files are deleted when their job ends, and copies of Firefox's cookie
/// database right after an import; one left by a crash is removed here, before any job
/// runs.
fn remove_stale_cookie_files(work_dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(work_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        let removed = if is_dir && vids_core::firefox::is_temp_dir_name(&name) {
            fs::remove_dir_all(entry.path())
        } else if !is_dir && is_job_cookie_file(&name) {
            fs::remove_file(entry.path())
        } else {
            continue;
        };
        if let Err(error) = removed {
            eprintln!("could not remove stale cookie file {name}: {error}");
        }
    }
}

fn is_job_cookie_file(name: &str) -> bool {
    name.starts_with("cookies-") && name.ends_with(".txt")
}

fn is_main_webview(label: &str) -> bool {
    label == "main"
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
    use super::{is_job_cookie_file, is_main_webview, remove_stale_cookie_files};

    #[test]
    fn stale_cookie_files_are_recognized() {
        assert!(is_job_cookie_file("cookies-12-1.txt"));
        assert!(!is_job_cookie_file("job-12-1"));
        assert!(!is_job_cookie_file("cookies.json"));
        // Source checks and previews name theirs the same way.
        assert!(is_job_cookie_file("cookies-source-4-9.txt"));
        assert!(is_job_cookie_file("cookies-preview-2.txt"));
    }

    #[test]
    fn stale_cookie_files_and_firefox_copies_are_removed_at_start() {
        let work = tempfile::tempdir().unwrap();
        let path = |name: &str| work.path().join(name);
        std::fs::write(path("cookies-3-1.txt"), "x").unwrap();
        std::fs::create_dir(path("firefox-cookies-9-1-1")).unwrap();
        std::fs::write(path("firefox-cookies-9-1-1").join("cookies.sqlite"), "x").unwrap();
        std::fs::create_dir(path("job-3-1")).unwrap();
        std::fs::write(path("job-3-1").join("info.json"), "{}").unwrap();
        std::fs::write(path("firefox-cookies-not-a-folder"), "x").unwrap();

        remove_stale_cookie_files(work.path());

        assert!(!path("cookies-3-1.txt").exists());
        assert!(!path("firefox-cookies-9-1-1").exists());
        assert!(path("job-3-1").join("info.json").exists());
        assert!(path("firefox-cookies-not-a-folder").exists());
    }

    #[test]
    fn only_main_webview_can_invoke_app_commands() {
        assert!(is_main_webview("main"));
        assert!(!is_main_webview("browser"));
        assert!(!is_main_webview("browser-helper-1"));
        assert!(!is_main_webview("Main"));
    }
}

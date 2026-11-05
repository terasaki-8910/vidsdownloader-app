//! The add-site report: what yt-dlp makes of the URL just added in Settings > Sites. One
//! flat `-J` run without downloading. The first run goes without any login, which is what
//! shows whether a login is needed; checking again after a login (`with_login`) gives the
//! run the cookies of the site's selected account, prepared as a job's are (a temporary
//! file, deleted when the run ends).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tauri::{AppHandle, State};
use vids_core::args::Cookies;
use vids_core::executor::{ExecutorConfig, YtDlpExecutor};
use vids_core::queue::CancelToken;
use vids_core::site_probe::{SiteProbeReport, classify};

use crate::collector;
use crate::sources::managed_tools;
use crate::state::{self, AppState, TARGET_OS};

/// A probe that runs longer than this ends as a failure.
const SITE_PROBE_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_URL_CHARS: usize = 2_048;

/// Numbers the temporary cookie files of probes, which can overlap.
static COOKIE_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[tauri::command]
pub async fn site_probe(
    url: String,
    with_login: Option<bool>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SiteProbeReport, String> {
    let url = url.trim().to_owned();
    if url.chars().count() > MAX_URL_CHARS || !collector::is_http_url(&url) {
        return Err("url must be an http(s) URL".to_owned());
    }
    let tools = match managed_tools(&state) {
        Ok(tools) => tools,
        Err(error) => return Ok(classify(Err(error), false)),
    };
    // `_temporary` keeps the cookie file until yt-dlp has finished. A site without an
    // account, or an account without a login, gives no cookies: the run is then the same
    // as the first one, and the report says it did not use the login.
    let (cookies, _temporary) = if with_login.unwrap_or(false) {
        let file_name = format!(
            "cookies-site-probe-{}.txt",
            COOKIE_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let prepared = state::account_cookies(
            &state.store,
            &state.work_dir,
            &file_name,
            "site probe",
            &url,
            |account_id, domains| {
                state::in_app_cookie_text(&state.browser, &app, account_id, domains)
            },
        )
        .await;
        match prepared {
            Some(prepared) => (prepared.cookies, prepared.temporary),
            None => (Cookies::None, None),
        }
    } else {
        (Cookies::None, None)
    };
    let logged_in = !matches!(cookies, Cookies::None);
    let executor = YtDlpExecutor::new(
        ExecutorConfig {
            tools,
            work_dir: state.work_dir.clone(),
            target_os: TARGET_OS,
            gallery_dl: None,
        },
        Arc::clone(&state.store),
    )
    .with_list_probe_timeout(SITE_PROBE_TIMEOUT);
    // Only the app quitting ends it early, which takes yt-dlp down with the app.
    let result = executor
        .probe_list_json(&url, cookies, &CancelToken::new())
        .await;
    Ok(classify(result, logged_in))
}

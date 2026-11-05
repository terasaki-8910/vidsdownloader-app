use tauri::{AppHandle, Emitter, State, Url};
use tauri_plugin_dialog::DialogExt;

use crate::browser::{BrowserBounds, BrowserMode, BrowserProfile};
use crate::sites::{self, Account, LoginMethod, Site};
use crate::state::{AppState, lock_recover};

#[tauri::command]
pub async fn sites_list(state: State<'_, AppState>) -> Result<Vec<Site>, String> {
    sites::list_sites(&lock_recover(&state.store))
}

#[tauri::command]
pub async fn site_add(
    url: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Site, String> {
    let site = sites::add_site(&lock_recover(&state.store), &url, unix_now())?;
    emit_sites(&app, &state)?;
    Ok(site)
}

#[tauri::command]
pub async fn site_remove(
    site_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let site = sites::site_by_id(&lock_recover(&state.store), site_id)?;
    if site.builtin {
        return Err("built-in sites cannot be removed".to_owned());
    }
    for account in &site.accounts {
        state.browser.delete_account_data(&app, account.id)?;
    }
    sites::remove_site(&lock_recover(&state.store), site_id)?;
    emit_sites(&app, &state)
}

#[tauri::command]
pub async fn site_set_ad_block(
    site_id: i64,
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    sites::set_ad_block(&lock_recover(&state.store), site_id, enabled, unix_now())?;
    state.browser.reapply_ad_block()?;
    emit_sites(&app, &state)
}

#[tauri::command]
pub async fn account_add(
    site_id: i64,
    label: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Account, String> {
    let account = sites::add_account(&lock_recover(&state.store), site_id, &label, unix_now())?;
    emit_sites(&app, &state)?;
    Ok(account)
}

#[tauri::command]
pub async fn account_remove(
    account_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (site, _) = sites::site_and_account(&lock_recover(&state.store), account_id)?;
    if site.accounts.len() == 1 {
        return Err("a site's last account cannot be removed".to_owned());
    }
    state.browser.delete_account_data(&app, account_id)?;
    sites::remove_account(&lock_recover(&state.store), account_id, unix_now())?;
    emit_sites(&app, &state)
}

#[tauri::command]
pub async fn account_rename(
    account_id: i64,
    label: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    sites::rename_account(&lock_recover(&state.store), account_id, &label, unix_now())?;
    emit_sites(&app, &state)
}

#[tauri::command]
pub async fn account_select(
    site_id: i64,
    account_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    sites::select_account(&lock_recover(&state.store), site_id, account_id, unix_now())?;
    emit_sites(&app, &state)
}

#[tauri::command]
pub async fn account_set_method(
    account_id: i64,
    method: LoginMethod,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    sites::set_account_method(&lock_recover(&state.store), account_id, method, unix_now())?;
    emit_sites(&app, &state)
}

#[tauri::command]
pub async fn account_logout(
    account_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (_, account) = sites::site_and_account(&lock_recover(&state.store), account_id)?;
    if account.method != LoginMethod::InApp {
        return Err("only in-app accounts can be logged out".to_owned());
    }
    state.browser.clear_account(&app, account_id).await?;
    sites::set_account_cookie_status(
        &lock_recover(&state.store),
        account_id,
        None,
        None,
        unix_now(),
    )?;
    emit_sites(&app, &state)
}

#[tauri::command]
pub async fn firefox_profiles() -> Result<Vec<String>, String> {
    Ok(crate::login_import::load_profiles_ini()
        .await?
        .map(|(ini, _)| ini.names())
        .unwrap_or_default())
}

#[tauri::command]
pub async fn pick_cookies_file(
    app: AppHandle,
    window: tauri::Window,
) -> Result<Option<String>, String> {
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .add_filter("cookies.txt", &["txt"])
            .blocking_pick_file()
            .map(|path| path.into_path().map_err(|error| error.to_string()))
            .transpose()
    })
    .await
    .map_err(|error| format!("cookie file picker task failed: {error}"))??;
    selected
        .map(|path| {
            if !path.is_absolute() {
                return Err("file picker returned a relative path".to_owned());
            }
            Ok(path.to_string_lossy().into_owned())
        })
        .transpose()
}

#[tauri::command]
pub async fn site_for_url(url: String, state: State<'_, AppState>) -> Result<Option<i64>, String> {
    sites::site_for_url(&lock_recover(&state.store), &url)
}

#[tauri::command]
pub async fn browser_open(
    url: Option<String>,
    account_id: Option<i64>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let Some(url) = url else {
        return state.browser.open(
            &app,
            None,
            BrowserProfile::General,
            BrowserMode::Browse,
            None,
            None,
        );
    };
    let parsed = parse_browser_url(&url)?;
    let matching_site_id = sites::site_for_url(&lock_recover(&state.store), parsed.as_str())?;
    let (profile, browser_account_id) = if let Some(account_id) = account_id {
        let (_, account) = sites::site_and_account(&lock_recover(&state.store), account_id)?;
        if account.method != LoginMethod::InApp {
            return Err("the browser can only use an in-app account".to_owned());
        }
        (BrowserProfile::Account(account_id), Some(account_id))
    } else {
        let selected =
            sites::selected_account_for_url(&lock_recover(&state.store), parsed.as_str())?;
        selected
            .filter(|(_, account)| account.method == LoginMethod::InApp)
            .map_or((BrowserProfile::General, None), |(_, account)| {
                (BrowserProfile::Account(account.id), Some(account.id))
            })
    };
    state.browser.open(
        &app,
        Some(parsed),
        profile,
        BrowserMode::Browse,
        matching_site_id,
        browser_account_id,
    )
}

#[tauri::command]
pub async fn browser_set_bounds(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.browser.set_bounds(BrowserBounds {
        x,
        y,
        width,
        height,
    })
}

#[tauri::command]
pub async fn browser_set_visible(
    visible: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.browser.set_visible(&app, visible)
}

#[tauri::command]
pub async fn browser_back(state: State<'_, AppState>) -> Result<(), String> {
    state.browser.eval("history.back()")
}

#[tauri::command]
pub async fn browser_forward(state: State<'_, AppState>) -> Result<(), String> {
    state.browser.eval("history.forward()")
}

#[tauri::command]
pub async fn browser_reload(state: State<'_, AppState>) -> Result<(), String> {
    state.browser.reload()
}

#[tauri::command]
pub async fn login_start(
    site_id: i64,
    account_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let site = sites::site_by_id(&lock_recover(&state.store), site_id)?;
    let account = site
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| format!("account {account_id} does not belong to site {site_id}"))?;
    if account.method != LoginMethod::InApp {
        return Err("only in-app accounts can use the login browser".to_owned());
    }
    state.browser.open(
        &app,
        Some(parse_browser_url(&site.login_url)?),
        BrowserProfile::Account(account_id),
        BrowserMode::Login,
        Some(site_id),
        Some(account_id),
    )
}

#[tauri::command]
pub async fn login_finish(app: AppHandle, state: State<'_, AppState>) -> Result<Account, String> {
    let (site_id, account_id) = state.browser.login_context()?;
    let site = sites::site_by_id(&lock_recover(&state.store), site_id)?;
    let cookies = state
        .browser
        .cookies_for_site(&app, account_id, &site.domains)
        .await?;
    let now = unix_now();
    let account = sites::set_account_cookie_status(
        &lock_recover(&state.store),
        account_id,
        Some(cookies.len() as u64),
        Some(now),
        now,
    )?;
    state.browser.start_page(&app)?;
    emit_sites(&app, &state)?;
    Ok(account)
}

#[tauri::command]
pub async fn login_cancel(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.browser.login_context()?;
    state.browser.start_page(&app)
}

fn emit_sites(app: &AppHandle, state: &State<'_, AppState>) -> Result<(), String> {
    let sites = sites::list_sites(&lock_recover(&state.store))?;
    app.emit("sites-updated", sites)
        .map_err(|error| error.to_string())
}

fn parse_browser_url(url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url.trim()).map_err(|error| format!("invalid URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.has_host() {
        return Err("url must be an http(s) URL with a host".to_owned());
    }
    Ok(parsed)
}

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

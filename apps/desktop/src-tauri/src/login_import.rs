//! Importing logins from Firefox into the in-app browser's accounts (Windows).
//!
//! The import runs here, from Firefox's cookie database (read through a temporary copy,
//! see `vids_core::firefox`) into each account's WebView2 profile; the app's own webview
//! only ever gets counts. Only accounts without cookies of their own are filled, so an
//! in-app login is never mixed with Firefox's.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use vids_core::firefox::{self, ProfilesIni};

use crate::browser::{CookieImport, record_cookie_count};
use crate::sites::{self, LoginMethod, Site};
use crate::state::{AppState, lock_recover};

/// One import at a time: two would race on the same accounts.
static IMPORT_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirefoxProfilesInfo {
    pub profiles: Vec<String>,
    pub default_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedSite {
    pub site_id: i64,
    pub name: String,
    /// Cookies added to the site's selected account.
    pub cookies: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipReason {
    /// The selected account already has cookies of its own.
    LoggedIn,
    /// The selected account uses Firefox or a cookies.txt, not the in-app browser.
    NotInApp,
    /// Firefox has no cookies for the site.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedSite {
    pub site_id: i64,
    pub name: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirefoxImportResult {
    pub imported: Vec<ImportedSite>,
    pub skipped: Vec<SkippedSite>,
}

/// Firefox's `profiles.ini` and the folder holding it; `None` when Firefox is not set up
/// on this computer.
pub async fn load_profiles_ini() -> Result<Option<(ProfilesIni, PathBuf)>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(path) = firefox::profiles_ini_path() else {
            return Ok(None);
        };
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                Ok(Some((ProfilesIni::parse(&contents), dir)))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("could not read Firefox profiles.ini: {error}")),
        }
    })
    .await
    .map_err(|error| format!("Firefox profile task failed: {error}"))?
}

#[tauri::command]
pub async fn firefox_profiles_info() -> Result<FirefoxProfilesInfo, String> {
    Ok(load_profiles_ini().await?.map_or_else(
        || FirefoxProfilesInfo {
            profiles: Vec::new(),
            default_profile: None,
        },
        |(ini, _)| FirefoxProfilesInfo {
            profiles: ini.names(),
            default_profile: ini.default_profile().map(str::to_owned),
        },
    ))
}

#[tauri::command]
pub async fn firefox_import(
    profile: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<FirefoxImportResult, String> {
    if !cfg!(target_os = "windows") {
        return Err("not available on this system yet".to_owned());
    }
    if IMPORT_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("an import is already running".to_owned());
    }
    let _running = Running;
    import(&profile, &app, &state).await
}

/// Clears `IMPORT_RUNNING` however the import ends.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        IMPORT_RUNNING.store(false, Ordering::SeqCst);
    }
}

async fn import(
    profile: &str,
    app: &AppHandle,
    state: &State<'_, AppState>,
) -> Result<FirefoxImportResult, String> {
    let profile_dir = load_profiles_ini()
        .await?
        .and_then(|(ini, dir)| ini.profile_dir(&dir, profile))
        .ok_or_else(|| format!("Firefox profile {profile} was not found"))?;
    let sites = sites::list_sites(&lock_recover(&state.store))?;
    let plans: Vec<(&Site, Plan)> = sites.iter().map(|site| (site, plan(site))).collect();

    let domains: Vec<String> = plans
        .iter()
        .filter(|(_, plan)| matches!(plan, Plan::Import { .. }))
        .flat_map(|(site, _)| site.domains.iter().cloned())
        .collect();
    let cookies = if domains.is_empty() {
        Vec::new()
    } else {
        let work_dir = state.work_dir.clone();
        let now = unix_now();
        tauri::async_runtime::spawn_blocking(move || {
            firefox::read_cookies(&profile_dir, &work_dir, &domains, now)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("Firefox cookie task failed: {error}"))??
    };

    let mut result = FirefoxImportResult::default();
    let mut counts = Vec::new();
    let mut errors = Vec::new();
    for (site, plan) in plans {
        let skip = |reason| SkippedSite {
            site_id: site.id,
            name: site.name.clone(),
            reason,
        };
        let account_id = match plan {
            Plan::Skip(reason) => {
                result.skipped.push(skip(reason));
                continue;
            }
            Plan::Import { account_id } => account_id,
        };
        let site_cookies = cookies_for_domains(&cookies, &site.domains);
        if site_cookies.is_empty() {
            result.skipped.push(skip(SkipReason::None));
            continue;
        }
        let offered = site_cookies.len();
        match state
            .browser
            .import_cookies(app, account_id, &site.domains, site_cookies)
            .await
        {
            Ok(CookieImport::AlreadyHasCookies { total }) => {
                counts.push((account_id, total));
                result.skipped.push(skip(SkipReason::LoggedIn));
            }
            Ok(CookieImport::Added { added: 0, total }) => {
                counts.push((account_id, total));
                errors.push(format!(
                    "{}: none of {offered} cookies could be added",
                    site.name
                ));
            }
            Ok(CookieImport::Added { added, total }) => {
                counts.push((account_id, total));
                result.imported.push(ImportedSite {
                    site_id: site.id,
                    name: site.name.clone(),
                    cookies: added as u64,
                });
            }
            Err(error) => errors.push(format!("{}: {error}", site.name)),
        }
    }

    let store = lock_recover(&state.store);
    let now = unix_now();
    for (account_id, total) in counts {
        if let Err(error) = record_cookie_count(&store, account_id, total, now) {
            eprintln!("could not record cookies for account {account_id}: {error}");
        }
    }
    let updated = sites::list_sites(&store)?;
    drop(store);
    app.emit("sites-updated", updated)
        .map_err(|error| error.to_string())?;

    match errors.as_slice() {
        [] => Ok(result),
        [only] => Err(only.clone()),
        [first, rest @ ..] => Err(format!("{first} (and {} more)", rest.len())),
    }
}

/// What the import does with one site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    Skip(SkipReason),
    /// Fill this in-app account, which has no cookies recorded yet.
    Import {
        account_id: i64,
    },
}

fn plan(site: &Site) -> Plan {
    let Some(account) = site
        .selected_account_id
        .and_then(|id| site.accounts.iter().find(|account| account.id == id))
    else {
        return Plan::Skip(SkipReason::NotInApp);
    };
    if account.method != LoginMethod::InApp {
        Plan::Skip(SkipReason::NotInApp)
    } else if account.cookie_count.is_some_and(|count| count > 0) {
        Plan::Skip(SkipReason::LoggedIn)
    } else {
        Plan::Import {
            account_id: account.id,
        }
    }
}

/// The cookies that belong to one of a site's domains.
fn cookies_for_domains(cookies: &[firefox::Cookie], domains: &[String]) -> Vec<firefox::Cookie> {
    cookies
        .iter()
        .filter(|cookie| {
            domains
                .iter()
                .any(|domain| firefox::host_matches(&cookie.host, domain))
        })
        .cloned()
        .collect()
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
    use crate::sites::Account;

    fn site(accounts: Vec<Account>, selected: Option<i64>) -> Site {
        Site {
            id: 1,
            name: "Example".to_owned(),
            domains: vec!["example.com".to_owned()],
            login_url: "https://example.com/login".to_owned(),
            builtin: false,
            accounts,
            selected_account_id: selected,
            ad_block: true,
            flow_count: 0,
        }
    }

    fn account(id: i64, method: LoginMethod, cookie_count: Option<u64>) -> Account {
        Account {
            id,
            label: "Main".to_owned(),
            method,
            cookie_count,
            updated_at: None,
        }
    }

    #[test]
    fn plan_fills_only_in_app_accounts_without_cookies() {
        assert_eq!(
            plan(&site(vec![account(4, LoginMethod::InApp, None)], Some(4))),
            Plan::Import { account_id: 4 }
        );
        assert_eq!(
            plan(&site(
                vec![account(4, LoginMethod::InApp, Some(0))],
                Some(4)
            )),
            Plan::Import { account_id: 4 }
        );
        assert_eq!(
            plan(&site(
                vec![account(4, LoginMethod::InApp, Some(12))],
                Some(4)
            )),
            Plan::Skip(SkipReason::LoggedIn)
        );
        let firefox = LoginMethod::Firefox {
            profile: "default-release".to_owned(),
        };
        assert_eq!(
            plan(&site(vec![account(4, firefox, None)], Some(4))),
            Plan::Skip(SkipReason::NotInApp)
        );
        let cookies_txt = LoginMethod::CookiesTxt {
            path: "C:\\cookies.txt".to_owned(),
        };
        assert_eq!(
            plan(&site(vec![account(4, cookies_txt, None)], Some(4))),
            Plan::Skip(SkipReason::NotInApp)
        );
        assert_eq!(
            plan(&site(vec![account(4, LoginMethod::InApp, None)], None)),
            Plan::Skip(SkipReason::NotInApp)
        );
    }

    #[test]
    fn plan_uses_the_selected_account() {
        let accounts = vec![
            account(4, LoginMethod::InApp, Some(9)),
            account(5, LoginMethod::InApp, None),
        ];
        assert_eq!(
            plan(&site(accounts.clone(), Some(5))),
            Plan::Import { account_id: 5 }
        );
        assert_eq!(
            plan(&site(accounts, Some(4))),
            Plan::Skip(SkipReason::LoggedIn)
        );
    }

    #[test]
    fn site_cookies_are_the_ones_of_its_domains() {
        let cookie = |host: &str| firefox::Cookie {
            host: host.to_owned(),
            name: format!("n-{host}"),
            value: "v".to_owned(),
            path: "/".to_owned(),
            expires: None,
            secure: true,
            http_only: true,
            same_site: firefox::SameSite::Lax,
        };
        let cookies = vec![
            cookie(".x.com"),
            cookie("twitter.com"),
            cookie(".example.com"),
            cookie("notx.com"),
        ];
        let hosts = |domains: &[&str]| {
            let domains: Vec<String> = domains.iter().map(|domain| (*domain).to_owned()).collect();
            cookies_for_domains(&cookies, &domains)
                .into_iter()
                .map(|cookie| cookie.host)
                .collect::<Vec<_>>()
        };
        assert_eq!(hosts(&["x.com", "twitter.com"]), [".x.com", "twitter.com"]);
        assert_eq!(hosts(&["example.com"]), [".example.com"]);
        assert!(hosts(&["nicovideo.jp"]).is_empty());
    }

    #[test]
    fn results_match_the_contract() {
        let result = FirefoxImportResult {
            imported: vec![ImportedSite {
                site_id: 1,
                name: "YouTube".to_owned(),
                cookies: 14,
            }],
            skipped: vec![
                SkippedSite {
                    site_id: 2,
                    name: "X".to_owned(),
                    reason: SkipReason::LoggedIn,
                },
                SkippedSite {
                    site_id: 3,
                    name: "Twitch".to_owned(),
                    reason: SkipReason::NotInApp,
                },
                SkippedSite {
                    site_id: 4,
                    name: "Bilibili".to_owned(),
                    reason: SkipReason::None,
                },
            ],
        };
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({
                "imported": [{ "siteId": 1, "name": "YouTube", "cookies": 14 }],
                "skipped": [
                    { "siteId": 2, "name": "X", "reason": "loggedIn" },
                    { "siteId": 3, "name": "Twitch", "reason": "notInApp" },
                    { "siteId": 4, "name": "Bilibili", "reason": "none" }
                ]
            })
        );
        assert_eq!(
            serde_json::to_value(FirefoxProfilesInfo {
                profiles: vec!["default-release".to_owned()],
                default_profile: Some("default-release".to_owned()),
            })
            .unwrap(),
            serde_json::json!({
                "profiles": ["default-release"],
                "defaultProfile": "default-release"
            })
        );
    }
}

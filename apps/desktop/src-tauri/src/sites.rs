use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tauri::Url;
use vids_core::store::{SiteProfile, Store};

const MAIN_ACCOUNT_LABEL: &str = "メイン";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LoginMethod {
    InApp,
    Firefox { profile: String },
    CookiesTxt { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: i64,
    pub label: String,
    pub method: LoginMethod,
    pub cookie_count: Option<u64>,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteProfileData {
    pub domains: Vec<String>,
    pub login_url: String,
    pub builtin: bool,
    pub accounts: Vec<Account>,
    pub selected_account_id: Option<i64>,
    #[serde(default = "default_ad_block")]
    pub ad_block: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub id: i64,
    pub name: String,
    pub domains: Vec<String>,
    pub login_url: String,
    pub builtin: bool,
    pub accounts: Vec<Account>,
    pub selected_account_id: Option<i64>,
    pub ad_block: bool,
}

impl Site {
    pub(crate) fn from_stored(profile: SiteProfile) -> Result<Self, String> {
        let data: SiteProfileData = serde_json::from_value(profile.profile_json)
            .map_err(|error| format!("invalid site profile '{}': {error}", profile.name))?;
        Ok(Self {
            id: profile.id,
            name: profile.name,
            domains: data.domains,
            login_url: data.login_url,
            builtin: data.builtin,
            accounts: data.accounts,
            selected_account_id: data.selected_account_id,
            ad_block: data.ad_block,
        })
    }

    fn data(&self) -> SiteProfileData {
        SiteProfileData {
            domains: self.domains.clone(),
            login_url: self.login_url.clone(),
            builtin: self.builtin,
            accounts: self.accounts.clone(),
            selected_account_id: self.selected_account_id,
            ad_block: self.ad_block,
        }
    }
}

struct BuiltinSite {
    name: &'static str,
    domains: &'static [&'static str],
    login_url: &'static str,
}

const BUILTIN_SITES: &[BuiltinSite] = &[
    BuiltinSite {
        name: "YouTube",
        domains: &["youtube.com"],
        login_url: "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2F",
    },
    BuiltinSite {
        name: "ニコニコ動画",
        domains: &["nicovideo.jp"],
        login_url: "https://account.nicovideo.jp/login",
    },
    BuiltinSite {
        name: "X",
        domains: &["x.com", "twitter.com"],
        login_url: "https://x.com/i/flow/login",
    },
    BuiltinSite {
        name: "Twitch",
        domains: &["twitch.tv"],
        login_url: "https://www.twitch.tv/login",
    },
    BuiltinSite {
        name: "Bilibili",
        domains: &["bilibili.com"],
        login_url: "https://passport.bilibili.com/login",
    },
];

pub fn seed_builtin_sites(store: &Store, now: i64) -> Result<(), String> {
    let existing = list_sites(store)?;
    let names: HashSet<&str> = existing.iter().map(|site| site.name.as_str()).collect();
    let mut next_id = next_account_id(&existing);
    for builtin in BUILTIN_SITES {
        if names.contains(builtin.name) {
            continue;
        }
        let account = Account {
            id: next_id,
            label: MAIN_ACCOUNT_LABEL.to_owned(),
            method: LoginMethod::InApp,
            cookie_count: None,
            updated_at: None,
        };
        next_id += 1;
        let data = SiteProfileData {
            domains: builtin
                .domains
                .iter()
                .map(|domain| (*domain).to_owned())
                .collect(),
            login_url: builtin.login_url.to_owned(),
            builtin: true,
            selected_account_id: Some(account.id),
            accounts: vec![account],
            ad_block: true,
        };
        save_data(store, builtin.name, &data, now)?;
    }
    Ok(())
}

pub fn list_sites(store: &Store) -> Result<Vec<Site>, String> {
    store
        .list_site_profiles()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(Site::from_stored)
        .collect()
}

pub fn site_by_id(store: &Store, site_id: i64) -> Result<Site, String> {
    list_sites(store)?
        .into_iter()
        .find(|site| site.id == site_id)
        .ok_or_else(|| format!("site {site_id} was not found"))
}

pub fn site_and_account(store: &Store, account_id: i64) -> Result<(Site, Account), String> {
    for site in list_sites(store)? {
        if let Some(account) = site
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
        {
            return Ok((site, account));
        }
    }
    Err(format!("account {account_id} was not found"))
}

pub fn site_for_url_in(sites: &[Site], url: &str) -> Option<i64> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?.trim_end_matches('.');
    sites.iter().find_map(|site| {
        site.domains
            .iter()
            .any(|domain| host_matches_domain(host, domain))
            .then_some(site.id)
    })
}

pub fn site_for_url(store: &Store, url: &str) -> Result<Option<i64>, String> {
    Ok(site_for_url_in(&list_sites(store)?, url))
}

pub fn selected_account_for_url(
    store: &Store,
    url: &str,
) -> Result<Option<(Site, Account)>, String> {
    let sites = list_sites(store)?;
    let Some(site_id) = site_for_url_in(&sites, url) else {
        return Ok(None);
    };
    let site = sites
        .into_iter()
        .find(|site| site.id == site_id)
        .expect("site id came from the same collection");
    let account = site
        .selected_account_id
        .and_then(|id| site.accounts.iter().find(|account| account.id == id))
        .cloned();
    Ok(account.map(|account| (site, account)))
}

pub fn add_site(store: &Store, url: &str, now: i64) -> Result<Site, String> {
    let parsed = parse_http_url(url)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "url must have a host".to_owned())?;
    let domain = strip_www(host).trim_end_matches('.').to_ascii_lowercase();
    let sites = list_sites(store)?;
    if sites.iter().any(|site| {
        site.domains
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&domain))
    }) {
        return Err(format!("a site already has domain {domain}"));
    }
    if sites.iter().any(|site| site.name == domain) {
        return Err(format!("a site named {domain} already exists"));
    }
    let account = Account {
        id: next_account_id(&sites),
        label: MAIN_ACCOUNT_LABEL.to_owned(),
        method: LoginMethod::InApp,
        cookie_count: None,
        updated_at: None,
    };
    let data = SiteProfileData {
        domains: vec![domain.clone()],
        login_url: url.trim().to_owned(),
        builtin: false,
        selected_account_id: Some(account.id),
        accounts: vec![account],
        ad_block: true,
    };
    let id = save_data(store, &domain, &data, now)?;
    Ok(Site {
        id,
        name: domain,
        domains: data.domains,
        login_url: data.login_url,
        builtin: data.builtin,
        accounts: data.accounts,
        selected_account_id: data.selected_account_id,
        ad_block: data.ad_block,
    })
}

pub fn set_ad_block(store: &Store, site_id: i64, enabled: bool, now: i64) -> Result<(), String> {
    let mut site = site_by_id(store, site_id)?;
    site.ad_block = enabled;
    save_site(store, &site, now).map(|_| ())
}

pub fn remove_site(store: &Store, site_id: i64) -> Result<Site, String> {
    let site = site_by_id(store, site_id)?;
    if site.builtin {
        return Err("built-in sites cannot be removed".to_owned());
    }
    store
        .delete_site_profile(&site.name)
        .map_err(|error| error.to_string())?;
    Ok(site)
}

pub fn add_account(store: &Store, site_id: i64, label: &str, now: i64) -> Result<Account, String> {
    let label = validate_label(label)?;
    let sites = list_sites(store)?;
    let next_id = next_account_id(&sites);
    let mut site = sites
        .into_iter()
        .find(|site| site.id == site_id)
        .ok_or_else(|| format!("site {site_id} was not found"))?;
    let account = Account {
        id: next_id,
        label,
        method: LoginMethod::InApp,
        cookie_count: None,
        updated_at: None,
    };
    site.accounts.push(account.clone());
    save_site(store, &site, now)?;
    Ok(account)
}

pub fn remove_account(store: &Store, account_id: i64, now: i64) -> Result<Account, String> {
    let (mut site, account) = site_and_account(store, account_id)?;
    if site.accounts.len() == 1 {
        return Err("a site's last account cannot be removed".to_owned());
    }
    site.accounts.retain(|item| item.id != account_id);
    if site.selected_account_id == Some(account_id) {
        site.selected_account_id = site.accounts.first().map(|item| item.id);
    }
    save_site(store, &site, now)?;
    Ok(account)
}

pub fn rename_account(
    store: &Store,
    account_id: i64,
    label: &str,
    now: i64,
) -> Result<Account, String> {
    let label = validate_label(label)?;
    update_account(store, account_id, now, |account| account.label = label)
}

pub fn select_account(
    store: &Store,
    site_id: i64,
    account_id: i64,
    now: i64,
) -> Result<(), String> {
    let mut site = site_by_id(store, site_id)?;
    if !site.accounts.iter().any(|account| account.id == account_id) {
        return Err(format!(
            "account {account_id} does not belong to site {site_id}"
        ));
    }
    site.selected_account_id = Some(account_id);
    save_site(store, &site, now).map(|_| ())
}

pub fn set_account_method(
    store: &Store,
    account_id: i64,
    method: LoginMethod,
    now: i64,
) -> Result<Account, String> {
    update_account(store, account_id, now, |account| {
        account.method = method;
        account.cookie_count = None;
        account.updated_at = None;
    })
}

pub fn set_account_cookie_status(
    store: &Store,
    account_id: i64,
    cookie_count: Option<u64>,
    updated_at: Option<i64>,
    now: i64,
) -> Result<Account, String> {
    update_account(store, account_id, now, |account| {
        account.cookie_count = cookie_count;
        account.updated_at = updated_at;
    })
}

fn update_account(
    store: &Store,
    account_id: i64,
    now: i64,
    change: impl FnOnce(&mut Account),
) -> Result<Account, String> {
    let (mut site, _) = site_and_account(store, account_id)?;
    let account = site
        .accounts
        .iter_mut()
        .find(|account| account.id == account_id)
        .expect("account came from this site");
    change(account);
    let updated = account.clone();
    save_site(store, &site, now)?;
    Ok(updated)
}

fn save_site(store: &Store, site: &Site, now: i64) -> Result<i64, String> {
    save_data(store, &site.name, &site.data(), now)
}

fn save_data(store: &Store, name: &str, data: &SiteProfileData, now: i64) -> Result<i64, String> {
    let value = serde_json::to_value(data).map_err(|error| error.to_string())?;
    store
        .upsert_site_profile(name, value, now)
        .map_err(|error| error.to_string())
}

fn next_account_id(sites: &[Site]) -> i64 {
    sites
        .iter()
        .flat_map(|site| site.accounts.iter())
        .map(|account| account.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn parse_http_url(url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url.trim()).map_err(|error| format!("invalid URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.has_host() {
        return Err("url must be an http(s) URL with a host".to_owned());
    }
    Ok(parsed)
}

fn strip_www(host: &str) -> &str {
    host.strip_prefix("www.")
        .or_else(|| host.strip_prefix("WWW."))
        .unwrap_or(host)
}

fn host_matches_domain(host: &str, domain: &str) -> bool {
    let host = host.trim_end_matches('.');
    let domain = domain.trim().trim_matches('.');
    host.eq_ignore_ascii_case(domain)
        || (host.len() > domain.len()
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
            && host[host.len() - domain.len()..].eq_ignore_ascii_case(domain))
}

fn validate_label(label: &str) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() {
        Err("account label cannot be empty".to_owned())
    } else {
        Ok(label.to_owned())
    }
}

const fn default_ad_block() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_json_round_trip_uses_the_contract_shape() {
        let data = SiteProfileData {
            domains: vec!["example.com".to_owned()],
            login_url: "https://example.com/login".to_owned(),
            builtin: false,
            accounts: vec![Account {
                id: 9,
                label: "Work".to_owned(),
                method: LoginMethod::Firefox {
                    profile: "default-release".to_owned(),
                },
                cookie_count: Some(3),
                updated_at: Some(42),
            }],
            selected_account_id: Some(9),
            ad_block: false,
        };
        let value = serde_json::to_value(&data).unwrap();
        assert_eq!(value["loginUrl"], "https://example.com/login");
        assert_eq!(value["accounts"][0]["method"]["kind"], "firefox");
        assert_eq!(value["selectedAccountId"], 9);
        assert_eq!(value["adBlock"], false);
        assert_eq!(
            serde_json::from_value::<SiteProfileData>(value).unwrap(),
            data
        );
    }

    #[test]
    fn matching_accepts_exact_and_subdomains_but_not_lookalikes() {
        let sites = vec![Site {
            id: 7,
            name: "YouTube".to_owned(),
            domains: vec!["youtube.com".to_owned()],
            login_url: String::new(),
            builtin: true,
            accounts: Vec::new(),
            selected_account_id: None,
            ad_block: true,
        }];
        assert_eq!(
            site_for_url_in(&sites, "https://youtube.com/watch"),
            Some(7)
        );
        assert_eq!(
            site_for_url_in(&sites, "https://www.youtube.com/watch"),
            Some(7)
        );
        assert_eq!(
            site_for_url_in(&sites, "https://m.YouTube.com/watch"),
            Some(7)
        );
        assert_eq!(
            site_for_url_in(&sites, "https://notyoutube.com/watch"),
            None
        );
        assert_eq!(
            site_for_url_in(&sites, "https://youtube.com.evil.test"),
            None
        );

        let overlapping = vec![
            Site {
                id: 1,
                name: "A".to_owned(),
                domains: vec!["video.example.com".to_owned()],
                ..sites[0].clone()
            },
            Site {
                id: 2,
                name: "B".to_owned(),
                domains: vec!["example.com".to_owned()],
                ..sites[0].clone()
            },
        ];
        assert_eq!(
            site_for_url_in(&overlapping, "https://video.example.com/watch"),
            Some(1)
        );
    }

    #[test]
    fn builtin_seeding_is_idempotent_and_never_overwrites() {
        let store = Store::open_in_memory().unwrap();
        seed_builtin_sites(&store, 10).unwrap();
        let first = list_sites(&store).unwrap();
        assert_eq!(first.len(), 5);
        assert!(first.iter().all(|site| site.accounts.len() == 1));
        seed_builtin_sites(&store, 20).unwrap();
        assert_eq!(list_sites(&store).unwrap(), first);

        let mut youtube = first
            .into_iter()
            .find(|site| site.name == "YouTube")
            .unwrap();
        youtube.login_url = "https://example.com/custom".to_owned();
        save_site(&store, &youtube, 30).unwrap();
        seed_builtin_sites(&store, 40).unwrap();
        assert_eq!(
            site_by_id(&store, youtube.id).unwrap().login_url,
            youtube.login_url
        );
    }

    #[test]
    fn ad_block_defaults_true_when_absent_and_round_trips_through_store() {
        let value = serde_json::json!({
            "domains": ["example.com"],
            "loginUrl": "https://example.com/login",
            "builtin": false,
            "accounts": [],
            "selectedAccountId": null
        });
        let data: SiteProfileData = serde_json::from_value(value).unwrap();
        assert!(data.ad_block);

        let store = Store::open_in_memory().unwrap();
        save_data(&store, "example.com", &data, 1).unwrap();
        let site = list_sites(&store).unwrap().pop().unwrap();
        assert!(site.ad_block);
        set_ad_block(&store, site.id, false, 2).unwrap();
        assert!(!site_by_id(&store, site.id).unwrap().ad_block);
        assert_eq!(
            store
                .get_site_profile("example.com")
                .unwrap()
                .unwrap()
                .profile_json["adBlock"],
            false
        );
    }

    #[test]
    fn add_site_derives_domain_and_refuses_duplicates() {
        let store = Store::open_in_memory().unwrap();
        let site = add_site(&store, "https://www.Example.com/login?q=1", 1).unwrap();
        assert_eq!(site.name, "example.com");
        assert_eq!(site.domains, ["example.com"]);
        assert_eq!(site.accounts[0].label, MAIN_ACCOUNT_LABEL);
        assert!(add_site(&store, "https://example.com/other", 2).is_err());
        assert!(add_site(&store, "file:///tmp/cookies", 2).is_err());
    }

    #[test]
    fn account_rules_keep_ids_unique_and_selection_valid() {
        let store = Store::open_in_memory().unwrap();
        let site = add_site(&store, "https://example.com/login", 1).unwrap();
        let first = site.accounts[0].clone();
        assert!(remove_account(&store, first.id, 2).is_err());

        let second = add_account(&store, site.id, " Second ", 3).unwrap();
        assert!(second.id > first.id);
        select_account(&store, site.id, second.id, 4).unwrap();
        assert!(select_account(&store, site.id, 999, 5).is_err());
        remove_account(&store, second.id, 6).unwrap();
        assert_eq!(
            site_by_id(&store, site.id).unwrap().selected_account_id,
            Some(first.id)
        );

        let third = add_account(&store, site.id, "Third", 7).unwrap();
        select_account(&store, site.id, first.id, 8).unwrap();
        remove_account(&store, first.id, 9).unwrap();
        assert_eq!(
            site_by_id(&store, site.id).unwrap().selected_account_id,
            Some(third.id)
        );
    }
}

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State, Url};
use vids_core::store::Store;

use crate::flow::FlowScope;
use crate::method_options::{
    overrides_dest_dir, overrides_preset, validate_dest_dir, with_options,
};
use crate::models::Preset;
use crate::sites;
use crate::state::{AppState, lock_recover};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MethodKind {
    Rule,
    Flow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct QueueMethod {
    pub kind: MethodKind,
    pub id: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectMethod {
    kind: MethodKind,
    id: i64,
    name: String,
    site_id: Option<i64>,
    site_name: String,
    host: String,
    page_url: String,
    shape: Option<String>,
    step_count: Option<usize>,
    scopes: Option<Vec<FlowScope>>,
    dest_dir: Option<String>,
    preset: Option<Preset>,
    last_used_at: Option<i64>,
}

pub(crate) fn method_options(store: &Store, method: QueueMethod) -> Option<serde_json::Value> {
    match method.kind {
        MethodKind::Rule => store.get_rule(method.id).ok().map(|row| row.options),
        MethodKind::Flow => store
            .list_all_flows()
            .ok()?
            .into_iter()
            .find(|row| row.id == method.id)
            .map(|row| row.options),
    }
}

pub(crate) fn mark_method_used(store: &Store, method: QueueMethod, now: i64) -> Result<(), String> {
    match method.kind {
        MethodKind::Rule => store.set_rule_last_used(method.id, now),
        MethodKind::Flow => store.set_flow_last_used(method.id, now),
    }
    .map_err(|error| error.to_string())
}

pub(crate) fn record_method_use(
    store: &Store,
    method: Option<QueueMethod>,
    queued: bool,
    now: i64,
) -> Result<(), String> {
    if queued && let Some(method) = method {
        mark_method_used(store, method, now)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn methods_list(state: State<'_, AppState>) -> Result<Vec<CollectMethod>, String> {
    let store = lock_recover(&state.store);
    list_methods(&store)
}

fn list_methods(store: &Store) -> Result<Vec<CollectMethod>, String> {
    let registered = sites::list_sites(store)?;
    let mut result = Vec::new();
    for rule in store.list_rules().map_err(|error| error.to_string())? {
        let page_url = rule
            .match_pattern
            .strip_suffix('*')
            .unwrap_or(&rule.match_pattern);
        let parsed = host_and_name(page_url);
        let (host, fallback_name, site_id) = match parsed {
            Some((host, fallback_name)) => (
                host,
                fallback_name,
                sites::site_for_url_in(&registered, page_url),
            ),
            None => (page_url.to_owned(), page_url.to_owned(), None),
        };
        let site_name = registered
            .iter()
            .find(|site| Some(site.id) == site_id)
            .map(|site| site.name.clone())
            .unwrap_or(fallback_name);
        result.push(CollectMethod {
            kind: MethodKind::Rule,
            id: rule.id,
            name: rule.name,
            site_id,
            site_name,
            host,
            page_url: page_url.to_owned(),
            shape: rule
                .rule_json
                .pointer("/items/href")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            step_count: None,
            scopes: None,
            dest_dir: overrides_dest_dir(&rule.options),
            preset: overrides_preset(&rule.options),
            last_used_at: rule.last_used_at,
        });
    }
    for flow in store.list_all_flows().map_err(|error| error.to_string())? {
        let site_name = match host_and_name(&flow.start_url) {
            Some((_, fallback_name)) => flow
                .site_id
                .and_then(|id| registered.iter().find(|site| site.id == id))
                .map(|site| site.name.clone())
                .unwrap_or(fallback_name),
            None => flow
                .host
                .strip_prefix("www.")
                .unwrap_or(&flow.host)
                .to_owned(),
        };
        let step_count = flow.steps_json.as_array().map_or(0, Vec::len);
        let scopes = serde_json::from_value(flow.scopes_json.clone()).unwrap_or_default();
        result.push(CollectMethod {
            kind: MethodKind::Flow,
            id: flow.id,
            name: flow.name,
            site_id: flow.site_id,
            site_name,
            host: flow.host,
            page_url: flow.start_url,
            shape: None,
            step_count: Some(step_count),
            scopes: Some(scopes),
            dest_dir: overrides_dest_dir(&flow.options),
            preset: overrides_preset(&flow.options),
            last_used_at: flow.last_used_at,
        });
    }
    Ok(result)
}

#[tauri::command]
pub async fn method_set_options(
    kind: MethodKind,
    id: i64,
    dest_dir: Option<String>,
    preset: Option<Preset>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate_dest_dir(dest_dir.as_deref())?;
    let store = lock_recover(&state.store);
    match kind {
        MethodKind::Rule => {
            let row = store.get_rule(id).map_err(|error| error.to_string())?;
            store.set_rule_options(id, with_options(row.options, dest_dir.as_deref(), preset))
        }
        MethodKind::Flow => {
            let row = store
                .list_all_flows()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|row| row.id == id)
                .ok_or_else(|| format!("flow {id} was not found"))?;
            store.set_flow_options(id, with_options(row.options, dest_dir.as_deref(), preset))
        }
    }
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn method_rename(
    kind: MethodKind,
    id: i64,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name must not be empty".to_owned());
    }
    let now = unix_now();
    match kind {
        MethodKind::Rule => lock_recover(&state.store)
            .rename_rule(id, name, now)
            .map_err(|error| error.to_string()),
        MethodKind::Flow => state.flow.rename(id, name, now),
    }
}

#[tauri::command]
pub async fn method_delete(
    kind: MethodKind,
    id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    match kind {
        MethodKind::Rule => {
            let store = lock_recover(&state.store);
            store.get_rule(id).map_err(|error| error.to_string())?;
            if let Some(source) = store
                .list_sources()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|source| source.rule_id == Some(id))
            {
                return Err(format!("rule-in-use:{}", source.name));
            }
            store.delete_rule(id).map_err(|error| error.to_string())
        }
        MethodKind::Flow => {
            state.flow.delete(id)?;
            crate::account_commands::emit_sites(&app, &state)
        }
    }
}

fn host_and_name(url: &str) -> Option<(String, String)> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    if host.contains('*') {
        return None;
    }
    Some((
        host.clone(),
        host.strip_prefix("www.").unwrap_or(&host).to_owned(),
    ))
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vids_core::store::NewFlow;

    #[test]
    fn unknown_method_is_ignored_and_queued_method_records_last_use() {
        let store = Store::open_in_memory().unwrap();
        let id = store
            .add_rule("Rule", "https://example.com/*", json!({"name": "Rule"}), 1)
            .unwrap();
        let known = QueueMethod {
            kind: MethodKind::Rule,
            id,
        };
        let unknown = QueueMethod {
            kind: MethodKind::Rule,
            id: id + 1,
        };

        assert_eq!(method_options(&store, unknown), None);
        record_method_use(&store, Some(known), false, 50).unwrap();
        assert_eq!(store.get_rule(id).unwrap().last_used_at, None);
        record_method_use(&store, Some(known), true, 60).unwrap();
        assert_eq!(store.get_rule(id).unwrap().last_used_at, Some(60));
    }

    #[test]
    fn invalid_rule_url_does_not_hide_the_other_rules() {
        let store = Store::open_in_memory().unwrap();
        store
            .add_rule(
                "Wildcard host",
                "https://*.example.com/*",
                json!({"items": {"href": "/watch/{id}"}}),
                1,
            )
            .unwrap();
        store
            .add_rule(
                "Normal",
                "https://www.example.org/videos*",
                json!({"items": {"href": "/watch/{id}"}}),
                2,
            )
            .unwrap();

        let methods = list_methods(&store).unwrap();
        assert_eq!(methods.len(), 2);
        let invalid = methods
            .iter()
            .find(|method| method.name == "Wildcard host")
            .unwrap();
        assert_eq!(invalid.page_url, "https://*.example.com/");
        assert_eq!(invalid.host, invalid.page_url);
        assert_eq!(invalid.site_name, invalid.page_url);
        assert_eq!(invalid.site_id, None);
        let normal = methods
            .iter()
            .find(|method| method.name == "Normal")
            .unwrap();
        assert_eq!(normal.host, "www.example.org");
        assert_eq!(normal.site_name, "example.org");
    }

    #[test]
    fn invalid_flow_fields_use_row_fallbacks() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert_flow(
                NewFlow {
                    site_id: None,
                    host: "www.saved-host.example".to_owned(),
                    name: "Broken flow".to_owned(),
                    start_url: "not a URL".to_owned(),
                    steps_json: json!("not steps"),
                    scopes_json: json!({"not": "scopes"}),
                },
                1,
            )
            .unwrap();

        let methods = list_methods(&store).unwrap();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].site_name, "saved-host.example");
        assert_eq!(methods[0].step_count, Some(0));
        assert_eq!(methods[0].scopes, Some(Vec::new()));
    }
}

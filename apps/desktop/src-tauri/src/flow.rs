//! Prototype recorder and replay for generic click flows in the in-app browser.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, State, Url};
use vids_core::page_media::{MediaCandidate, add_candidate};
use vids_core::store::{Flow as StoredFlow, NewFlow, Store};

use crate::browser::{BrowserManager, BrowserProfile, MEDIA_SCRIPT, ResourceResponse};
use crate::page_media::PLAY_SCRIPT;
use crate::sites;
use crate::state::{AppState, lock_recover};

const RECORD_POLL: Duration = Duration::from_millis(500);
const STEP_POLL: Duration = Duration::from_millis(250);
const STEP_LIMIT: Duration = Duration::from_secs(10);
const MEDIA_LIMIT: Duration = Duration::from_secs(20);
const PLAY_AFTER: Duration = Duration::from_secs(3);
const REPLAY_PAUSE: Duration = Duration::from_secs(1);
const MAX_REPLAYS: usize = 30;
const MAX_NAME_CHARS: usize = 80;
const STEP_MISMATCH_PREFIX: &str = "flow-step-mismatch:";
const RECORDER_SCRIPT: &str = include_str!("flow_recorder.js");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowStep {
    pub pattern: String,
    pub label: String,
    pub url: String,
    pub match_count: usize,
    pub row_index: usize,
    pub row_count: usize,
    pub page_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flow {
    pub start_url: String,
    pub steps: Vec<FlowStep>,
    pub media: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowSaveInput {
    pub start_url: String,
    pub steps: Vec<FlowStep>,
    pub scopes: Vec<FlowScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFlow {
    pub id: i64,
    pub site_id: Option<i64>,
    pub host: String,
    pub name: String,
    pub start_url: String,
    pub steps: Vec<FlowStep>,
    pub scopes: Vec<FlowScope>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlowScope {
    This,
    RestOfRow,
    AllOfKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowReplayItem {
    pub replay_label: String,
    pub media: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowReplayResult {
    pub results: Vec<FlowReplayItem>,
    pub capped: bool,
    pub total: usize,
}

#[derive(Debug)]
struct RecordSession {
    generation: u64,
    start_url: String,
    steps: Vec<FlowStep>,
    media: Option<String>,
    seen_media: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplayPlan {
    choices: Vec<Vec<usize>>,
    capped: bool,
}

pub struct FlowManager {
    browser: Arc<BrowserManager>,
    store: Arc<Mutex<Store>>,
    recording: Mutex<Option<RecordSession>>,
    record_generation: AtomicU64,
    replay_running: AtomicBool,
    replay_cancelled: AtomicBool,
}

impl FlowManager {
    pub fn new(browser: Arc<BrowserManager>, store: Arc<Mutex<Store>>) -> Self {
        Self {
            browser,
            store,
            recording: Mutex::new(None),
            record_generation: AtomicU64::new(0),
            replay_running: AtomicBool::new(false),
            replay_cancelled: AtomicBool::new(false),
        }
    }

    pub fn start(self: &Arc<Self>, app: &AppHandle) -> Result<(), String> {
        let start_url = self
            .browser
            .current_page_url()
            .ok_or_else(|| "browser page is not open".to_owned())?;
        validate_url(&start_url)?;
        let generation = self.record_generation.fetch_add(1, Ordering::SeqCst) + 1;
        *lock_recover(&self.recording) = Some(RecordSession {
            generation,
            start_url,
            steps: Vec::new(),
            media: None,
            seen_media: HashSet::new(),
        });
        self.browser.set_page_load_script(Some(RECORDER_SCRIPT));
        let manager = Arc::clone(self);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                if manager.poll_recording(&app, generation).await.is_err() {
                    return;
                }
                tokio::time::sleep(RECORD_POLL).await;
            }
        });
        Ok(())
    }

    pub async fn stop(&self, app: &AppHandle) -> Result<Flow, String> {
        let generation = lock_recover(&self.recording)
            .as_ref()
            .map(|session| session.generation)
            .ok_or_else(|| "no flow recording is open".to_owned())?;
        let _ = self.poll_recording(app, generation).await;
        self.record_generation.fetch_add(1, Ordering::SeqCst);
        self.browser.set_page_load_script(None);
        let session = lock_recover(&self.recording)
            .take()
            .ok_or_else(|| "flow recording changed while stopping".to_owned())?;
        Ok(Flow {
            start_url: session.start_url,
            steps: session.steps,
            media: session.media,
        })
    }

    async fn poll_recording(&self, app: &AppHandle, generation: u64) -> Result<(), String> {
        if self.record_generation.load(Ordering::SeqCst) != generation {
            return Err("recording ended".to_owned());
        }
        let script = format!("{RECORDER_SCRIPT}\nwindow.__vdlFlowRecorder.drain()");
        let drained = self
            .browser
            .execute(&script)
            .await
            .ok()
            .and_then(|value| serde_json::from_value::<Vec<FlowStep>>(value).ok())
            .unwrap_or_default();
        let current_url = self.browser.current_page_url();
        let played = current_url
            .as_deref()
            .map(|url| self.browser.played_media(url))
            .unwrap_or_default();
        let mut recording = lock_recover(&self.recording);
        let session = recording
            .as_mut()
            .filter(|session| session.generation == generation)
            .ok_or_else(|| "recording ended".to_owned())?;
        let added = drained.len();
        if added > 0 {
            session.steps.extend(drained);
            session.media = None;
        }
        let new_media = played
            .iter()
            .find(|media| !session.seen_media.contains(&media.url))
            .map(|media| media.url.clone());
        session
            .seen_media
            .extend(played.into_iter().map(|media| media.url));
        if !session.steps.is_empty() && session.media.is_none() {
            session.media = new_media;
        }
        let step_count = session.steps.len();
        drop(recording);
        if added > 0 {
            let _ = app.emit("flow-record-progress", json!({ "steps": step_count }));
        }
        Ok(())
    }

    pub fn cancel_replay(&self) {
        self.replay_cancelled.store(true, Ordering::SeqCst);
    }

    pub fn save(&self, name: &str, flow: FlowSaveInput, now: i64) -> Result<SavedFlow, String> {
        let name = validate_name(name)?;
        let replay_flow = Flow {
            start_url: flow.start_url.clone(),
            steps: flow.steps.clone(),
            media: None,
        };
        validate_flow(&replay_flow, &flow.scopes)?;
        let host = host_for_url(&flow.start_url)?;
        let store = lock_recover(&self.store);
        let site_id = sites::site_for_url(&store, &flow.start_url)?;
        let id = store
            .insert_flow(
                NewFlow {
                    site_id,
                    host: host.clone(),
                    name: name.clone(),
                    start_url: flow.start_url.clone(),
                    steps_json: serde_json::to_value(&flow.steps)
                        .map_err(|error| error.to_string())?,
                    scopes_json: serde_json::to_value(&flow.scopes)
                        .map_err(|error| error.to_string())?,
                },
                now,
            )
            .map_err(|error| error.to_string())?;
        Ok(SavedFlow {
            id,
            site_id,
            host,
            name,
            start_url: flow.start_url,
            steps: flow.steps,
            scopes: flow.scopes,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn list(&self, url: &str) -> Result<Vec<SavedFlow>, String> {
        let host = host_for_url(url)?;
        lock_recover(&self.store)
            .list_flows_by_host(&host)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(saved_flow_from_store)
            .collect()
    }

    pub fn list_for_site(&self, site_id: i64) -> Result<Vec<SavedFlow>, String> {
        lock_recover(&self.store)
            .list_flows_by_site(site_id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(saved_flow_from_store)
            .collect()
    }

    pub fn rename(&self, id: i64, name: &str, now: i64) -> Result<(), String> {
        let name = validate_name(name)?;
        lock_recover(&self.store)
            .rename_flow(id, &name, now)
            .map_err(|error| error.to_string())
    }

    pub fn delete(&self, id: i64) -> Result<(), String> {
        lock_recover(&self.store)
            .delete_flow(id)
            .map_err(|error| error.to_string())
    }

    pub async fn replay(
        &self,
        app: &AppHandle,
        flow: Flow,
        scopes: Vec<FlowScope>,
    ) -> Result<FlowReplayResult, String> {
        if self.replay_running.swap(true, Ordering::SeqCst) {
            return Err("a flow replay is already running".to_owned());
        }
        self.replay_cancelled.store(false, Ordering::SeqCst);
        let result = self.replay_inner(app, flow, scopes).await;
        self.replay_running.store(false, Ordering::SeqCst);
        result
    }

    async fn replay_inner(
        &self,
        app: &AppHandle,
        flow: Flow,
        scopes: Vec<FlowScope>,
    ) -> Result<FlowReplayResult, String> {
        validate_flow(&flow, &scopes)?;
        let plan = replay_plan(&flow.steps, &scopes, MAX_REPLAYS);
        let total = plan.choices.len();
        let profile = self.profile_for_url(&flow.start_url);
        let start_url = Url::parse(&flow.start_url).map_err(|error| error.to_string())?;
        let mut results = Vec::with_capacity(total);
        let _ = app.emit("flow-progress", json!({ "done": 0, "total": total }));

        for (replay_index, choices) in plan.choices.iter().enumerate() {
            self.ensure_not_cancelled()?;
            let mut page = self
                .browser
                .open_media_page(app, profile, start_url.clone())?;
            let mut responses = page.take_responses();
            let mut baseline = HashSet::new();
            let mut clicked_labels = Vec::with_capacity(flow.steps.len());
            for (step_index, (step, choice)) in flow.steps.iter().zip(choices).enumerate() {
                let clicked = self
                    .wait_and_click(
                        &page,
                        step,
                        *choice,
                        step_index + 1 == flow.steps.len(),
                        &mut responses,
                        &mut baseline,
                    )
                    .await?;
                clicked_labels.push(clicked);
            }
            let media = self
                .wait_for_media(&page, &mut responses, &baseline)
                .await?;
            let replay_label = replay_label(&flow.steps, &scopes, &clicked_labels);
            results.push(FlowReplayItem {
                replay_label,
                media,
            });
            let _ = app.emit(
                "flow-progress",
                json!({ "done": replay_index + 1, "total": total }),
            );
            drop(page);
            if replay_index + 1 < total {
                self.cancelable_sleep(REPLAY_PAUSE).await?;
            }
        }
        Ok(FlowReplayResult {
            results,
            capped: plan.capped,
            total,
        })
    }

    async fn wait_and_click(
        &self,
        page: &crate::browser::CheckPage,
        step: &FlowStep,
        choice: usize,
        last: bool,
        responses: &mut Option<tokio::sync::mpsc::UnboundedReceiver<ResourceResponse>>,
        baseline: &mut HashSet<String>,
    ) -> Result<String, String> {
        let started = Instant::now();
        loop {
            self.ensure_not_cancelled()?;
            if started.elapsed() >= STEP_LIMIT {
                return Err(format!("{STEP_MISMATCH_PREFIX}{}", step.label));
            }
            let script = click_script(&step.pattern, choice, false);
            let value = page.execute(&script).await.unwrap_or(Value::Null);
            if value.get("ready").and_then(Value::as_bool) == Some(true) {
                if last {
                    drain_responses(responses, baseline, None);
                    collect_timing_urls(page, baseline).await;
                }
                let clicked = page
                    .execute(&click_script(&step.pattern, choice, true))
                    .await?;
                if clicked.get("clicked").and_then(Value::as_bool) == Some(true) {
                    // The text of the element actually pressed, so each result is named after
                    // its own card rather than the one pressed while recording.
                    let label = clicked
                        .get("label")
                        .and_then(Value::as_str)
                        .filter(|label| !label.is_empty())
                        .unwrap_or(&step.label);
                    return Ok(label.to_owned());
                }
            }
            self.cancelable_sleep(STEP_POLL).await?;
        }
    }

    async fn wait_for_media(
        &self,
        page: &crate::browser::CheckPage,
        responses: &mut Option<tokio::sync::mpsc::UnboundedReceiver<ResourceResponse>>,
        baseline: &HashSet<String>,
    ) -> Result<Option<String>, String> {
        let started = Instant::now();
        let mut played = false;
        let mut candidates = Vec::new();
        loop {
            self.ensure_not_cancelled()?;
            drain_responses(
                responses,
                &mut HashSet::new(),
                Some((&mut candidates, baseline)),
            );
            if let Ok(value) = page.execute(MEDIA_SCRIPT).await {
                observe_timing(&mut candidates, baseline, &value);
            }
            if let Some(candidate) = candidates.first() {
                return Ok(Some(candidate.url.clone()));
            }
            let elapsed = started.elapsed();
            if elapsed >= MEDIA_LIMIT {
                return Ok(None);
            }
            if !played && elapsed >= PLAY_AFTER {
                played = true;
                let _ = page.execute(PLAY_SCRIPT).await;
            }
            self.cancelable_sleep(STEP_POLL).await?;
        }
    }

    async fn cancelable_sleep(&self, duration: Duration) -> Result<(), String> {
        let mut elapsed = Duration::ZERO;
        while elapsed < duration {
            self.ensure_not_cancelled()?;
            let slice = (duration - elapsed).min(STEP_POLL);
            tokio::time::sleep(slice).await;
            elapsed += slice;
        }
        Ok(())
    }

    fn ensure_not_cancelled(&self) -> Result<(), String> {
        if self.replay_cancelled.load(Ordering::SeqCst) {
            Err("flow replay canceled".to_owned())
        } else {
            Ok(())
        }
    }

    fn profile_for_url(&self, url: &str) -> BrowserProfile {
        sites::selected_account_for_url(&lock_recover(&self.store), url)
            .ok()
            .flatten()
            .map_or(BrowserProfile::General, |(_, account)| {
                BrowserProfile::Account(account.id)
            })
    }
}

fn validate_url(url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|error| error.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("flow URL must be an http(s) URL with a host".to_owned());
    }
    Ok(())
}

fn host_for_url(url: &str) -> Result<String, String> {
    let parsed = Url::parse(url).map_err(|error| error.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("flow URL must be an http(s) URL with a host".to_owned());
    }
    parsed
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "flow URL must be an http(s) URL with a host".to_owned())
}

fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("flow name must not be empty".to_owned());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "flow name must be at most {MAX_NAME_CHARS} characters"
        ));
    }
    Ok(name.to_owned())
}

fn saved_flow_from_store(flow: StoredFlow) -> Result<SavedFlow, String> {
    let steps: Vec<FlowStep> =
        serde_json::from_value(flow.steps_json).map_err(|error| error.to_string())?;
    let scopes: Vec<FlowScope> =
        serde_json::from_value(flow.scopes_json).map_err(|error| error.to_string())?;
    validate_flow(
        &Flow {
            start_url: flow.start_url.clone(),
            steps: steps.clone(),
            media: None,
        },
        &scopes,
    )?;
    Ok(SavedFlow {
        id: flow.id,
        site_id: flow.site_id,
        host: flow.host,
        name: flow.name,
        start_url: flow.start_url,
        steps,
        scopes,
        created_at: flow.created_at,
        updated_at: flow.updated_at,
    })
}

fn validate_flow(flow: &Flow, scopes: &[FlowScope]) -> Result<(), String> {
    validate_url(&flow.start_url)?;
    if flow.steps.is_empty() {
        return Err("the flow has no steps".to_owned());
    }
    if flow.steps.len() != scopes.len() {
        return Err("one replay scope is required for every step".to_owned());
    }
    for (step, scope) in flow.steps.iter().zip(scopes) {
        if step.pattern.is_empty() || step.pattern.len() > 2_048 || step.label.len() > 240 {
            return Err("the flow contains an invalid step".to_owned());
        }
        if *scope != FlowScope::This && step.match_count < 2 {
            return Err("a step with one match cannot repeat".to_owned());
        }
        if step.match_count == 0
            || step.page_index >= step.match_count
            || step.row_count == 0
            || step.row_index >= step.row_count
        {
            return Err("the flow contains invalid match positions".to_owned());
        }
    }
    Ok(())
}

fn indexes(step: &FlowStep, scope: FlowScope) -> Vec<usize> {
    match scope {
        FlowScope::This => vec![step.page_index],
        FlowScope::RestOfRow => {
            let row_start = step.page_index.saturating_sub(step.row_index);
            (step.page_index
                ..row_start
                    .saturating_add(step.row_count)
                    .min(step.match_count))
                .collect()
        }
        FlowScope::AllOfKind => (0..step.match_count).collect(),
    }
}

fn replay_plan(steps: &[FlowStep], scopes: &[FlowScope], cap: usize) -> ReplayPlan {
    let options = steps
        .iter()
        .zip(scopes)
        .map(|(step, scope)| indexes(step, *scope))
        .collect::<Vec<_>>();
    let uncapped = options
        .iter()
        .fold(1_usize, |total, values| total.saturating_mul(values.len()));
    let mut choices = vec![Vec::new()];
    for values in options {
        let mut next = Vec::new();
        'outer: for prefix in &choices {
            for value in &values {
                if next.len() == cap {
                    break 'outer;
                }
                let mut row = prefix.clone();
                row.push(*value);
                next.push(row);
            }
        }
        choices = next;
    }
    ReplayPlan {
        choices,
        capped: uncapped > cap,
    }
}

/// Names one replay by the elements its repeating steps pressed (`clicked` holds the text of
/// each step's pressed element), or by the last step when nothing repeats.
fn replay_label(steps: &[FlowStep], scopes: &[FlowScope], clicked: &[String]) -> String {
    let labels = scopes
        .iter()
        .zip(clicked)
        .filter(|(scope, _)| **scope != FlowScope::This)
        .map(|(_, label)| label.clone())
        .collect::<Vec<_>>();
    if labels.is_empty() {
        clicked
            .last()
            .cloned()
            .or_else(|| steps.last().map(|step| step.label.clone()))
            .unwrap_or_default()
    } else {
        labels.join(" / ")
    }
}

fn click_script(pattern: &str, index: usize, click: bool) -> String {
    let pattern = serde_json::to_string(pattern).expect("a string serializes");
    format!(
        r#"(() => {{
          try {{
            const matches = [...document.querySelectorAll({pattern})];
            const element = matches[{index}];
            if (!element) return {{ ready: false, count: matches.length }};
            if ({click}) {{
              element.scrollIntoView({{ block: "center", inline: "nearest", behavior: "auto" }});
              const label = (element.getAttribute("aria-label") || element.innerText || "")
                .replace(/\s+/g, " ").trim().slice(0, 60);
              element.click();
              return {{ ready: true, clicked: true, count: matches.length, label }};
            }}
            return {{ ready: true, clicked: false, count: matches.length }};
          }} catch (_) {{
            return {{ ready: false, clicked: false, count: 0 }};
          }}
        }})()"#,
    )
}

async fn collect_timing_urls(page: &crate::browser::CheckPage, urls: &mut HashSet<String>) {
    if let Ok(value) = page.execute(MEDIA_SCRIPT).await
        && let Some(values) = value.as_array()
    {
        urls.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
    }
}

fn drain_responses(
    responses: &mut Option<tokio::sync::mpsc::UnboundedReceiver<ResourceResponse>>,
    seen: &mut HashSet<String>,
    mut candidates: Option<(&mut Vec<MediaCandidate>, &HashSet<String>)>,
) {
    while let Some(response) = responses
        .as_mut()
        .and_then(|receiver| receiver.try_recv().ok())
    {
        if let Some((items, baseline)) = candidates.as_mut()
            && !baseline.contains(&response.url)
        {
            add_candidate(
                items,
                MediaCandidate {
                    url: response.url.clone(),
                    status: response.status,
                    content_type: response.content_type.clone(),
                    content_length: response.content_length,
                },
            );
        }
        seen.insert(response.url);
    }
}

fn observe_timing(candidates: &mut Vec<MediaCandidate>, baseline: &HashSet<String>, value: &Value) {
    let Some(urls) = value.as_array() else {
        return;
    };
    for url in urls.iter().filter_map(Value::as_str) {
        if baseline.contains(url) {
            continue;
        }
        add_candidate(
            candidates,
            MediaCandidate {
                url: url.to_owned(),
                status: 200,
                content_type: None,
                content_length: None,
            },
        );
    }
}

#[tauri::command]
pub async fn flow_record_start(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.flow.start(&app)
}

#[tauri::command]
pub async fn flow_record_stop(app: AppHandle, state: State<'_, AppState>) -> Result<Flow, String> {
    state.flow.stop(&app).await
}

#[tauri::command]
pub async fn flow_save(
    name: String,
    flow: FlowSaveInput,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SavedFlow, String> {
    let saved = state.flow.save(&name, flow, unix_now())?;
    crate::account_commands::emit_sites(&app, &state)?;
    Ok(saved)
}

#[tauri::command]
pub async fn flow_list(url: String, state: State<'_, AppState>) -> Result<Vec<SavedFlow>, String> {
    state.flow.list(&url)
}

#[tauri::command]
pub async fn flow_list_for_site(
    site_id: i64,
    state: State<'_, AppState>,
) -> Result<Vec<SavedFlow>, String> {
    state.flow.list_for_site(site_id)
}

#[tauri::command]
pub async fn flow_rename(id: i64, name: String, state: State<'_, AppState>) -> Result<(), String> {
    state.flow.rename(id, &name, unix_now())
}

#[tauri::command]
pub async fn flow_delete(
    id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.flow.delete(id)?;
    crate::account_commands::emit_sites(&app, &state)
}

#[tauri::command]
pub async fn flow_replay(
    flow: Flow,
    scopes: Vec<FlowScope>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<FlowReplayResult, String> {
    state.flow.replay(&app, flow, scopes).await
}

#[tauri::command]
pub async fn flow_replay_cancel(state: State<'_, AppState>) -> Result<(), String> {
    state.flow.cancel_replay();
    Ok(())
}

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::{FlowScope, FlowStep, indexes, replay_label, replay_plan};

    fn step(match_count: usize, row_index: usize, row_count: usize, page_index: usize) -> FlowStep {
        FlowStep {
            pattern: "div.card".to_owned(),
            label: "Card".to_owned(),
            url: "https://example.test/".to_owned(),
            match_count,
            row_index,
            row_count,
            page_index,
        }
    }

    #[test]
    fn rest_of_row_from_second_of_five_uses_indexes_one_through_four() {
        assert_eq!(
            indexes(&step(10, 1, 5, 1), FlowScope::RestOfRow),
            (1..5).collect::<Vec<_>>()
        );
    }

    #[test]
    fn all_of_kind_over_two_rows_uses_all_ten() {
        assert_eq!(
            indexes(&step(10, 1, 5, 1), FlowScope::AllOfKind),
            (0..10).collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_repeating_steps_form_an_ordered_nested_product() {
        let plan = replay_plan(
            &[step(3, 0, 3, 0), step(2, 0, 2, 0)],
            &[FlowScope::AllOfKind, FlowScope::AllOfKind],
            30,
        );
        assert_eq!(
            plan.choices,
            vec![
                vec![0, 0],
                vec![0, 1],
                vec![1, 0],
                vec![1, 1],
                vec![2, 0],
                vec![2, 1]
            ]
        );
        assert!(!plan.capped);
    }

    #[test]
    fn replay_product_is_capped_at_thirty() {
        let plan = replay_plan(
            &[step(10, 0, 5, 0), step(10, 0, 5, 0)],
            &[FlowScope::AllOfKind, FlowScope::AllOfKind],
            30,
        );
        assert_eq!(plan.choices.len(), 30);
        assert!(plan.capped);
    }

    #[test]
    fn replay_label_names_the_element_each_replay_pressed() {
        let clicked = ["動画 3 サンプル番組".to_owned(), "再生".to_owned()];
        assert_eq!(
            replay_label(&[], &[FlowScope::RestOfRow, FlowScope::This], &clicked),
            "動画 3 サンプル番組"
        );
        assert_eq!(
            replay_label(&[], &[FlowScope::This, FlowScope::This], &clicked),
            "再生"
        );
    }
}

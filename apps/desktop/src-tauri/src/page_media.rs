//! Find media loaded by a page that yt-dlp itself does not support.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Url};
use vids_core::args::Preset;
use vids_core::executor::{PageMedia, PageMediaChoice};
use vids_core::page_media::{MediaCandidate, MediaChoice, PlayerHint, QualityLimit, add_candidate};
use vids_core::queue::{CancelToken, JobFailure};
use vids_core::store::Store;
use vids_core::types::ErrorCategory;

use crate::browser::{BrowserManager, BrowserProfile, MEDIA_SCRIPT, ResourceResponse};
use crate::collector::COLLECTOR_SCRIPT;
use crate::sites;
use crate::state::lock_recover;

const FIRST_CANDIDATE_GRACE: Duration = Duration::from_secs(3);
const PLAY_AFTER: Duration = Duration::from_secs(10);
const SEARCH_LIMIT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) const PLAY_SCRIPT: &str = r#"(() => {
  document.querySelectorAll("video").forEach((video) => {
    video.muted = true;
    video.play().catch(() => {});
  });
  return true;
})()"#;

const PLAYER_HINTS_SCRIPT: &str = r#"(() => {
  try {
    return Array.from(document.querySelectorAll("video"), (video) => {
      const rect = video.getBoundingClientRect();
      return {
        src: video.currentSrc,
        area: Math.round(rect.width * rect.height),
        playing: !video.paused && !video.ended,
      };
    });
  } catch (_) {
    return [];
  }
})()"#;

pub struct PageMediaRequest {
    pub url: String,
    pub preset: Preset,
    pub cancel: CancelToken,
}

pub enum PageMediaSearchResult {
    One(PageMedia),
    Ask {
        media: PageMedia,
        choices: Vec<PageMediaChoice>,
    },
}

pub trait PageMediaFinder: Send + Sync {
    fn find(
        &self,
        request: PageMediaRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PageMediaSearchResult>, JobFailure>> + Send + '_>>;
}

pub struct BrowserPageMediaFinder {
    app: AppHandle,
    browser: Arc<BrowserManager>,
    store: Arc<Mutex<Store>>,
    one_at_a_time: tokio::sync::Mutex<()>,
}

impl BrowserPageMediaFinder {
    pub fn new(app: AppHandle, browser: Arc<BrowserManager>, store: Arc<Mutex<Store>>) -> Self {
        Self {
            app,
            browser,
            store,
            one_at_a_time: tokio::sync::Mutex::new(()),
        }
    }

    async fn search(
        &self,
        request: PageMediaRequest,
    ) -> Result<Option<PageMediaSearchResult>, JobFailure> {
        let _search = tokio::select! {
            guard = self.one_at_a_time.lock() => guard,
            () = request.cancel.cancelled() => return Err(canceled()),
        };
        let url = Url::parse(&request.url).map_err(|error| failure(error.to_string()))?;
        let profile = self.profile_for_url(&request.url);
        let mut page = self
            .browser
            .open_media_page(&self.app, profile, url)
            .map_err(failure)?;
        let mut responses = page.take_responses();
        let started = Instant::now();
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut candidates = Vec::new();
        let mut first_candidate = None;
        let mut facts = None;
        let mut played = false;

        loop {
            if request.cancel.is_cancelled() {
                return Err(canceled());
            }
            while let Some(response) = responses
                .as_mut()
                .and_then(|receiver| receiver.try_recv().ok())
            {
                observe_response(&mut candidates, response);
            }
            if first_candidate.is_none() && !candidates.is_empty() {
                first_candidate = Some(Instant::now());
            }
            let elapsed = started.elapsed();
            if first_candidate.is_some_and(|at| at.elapsed() >= FIRST_CANDIDATE_GRACE)
                || elapsed >= SEARCH_LIMIT
            {
                break;
            }

            if facts.is_none() && page.is_loaded() {
                facts = execute(&page, &facts_script(), &request.cancel)
                    .await
                    .ok()
                    .and_then(parse_facts);
            }
            if !played && elapsed >= PLAY_AFTER && candidates.is_empty() {
                played = true;
                let _ = execute(&page, PLAY_SCRIPT, &request.cancel).await;
            }

            let tick = async {
                ticker.tick().await;
            };
            tokio::select! {
                () = request.cancel.cancelled() => return Err(canceled()),
                () = tick => {
                    if let Ok(value) = execute(&page, MEDIA_SCRIPT, &request.cancel).await {
                        observe_urls(&mut candidates, &value);
                    }
                }
            }
        }

        if facts.is_none() {
            facts = execute(&page, &facts_script(), &request.cancel)
                .await
                .ok()
                .and_then(parse_facts);
        }
        let players = execute(&page, PLAYER_HINTS_SCRIPT, &request.cancel)
            .await
            .ok()
            .and_then(|value| serde_json::from_value::<Vec<PlayerHint>>(value).ok())
            .unwrap_or_default();
        drop(page);
        if candidates.is_empty() {
            return Ok(None);
        }
        let referer = request.url.clone();
        let limit = QualityLimit::from(request.preset);
        let chosen = tokio::select! {
            () = request.cancel.cancelled() => return Err(canceled()),
            result = tokio::task::spawn_blocking(move || vids_core::page_media::choose_remote_media(&candidates, &referer, limit, &players)) => {
                result.map_err(|error| failure(format!("page media selector failed: {error}")))?
            }
        };
        let facts = facts.unwrap_or_default();
        let make_media = |url| PageMedia {
            url,
            referer: request.url.clone(),
            title: bounded(facts.title.clone(), 300),
            uploader: facts.uploader.clone().and_then(|value| bounded(value, 200)),
            published_at: facts.published_at,
        };
        match chosen {
            MediaChoice::One(url) => Ok(Some(PageMediaSearchResult::One(make_media(url)))),
            MediaChoice::Ask(choices) => {
                let media = make_media(choices[0].url.clone());
                let choices = choices
                    .into_iter()
                    .map(|choice| PageMediaChoice {
                        url: choice.url,
                        kind: choice.kind.label().to_owned(),
                        duration_seconds: choice.duration_seconds,
                    })
                    .collect();
                Ok(Some(PageMediaSearchResult::Ask { media, choices }))
            }
            MediaChoice::None => Ok(None),
        }
    }

    fn profile_for_url(&self, url: &str) -> BrowserProfile {
        let selected = sites::selected_account_for_url(&lock_recover(&self.store), url)
            .ok()
            .flatten();
        match selected.map(|(_, account)| account.id) {
            Some(id) => BrowserProfile::Account(id),
            _ => BrowserProfile::General,
        }
    }
}

impl PageMediaFinder for BrowserPageMediaFinder {
    fn find(
        &self,
        request: PageMediaRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PageMediaSearchResult>, JobFailure>> + Send + '_>>
    {
        Box::pin(self.search(request))
    }
}

fn facts_script() -> String {
    format!("{COLLECTOR_SCRIPT}\nwindow.__vdlCollector.pageFacts()")
}

async fn execute(
    page: &crate::browser::CheckPage,
    script: &str,
    cancel: &CancelToken,
) -> Result<serde_json::Value, JobFailure> {
    tokio::select! {
        () = cancel.cancelled() => Err(canceled()),
        result = page.execute(script) => result.map_err(failure),
    }
}

fn observe_response(candidates: &mut Vec<MediaCandidate>, response: ResourceResponse) {
    add_candidate(
        candidates,
        MediaCandidate {
            url: response.url,
            status: response.status,
            content_type: response.content_type,
            content_length: response.content_length,
        },
    );
}

fn observe_urls(candidates: &mut Vec<MediaCandidate>, value: &serde_json::Value) {
    let Some(urls) = value.as_array() else {
        return;
    };
    for url in urls.iter().filter_map(serde_json::Value::as_str) {
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

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageFacts {
    #[serde(default)]
    title: String,
    uploader: Option<String>,
    published_at: Option<i64>,
}

fn parse_facts(value: serde_json::Value) -> Option<PageFacts> {
    serde_json::from_value(value).ok()
}

fn bounded(value: String, max: usize) -> Option<String> {
    let trimmed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(max).collect())
}

fn failure(message: impl Into<String>) -> JobFailure {
    JobFailure {
        category: ErrorCategory::Unknown,
        message: message.into(),
    }
}

fn canceled() -> JobFailure {
    failure("canceled")
}

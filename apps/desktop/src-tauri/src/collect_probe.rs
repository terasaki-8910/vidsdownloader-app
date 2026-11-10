//! Details for the collect tray's filter and sort: while the filter is open the tray asks for
//! each found item to be looked up with yt-dlp (one flat `-J` run on the item's page, with the
//! site's login cookies like any other run of the app), and what yt-dlp says about its
//! length, uploader and publish date comes back as a `collect-probe` event. Three runs go at
//! once; every result, failures included, is kept in memory by URL for the life of the app, so
//! an item is looked up once.
//!
//! `ProbePool` holds the queue, the limit of three, the cache and the cancel; the yt-dlp run
//! behind it is a `ProbeBackend`, so the pool is tested with a fake one.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use vids_core::args::Cookies;
use vids_core::executor::{ExecutorConfig, YtDlpExecutor};
use vids_core::item_meta::{ItemMeta, item_meta};
use vids_core::queue::CancelToken;

use crate::collector;
use crate::sources::managed_tools;
use crate::state::{self, AccountCookies, AppState, TARGET_OS, lock_recover};

/// Runs at once.
const MAX_CONCURRENT: usize = 3;
/// A run that takes longer than this ends as a failure (yt-dlp is ended).
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// The last resort around a whole run, cookies included, in case something never returns.
const PROBE_HARD_LIMIT: Duration = Duration::from_secs(120);
/// The most URLs one `collect_probe_start` (or snapshot) takes: a collect session's own limit.
const MAX_URLS_PER_CALL: usize = 2_000;
const MAX_URL_CHARS: usize = 2_048;
/// Results kept; the oldest are dropped first.
const CACHE_CAP: usize = 5_000;

/// How much is left to do: URLs waiting, and runs going on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub queued: usize,
    pub running: usize,
}

/// One finished look-up, as the tray receives it (`collect-probe`). `meta` is `None` when the
/// run failed, whatever the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeEvent {
    pub url: String,
    pub meta: Option<ItemMeta>,
    pub queued: usize,
    pub running: usize,
}

/// A cached result: `meta` is `None` for a failed look-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub url: String,
    pub meta: Option<ItemMeta>,
}

/// What `collect_probe_snapshot` returns: the cached results of the URLs asked for, and the
/// progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeSnapshot {
    pub results: Vec<ProbeResult>,
    pub queued: usize,
    pub running: usize,
}

/// The run behind the pool.
pub trait ProbeBackend: Send + Sync + 'static {
    /// Looks `url` up. `None`: it failed, whatever the reason. A run that `cancel` ends is
    /// not looked at.
    fn probe<'a>(
        &'a self,
        url: &'a str,
        cancel: &'a CancelToken,
    ) -> Pin<Box<dyn Future<Output = Option<ItemMeta>> + Send + 'a>>;

    /// Nothing is queued or running any more: what was prepared for the runs (cookie files)
    /// can go.
    fn idle(&self);
}

type EventSink = Arc<dyn Fn(ProbeEvent) + Send + Sync>;

/// The probes of one app run.
pub struct ProbePool<B: ProbeBackend> {
    backend: Arc<B>,
    on_event: EventSink,
    inner: Mutex<Inner>,
    /// Held while an event's progress is read and sent, so events reach the tray in the order
    /// their progress was read (the last one never shows a stale count).
    emit_gate: Mutex<()>,
}

struct Inner {
    cache: ResultCache,
    queue: VecDeque<String>,
    queued: HashSet<String>,
    running: HashMap<u64, Running>,
    next_job: u64,
}

struct Running {
    url: String,
    cancel: CancelToken,
}

impl Inner {
    /// Known, waiting or running (a run that was cancelled does not count: its result is
    /// dropped, so the URL has to be asked again).
    fn is_taken(&self, url: &str) -> bool {
        self.cache.get(url).is_some()
            || self.queued.contains(url)
            || self
                .running
                .values()
                .any(|run| run.url == url && !run.cancel.is_cancelled())
    }

    fn progress(&self) -> Progress {
        Progress {
            queued: self.queue.len(),
            running: self
                .running
                .values()
                .filter(|run| !run.cancel.is_cancelled())
                .count(),
        }
    }

    fn is_idle(&self) -> bool {
        self.queue.is_empty() && self.running.is_empty()
    }
}

/// Results by URL, the oldest dropped first beyond `cap`.
struct ResultCache {
    cap: usize,
    entries: HashMap<String, Option<ItemMeta>>,
    order: VecDeque<String>,
}

impl ResultCache {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, url: &str) -> Option<&Option<ItemMeta>> {
        self.entries.get(url)
    }

    fn insert(&mut self, url: String, meta: Option<ItemMeta>) {
        if self.entries.insert(url.clone(), meta).is_none() {
            self.order.push_back(url);
        }
        while self.entries.len() > self.cap {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }
}

/// The URLs of one call: at most 2000, each an http(s) URL of at most 2048 characters. One
/// that is not makes the whole call an error.
fn validate_urls(urls: Vec<String>) -> Result<Vec<String>, String> {
    if urls.len() > MAX_URLS_PER_CALL {
        return Err(format!("at most {MAX_URLS_PER_CALL} urls per call"));
    }
    if urls
        .iter()
        .any(|url| url.chars().count() > MAX_URL_CHARS || !collector::is_http_url(url))
    {
        return Err(format!(
            "every url must be an http(s) URL of at most {MAX_URL_CHARS} characters"
        ));
    }
    Ok(urls)
}

impl<B: ProbeBackend> ProbePool<B> {
    pub fn new(backend: Arc<B>, on_event: EventSink) -> Arc<Self> {
        Self::with_cache_cap(backend, on_event, CACHE_CAP)
    }

    fn with_cache_cap(backend: Arc<B>, on_event: EventSink, cache_cap: usize) -> Arc<Self> {
        Arc::new(Self {
            backend,
            on_event,
            inner: Mutex::new(Inner {
                cache: ResultCache::new(cache_cap),
                queue: VecDeque::new(),
                queued: HashSet::new(),
                running: HashMap::new(),
                next_job: 1,
            }),
            emit_gate: Mutex::new(()),
        })
    }

    /// Queues the URLs that are not known, queued or running, and starts runs up to the
    /// limit. Spawns tasks, so it needs a Tokio runtime.
    pub fn start(self: &Arc<Self>, urls: Vec<String>) -> Result<Progress, String> {
        let urls = validate_urls(urls)?;
        {
            let mut inner = lock_recover(&self.inner);
            for url in urls {
                if inner.is_taken(&url) {
                    continue;
                }
                inner.queued.insert(url.clone());
                inner.queue.push_back(url);
            }
        }
        self.pump();
        Ok(self.progress())
    }

    /// The cached results of those of `urls` that have one, in the order asked.
    pub fn snapshot(&self, urls: Vec<String>) -> Result<ProbeSnapshot, String> {
        let urls = validate_urls(urls)?;
        let inner = lock_recover(&self.inner);
        let results = urls
            .into_iter()
            .filter_map(|url| {
                let meta = inner.cache.get(&url)?.clone();
                Some(ProbeResult { url, meta })
            })
            .collect();
        let progress = inner.progress();
        Ok(ProbeSnapshot {
            results,
            queued: progress.queued,
            running: progress.running,
        })
    }

    /// Empties the queue and ends the runs that are going on; what they would have found is
    /// dropped (not cached, so a later start asks again). The cache is kept.
    pub fn cancel(&self) {
        {
            let mut inner = lock_recover(&self.inner);
            inner.queue.clear();
            inner.queued.clear();
            for run in inner.running.values() {
                run.cancel.cancel();
            }
        }
        self.release_if_idle();
    }

    pub fn progress(&self) -> Progress {
        lock_recover(&self.inner).progress()
    }

    /// Starts runs while there is room and something waits.
    fn pump(self: &Arc<Self>) {
        loop {
            let (id, url, cancel) = {
                let mut inner = lock_recover(&self.inner);
                if inner.running.len() >= MAX_CONCURRENT {
                    break;
                }
                let Some(url) = inner.queue.pop_front() else {
                    break;
                };
                inner.queued.remove(&url);
                let id = inner.next_job;
                inner.next_job += 1;
                let cancel = CancelToken::new();
                inner.running.insert(
                    id,
                    Running {
                        url: url.clone(),
                        cancel: cancel.clone(),
                    },
                );
                (id, url, cancel)
            };
            let pool = Arc::clone(self);
            tokio::spawn(async move {
                // The run is a task of its own, so a panic in it is one failed look-up and
                // not a run that never ends.
                let run = tokio::spawn({
                    let pool = Arc::clone(&pool);
                    let url = url.clone();
                    async move {
                        tokio::time::timeout(PROBE_HARD_LIMIT, pool.backend.probe(&url, &cancel))
                            .await
                            .unwrap_or(None)
                    }
                });
                let meta = run.await.unwrap_or(None);
                let finished = pool.finish(id, url, meta);
                pool.pump();
                if let Some((url, meta)) = finished {
                    pool.report(url, meta);
                }
                pool.release_if_idle();
            });
        }
    }

    /// Tells the tray about a finished look-up, with the progress at that moment.
    fn report(&self, url: String, meta: Option<ItemMeta>) {
        let _in_order = lock_recover(&self.emit_gate);
        let progress = self.progress();
        (self.on_event)(ProbeEvent {
            url,
            meta,
            queued: progress.queued,
            running: progress.running,
        });
    }

    /// Records a run's result; `None` when the run was cancelled (nothing to report).
    fn finish(
        &self,
        id: u64,
        url: String,
        meta: Option<ItemMeta>,
    ) -> Option<(String, Option<ItemMeta>)> {
        let mut inner = lock_recover(&self.inner);
        let run = inner.running.remove(&id)?;
        if run.cancel.is_cancelled() {
            return None;
        }
        inner.cache.insert(url.clone(), meta.clone());
        Some((url, meta))
    }

    fn release_if_idle(&self) {
        let idle = lock_recover(&self.inner).is_idle();
        if idle {
            self.backend.idle();
        }
    }
}

/// The real run: yt-dlp on the item's page, with the cookies of the page's site.
pub struct YtDlpBackend {
    app: AppHandle,
    /// The cookies prepared for each account, shared by the runs that need them and dropped
    /// (the files deleted once no run holds them) when nothing is queued or running.
    cookies: Mutex<HashMap<i64, Arc<AccountCookies>>>,
    /// One account's cookies are prepared by one run at a time; the others wait and share.
    preparing: tokio::sync::Mutex<()>,
    sequence: AtomicU64,
}

impl YtDlpBackend {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            cookies: Mutex::new(HashMap::new()),
            preparing: tokio::sync::Mutex::new(()),
            sequence: AtomicU64::new(1),
        }
    }

    fn state(&self) -> State<'_, AppState> {
        self.app.state::<AppState>()
    }

    /// The prepared cookies of the account selected for the URL's site; `None` when the site
    /// has no selected account (the run goes without a login).
    async fn cookies_for(&self, url: &str) -> Option<Arc<AccountCookies>> {
        let state = self.state();
        let account_id = {
            let store = lock_recover(&state.store);
            match crate::sites::selected_account_for_url(&store, url) {
                Ok(Some((_, account))) => account.id,
                Ok(None) => return None,
                Err(error) => {
                    eprintln!("could not select account cookies for collect probe: {error}");
                    return None;
                }
            }
        };
        if let Some(found) = lock_recover(&self.cookies).get(&account_id) {
            return Some(Arc::clone(found));
        }
        let _one_at_a_time = self.preparing.lock().await;
        if let Some(found) = lock_recover(&self.cookies).get(&account_id) {
            return Some(Arc::clone(found));
        }
        // The name starts with `cookies-` and ends in `.txt`, so a copy left by a crash is
        // removed at the next start.
        let file_name = format!(
            "cookies-collect-probe-{account_id}-{}.txt",
            self.sequence.fetch_add(1, Ordering::Relaxed)
        );
        let prepared = state::account_cookies(
            &state.store,
            &state.work_dir,
            &file_name,
            "collect probe",
            url,
            |account_id, domains| {
                state::in_app_cookie_text(&state.browser, &self.app, account_id, domains)
            },
        )
        .await?;
        let prepared = Arc::new(prepared);
        lock_recover(&self.cookies).insert(account_id, Arc::clone(&prepared));
        Some(prepared)
    }

    async fn run(&self, url: &str, cancel: &CancelToken) -> Option<ItemMeta> {
        let state = self.state();
        let tools = managed_tools(&state).ok()?;
        let prepared = self.cookies_for(url).await;
        let cookies = prepared
            .as_ref()
            .map_or(Cookies::None, |prepared| prepared.cookies.clone());
        let executor = YtDlpExecutor::new(
            ExecutorConfig {
                tools,
                work_dir: state.work_dir.clone(),
                target_os: TARGET_OS,
                gallery_dl: None,
            },
            Arc::clone(&state.store),
        )
        .with_list_probe_timeout(PROBE_TIMEOUT);
        let json = executor.probe_list_json(url, cookies, cancel).await.ok()?;
        // `prepared` keeps the cookie file until yt-dlp has finished.
        drop(prepared);
        Some(item_meta(&json))
    }
}

impl ProbeBackend for YtDlpBackend {
    fn probe<'a>(
        &'a self,
        url: &'a str,
        cancel: &'a CancelToken,
    ) -> Pin<Box<dyn Future<Output = Option<ItemMeta>> + Send + 'a>> {
        Box::pin(self.run(url, cancel))
    }

    fn idle(&self) {
        lock_recover(&self.cookies).clear();
    }
}

pub type CollectProbe = ProbePool<YtDlpBackend>;

/// The app's probe pool: results go to the webview as `collect-probe` events.
pub fn create(app: &AppHandle) -> Arc<CollectProbe> {
    let emitter = app.clone();
    ProbePool::new(
        Arc::new(YtDlpBackend::new(app.clone())),
        Arc::new(move |event| {
            let _ = emitter.emit("collect-probe", event);
        }),
    )
}

/// Starts looking the found items up (the ones not known, queued or running); resolves with
/// the progress. Rejects a URL that is not an http(s) URL of at most 2048 characters, and
/// more than 2000 URLs.
#[tauri::command]
pub async fn collect_probe_start(
    urls: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Progress, String> {
    state.collect_probe.start(urls)
}

/// The results already known for those URLs, and the progress.
#[tauri::command]
pub async fn collect_probe_snapshot(
    urls: Vec<String>,
    state: State<'_, AppState>,
) -> Result<ProbeSnapshot, String> {
    state.collect_probe.snapshot(urls)
}

/// Empties the queue and ends the runs going on; known results are kept.
#[tauri::command]
pub async fn collect_probe_cancel(state: State<'_, AppState>) -> Result<(), String> {
    state.collect_probe.cancel();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use tokio::sync::Semaphore;

    use super::*;

    /// Finds the URLs that contain `/ok/` and fails the others. Holds every run until `gate`
    /// has a permit, so a test decides when runs end.
    struct FakeBackend {
        gate: Arc<Semaphore>,
        probed: Mutex<Vec<String>>,
        now: AtomicUsize,
        most: AtomicUsize,
        idles: AtomicUsize,
    }

    impl FakeBackend {
        fn new(open: bool) -> Arc<Self> {
            Arc::new(Self {
                gate: Arc::new(Semaphore::new(if open {
                    Semaphore::MAX_PERMITS
                } else {
                    0
                })),
                probed: Mutex::new(Vec::new()),
                now: AtomicUsize::new(0),
                most: AtomicUsize::new(0),
                idles: AtomicUsize::new(0),
            })
        }

        fn probed(&self) -> Vec<String> {
            lock_recover(&self.probed).clone()
        }
    }

    impl ProbeBackend for FakeBackend {
        fn probe<'a>(
            &'a self,
            url: &'a str,
            cancel: &'a CancelToken,
        ) -> Pin<Box<dyn Future<Output = Option<ItemMeta>> + Send + 'a>> {
            Box::pin(async move {
                lock_recover(&self.probed).push(url.to_owned());
                let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
                self.most.fetch_max(now, Ordering::SeqCst);
                let answer = tokio::select! {
                    _ = cancel.cancelled() => None,
                    permit = self.gate.acquire() => {
                        permit.expect("gate stays open").forget();
                        // Long enough for a runner that starts too many to show it.
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        // Every URL named `.../ok/...` is found, anything else fails.
                        url.contains("/ok/").then(|| ItemMeta {
                            uploader: Some("NASA".to_owned()),
                            duration_s: Some(185),
                            published_at: Some(1_793_664_000),
                        })
                    }
                };
                self.now.fetch_sub(1, Ordering::SeqCst);
                answer
            })
        }

        fn idle(&self) {
            self.idles.fetch_add(1, Ordering::SeqCst);
        }
    }

    type Events = Arc<Mutex<Vec<ProbeEvent>>>;

    fn pool(backend: &Arc<FakeBackend>, cache_cap: usize) -> (Arc<ProbePool<FakeBackend>>, Events) {
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let pool = ProbePool::with_cache_cap(
            Arc::clone(backend),
            Arc::new(move |event| lock_recover(&sink).push(event)),
            cache_cap,
        );
        (pool, events)
    }

    fn ok(n: usize) -> String {
        format!("https://example.com/ok/{n}")
    }

    fn bad(n: usize) -> String {
        format!("https://example.com/bad/{n}")
    }

    async fn until(mut condition: impl FnMut() -> bool) {
        for _ in 0..1_000 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the condition did not come true in 5 s");
    }

    #[test]
    fn urls_must_be_http_and_bounded() {
        let fine = vec![
            "https://example.com/watch?v=1".to_owned(),
            "http://example.com/".to_owned(),
        ];
        assert_eq!(validate_urls(fine.clone()).unwrap(), fine);
        assert_eq!(validate_urls(Vec::new()).unwrap(), Vec::<String>::new());

        for bad in [
            "ftp://example.com/a",
            "file:///C:/secret.txt",
            "javascript:alert(1)",
            "blob:https://example.com/1234",
            "https://",
            "not a url",
            "",
        ] {
            assert!(
                validate_urls(vec!["https://example.com/a".to_owned(), bad.to_owned()]).is_err(),
                "{bad}"
            );
        }

        // 2048 characters are fine, 2049 are not (counted in characters, not bytes).
        let prefix = "https://example.com/";
        let limit = format!("{prefix}{}", "a".repeat(MAX_URL_CHARS - prefix.len()));
        assert_eq!(limit.chars().count(), MAX_URL_CHARS);
        assert!(validate_urls(vec![limit.clone()]).is_ok());
        assert!(validate_urls(vec![format!("{limit}a")]).is_err());
        let wide = format!("{prefix}{}", "あ".repeat(MAX_URL_CHARS - prefix.len()));
        assert!(validate_urls(vec![wide]).is_ok());

        // 2000 per call.
        let many = |count: usize| {
            (0..count)
                .map(|n| format!("https://example.com/{n}"))
                .collect::<Vec<_>>()
        };
        assert!(validate_urls(many(MAX_URLS_PER_CALL)).is_ok());
        assert!(validate_urls(many(MAX_URLS_PER_CALL + 1)).is_err());
    }

    #[test]
    fn the_cache_drops_the_oldest_results_beyond_its_cap() {
        let mut cache = ResultCache::new(3);
        let meta = |seconds| {
            Some(ItemMeta {
                duration_s: Some(seconds),
                ..ItemMeta::default()
            })
        };
        for (n, seconds) in (1..=5u64).enumerate() {
            cache.insert(format!("https://example.com/{}", n + 1), meta(seconds));
        }
        assert_eq!(cache.entries.len(), 3);
        assert!(cache.get("https://example.com/1").is_none());
        assert!(cache.get("https://example.com/2").is_none());
        assert_eq!(cache.get("https://example.com/5"), Some(&meta(5)));
        // A result for a URL that is kept replaces it without adding a place.
        cache.insert("https://example.com/3".to_owned(), None);
        assert_eq!(cache.get("https://example.com/3"), Some(&None));
        assert_eq!(cache.entries.len(), 3);
        cache.insert("https://example.com/6".to_owned(), None);
        assert!(cache.get("https://example.com/3").is_none());
        assert_eq!(cache.order.len(), cache.entries.len());
        assert_eq!(CACHE_CAP, 5_000);
    }

    #[tokio::test]
    async fn a_url_is_queued_once_whether_it_is_waiting_running_or_known() {
        let backend = FakeBackend::new(false);
        let (pool, _events) = pool(&backend, CACHE_CAP);

        // The same URL twice in one call, and five different ones: 3 run, 2 wait.
        let first = pool
            .start(vec![ok(1), ok(2), ok(1), ok(3), ok(4), ok(5)])
            .unwrap();
        assert_eq!(
            first,
            Progress {
                queued: 2,
                running: 3
            }
        );

        // Asking again for running and waiting ones adds nothing; a new one waits.
        let second = pool.start(vec![ok(1), ok(4), ok(6)]).unwrap();
        assert_eq!(
            second,
            Progress {
                queued: 3,
                running: 3
            }
        );

        // Let everything finish; every URL is looked up exactly once.
        backend.gate.add_permits(100);
        until(|| pool.progress() == Progress::default() && backend.probed().len() == 6).await;
        until(|| pool.snapshot(vec![ok(6)]).unwrap().results.len() == 1).await;
        let mut probed = backend.probed();
        probed.sort();
        assert_eq!(probed, (1..=6).map(ok).collect::<Vec<_>>());

        // Known results are not looked up again, failed ones neither.
        let failing = pool.start(vec![bad(1)]).unwrap();
        assert_eq!(failing.queued + failing.running, 1);
        until(|| pool.snapshot(vec![bad(1)]).unwrap().results.len() == 1).await;
        let again = pool.start(vec![ok(1), bad(1)]).unwrap();
        assert_eq!(again, Progress::default());
        assert_eq!(backend.probed().len(), 7);
    }

    #[tokio::test]
    async fn at_most_three_runs_go_at_once() {
        let backend = FakeBackend::new(true);
        let (pool, _events) = pool(&backend, CACHE_CAP);
        pool.start((0..20).map(ok).collect()).unwrap();
        until(|| backend.probed().len() == 20 && backend.now.load(Ordering::SeqCst) == 0).await;
        until(|| pool.progress() == Progress::default()).await;
        assert_eq!(backend.most.load(Ordering::SeqCst), MAX_CONCURRENT);
    }

    #[tokio::test]
    async fn every_finished_run_is_reported_with_its_result_and_the_progress() {
        let backend = FakeBackend::new(true);
        let (pool, events) = pool(&backend, CACHE_CAP);
        pool.start(vec![ok(1), bad(2), ok(3), ok(4)]).unwrap();
        until(|| lock_recover(&events).len() == 4).await;

        let events = lock_recover(&events).clone();
        let found = ProbeResult {
            url: ok(1),
            meta: Some(ItemMeta {
                uploader: Some("NASA".to_owned()),
                duration_s: Some(185),
                published_at: Some(1_793_664_000),
            }),
        };
        let event_for = |url: String| events.iter().find(|event| event.url == url).unwrap();
        assert_eq!(event_for(ok(1)).meta, found.meta);
        // A failed look-up is reported with no meta.
        assert_eq!(event_for(bad(2)).meta, None);
        // The last one reported says nothing is left.
        let last = events.last().unwrap();
        assert_eq!((last.queued, last.running), (0, 0));

        let snapshot = pool.snapshot(vec![bad(2), ok(1), ok(9)]).unwrap();
        assert_eq!(snapshot.results.len(), 2);
        assert_eq!(snapshot.results[0].url, bad(2));
        assert_eq!(snapshot.results[0].meta, None);
        assert_eq!(snapshot.results[1], found);
        assert_eq!((snapshot.queued, snapshot.running), (0, 0));
    }

    #[tokio::test]
    async fn cancel_empties_the_queue_ends_the_runs_and_keeps_the_cache() {
        let backend = FakeBackend::new(false);
        let (pool, events) = pool(&backend, CACHE_CAP);

        // One look-up is finished and cached before the rest start.
        backend.gate.add_permits(1);
        pool.start(vec![ok(0)]).unwrap();
        until(|| pool.snapshot(vec![ok(0)]).unwrap().results.len() == 1).await;
        let reported = lock_recover(&events).len();
        assert_eq!(reported, 1);

        // Six more: three run (held at the gate), three wait.
        let progress = pool.start((1..=6).map(ok).collect()).unwrap();
        assert_eq!(
            progress,
            Progress {
                queued: 3,
                running: 3
            }
        );
        until(|| backend.probed().len() == 4).await;

        pool.cancel();
        assert_eq!(pool.progress().queued, 0);
        assert_eq!(pool.progress().running, 0);

        // The ended runs report nothing and cache nothing; the waiting ones never ran.
        until(|| backend.now.load(Ordering::SeqCst) == 0).await;
        until(|| backend.idles.load(Ordering::SeqCst) >= 1).await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(backend.probed().len(), 4);
        assert_eq!(lock_recover(&events).len(), reported);
        let snapshot = pool.snapshot((0..=6).map(ok).collect()).unwrap();
        assert_eq!(
            snapshot
                .results
                .iter()
                .map(|result| result.url.clone())
                .collect::<Vec<_>>(),
            vec![ok(0)]
        );

        // The same URLs are asked again later: they were not cached as failures.
        backend.gate.add_permits(100);
        let again = pool.start((1..=6).map(ok).collect()).unwrap();
        assert_eq!(again.queued + again.running, 6);
        until(|| {
            pool.snapshot((1..=6).map(ok).collect())
                .unwrap()
                .results
                .len()
                == 6
        })
        .await;
        assert!(
            pool.snapshot((1..=6).map(ok).collect())
                .unwrap()
                .results
                .iter()
                .all(|result| result.meta.is_some())
        );
    }

    #[tokio::test]
    async fn the_backend_is_told_when_nothing_is_left_to_do() {
        let backend = FakeBackend::new(true);
        let (pool, _events) = pool(&backend, CACHE_CAP);
        pool.start((0..5).map(ok).collect()).unwrap();
        until(|| backend.idles.load(Ordering::SeqCst) >= 1).await;
        assert_eq!(pool.progress(), Progress::default());
        // Cancelling when idle tells it again (nothing to delete is fine).
        let before = backend.idles.load(Ordering::SeqCst);
        pool.cancel();
        assert_eq!(backend.idles.load(Ordering::SeqCst), before + 1);
    }

    #[tokio::test]
    async fn a_bad_url_rejects_the_call_without_queueing_anything() {
        let backend = FakeBackend::new(true);
        let (pool, _events) = pool(&backend, CACHE_CAP);
        assert!(
            pool.start(vec![ok(1), "ftp://example.com/x".to_owned()])
                .is_err()
        );
        assert!(pool.snapshot(vec!["nonsense".to_owned()]).is_err());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(backend.probed().is_empty());
        assert_eq!(pool.progress(), Progress::default());
    }

    #[tokio::test]
    async fn the_cache_cap_applies_to_the_pool() {
        let backend = FakeBackend::new(true);
        let (pool, _events) = pool(&backend, 4);
        pool.start((0..10).map(ok).collect()).unwrap();
        until(|| backend.probed().len() == 10 && pool.progress() == Progress::default()).await;
        until(|| backend.idles.load(Ordering::SeqCst) >= 1).await;
        let known = pool
            .snapshot((0..10).map(ok).collect())
            .unwrap()
            .results
            .len();
        assert_eq!(known, 4);
    }
}

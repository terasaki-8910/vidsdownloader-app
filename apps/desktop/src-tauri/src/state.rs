use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tauri::{AppHandle, Manager};
use vids_core::args::{Cookies, ToolPaths};
use vids_core::executor::{ExecutorConfig, JobOptions, YtDlpExecutor};
use vids_core::naming::TargetOs;
use vids_core::queue::{JobContext, JobExecutor, JobFailure, JobOutcome, Queue};
use vids_core::store::Store;
use vids_core::tools::{Tool, ToolManager};
use vids_core::types::{ErrorCategory, JobPhase};

use crate::models::Settings;
use crate::{browser, collector, notify, page_media, sites, sources};

#[cfg(target_os = "windows")]
pub const TARGET_OS: TargetOs = TargetOs::Windows;
#[cfg(target_os = "macos")]
pub const TARGET_OS: TargetOs = TargetOs::MacOs;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
compile_error!("vidsdownloader supports only Windows and macOS");

pub struct ShellExecutor {
    pub tools: Arc<ToolManager>,
    pub work_dir: PathBuf,
    pub store: Arc<Mutex<Store>>,
    pub app: AppHandle,
    pub browser: Arc<browser::BrowserManager>,
    pub page_media: Arc<dyn page_media::PageMediaFinder>,
}

trait AttemptRunner: Send + Sync {
    fn run(
        &self,
        job: JobContext,
    ) -> Pin<Box<dyn Future<Output = Result<JobOutcome, JobFailure>> + Send + '_>>;
}

impl AttemptRunner for YtDlpExecutor {
    fn run(
        &self,
        job: JobContext,
    ) -> Pin<Box<dyn Future<Output = Result<JobOutcome, JobFailure>> + Send + '_>> {
        Box::pin(JobExecutor::run(self, job))
    }
}

impl JobExecutor for ShellExecutor {
    async fn run(&self, mut job: JobContext) -> Result<JobOutcome, JobFailure> {
        // A page's own images are saved over HTTP by the executor itself: no tools needed.
        let page_images = job
            .options
            .get("page_images")
            .is_some_and(|value| !value.is_null());
        let yt_dlp = self.tools.resolve(Tool::YtDlp, TARGET_OS);
        let ffmpeg = self.tools.resolve(Tool::Ffmpeg, TARGET_OS);
        let deno = self.tools.resolve(Tool::Deno, TARGET_OS);
        let gallery_dl = self.tools.resolve(Tool::GalleryDl, TARGET_OS);
        let tools = match (yt_dlp, ffmpeg, deno) {
            (Some(yt_dlp), Some(ffmpeg), Some(deno)) => ToolPaths {
                yt_dlp: yt_dlp.path,
                ffmpeg_dir: ffmpeg.path,
                deno: deno.path,
            },
            _ if page_images => ToolPaths {
                yt_dlp: PathBuf::new(),
                ffmpeg_dir: PathBuf::new(),
                deno: PathBuf::new(),
            },
            _ => {
                return Err(JobFailure {
                    category: ErrorCategory::Unknown,
                    message: "required tools are not installed".to_owned(),
                });
            }
        };

        let stored_options = job.options.clone();
        // Page-image jobs too: the executor sends the account's cookies to the site's own
        // hosts only.
        let _temporary_cookie_file = self.apply_account_cookies(&mut job).await;
        let executor = YtDlpExecutor::new(
            ExecutorConfig {
                tools,
                work_dir: self.work_dir.clone(),
                target_os: TARGET_OS,
                gallery_dl: gallery_dl.map(|tool| tool.path),
            },
            Arc::clone(&self.store),
        );
        let outcome = run_with_page_media(
            &executor,
            self.page_media.as_ref(),
            job,
            stored_options.clone(),
            page_images,
        )
        .await?;

        if !outcome.entries.is_empty() {
            let mut options: JobOptions =
                serde_json::from_value(stored_options).map_err(|error| JobFailure {
                    category: ErrorCategory::Unknown,
                    message: format!("could not read child job options: {error}"),
                })?;
            options.cookies = Cookies::None;
            let state = self.app.state::<AppState>();
            for entry in &outcome.entries {
                options.playlist_item = entry.playlist_item;
                let options = serde_json::to_value(&options).map_err(|error| JobFailure {
                    category: ErrorCategory::Unknown,
                    message: format!("could not serialize child job options: {error}"),
                })?;
                state.queue.enqueue(entry.url.clone(), options);
            }
        }

        Ok(outcome)
    }
}

async fn run_with_page_media(
    executor: &dyn AttemptRunner,
    finder: &dyn page_media::PageMediaFinder,
    mut job: JobContext,
    stored_options: serde_json::Value,
    page_images: bool,
) -> Result<JobOutcome, JobFailure> {
    let mut runtime: JobOptions =
        serde_json::from_value(job.options.clone()).map_err(|error| JobFailure {
            category: ErrorCategory::Unknown,
            message: format!("could not read job options: {error}"),
        })?;
    if !runtime.page_media_choices.is_empty() {
        return Err(page_media_choice());
    }
    if let Some(media) = runtime.page_media.as_ref() {
        job.url = media.url.clone();
        return executor.run(job).await;
    }

    let retry = RetryContext {
        id: job.id,
        page_url: job.url.clone(),
        attempt: job.attempt,
        sink: job.sink.clone(),
        cancel: job.cancel.clone(),
    };
    match executor.run(job).await {
        Ok(outcome) => return Ok(outcome),
        Err(error)
            if error.category != ErrorCategory::UnsupportedUrl
                || page_images
                || retry.cancel.is_cancelled()
                || !is_http_url(&retry.page_url) =>
        {
            return Err(error);
        }
        Err(_) => {}
    }

    retry.sink.set_phase(Some(JobPhase::PageSearch));
    let found = finder
        .find(page_media::PageMediaRequest {
            url: retry.page_url.clone(),
            preset: runtime.preset,
            cancel: retry.cancel.clone(),
            sink: retry.sink.clone(),
        })
        .await;
    retry.sink.set_phase(None);
    if retry.cancel.is_cancelled() {
        return Err(JobFailure {
            category: ErrorCategory::Unknown,
            message: "canceled".to_owned(),
        });
    }
    let found = found?.ok_or_else(page_media_not_found)?;
    let (media, choices) = match found {
        page_media::PageMediaSearchResult::One(media) => (media, Vec::new()),
        page_media::PageMediaSearchResult::Ask { media, choices } => (media, choices),
        page_media::PageMediaSearchResult::Gate => return Err(page_gate()),
    };
    runtime.page_media = Some(media.clone());
    runtime.page_media_choices = choices.clone();

    let mut persisted: JobOptions =
        serde_json::from_value(stored_options).map_err(|error| JobFailure {
            category: ErrorCategory::Unknown,
            message: format!("could not read stored job options: {error}"),
        })?;
    persisted.page_media = Some(media.clone());
    persisted.page_media_choices = choices;
    persisted.cookies = Cookies::None;
    persisted.cookie_domains.clear();
    let persisted = serde_json::to_value(persisted).map_err(|error| JobFailure {
        category: ErrorCategory::Unknown,
        message: format!("could not store page media: {error}"),
    })?;
    retry.sink.set_options(persisted);

    if !runtime.page_media_choices.is_empty() {
        return Err(page_media_choice());
    }

    let options = serde_json::to_value(runtime).map_err(|error| JobFailure {
        category: ErrorCategory::Unknown,
        message: format!("could not prepare page media: {error}"),
    })?;
    executor
        .run(JobContext {
            id: retry.id,
            url: media.url,
            options,
            attempt: retry.attempt,
            sink: retry.sink,
            cancel: retry.cancel,
        })
        .await
}

struct RetryContext {
    id: vids_core::queue::JobId,
    page_url: String,
    attempt: u32,
    sink: vids_core::queue::JobSink,
    cancel: vids_core::queue::CancelToken,
}

fn is_http_url(value: &str) -> bool {
    tauri::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn page_media_not_found() -> JobFailure {
    JobFailure {
        category: ErrorCategory::PageMediaNotFound,
        message: "no video was found on the page".to_owned(),
    }
}

fn page_gate() -> JobFailure {
    JobFailure {
        category: ErrorCategory::PageGate,
        message: "stopped at a confirmation page".to_owned(),
    }
}

fn page_media_choice() -> JobFailure {
    JobFailure {
        category: ErrorCategory::PageMediaChoice,
        message: "several videos were found".to_owned(),
    }
}

impl ShellExecutor {
    async fn apply_account_cookies(&self, job: &mut JobContext) -> Option<TemporaryCookieFile> {
        prepare_account_cookies(
            &self.store,
            &self.work_dir,
            job.id.0,
            job.attempt,
            &job.url,
            &mut job.options,
            |account_id, domains| in_app_cookie_text(&self.browser, &self.app, account_id, domains),
        )
        .await
    }
}

/// The Netscape cookie file text of an in-app account's cookies for the site's `domains`,
/// or `None` when the account has no browser folder yet. Read every time (a login made by
/// browsing may not be recorded yet), but only once the account has a browser folder, so
/// unused accounts cost nothing.
pub async fn in_app_cookie_text(
    browser: &browser::BrowserManager,
    app: &AppHandle,
    account_id: i64,
    domains: Vec<String>,
) -> Result<Option<String>, String> {
    if !browser.has_profile(account_id) {
        return Ok(None);
    }
    let cookies = browser.cookies_for_site(app, account_id, &domains).await?;
    Ok(Some(browser::netscape_cookie_file(&cookies)))
}

/// The cookies a run for one URL gets from the selected account of the site whose domain
/// matches it.
pub struct AccountCookies {
    /// What yt-dlp is given; `Cookies::None` when they could not be prepared.
    pub cookies: Cookies,
    /// The site's domains when there are cookies, else none.
    pub domains: Vec<String>,
    /// Deletes the temporary cookie file when dropped; keep it until the run ends.
    pub temporary: Option<TemporaryCookieFile>,
}

/// The selected account's cookies of the site whose domain matches `url`: an in-app
/// account as a temporary Netscape file, a user's cookies.txt as a temporary copy (the
/// user's file is never modified), a Firefox account as the profile name. A temporary file
/// is written user-only to `work_dir/<file_name>` (a name starting with `cookies-` and
/// ending in `.txt`, so a copy left by a crash is removed at the next start) and deleted
/// when `temporary` is dropped. `in_app_cookies` gets the account id and the site's domains
/// and returns the Netscape file text of an in-app account's cookies, or `None` when the
/// account has no browser folder yet. `None` when `url` matches no site with a selected
/// account; cookies that cannot be prepared are `Cookies::None`. `label` names the run in
/// log lines.
pub async fn account_cookies<F, Fut>(
    store: &Mutex<Store>,
    work_dir: &Path,
    file_name: &str,
    label: &str,
    url: &str,
    in_app_cookies: F,
) -> Option<AccountCookies>
where
    F: FnOnce(i64, Vec<String>) -> Fut,
    Fut: Future<Output = Result<Option<String>, String>>,
{
    let selected = {
        let store = lock_recover(store);
        match sites::selected_account_for_url(&store, url) {
            Ok(selected) => selected,
            Err(error) => {
                eprintln!("could not select account cookies for {label}: {error}");
                return None;
            }
        }
    };
    let (site, account) = selected?;
    let path = work_dir.join(file_name);
    let mut temporary = None;
    let cookies = match account.method {
        sites::LoginMethod::InApp => {
            match in_app_cookies(account.id, site.domains.clone())
                .await
                .and_then(|text| match text {
                    Some(text) => {
                        browser::write_private_file(&path, text.as_bytes()).map(|()| true)
                    }
                    None => Ok(false),
                }) {
                Ok(true) => {
                    temporary = Some(TemporaryCookieFile::new(path.clone()));
                    Cookies::File(path)
                }
                Ok(false) => Cookies::None,
                Err(error) => {
                    eprintln!("could not prepare in-app cookies for {label}: {error}");
                    Cookies::None
                }
            }
        }
        sites::LoginMethod::Firefox { profile } => Cookies::Firefox { profile },
        sites::LoginMethod::CookiesTxt { path: source } => match fs::read(&source)
            .map_err(|error| format!("could not read {source}: {error}"))
            .and_then(|bytes| browser::write_private_file(&path, &bytes))
        {
            Ok(()) => {
                temporary = Some(TemporaryCookieFile::new(path.clone()));
                Cookies::File(path)
            }
            Err(error) => {
                eprintln!("could not prepare cookies.txt for {label}: {error}");
                Cookies::None
            }
        },
    };
    let domains = if matches!(cookies, Cookies::None) {
        Vec::new()
    } else {
        site.domains
    };
    Some(AccountCookies {
        cookies,
        domains,
        temporary,
    })
}

/// Gives the job the selected account's cookies of the site whose domain matches the job's
/// URL (for a page-images job that is the page's URL), as `cookies` and `cookie_domains`
/// in its options (see [`account_cookies`]); the temporary file is
/// `cookies-<job>-<attempt>.txt`, deleted when the returned guard is dropped. A job whose
/// URL matches no site, or whose cookies cannot be prepared, runs without cookies.
async fn prepare_account_cookies<F, Fut>(
    store: &Mutex<Store>,
    work_dir: &Path,
    job_id: u64,
    attempt: u32,
    url: &str,
    job_options: &mut serde_json::Value,
    in_app_cookies: F,
) -> Option<TemporaryCookieFile>
where
    F: FnOnce(i64, Vec<String>) -> Fut,
    Fut: Future<Output = Result<Option<String>, String>>,
{
    let label = format!("job {job_id}");
    let file_name = format!("cookies-{job_id}-{attempt}.txt");
    let prepared =
        account_cookies(store, work_dir, &file_name, &label, url, in_app_cookies).await?;
    let mut options: JobOptions = match serde_json::from_value(job_options.clone()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("could not read cookie options for job {job_id}: {error}");
            return None;
        }
    };
    options.cookies = prepared.cookies;
    // A page-images job sends the cookies only to these domains and their subdomains.
    options.cookie_domains = prepared.domains;
    match serde_json::to_value(options) {
        Ok(options) => *job_options = options,
        Err(error) => {
            eprintln!("could not save cookie options for job {job_id}: {error}");
            return None;
        }
    }
    prepared.temporary
}

pub struct TemporaryCookieFile {
    path: PathBuf,
}

impl TemporaryCookieFile {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TemporaryCookieFile {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "could not delete temporary cookie file {}: {error}",
                self.path.display()
            );
        }
    }
}

pub struct AppState {
    pub settings: Mutex<Settings>,
    pub store: Arc<Mutex<Store>>,
    /// Where jobs keep their attempt folders (`job-<id>-<attempt>`) and cookie files.
    pub work_dir: PathBuf,
    pub tools: Arc<ToolManager>,
    pub queue: Queue<ShellExecutor>,
    pub installation_running: Arc<AtomicBool>,
    pub browser: Arc<browser::BrowserManager>,
    pub collector: Arc<collector::CollectorManager>,
    pub sources: Arc<sources::SourceManager>,
    pub notifier: notify::Notifier,
}

pub fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::Value;
    use vids_core::args::{Embed, Preset};
    use vids_core::comments::AssSettings;
    use vids_core::executor::PageMedia;
    use vids_core::naming::{NameTemplate, Part, Token};
    use vids_core::page_images::{PageImage, PageImages};
    use vids_core::queue::{JobId, QueueEvent};
    use vids_core::types::JobState;

    use super::*;

    const PAGE_URL: &str = "https://blog.example.com/gallery/post-1";
    const COOKIES_TXT: &str = "# Netscape HTTP Cookie File\n\
                               .blog.example.com\tTRUE\t/\tTRUE\t0\tsid\tsecret\n";

    /// The options of a page-images job, as the queue stores them (no cookies yet).
    fn page_image_options() -> Value {
        serde_json::to_value(JobOptions {
            dest_dir: PathBuf::from("downloads"),
            template: NameTemplate {
                folders: Vec::new(),
                file: vec![Part::Field {
                    field: Token::Title,
                }],
            },
            tz_offset_seconds: 0,
            site_name: None,
            preset: Preset::Best,
            avoid_av1: false,
            embed: Embed {
                subtitles: false,
                subtitle_langs: Vec::new(),
                keep_subtitle_files: false,
                thumbnail: false,
                metadata: false,
                chapters: false,
            },
            comments: false,
            ass: AssSettings::default(),
            cookies: Cookies::None,
            filter: None,
            playlist_item: None,
            force_save: false,
            page_images: Some(PageImages {
                title: Some("Post".to_owned()),
                uploader: None,
                published_at: None,
                id: Some("post-1".to_owned()),
                images: vec![PageImage {
                    url: "https://img.example.com/1.jpg".to_owned(),
                    index: 1,
                }],
            }),
            page_media: None,
            page_media_choices: Vec::new(),
            cookie_domains: Vec::new(),
        })
        .unwrap()
    }

    /// A store with one site, blog.example.com, whose selected account uses `method`.
    fn store_with_account(method: sites::LoginMethod) -> Mutex<Store> {
        let store = Store::open_in_memory().unwrap();
        let site = sites::add_site(&store, "https://blog.example.com/", 1).unwrap();
        sites::set_account_method(&store, site.accounts[0].id, method, 2).unwrap();
        Mutex::new(store)
    }

    async fn no_browser(_account_id: i64, _domains: Vec<String>) -> Result<Option<String>, String> {
        Err("the in-app browser was not expected".to_owned())
    }

    fn read_options(options: &Value) -> JobOptions {
        serde_json::from_value(options.clone()).unwrap()
    }

    fn video_options() -> Value {
        let mut options = read_options(&page_image_options());
        options.page_images = None;
        serde_json::to_value(options).unwrap()
    }

    struct FakeAttempt {
        results: Mutex<VecDeque<Result<JobOutcome, JobFailure>>>,
        urls: Mutex<Vec<String>>,
    }

    impl AttemptRunner for FakeAttempt {
        fn run(
            &self,
            job: JobContext,
        ) -> Pin<Box<dyn Future<Output = Result<JobOutcome, JobFailure>> + Send + '_>> {
            lock_recover(&self.urls).push(job.url);
            let result = lock_recover(&self.results)
                .pop_front()
                .expect("a fake result was prepared");
            Box::pin(async move { result })
        }
    }

    struct FakeFinder {
        result: Mutex<Option<Result<Option<page_media::PageMediaSearchResult>, JobFailure>>>,
        calls: AtomicUsize,
    }

    impl page_media::PageMediaFinder for FakeFinder {
        fn find(
            &self,
            _request: page_media::PageMediaRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Option<page_media::PageMediaSearchResult>, JobFailure>>
                    + Send
                    + '_,
            >,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let result = lock_recover(&self.result)
                .take()
                .expect("the fake finder is called once");
            Box::pin(async move { result })
        }
    }

    struct WrapperFake {
        attempt: Arc<FakeAttempt>,
        finder: Arc<FakeFinder>,
    }

    impl JobExecutor for WrapperFake {
        async fn run(&self, job: JobContext) -> Result<JobOutcome, JobFailure> {
            let stored = job.options.clone();
            run_with_page_media(
                self.attempt.as_ref(),
                self.finder.as_ref(),
                job,
                stored,
                false,
            )
            .await
        }
    }

    fn unsupported() -> JobFailure {
        JobFailure {
            category: ErrorCategory::UnsupportedUrl,
            message: "Unsupported URL".to_owned(),
        }
    }

    async fn finished(
        events: &mut tokio::sync::broadcast::Receiver<QueueEvent>,
        id: JobId,
    ) -> (JobState, Option<Result<JobOutcome, JobFailure>>) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let QueueEvent::Finished {
                    id: finished,
                    state,
                    outcome_or_error,
                } = events.recv().await.unwrap()
                    && finished == id
                {
                    return (state, outcome_or_error);
                }
            }
        })
        .await
        .expect("the job finishes")
    }

    #[tokio::test]
    async fn unsupported_without_found_media_reports_phase_and_page_media_not_found() {
        let attempt = Arc::new(FakeAttempt {
            results: Mutex::new(VecDeque::from([Err(unsupported())])),
            urls: Mutex::new(Vec::new()),
        });
        let finder = Arc::new(FakeFinder {
            result: Mutex::new(Some(Ok(None))),
            calls: AtomicUsize::new(0),
        });
        let queue = Queue::new(
            WrapperFake {
                attempt,
                finder: Arc::clone(&finder),
            },
            1,
        );
        let mut events = queue.subscribe();
        let mut phase_events = queue.subscribe();
        let id = queue.enqueue(PAGE_URL, video_options());

        let (state, result) = finished(&mut events, id).await;

        assert_eq!(state, JobState::Failed);
        assert_eq!(
            result.unwrap().unwrap_err().category,
            ErrorCategory::PageMediaNotFound
        );
        assert_eq!(finder.calls.load(Ordering::SeqCst), 1);
        let mut phases = Vec::new();
        while let Ok(event) = phase_events.try_recv() {
            if let QueueEvent::Phase { phase, .. } = event {
                phases.push(phase);
            }
        }
        assert_eq!(
            phases,
            [Some(JobPhase::PageSearch), None],
            "the row names and then clears the page search"
        );
        assert_eq!(queue.snapshot()[0].phase, None);
    }

    #[tokio::test]
    async fn a_confirmation_page_reports_page_gate() {
        let attempt = Arc::new(FakeAttempt {
            results: Mutex::new(VecDeque::from([Err(unsupported())])),
            urls: Mutex::new(Vec::new()),
        });
        let finder = Arc::new(FakeFinder {
            result: Mutex::new(Some(Ok(Some(page_media::PageMediaSearchResult::Gate)))),
            calls: AtomicUsize::new(0),
        });
        let queue = Queue::new(
            WrapperFake {
                attempt,
                finder: Arc::clone(&finder),
            },
            1,
        );
        let mut events = queue.subscribe();
        let id = queue.enqueue(PAGE_URL, video_options());

        let (state, result) = finished(&mut events, id).await;

        assert_eq!(state, JobState::Failed);
        assert_eq!(
            result.unwrap().unwrap_err().category,
            ErrorCategory::PageGate
        );
        assert_eq!(finder.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn found_media_is_stored_and_the_second_attempt_uses_its_url() {
        let media = PageMedia {
            url: "https://cdn.example.com/master.m3u8".to_owned(),
            referer: PAGE_URL.to_owned(),
            title: Some("Page title".to_owned()),
            uploader: Some("Page author".to_owned()),
            published_at: Some(1_700_000_000),
        };
        let attempt = Arc::new(FakeAttempt {
            results: Mutex::new(VecDeque::from([
                Err(unsupported()),
                Ok(JobOutcome::default()),
            ])),
            urls: Mutex::new(Vec::new()),
        });
        let finder = Arc::new(FakeFinder {
            result: Mutex::new(Some(Ok(Some(page_media::PageMediaSearchResult::One(
                media.clone(),
            ))))),
            calls: AtomicUsize::new(0),
        });
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let queue = Queue::with_store(
            WrapperFake {
                attempt: Arc::clone(&attempt),
                finder,
            },
            1,
            Arc::clone(&store),
        );
        let mut events = queue.subscribe();
        let id = queue.enqueue(PAGE_URL, video_options());
        assert_eq!(finished(&mut events, id).await.0, JobState::Done);

        assert_eq!(
            lock_recover(&attempt.urls).as_slice(),
            [PAGE_URL, media.url.as_str()]
        );
        let stored = lock_recover(&store).get_job(id.0 as i64).unwrap();
        let stored: JobOptions = serde_json::from_value(stored.options_json).unwrap();
        assert_eq!(stored.page_media, Some(media));
        assert_eq!(stored.cookies, Cookies::None);
        assert!(stored.cookie_domains.is_empty());
    }

    #[tokio::test]
    async fn stored_media_retry_skips_the_finder_and_uses_the_stream_directly() {
        let media = PageMedia {
            url: "https://cdn.example.com/video.mp4".to_owned(),
            referer: PAGE_URL.to_owned(),
            title: Some("Page title".to_owned()),
            uploader: None,
            published_at: None,
        };
        let mut options = read_options(&video_options());
        options.page_media = Some(media.clone());
        let attempt = Arc::new(FakeAttempt {
            results: Mutex::new(VecDeque::from([Ok(JobOutcome::default())])),
            urls: Mutex::new(Vec::new()),
        });
        let finder = Arc::new(FakeFinder {
            result: Mutex::new(Some(Ok(None))),
            calls: AtomicUsize::new(0),
        });
        let queue = Queue::new(
            WrapperFake {
                attempt: Arc::clone(&attempt),
                finder: Arc::clone(&finder),
            },
            1,
        );
        let mut events = queue.subscribe();
        let id = queue.enqueue(PAGE_URL, serde_json::to_value(options).unwrap());
        assert_eq!(finished(&mut events, id).await.0, JobState::Done);

        assert_eq!(lock_recover(&attempt.urls).as_slice(), [media.url.as_str()]);
        assert_eq!(finder.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn tied_media_are_stored_and_the_job_stops_for_a_choice() {
        let media = PageMedia {
            url: "https://cdn.example.com/ep01.m3u8".to_owned(),
            referer: PAGE_URL.to_owned(),
            title: Some("Page title".to_owned()),
            uploader: Some("Page author".to_owned()),
            published_at: Some(1_700_000_000),
        };
        let choices = vec![
            vids_core::executor::PageMediaChoice {
                url: media.url.clone(),
                kind: "HLS".to_owned(),
                duration_seconds: Some(1_420),
            },
            vids_core::executor::PageMediaChoice {
                url: "https://cdn.example.com/ep02.m3u8".to_owned(),
                kind: "HLS".to_owned(),
                duration_seconds: Some(1_420),
            },
        ];
        let attempt = Arc::new(FakeAttempt {
            results: Mutex::new(VecDeque::from([Err(unsupported())])),
            urls: Mutex::new(Vec::new()),
        });
        let finder = Arc::new(FakeFinder {
            result: Mutex::new(Some(Ok(Some(page_media::PageMediaSearchResult::Ask {
                media: media.clone(),
                choices: choices.clone(),
            })))),
            calls: AtomicUsize::new(0),
        });
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let queue = Queue::with_store(WrapperFake { attempt, finder }, 1, Arc::clone(&store));
        let mut events = queue.subscribe();
        let id = queue.enqueue(PAGE_URL, video_options());

        let (state, result) = finished(&mut events, id).await;

        assert_eq!(state, JobState::Failed);
        assert_eq!(
            result.unwrap().unwrap_err().category,
            ErrorCategory::PageMediaChoice
        );
        let stored = lock_recover(&store).get_job(id.0 as i64).unwrap();
        let stored: JobOptions = serde_json::from_value(stored.options_json).unwrap();
        assert_eq!(stored.page_media, Some(media));
        assert_eq!(stored.page_media_choices, choices);
    }

    #[test]
    fn page_media_options_round_trip() {
        let mut options = read_options(&video_options());
        let media = PageMedia {
            url: "https://cdn.example.com/master.m3u8".to_owned(),
            referer: PAGE_URL.to_owned(),
            title: Some("Title".to_owned()),
            uploader: Some("Uploader".to_owned()),
            published_at: Some(123),
        };
        options.page_media = Some(media.clone());
        let json = serde_json::to_value(options).unwrap();
        assert_eq!(json["page_media"]["referer"], PAGE_URL);
        assert_eq!(
            serde_json::from_value::<JobOptions>(json)
                .unwrap()
                .page_media,
            Some(media)
        );
    }

    #[tokio::test]
    async fn a_page_images_job_gets_a_temporary_copy_of_the_cookies_txt_and_the_sites_domains() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("my cookies.txt");
        fs::write(&source, COOKIES_TXT).unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let store = store_with_account(sites::LoginMethod::CookiesTxt {
            path: source.to_string_lossy().into_owned(),
        });
        let mut options = page_image_options();

        let guard =
            prepare_account_cookies(&store, &work, 7, 2, PAGE_URL, &mut options, no_browser).await;

        let copy = work.join("cookies-7-2.txt");
        let read = read_options(&options);
        assert_eq!(read.cookies, Cookies::File(copy.clone()));
        assert_eq!(read.cookie_domains, ["blog.example.com"]);
        assert!(read.page_images.is_some(), "the page images stay");
        assert_eq!(fs::read_to_string(&copy).unwrap(), COOKIES_TXT);
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            COOKIES_TXT,
            "the user's file is untouched"
        );
        assert!(guard.is_some());
        drop(guard);
        assert!(!copy.exists(), "the copy is deleted after the job");
        assert!(source.exists());
    }

    #[tokio::test]
    async fn a_page_images_job_of_a_firefox_account_gets_the_profile_and_the_sites_domains() {
        let temp = tempfile::tempdir().unwrap();
        let store = store_with_account(sites::LoginMethod::Firefox {
            profile: "default-release".to_owned(),
        });
        let mut options = page_image_options();

        let guard = prepare_account_cookies(
            &store,
            temp.path(),
            7,
            1,
            PAGE_URL,
            &mut options,
            no_browser,
        )
        .await;

        let read = read_options(&options);
        assert_eq!(
            read.cookies,
            Cookies::Firefox {
                profile: "default-release".to_owned()
            }
        );
        assert_eq!(read.cookie_domains, ["blog.example.com"]);
        assert!(
            guard.is_none(),
            "Firefox's cookies are read by the executor"
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn an_in_app_accounts_cookies_are_written_for_the_job_and_deleted_after_it() {
        let temp = tempfile::tempdir().unwrap();
        let store = store_with_account(sites::LoginMethod::InApp);
        let account_id = sites::selected_account_for_url(&store.lock().unwrap(), PAGE_URL)
            .unwrap()
            .unwrap()
            .1
            .id;
        let mut options = page_image_options();

        let guard = prepare_account_cookies(
            &store,
            temp.path(),
            7,
            1,
            PAGE_URL,
            &mut options,
            |id, domains| async move {
                assert_eq!(id, account_id);
                assert_eq!(domains, ["blog.example.com"]);
                Ok(Some(COOKIES_TXT.to_owned()))
            },
        )
        .await;

        let file = temp.path().join("cookies-7-1.txt");
        let read = read_options(&options);
        assert_eq!(read.cookies, Cookies::File(file.clone()));
        assert_eq!(read.cookie_domains, ["blog.example.com"]);
        assert_eq!(fs::read_to_string(&file).unwrap(), COOKIES_TXT);
        drop(guard);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn without_usable_cookies_the_job_runs_without_cookies_and_without_domains() {
        let temp = tempfile::tempdir().unwrap();
        // An in-app account that has not logged in yet (no browser folder), one whose
        // cookies cannot be read, and a cookies.txt that is gone.
        for (method, in_app) in [
            (sites::LoginMethod::InApp, Ok(None)),
            (sites::LoginMethod::InApp, Err("no webview".to_owned())),
            (
                sites::LoginMethod::CookiesTxt {
                    path: temp.path().join("gone.txt").to_string_lossy().into_owned(),
                },
                Err("not asked".to_owned()),
            ),
        ] {
            let store = store_with_account(method);
            let mut options = page_image_options();
            let guard = prepare_account_cookies(
                &store,
                temp.path(),
                7,
                1,
                PAGE_URL,
                &mut options,
                |_, _| async move { in_app },
            )
            .await;

            let read = read_options(&options);
            assert_eq!(read.cookies, Cookies::None);
            assert!(read.cookie_domains.is_empty());
            assert!(guard.is_none());
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn a_page_on_no_registered_site_leaves_the_options_as_they_are() {
        let temp = tempfile::tempdir().unwrap();
        let store = store_with_account(sites::LoginMethod::Firefox {
            profile: "default-release".to_owned(),
        });
        let mut options = page_image_options();
        let before = options.clone();

        let guard = prepare_account_cookies(
            &store,
            temp.path(),
            7,
            1,
            "https://elsewhere.test/post",
            &mut options,
            no_browser,
        )
        .await;

        assert!(guard.is_none());
        assert_eq!(options, before);
    }

    #[tokio::test]
    async fn a_source_check_gets_its_own_cookie_file_deleted_after_the_check() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("cookies.txt");
        fs::write(&source, COOKIES_TXT).unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let store = store_with_account(sites::LoginMethod::CookiesTxt {
            path: source.to_string_lossy().into_owned(),
        });

        let prepared = account_cookies(
            &store,
            &work,
            "cookies-source-4-9.txt",
            "source 4",
            "https://blog.example.com/list",
            no_browser,
        )
        .await
        .expect("the URL matches the site");

        let file = work.join("cookies-source-4-9.txt");
        assert_eq!(prepared.cookies, Cookies::File(file.clone()));
        assert_eq!(prepared.domains, ["blog.example.com"]);
        assert_eq!(fs::read_to_string(&file).unwrap(), COOKIES_TXT);
        drop(prepared);
        assert!(!file.exists(), "the check's copy is deleted");
        assert_eq!(fs::read_to_string(&source).unwrap(), COOKIES_TXT);

        assert!(
            account_cookies(
                &store,
                &work,
                "cookies-source-4-10.txt",
                "source 4",
                "https://elsewhere.test/list",
                no_browser,
            )
            .await
            .is_none()
        );
        assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
    }
}

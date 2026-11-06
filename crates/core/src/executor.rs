//! Job execution, from probing through library registration: one video with yt-dlp, an
//! image post with gallery-dl, or a page's own images saved directly over HTTP.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::args::{
    self, CommentsSource, Cookies, DownloadOptions, Embed, PageHeaders, Preset, ProbeOptions,
    ToolPaths,
};
use crate::comment_files::{self, COMMENTS_ONLY_UNSUPPORTED, FetchedComments};
use crate::comments::{self, AssSettings, VideoSize};
use crate::cookie_jar::{CookieJar, SiteCookies};
use crate::filters::{self, Decision, Filter, SkipReason};
use crate::firefox::{self, ProfilesIni};
use crate::gallery_dl::{self, GalleryPost};
use crate::naming::{self, NameTemplate, NamingContext, NamingError, TargetOs, VideoInfo};
use crate::page_images::{self, ImageError, ImageFetcher, PageImage, PageImages};
use crate::parse::{ExpectedStream, ProgressAggregator};
use crate::queue::{
    AlreadySaved, CancelToken, JobContext, JobDetails, JobExecutor, JobFailure, JobOutcome,
    ListEntry,
};
use crate::runner::{self, ExitInfo, RunSpec};
use crate::saved;
use crate::store::{NewLibraryItem, Store, StoreError};
use crate::types::{ErrorCategory, JobState, OutputLine, ParsedLine};

const CANCEL_GRACE: Duration = Duration::from_secs(2);
/// A list probe that runs longer than this is ended, so a hung yt-dlp cannot hold up the
/// checks waiting behind it.
pub const LIST_PROBE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Managed tools and per-machine paths used by the executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Verified yt-dlp, ffmpeg and Deno locations.
    pub tools: ToolPaths,
    /// Parent of disposable per-attempt directories.
    pub work_dir: PathBuf,
    /// Filesystem rules used for output-name length checks.
    pub target_os: TargetOs,
    /// Verified gallery-dl executable. Image fallback is disabled when absent.
    pub gallery_dl: Option<PathBuf>,
}

/// Serializable options attached to one queued job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOptions {
    /// Directory that receives the completed media file.
    pub dest_dir: PathBuf,
    /// Per-job relative folder and file template.
    pub template: NameTemplate,
    /// Local UTC offset used for publish-date naming and filtering.
    pub tz_offset_seconds: i32,
    /// User-facing site name overriding the extractor's friendly name.
    pub site_name: Option<String>,
    /// Download quality or audio preset.
    pub preset: Preset,
    /// Prefer VP9, then H.264, over AV1 at the selected resolution.
    #[serde(default)]
    pub avoid_av1: bool,
    /// Metadata, thumbnail and subtitle embedding choices.
    pub embed: Embed,
    /// Whether supported scrolling comments should be fetched and converted.
    pub comments: bool,
    /// Scrolling-comment ASS rendering settings.
    #[serde(default)]
    pub ass: AssSettings,
    /// Cookie source used by both probing and downloading.
    pub cookies: Cookies,
    /// Optional cross-site metadata filter.
    pub filter: Option<Filter>,
    /// One-based item selected from a list URL.
    #[serde(default)]
    pub playlist_item: Option<u32>,
    /// Download even when the video is already saved.
    #[serde(default)]
    pub force_save: bool,
    /// The page's own images, chosen in the in-app browser: the job saves them directly,
    /// without yt-dlp or gallery-dl. Absent in rows written before page images existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_images: Option<PageImages>,
    /// A stream found by running an otherwise unsupported page. Kept in persisted job
    /// options so retries go straight to the stream while retaining the page's facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_media: Option<PageMedia>,
    /// Equally plausible streams found on the page. A non-empty list pauses the job until
    /// the user chooses one; `page_media` retains the page facts meanwhile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page_media_choices: Vec<PageMediaChoice>,
    /// The domains `cookies` belong to (the matched site's). A page-images job sends
    /// cookies only to these hosts and their subdomains; with none listed it sends none.
    /// Set per attempt by whoever prepares the cookies, never stored with the job.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cookie_domains: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageMedia {
    pub url: String,
    pub referer: String,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub published_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageMediaChoice {
    pub url: String,
    pub kind: String,
    pub duration_seconds: Option<u64>,
}

/// Executes one queued URL with yt-dlp and records completed downloads.
pub struct YtDlpExecutor {
    config: ExecutorConfig,
    store: Arc<Mutex<Store>>,
    program_override: Option<ProgramOverride>,
    gallery_program_override: Option<ProgramOverride>,
    max_image_bytes: u64,
    firefox_profiles_ini: Option<PathBuf>,
    list_probe_timeout: Duration,
}

/// One item of a list, with the key that identifies it from one check of the list to the
/// next (see [`list_items`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceItem {
    /// `"<ie_key or extractor_key>:<id>"` when the entry has both, else its own URL.
    pub key: String,
    /// What a job for this item downloads: the entry's own URL, else the list URL.
    pub url: String,
    /// The 1-based item to take from `url` when it is the list URL itself.
    pub playlist_item: Option<u32>,
    pub title: Option<String>,
}

/// What [`YtDlpExecutor::probe_list`] found at a URL.
#[derive(Debug, Clone, PartialEq)]
pub enum ListProbe {
    /// A list: yt-dlp's flat-playlist JSON (turn it into items with [`list_items`]) and the
    /// list's title (`title`, else `channel` / `uploader`, else the URL).
    List { json: Value, title: String },
    /// A single video or anything else that is not a list of entries.
    NotAList,
}

/// Numbers the scratch folders of list probes, which can run next to each other.
static LIST_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct ProgramOverride {
    program: PathBuf,
    leading_args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
}

impl YtDlpExecutor {
    /// Create an executor that runs the managed yt-dlp executable.
    pub fn new(config: ExecutorConfig, store: Arc<Mutex<Store>>) -> Self {
        Self {
            config,
            store,
            program_override: None,
            gallery_program_override: None,
            max_image_bytes: page_images::MAX_IMAGE_BYTES,
            firefox_profiles_ini: None,
            list_probe_timeout: LIST_PROBE_TIMEOUT,
        }
    }

    /// End list probes after `timeout` instead of [`LIST_PROBE_TIMEOUT`], for tests.
    pub fn with_list_probe_timeout(mut self, timeout: Duration) -> Self {
        self.list_probe_timeout = timeout;
        self
    }

    /// Read Firefox's profile list from this `profiles.ini` instead of the one of the
    /// current user, for tests.
    pub fn with_firefox_profiles_ini(mut self, path: PathBuf) -> Self {
        self.firefox_profiles_ini = Some(path);
        self
    }

    /// Lower the per-image size limit of page-image jobs, for tests.
    pub fn with_max_image_bytes(mut self, max_bytes: u64) -> Self {
        self.max_image_bytes = max_bytes;
        self
    }

    /// Replace yt-dlp with a program and leading arguments, for process-level tests.
    ///
    /// The yt-dlp arguments are appended after `leading_args` unchanged.
    pub fn with_program_override(mut self, program: PathBuf, leading_args: Vec<OsString>) -> Self {
        self.program_override = Some(ProgramOverride {
            program,
            leading_args,
            env: Vec::new(),
        });
        self
    }

    /// Replace yt-dlp and add child-only environment variables, for process-level tests.
    pub fn with_program_override_env(
        mut self,
        program: PathBuf,
        leading_args: Vec<OsString>,
        env: Vec<(OsString, OsString)>,
    ) -> Self {
        self.program_override = Some(ProgramOverride {
            program,
            leading_args,
            env,
        });
        self
    }

    /// Replace gallery-dl and add child-only environment variables for process tests.
    pub fn with_gallery_dl_program_override_env(
        mut self,
        program: PathBuf,
        leading_args: Vec<OsString>,
        env: Vec<(OsString, OsString)>,
    ) -> Self {
        self.gallery_program_override = Some(ProgramOverride {
            program,
            leading_args,
            env,
        });
        self
    }

    async fn execute(&self, job: JobContext) -> Result<JobOutcome, JobFailure> {
        let attempt_path = self
            .config
            .work_dir
            .join(format!("job-{}-{}", job.id, job.attempt));
        let attempt = AttemptDir::create(attempt_path)?;

        let options: JobOptions = serde_json::from_value(job.options.clone())
            .map_err(|error| failure(format!("could not read job options: {error}")))?;
        let comments_only = options.preset == Preset::CommentsOnly;
        if let Some(page) = &options.page_images {
            return self
                .execute_page_images(&job, &options, page, attempt.path())
                .await;
        }

        let _ = job.sink.set_state(JobState::Probing);
        let mut probe = match self.probe(&job, &options, attempt.path()).await {
            Ok(probe) => probe,
            Err(error)
                if error.category == ErrorCategory::NoVideo && self.config.gallery_dl.is_some() =>
            {
                return self
                    .execute_gallery(&job, &options, attempt.path(), error)
                    .await;
            }
            Err(error) => return Err(error),
        };
        if probe.json.get("_type").and_then(Value::as_str) == Some("playlist") {
            if options.playlist_item.is_none() {
                return list_outcome(&job.url, &probe.json);
            }
            probe = selected_playlist_entry(probe)?;
        }
        if let Some(page_media) = &options.page_media {
            apply_page_media_facts(&mut probe.json, page_media);
            probe.raw_json = serde_json::to_string(&probe.json)
                .map_err(|error| failure(format!("could not save page media metadata: {error}")))?;
        }

        let info_json = attempt.path().join("info.json");
        fs::write(&info_json, &probe.raw_json)
            .map_err(|error| io_failure("write probe JSON", &info_json, error))?;

        let info = video_info(&probe.json);
        let extractor_key = string_field(&probe.json, "extractor_key");
        let page_media_site = options
            .page_media
            .as_ref()
            .map(|media| page_images::site_from_url(&media.referer));
        let library_site = page_media_site.clone().or_else(|| extractor_key.clone());
        let site_name = page_media_site.unwrap_or_else(|| {
            options
                .site_name
                .clone()
                .unwrap_or_else(|| friendly_site(extractor_key.as_deref()))
        });
        job.sink.set_details(JobDetails {
            title: info.title.clone(),
            uploader: info.uploader.clone().or_else(|| info.channel.clone()),
            site: Some(site_name.clone()),
            published_at: info.timestamp,
            duration_s: info.duration,
            thumbnail_url: string_field(&probe.json, "thumbnail"),
        });
        if !comments_only
            && !options.force_save
            && let Some(saved) = self
                .already_saved(
                    library_site.as_deref(),
                    info.id.as_deref(),
                    &options.dest_dir,
                )
                .await?
        {
            return Ok(already_saved_outcome(saved));
        }
        if let Some(filter) = &options.filter {
            // Saving again must not be undone by the filter's "skip already downloaded".
            let is_downloaded = !options.force_save
                && match (library_site.as_deref(), info.id.as_deref()) {
                    (Some(site), Some(video_id)) => self
                        .store
                        .lock()
                        .map_err(|_| failure("library store lock is poisoned"))?
                        .has_library_item(site, video_id)
                        .map_err(|error| failure(format!("could not query library: {error}")))?,
                    _ => false,
                };
            if let Decision::Skip(reasons) =
                filters::evaluate(filter, &info, options.tz_offset_seconds, is_downloaded)
            {
                return Ok(JobOutcome {
                    output_path: None,
                    skipped: Some(
                        reasons
                            .iter()
                            .map(skip_reason)
                            .collect::<Vec<_>>()
                            .join("; "),
                    ),
                    ..JobOutcome::default()
                });
            }
        }

        let _ = job.sink.set_state(JobState::Ready);
        let mut naming_context = NamingContext {
            tz_offset_seconds: options.tz_offset_seconds,
            site_name,
            title_budget_bytes: None,
        };
        let destination = options.dest_dir.to_string_lossy();
        naming_context.title_budget_bytes = naming::title_budget(
            &options.template,
            &naming_context,
            &info,
            &destination,
            self.config.target_os,
        )
        .map_err(|error| match error {
            NamingError::DestinationTooLong => failure(format!(
                "destination is too long: {}",
                options.dest_dir.display()
            )),
            _ => failure(format!("could not fit output name: {error}")),
        })?;
        let output_template = naming::compile(&options.template, &naming_context, &info)
            .map_err(|error| failure(format!("could not compile output name: {error}")))?;

        let normal_comment_source = comment_source(options.comments, extractor_key.as_deref());
        if comments_only {
            let source = comment_source(true, extractor_key.as_deref());
            if source == CommentsSource::None {
                return Err(JobFailure {
                    category: ErrorCategory::Unknown,
                    message: COMMENTS_ONLY_UNSUPPORTED.to_owned(),
                });
            }
            let stem = naming::render_stem(
                &options.template,
                &naming_context,
                &info,
                self.config.target_os,
            )
            .map_err(|error| naming_failure(error, &options.dest_dir))?
            .into_iter()
            .fold(options.dest_dir.clone(), |path, component| {
                path.join(component)
            });
            let mut video_name = stem.file_name().map_or_else(OsString::new, OsString::from);
            video_name.push(".video");
            let video_path = stem.with_file_name(video_name);
            let video_size = video_size(&probe.json);
            let _ = job.sink.set_state(JobState::PostProcessing);
            let fetched = self
                .fetch_comments(
                    &job.url,
                    source,
                    &options.cookies,
                    (&options.ass, video_size),
                    &job.cancel,
                    attempt.path(),
                )
                .await?;
            let conflict = comment_files::conflict(&video_path, source, fetched.count);
            let (raw_path, _) = match conflict {
                // The quality-preset path has no dialog: the fuller raw document wins;
                // ties deliberately keep the existing pair.
                Some(existing) if existing.existing_count >= fetched.count => Ok({
                    comment_files::discard(&fetched);
                    comment_files::destination_paths(&video_path, source)
                        .expect("a supported source has destination paths")
                }),
                Some(_) => comment_files::install(&fetched, &video_path, source, true),
                None => comment_files::install(&fetched, &video_path, source, false),
            }
            .map_err(|error| io_failure("place comments", &video_path, error))?;
            let kept_count = comment_files::count_raw_file(&raw_path, source);
            return Ok(JobOutcome {
                output_path: Some(raw_path.to_string_lossy().into_owned()),
                comments_count: u64::try_from(kept_count).ok(),
                ..JobOutcome::default()
            });
        }
        let expected = expected_streams(&probe.json);
        let download_options = DownloadOptions {
            info_json,
            output_template,
            home_dir: options.dest_dir.clone(),
            temp_dir: attempt.path().join("tmp"),
            preset: options.preset,
            avoid_av1: options.avoid_av1,
            embed: options.embed.clone(),
            comments: normal_comment_source,
            cookies: options.cookies.clone(),
            match_filter: options.filter.as_ref().and_then(filters::to_match_filter),
            download_archive: None,
            playlist_items: options.playlist_item.map(|item| item.to_string()),
        };
        let Some(final_path) = self
            .download(
                &job,
                &download_options,
                page_headers(options.page_media.as_ref()).as_ref(),
                expected,
                attempt.path(),
            )
            .await?
        else {
            return Ok(JobOutcome {
                output_path: None,
                skipped: Some("rejected by yt-dlp's match filter".to_owned()),
                ..JobOutcome::default()
            });
        };

        let comments_path = make_comments(
            Path::new(&final_path),
            normal_comment_source,
            &options.ass,
            &probe.json,
        )?;
        let size_bytes = fs::metadata(&final_path)
            .ok()
            .and_then(|metadata| i64::try_from(metadata.len()).ok());
        let mut item = NewLibraryItem {
            job_id: None,
            site: library_site,
            video_id: info.id.clone(),
            url: options
                .page_media
                .as_ref()
                .map(|media| media.referer.clone())
                .or_else(|| string_field(&probe.json, "webpage_url")),
            title: info.title.clone(),
            uploader: info.uploader.clone().or_else(|| info.channel.clone()),
            published_at: info.timestamp,
            duration_s: info.duration,
            path: final_path.clone(),
            comments_path: comments_path.clone(),
            size_bytes,
        };
        match self.store.lock() {
            Ok(store) => {
                if let Ok(id) = i64::try_from(job.id.0) {
                    match store.get_job(id) {
                        Ok(_) => item.job_id = Some(id),
                        Err(StoreError::NotFound { .. }) => {}
                        Err(error) => {
                            eprintln!("could not match downloaded file to queue job: {error}");
                        }
                    }
                }
                if let Err(error) = store.add_library_item(item, unix_now()) {
                    eprintln!("could not add downloaded file to library: {error}");
                }
            }
            Err(_) => eprintln!("could not add downloaded file to library: store lock is poisoned"),
        }

        Ok(JobOutcome {
            output_path: Some(final_path),
            skipped: None,
            ..JobOutcome::default()
        })
    }

    /// Whether this video is already saved: the newest library record of `site` and `id`
    /// (kept even when its file has moved), else a file under `dest_dir` whose name
    /// carries `[id]`.
    async fn already_saved(
        &self,
        site: Option<&str>,
        id: Option<&str>,
        dest_dir: &Path,
    ) -> Result<Option<AlreadySaved>, JobFailure> {
        let Some(id) = id else {
            return Ok(None);
        };
        let recorded = match site {
            Some(site) => self
                .store
                .lock()
                .map_err(|_| failure("library store lock is poisoned"))?
                .latest_library_item(site, id)
                .map_err(|error| failure(format!("could not query library: {error}")))?,
            None => None,
        };
        let (id, dest_dir) = (id.to_owned(), dest_dir.to_owned());
        tokio::task::spawn_blocking(move || {
            if let Some(row) = recorded {
                if Path::new(&row.path).is_file() {
                    return Some(AlreadySaved {
                        saved_at: Some(row.added_at),
                        file_missing: false,
                        path: row.path,
                    });
                }
                // The recorded file is gone; the video may still sit elsewhere in the folder.
                return Some(match saved::find_saved_file(&dest_dir, &id) {
                    Some((found, _)) => AlreadySaved {
                        saved_at: Some(row.added_at),
                        file_missing: false,
                        path: found.to_string_lossy().into_owned(),
                    },
                    None => AlreadySaved {
                        saved_at: Some(row.added_at),
                        file_missing: true,
                        path: row.path,
                    },
                });
            }
            saved::find_saved_file(&dest_dir, &id).map(|(found, modified)| AlreadySaved {
                saved_at: modified,
                file_missing: false,
                path: found.to_string_lossy().into_owned(),
            })
        })
        .await
        .map_err(|error| failure(format!("could not look for a saved copy: {error}")))
    }

    async fn probe(
        &self,
        job: &JobContext,
        options: &JobOptions,
        cwd: &Path,
    ) -> Result<ProbeResult, JobFailure> {
        let headers = page_headers(options.page_media.as_ref());
        let arguments = args::probe_args_with_headers(
            &self.config.tools,
            &job.url,
            &ProbeOptions {
                flat_playlist: options.playlist_item.is_none(),
                playlist_items: options.playlist_item.map(|item| item.to_string()),
                cookies: options.cookies.clone(),
                // Same selection as the download, so `requested_formats` (the progress
                // totals) are the formats that will actually be fetched.
                format_selection: Some(options.preset),
                avoid_av1: options.avoid_av1,
                // niconico/Bilibili only extract comments when asked during extraction;
                // the download from info.json cannot fetch them afterwards.
                comment_subtitles: options.comments && options.preset != Preset::CommentsOnly,
            },
            headers.as_ref(),
        );
        self.run_probe(arguments, &job.cancel, None, cwd).await
    }

    /// Fetch and parse one supported comments document into `scratch`, without a video.
    pub async fn fetch_comments(
        &self,
        url: &str,
        source: CommentsSource,
        cookies: &Cookies,
        render: (&AssSettings, VideoSize),
        cancel: &CancelToken,
        scratch: &Path,
    ) -> Result<FetchedComments, JobFailure> {
        fs::create_dir_all(scratch)
            .map_err(|error| io_failure("create comments folder", scratch, error))?;
        let template = scratch.join("fetch.%(ext)s");
        let arguments = args::comments_args(
            &self.config.tools,
            url,
            &template.to_string_lossy(),
            source,
            cookies,
        )
        .map_err(|error| failure(error.to_string()))?;
        let process = runner::spawn(self.run_spec(arguments, scratch))
            .await
            .map_err(|error| failure(error.to_string()))?;
        let mut failure_details = YtDlpFailureDetails::default();
        let exit = consume_process(process, cancel, |line| {
            failure_details.observe(&line);
        })
        .await?;
        let yt_dlp_failure = (exit.code != Some(0)).then(|| failure_details.finish(exit.code));
        let staged = match &yt_dlp_failure {
            None => comment_files::stage_raw_file(scratch, source, render.0, render.1),
            Some(failure) => Err(failure.message.clone()),
        };
        // yt-dlp already asks niconico without logging in, but when its extractor cannot do
        // it at all, ask the site the same way its own watch page does.
        match staged {
            Ok(fetched) => Ok(fetched),
            Err(first) if source == CommentsSource::Niconico => {
                match self.fetch_niconico_guest_comments(url, scratch).await {
                    Ok(()) => comment_files::stage_raw_file(scratch, source, render.0, render.1)
                        .map_err(|second| {
                            failure(format!(
                                "{first} / ログインせずの取得も失敗しました: {second}"
                            ))
                        }),
                    Err(second) => {
                        let mut reported = yt_dlp_failure.unwrap_or_else(|| failure(first));
                        reported.message = format!(
                            "{} / ログインせずの取得も失敗しました: {second}",
                            reported.message
                        );
                        Err(reported)
                    }
                }
            }
            Err(first) => Err(yt_dlp_failure.unwrap_or_else(|| failure(first))),
        }
    }

    /// Writes the raw comments file yt-dlp would have written, fetched without a login.
    async fn fetch_niconico_guest_comments(&self, url: &str, scratch: &Path) -> Result<(), String> {
        let address = url.to_owned();
        let json = tokio::task::spawn_blocking(move || {
            crate::niconico_guest::fetch_comments_json(&address)
        })
        .await
        .map_err(|error| error.to_string())??;
        let suffix = comment_files::raw_suffix(CommentsSource::Niconico)
            .ok_or_else(|| "comments source is not supported".to_owned())?;
        let path = scratch.join(format!("fetch{suffix}"));
        fs::write(&path, json)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    }

    /// Runs yt-dlp once for a list URL (`-J --flat-playlist`, no format selection) with
    /// `cookies`, and reports whether the URL is a list. A failed run is a [`JobFailure`]
    /// classified from yt-dlp's `ERROR:` line, like a job's probe; cancelling `cancel`
    /// ends yt-dlp, and so does running longer than [`LIST_PROBE_TIMEOUT`] (a `Network`
    /// failure). The run gets its own scratch folder under the work folder, removed
    /// afterwards.
    pub async fn probe_list(
        &self,
        url: &str,
        cookies: Cookies,
        cancel: &CancelToken,
    ) -> Result<ListProbe, JobFailure> {
        let json = self.probe_list_json(url, cookies, cancel).await?;
        Ok(list_probe(url, json))
    }

    /// The run behind [`probe_list`](Self::probe_list), returning yt-dlp's JSON as it is,
    /// for callers that read more of it than whether it is a list.
    pub async fn probe_list_json(
        &self,
        url: &str,
        cookies: Cookies,
        cancel: &CancelToken,
    ) -> Result<Value, JobFailure> {
        let sequence = LIST_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let scratch = AttemptDir::create(
            self.config
                .work_dir
                .join(format!("list-{}-{sequence}", std::process::id())),
        )?;
        let arguments = args::probe_args(
            &self.config.tools,
            url,
            &ProbeOptions {
                flat_playlist: true,
                playlist_items: None,
                cookies,
                format_selection: None,
                avoid_av1: false,
                comment_subtitles: false,
            },
        );
        let time_limit = (
            self.list_probe_timeout,
            list_probe_timed_out(self.list_probe_timeout),
        );
        let probe = self
            .run_probe(arguments, cancel, Some(time_limit), scratch.path())
            .await?;
        Ok(probe.json)
    }

    /// Runs a probe (`-J`) and reads its JSON; the first classified `ERROR:` line, else
    /// the last stderr line, is the failure when yt-dlp exits unsuccessfully. With a
    /// `time_limit`, yt-dlp is ended when it runs longer and its failure is returned.
    async fn run_probe(
        &self,
        arguments: Vec<OsString>,
        cancel: &CancelToken,
        time_limit: Option<(Duration, JobFailure)>,
        cwd: &Path,
    ) -> Result<ProbeResult, JobFailure> {
        let process = runner::spawn(self.run_spec(arguments, cwd))
            .await
            .map_err(|error| failure(error.to_string()))?;
        let mut stdout = Vec::new();
        let mut first_error = None;
        let mut last_stderr = None;
        let exit = consume_process_within(process, cancel, time_limit, |line| {
            if line.stream == crate::types::OutputStream::Stdout {
                stdout.push(line.text.clone());
            } else {
                last_stderr = Some(line.text.clone());
                if first_error.is_none()
                    && let ParsedLine::Error { category, message } = crate::parse::parse_line(&line)
                {
                    first_error = Some(JobFailure { category, message });
                }
            }
        })
        .await?;
        if exit.code != Some(0) {
            return Err(first_error.unwrap_or_else(|| JobFailure {
                category: ErrorCategory::Unknown,
                message: last_stderr.unwrap_or_else(|| "yt-dlp probe failed".to_owned()),
            }));
        }
        // A Rust test binary used through the override writes a harness line before its
        // helper test starts. Real yt-dlp has no such prefix.
        let stdout = if self.program_override.is_some() {
            let first_json = stdout.iter().position(|line| {
                let line = line.trim_start();
                line.starts_with('{') || line.starts_with('[')
            });
            first_json.map_or(stdout.as_slice(), |index| &stdout[index..])
        } else {
            stdout.as_slice()
        };
        let raw_json = stdout.join("\n");
        let json = serde_json::from_str(&raw_json)
            .map_err(|error| failure(format!("could not read yt-dlp probe JSON: {error}")))?;
        Ok(ProbeResult { raw_json, json })
    }

    async fn download(
        &self,
        job: &JobContext,
        options: &DownloadOptions,
        headers: Option<&PageHeaders>,
        expected: Vec<ExpectedStream>,
        cwd: &Path,
    ) -> Result<Option<String>, JobFailure> {
        let arguments = args::download_args_with_headers(&self.config.tools, options, headers);
        let process = runner::spawn(self.run_spec(arguments, cwd))
            .await
            .map_err(|error| failure(error.to_string()))?;
        let mut aggregator = ProgressAggregator::new(expected);
        let mut downloading = false;
        let mut failure_details = YtDlpFailureDetails::default();
        let mut final_path = None;
        let exit = consume_process(process, &job.cancel, |line| {
            failure_details.observe(&line);
            match crate::parse::parse_line(&line) {
                ParsedLine::Download(progress) => {
                    if !downloading {
                        let _ = job.sink.set_state(JobState::Downloading);
                        downloading = true;
                    }
                    let progress = aggregator.update(&progress);
                    job.sink.set_progress(progress.fraction);
                    job.sink
                        .set_transfer(progress.speed_bytes_per_s, progress.eta_s);
                }
                ParsedLine::Postprocess(_) => {
                    // Postprocessing starts once every stream is downloaded, whatever the
                    // progress totals said.
                    job.sink.set_progress(Some(1.0));
                    let _ = job.sink.set_state(JobState::PostProcessing);
                }
                ParsedLine::FinalPath(path) => final_path = Some(path),
                _ => {}
            }
        })
        .await?;
        if exit.code != Some(0) {
            return Err(failure_details.finish(exit.code));
        }
        Ok(final_path)
    }

    async fn execute_gallery(
        &self,
        job: &JobContext,
        options: &JobOptions,
        cwd: &Path,
        no_video_failure: JobFailure,
    ) -> Result<JobOutcome, JobFailure> {
        let post = self.gallery_probe(job, options, cwd).await?;
        if post.files.is_empty() {
            return Err(no_video_failure);
        }

        let site_name = options
            .site_name
            .clone()
            .unwrap_or_else(|| post.site_name.clone());
        job.sink.set_details(JobDetails {
            title: post.info.title.clone(),
            uploader: post.info.uploader.clone(),
            site: Some(site_name.clone()),
            published_at: post.info.timestamp,
            duration_s: None,
            thumbnail_url: None,
        });
        if !options.force_save
            && let Some(saved) = self
                .already_saved(
                    post.category.as_deref(),
                    post.info.id.as_deref(),
                    &options.dest_dir,
                )
                .await?
        {
            return Ok(already_saved_outcome(saved));
        }
        if let Some(skipped) = self.filter_skip(
            options,
            post.category.as_deref(),
            post.info.id.as_deref(),
            &post.info,
        )? {
            return Ok(skipped);
        }

        let _ = job.sink.set_state(JobState::Ready);
        let mut naming_context = NamingContext {
            tz_offset_seconds: options.tz_offset_seconds,
            site_name,
            title_budget_bytes: None,
        };
        let destination = options.dest_dir.to_string_lossy();
        let title_len = post.info.title.as_deref().unwrap_or_default().len();
        let mut budget = title_len;
        for file in &post.files {
            let extension = naming::sanitize_file_part(&file.extension);
            let suffix = format!(" {}.{extension}", file.num);
            let candidate = naming::title_budget_with_suffix(
                &options.template,
                &naming_context,
                &post.info,
                &destination,
                self.config.target_os,
                &suffix,
            )
            .map_err(|error| naming_failure(error, &options.dest_dir))?
            .unwrap_or(title_len);
            budget = budget.min(candidate);
        }
        if budget < title_len {
            naming_context.title_budget_bytes = Some(budget);
        }
        let mut components = naming::render_stem(
            &options.template,
            &naming_context,
            &post.info,
            self.config.target_os,
        )
        .map_err(|error| failure(format!("could not render image output name: {error}")))?;
        let stem = components
            .pop()
            .ok_or_else(|| failure("image output template did not produce a file stem"))?;
        let folder = components
            .into_iter()
            .fold(options.dest_dir.clone(), |path, component| {
                path.join(component)
            });
        fs::create_dir_all(&folder)
            .map_err(|error| io_failure("create image output directory", &folder, error))?;
        let paths = self
            .gallery_download(job, options, &folder, &stem, post.files.len(), cwd)
            .await?;
        if paths.is_empty() {
            return Err(failure(
                "gallery-dl completed without reporting a saved file",
            ));
        }
        self.add_image_library_items(job, post.category.clone(), &post.info, &paths);
        Ok(JobOutcome {
            output_path: Some(paths[0].to_string_lossy().into_owned()),
            skipped: None,
            ..JobOutcome::default()
        })
    }

    async fn gallery_probe(
        &self,
        job: &JobContext,
        options: &JobOptions,
        cwd: &Path,
    ) -> Result<GalleryPost, JobFailure> {
        let arguments = gallery_dl::metadata_args(&job.url, &options.cookies);
        let process = runner::spawn(self.gallery_run_spec(arguments, cwd))
            .await
            .map_err(|error| failure(error.to_string()))?;
        let mut stdout = Vec::new();
        let mut first_error = None;
        let mut last_line = None;
        let exit = consume_process(process, &job.cancel, |line| {
            last_line = Some(line.text.clone());
            if line.stream == crate::types::OutputStream::Stdout {
                stdout.push(line.text.clone());
            }
            if first_error.is_none()
                && let Some((category, message)) = gallery_dl::classify_error_line(&line.text)
            {
                first_error = Some(JobFailure { category, message });
            }
        })
        .await?;
        if exit.code != Some(0) {
            return Err(first_error.unwrap_or_else(|| JobFailure {
                category: ErrorCategory::Unknown,
                message: last_line.unwrap_or_else(|| "gallery-dl metadata failed".to_owned()),
            }));
        }
        let stdout = if self.gallery_program_override.is_some() {
            let first_json = stdout
                .iter()
                .position(|line| line.trim_start().starts_with('['));
            first_json.map_or(stdout.as_slice(), |index| &stdout[index..])
        } else {
            stdout.as_slice()
        };
        gallery_dl::parse_metadata(&stdout.join("\n")).map_err(failure)
    }

    async fn gallery_download(
        &self,
        job: &JobContext,
        options: &JobOptions,
        folder: &Path,
        stem: &str,
        file_count: usize,
        cwd: &Path,
    ) -> Result<Vec<PathBuf>, JobFailure> {
        let arguments = gallery_dl::download_args(folder, stem, &job.url, &options.cookies);
        let process = runner::spawn(self.gallery_run_spec(arguments, cwd))
            .await
            .map_err(|error| failure(error.to_string()))?;
        let _ = job.sink.set_state(JobState::Downloading);
        job.sink.set_progress(None);
        let mut paths = Vec::new();
        let mut first_error = None;
        let exit = consume_process(process, &job.cancel, |line| {
            if first_error.is_none()
                && let Some((category, message)) = gallery_dl::classify_error_line(&line.text)
            {
                first_error = Some(JobFailure { category, message });
            }
            if line.stream == crate::types::OutputStream::Stdout
                && let Some(path) = gallery_dl::written_path(&line.text, folder)
                && !paths.contains(&path)
            {
                paths.push(path);
                job.sink
                    .set_progress(Some(paths.len() as f64 / file_count as f64));
            }
        })
        .await?;
        if exit.code != Some(0) {
            return Err(first_error.unwrap_or_else(|| JobFailure {
                category: ErrorCategory::Unknown,
                message: "gallery-dl download failed".to_owned(),
            }));
        }
        Ok(paths)
    }

    /// One library row per saved image file, all with the post's or page's facts.
    fn add_image_library_items(
        &self,
        job: &JobContext,
        site: Option<String>,
        info: &VideoInfo,
        paths: &[PathBuf],
    ) {
        if paths.is_empty() {
            return;
        }
        let Ok(store) = self.store.lock() else {
            eprintln!("could not add downloaded images to library: store lock is poisoned");
            return;
        };
        let job_id = i64::try_from(job.id.0)
            .ok()
            .filter(|id| store.get_job(*id).is_ok());
        for path in paths {
            let size_bytes = fs::metadata(path)
                .ok()
                .and_then(|metadata| i64::try_from(metadata.len()).ok());
            let item = NewLibraryItem {
                job_id,
                site: site.clone(),
                video_id: info.id.clone(),
                url: Some(job.url.clone()),
                title: info.title.clone(),
                uploader: info.uploader.clone(),
                published_at: info.timestamp,
                duration_s: None,
                path: path.to_string_lossy().into_owned(),
                comments_path: None,
                size_bytes,
            };
            if let Err(error) = store.add_library_item(item, unix_now()) {
                eprintln!("could not add downloaded image to library: {error}");
            }
        }
    }

    /// The outcome of a job its filter skips, or `None` when the filter lets it through (or
    /// there is none). "Already downloaded" looks up `site` and `id` in the library.
    fn filter_skip(
        &self,
        options: &JobOptions,
        site: Option<&str>,
        id: Option<&str>,
        info: &VideoInfo,
    ) -> Result<Option<JobOutcome>, JobFailure> {
        let Some(filter) = &options.filter else {
            return Ok(None);
        };
        // Saving again must not be undone by the filter's "skip already downloaded".
        let is_downloaded = !options.force_save
            && match (site, id) {
                (Some(site), Some(id)) => self
                    .store
                    .lock()
                    .map_err(|_| failure("library store lock is poisoned"))?
                    .has_library_item(site, id)
                    .map_err(|error| failure(format!("could not query library: {error}")))?,
                _ => false,
            };
        let Decision::Skip(reasons) =
            filters::evaluate(filter, info, options.tz_offset_seconds, is_downloaded)
        else {
            return Ok(None);
        };
        Ok(Some(JobOutcome {
            output_path: None,
            skipped: Some(
                reasons
                    .iter()
                    .map(skip_reason)
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            ..JobOutcome::default()
        }))
    }

    /// Saves a page's own images one by one: no probe, the page's facts as the details, the
    /// already-saved check on the site and page id, then each image named like a gallery-dl
    /// file (`<stem> <index>.<ext>`). An image that fails is skipped and counted; the job
    /// fails only when none was saved. The job's account cookies go to the site's own
    /// hosts only; `scratch` is where a copy of Firefox's cookie database may be made.
    async fn execute_page_images(
        &self,
        job: &JobContext,
        options: &JobOptions,
        page: &PageImages,
        scratch: &Path,
    ) -> Result<JobOutcome, JobFailure> {
        let images = page_image_list(page);
        let Some(first) = images.first() else {
            return Err(no_video("no page images were chosen"));
        };
        let site = options
            .site_name
            .clone()
            .unwrap_or_else(|| page_images::site_from_url(&job.url));
        let info = VideoInfo {
            id: page.id.clone(),
            title: page.title.clone(),
            uploader: page.uploader.clone(),
            timestamp: page.published_at,
            ..VideoInfo::default()
        };
        job.sink.set_details(JobDetails {
            title: info.title.clone(),
            uploader: info.uploader.clone(),
            site: Some(site.clone()),
            published_at: info.timestamp,
            duration_s: None,
            thumbnail_url: Some(first.url.clone()),
        });
        if !options.force_save
            && let Some(saved) = self
                .already_saved(Some(&site), info.id.as_deref(), &options.dest_dir)
                .await?
        {
            return Ok(already_saved_outcome(saved));
        }
        if let Some(skipped) = self.filter_skip(options, Some(&site), info.id.as_deref(), &info)? {
            return Ok(skipped);
        }

        let _ = job.sink.set_state(JobState::Ready);
        let (folder, stem) = self.page_image_names(options, &site, &info, &images)?;
        fs::create_dir_all(&folder)
            .map_err(|error| io_failure("create image output directory", &folder, error))?;

        // Read once for the whole job, not per image.
        let cookies = self.page_image_cookies(options, scratch).await?;

        let _ = job.sink.set_state(JobState::Downloading);
        job.sink.set_progress(Some(0.0));
        let mut fetcher = ImageFetcher::new(self.max_image_bytes);
        if let Some(cookies) = cookies {
            fetcher = fetcher.with_cookies(cookies);
        }
        let fetcher = Arc::new(fetcher);
        let total = images.len();
        let mut saved = Vec::new();
        let mut failed = Vec::new();
        for (position, image) in images.into_iter().enumerate() {
            if job.cancel.is_cancelled() {
                self.add_image_library_items(job, Some(site.clone()), &info, &saved);
                return Err(canceled());
            }
            let temp = page_images::temp_path(&folder, &stem, image.index);
            let mut task = {
                let (fetcher, cancel) = (Arc::clone(&fetcher), job.cancel.clone());
                let (referer, folder, stem) = (job.url.clone(), folder.clone(), stem.clone());
                tokio::task::spawn_blocking(move || {
                    fetcher.save(&image, &referer, &folder, &stem, &cancel)
                })
            };
            let joined = tokio::select! {
                biased;
                _ = job.cancel.cancelled() => {
                    // A download that is reading notices at its next chunk and removes its
                    // partial file; one stuck in a read is not waited for, so its partial
                    // file is removed here.
                    if let Ok(Ok(Ok(path))) = tokio::time::timeout(CANCEL_GRACE, &mut task).await {
                        saved.push(path);
                    }
                    remove_partial(&temp);
                    self.add_image_library_items(job, Some(site.clone()), &info, &saved);
                    return Err(canceled());
                }
                joined = &mut task => joined,
            };
            match joined {
                Ok(Ok(path)) => saved.push(path),
                Ok(Err(ImageError::Canceled)) => {
                    self.add_image_library_items(job, Some(site.clone()), &info, &saved);
                    return Err(canceled());
                }
                Ok(Err(error)) => failed.push(error),
                Err(error) => failed.push(ImageError::Io(format!("download task failed: {error}"))),
            }
            job.sink
                .set_progress(Some((position + 1) as f64 / total as f64));
        }

        self.add_image_library_items(job, Some(site), &info, &saved);
        let Some(first_saved) = saved.first() else {
            return Err(no_image_saved(&failed, total));
        };
        Ok(JobOutcome {
            output_path: Some(first_saved.to_string_lossy().into_owned()),
            // The existing outcome fields carry the note: the files are saved, some were not.
            // The UI parses this exact text (apps/desktop/src/lib/jobNote.ts); change both together.
            skipped: (!failed.is_empty())
                .then(|| format!("{} of {total} images could not be saved", failed.len())),
            ..JobOutcome::default()
        })
    }

    /// The job's account cookies for the site's domains, read from the cookie file or from
    /// Firefox's profile; `None` when the job has no cookies or no domains to send them to.
    /// A cookie source that cannot be read fails the job, as yt-dlp would, instead of
    /// quietly downloading as a logged-out visitor.
    async fn page_image_cookies(
        &self,
        options: &JobOptions,
        scratch: &Path,
    ) -> Result<Option<SiteCookies>, JobFailure> {
        if options.cookie_domains.is_empty() {
            return Ok(None);
        }
        let jar = match &options.cookies {
            Cookies::None => return Ok(None),
            Cookies::File(path) => {
                let bytes = fs::read(path)
                    .map_err(|error| io_failure("read the cookies file", path, error))?;
                CookieJar::parse_netscape(&String::from_utf8_lossy(&bytes))
            }
            Cookies::Firefox { profile } => {
                let profile = profile.clone();
                let domains = options.cookie_domains.clone();
                let scratch = scratch.to_path_buf();
                let ini = self.firefox_profiles_ini.clone();
                tokio::task::spawn_blocking(move || {
                    read_firefox_cookies(ini, &profile, &scratch, &domains)
                })
                .await
                .map_err(|error| failure(format!("Firefox cookie task failed: {error}")))??
            }
        };
        Ok(Some(SiteCookies::new(jar, &options.cookie_domains)))
    }

    /// The folder and file stem for a page's images, rendered from the user's template the
    /// way the gallery-dl path does, with room for ` <largest index>.<longest extension>`.
    fn page_image_names(
        &self,
        options: &JobOptions,
        site: &str,
        info: &VideoInfo,
        images: &[PageImage],
    ) -> Result<(PathBuf, String), JobFailure> {
        let mut naming_context = NamingContext {
            tz_offset_seconds: options.tz_offset_seconds,
            site_name: site.to_owned(),
            title_budget_bytes: None,
        };
        let largest = images.iter().map(|image| image.index).max().unwrap_or(1);
        let suffix = format!(
            " {largest}.{}",
            "x".repeat(page_images::MAX_EXTENSION_CHARS)
        );
        naming_context.title_budget_bytes = naming::title_budget_with_suffix(
            &options.template,
            &naming_context,
            info,
            &options.dest_dir.to_string_lossy(),
            self.config.target_os,
            &suffix,
        )
        .map_err(|error| naming_failure(error, &options.dest_dir))?;
        let mut components = naming::render_stem(
            &options.template,
            &naming_context,
            info,
            self.config.target_os,
        )
        .map_err(|error| failure(format!("could not render image output name: {error}")))?;
        let stem = components
            .pop()
            .ok_or_else(|| failure("image output template did not produce a file stem"))?;
        let folder = components
            .into_iter()
            .fold(options.dest_dir.clone(), |path, component| {
                path.join(component)
            });
        Ok((folder, stem))
    }

    fn run_spec(&self, arguments: Vec<OsString>, cwd: &Path) -> RunSpec {
        let (program, mut args, env) = self.program_override.clone().map_or_else(
            || {
                (
                    self.config.tools.yt_dlp.clone(),
                    Vec::<OsString>::new(),
                    Vec::new(),
                )
            },
            |overridden| (overridden.program, overridden.leading_args, overridden.env),
        );
        args.extend(arguments);
        RunSpec {
            program,
            args,
            cwd: Some(cwd.to_owned()),
            env,
        }
    }

    fn gallery_run_spec(&self, arguments: Vec<OsString>, cwd: &Path) -> RunSpec {
        let (program, mut args, env) = self.gallery_program_override.clone().map_or_else(
            || {
                (
                    self.config
                        .gallery_dl
                        .clone()
                        .expect("gallery fallback is checked before use"),
                    Vec::<OsString>::new(),
                    Vec::new(),
                )
            },
            |overridden| (overridden.program, overridden.leading_args, overridden.env),
        );
        args.extend(arguments);
        RunSpec {
            program,
            args,
            cwd: Some(cwd.to_owned()),
            env,
        }
    }
}

impl JobExecutor for YtDlpExecutor {
    async fn run(&self, job: JobContext) -> Result<JobOutcome, JobFailure> {
        self.execute(job).await
    }
}

/// The cookies of Firefox profile `profile` (a name in `profiles.ini`) for `domains`.
fn read_firefox_cookies(
    ini: Option<PathBuf>,
    profile: &str,
    scratch: &Path,
    domains: &[String],
) -> Result<CookieJar, JobFailure> {
    let ini_path = ini
        .or_else(firefox::profiles_ini_path)
        .ok_or_else(|| failure("Firefox's profile list was not found"))?;
    let contents = fs::read_to_string(&ini_path)
        .map_err(|error| io_failure("read Firefox's profile list", &ini_path, error))?;
    let ini_dir = ini_path.parent().unwrap_or_else(|| Path::new(""));
    let profile_dir = ProfilesIni::parse(&contents)
        .profile_dir(ini_dir, profile)
        .ok_or_else(|| failure(format!("Firefox profile {profile} was not found")))?;
    let cookies = firefox::read_cookies(&profile_dir, scratch, domains, unix_now())
        .map_err(|error| failure(error.to_string()))?;
    Ok(CookieJar::from_firefox(&cookies))
}

struct AttemptDir {
    path: PathBuf,
}

impl AttemptDir {
    fn create(path: PathBuf) -> Result<Self, JobFailure> {
        if path.exists() {
            fs::remove_dir_all(&path)
                .map_err(|error| io_failure("clear attempt directory", &path, error))?;
        }
        fs::create_dir_all(&path)
            .map_err(|error| io_failure("create attempt directory", &path, error))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AttemptDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "could not remove attempt directory {}: {error}",
                self.path.display()
            );
        }
    }
}

struct ProbeResult {
    raw_json: String,
    json: Value,
}

/// The chosen images in page order, one per URL and per number.
fn page_image_list(page: &PageImages) -> Vec<PageImage> {
    let mut images = page.images.clone();
    images.sort_by_key(|image| image.index);
    let mut urls = std::collections::HashSet::new();
    let mut indexes = std::collections::HashSet::new();
    images.retain(|image| urls.insert(image.url.clone()) && indexes.insert(image.index));
    images
}

fn remove_partial(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("could not remove partial image {}: {error}", path.display());
    }
}

/// Why a page-images job saved nothing, classified by what went wrong with its images.
fn no_image_saved(failed: &[ImageError], total: usize) -> JobFailure {
    let category = if failed
        .iter()
        .all(|error| matches!(error, ImageError::Status(404 | 410)))
    {
        ErrorCategory::Unavailable
    } else if failed
        .iter()
        .any(|error| matches!(error, ImageError::Status(429)))
    {
        ErrorCategory::RateLimited
    } else if failed
        .iter()
        .any(|error| matches!(error, ImageError::Network(_)))
    {
        ErrorCategory::Network
    } else {
        ErrorCategory::Unknown
    };
    let first = failed
        .first()
        .map_or_else(String::new, |error| format!(": {error}"));
    JobFailure {
        category,
        message: format!("none of the {total} images could be saved{first}"),
    }
}

fn already_saved_outcome(saved: AlreadySaved) -> JobOutcome {
    JobOutcome {
        output_path: None,
        skipped: Some("already saved".to_owned()),
        already_saved: Some(saved),
        ..JobOutcome::default()
    }
}

fn list_outcome(url: &str, playlist: &Value) -> Result<JobOutcome, JobFailure> {
    let entries = playlist
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| no_video("the list did not contain any video entries"))?;
    if entries.is_empty() {
        return Err(no_video("the list did not contain any video entries"));
    }
    let list_total = u32::try_from(entries.len()).unwrap_or(u32::MAX);
    let entries = entries
        .iter()
        .take(300)
        .enumerate()
        .map(|(offset, entry)| {
            let position = u32::try_from(offset + 1).expect("the list cap fits u32");
            let (url, playlist_item) = list_entry_target(url, entry, position);
            ListEntry {
                url,
                playlist_item,
                title: string_field(entry, "title"),
            }
        })
        .collect();
    Ok(JobOutcome {
        output_path: None,
        comments_count: None,
        skipped: None,
        entries,
        list_total: Some(list_total),
        already_saved: None,
    })
}

/// Where one entry of a flat list is downloaded from: its own http(s) `webpage_url` or
/// `url` when it has one other than the list's, else the list URL with the entry's
/// 1-based `playlist_index` (else `position`, its place in the list).
fn list_entry_target(list_url: &str, entry: &Value, position: u32) -> (String, Option<u32>) {
    let child_url = ["webpage_url", "url"]
        .into_iter()
        .filter_map(|key| string_field(entry, key))
        .find(|candidate| is_http_url(candidate) && candidate != list_url);
    match child_url {
        Some(child_url) => (child_url, None),
        None => (
            list_url.to_owned(),
            Some(u32_field(entry, "playlist_index").unwrap_or(position)),
        ),
    }
}

/// Every entry of yt-dlp's flat-playlist JSON for `list_url` (`-J --flat-playlist`), in
/// list order and without a cap. The URL and item number are chosen as for a list job's
/// children; the key is `"<ie_key or extractor_key>:<id>"` when the entry has both, else
/// the entry's own URL. An entry with neither (only reachable as an item number of the
/// list) has no stable key and is left out.
pub fn list_items(list_url: &str, playlist: &Value) -> Vec<SourceItem> {
    let Some(entries) = playlist.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .enumerate()
        .filter_map(|(offset, entry)| {
            let position = u32::try_from(offset + 1).unwrap_or(u32::MAX);
            let (url, playlist_item) = list_entry_target(list_url, entry, position);
            let own_url = playlist_item.is_none().then_some(url.as_str());
            let key = list_item_key(entry, own_url)?;
            Some(SourceItem {
                key,
                url,
                playlist_item,
                title: string_field(entry, "title"),
            })
        })
        .collect()
}

fn list_item_key(entry: &Value, own_url: Option<&str>) -> Option<String> {
    let nonempty = |key: &str| string_field(entry, key).filter(|value| !value.is_empty());
    let extractor = nonempty("ie_key").or_else(|| nonempty("extractor_key"));
    match (extractor, nonempty("id")) {
        (Some(extractor), Some(id)) => Some(format!("{extractor}:{id}")),
        _ => own_url.map(ToOwned::to_owned),
    }
}

/// A probe result as a list (with its title) or not: a list is `_type: "playlist"` with an
/// `entries` array, however short.
fn list_probe(url: &str, json: Value) -> ListProbe {
    let is_list = json.get("_type").and_then(Value::as_str) == Some("playlist")
        && json.get("entries").is_some_and(Value::is_array);
    if !is_list {
        return ListProbe::NotAList;
    }
    let title = ["title", "channel", "uploader"]
        .into_iter()
        .filter_map(|key| string_field(&json, key))
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| url.to_owned());
    ListProbe::List { json, title }
}

fn selected_playlist_entry(probe: ProbeResult) -> Result<ProbeResult, JobFailure> {
    let entries = probe
        .json
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| no_video("the selected playlist item did not contain a video"))?;
    if entries.is_empty() {
        return Err(no_video(
            "the selected playlist item did not contain a video",
        ));
    }
    if entries.len() != 1 || !entries[0].is_object() {
        return Err(failure(
            "the selected playlist item probe did not return exactly one video",
        ));
    }
    let json = entries[0].clone();
    let raw_json = serde_json::to_string(&json)
        .map_err(|error| failure(format!("could not serialize playlist item JSON: {error}")))?;
    Ok(ProbeResult { raw_json, json })
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

async fn consume_process(
    process: runner::RunningProcess,
    cancel: &crate::queue::CancelToken,
    consume: impl FnMut(OutputLine),
) -> Result<ExitInfo, JobFailure> {
    consume_process_within(process, cancel, None, consume).await
}

/// Feeds the process's lines to `consume` until it closes its output. Cancelling
/// `cancel`, or `time_limit`'s duration passing first, ends the process (gracefully, then
/// by force after a grace period) and fails with `canceled` or the limit's failure.
async fn consume_process_within(
    mut process: runner::RunningProcess,
    cancel: &crate::queue::CancelToken,
    time_limit: Option<(Duration, JobFailure)>,
    mut consume: impl FnMut(OutputLine),
) -> Result<ExitInfo, JobFailure> {
    let mut lines = process
        .take_lines()
        .ok_or_else(|| failure("process output was already taken"))?;
    let deadline = time_limit
        .as_ref()
        .map(|(limit, _)| tokio::time::Instant::now() + *limit);
    let expired = async move {
        match deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(expired);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                process.cancel(CANCEL_GRACE).await;
                drop(lines);
                let _ = process.wait().await;
                return Err(canceled());
            }
            () = &mut expired => {
                process.cancel(CANCEL_GRACE).await;
                drop(lines);
                let _ = process.wait().await;
                let (_, timed_out) = time_limit.expect("only a time limit expires");
                return Err(timed_out);
            }
            line = lines.recv() => match line {
                Some(line) => consume(line),
                None => break,
            }
        }
    }
    if cancel.is_cancelled() {
        process.cancel(CANCEL_GRACE).await;
    }
    let exit = process.wait().await;
    if cancel.is_cancelled() {
        Err(canceled())
    } else {
        Ok(exit)
    }
}

/// The failure of a list probe ended after `limit`.
fn list_probe_timed_out(limit: Duration) -> JobFailure {
    let plural = |count: u64| if count == 1 { "" } else { "s" };
    let seconds = limit.as_secs();
    let whole_seconds = limit.subsec_nanos() == 0 && seconds >= 1;
    let after = if whole_seconds && seconds.is_multiple_of(60) {
        let minutes = seconds / 60;
        format!("{minutes} minute{}", plural(minutes))
    } else if whole_seconds {
        format!("{seconds} second{}", plural(seconds))
    } else {
        format!("{} ms", limit.as_millis())
    };
    JobFailure {
        category: ErrorCategory::Network,
        message: format!("the list check timed out after {after}"),
    }
}

fn canceled() -> JobFailure {
    JobFailure {
        category: ErrorCategory::Unknown,
        message: "canceled".to_owned(),
    }
}

const FAILURE_DETAIL_LINES: usize = 5;
const FAILURE_DETAIL_LINE_CHARS: usize = 300;
const FAILURE_MESSAGE_CHARS: usize = 2_000;

/// The useful tail of yt-dlp's stderr and its first classified error. Download and
/// comments-only runs share this so their user-facing failure text cannot drift apart.
#[derive(Default)]
struct YtDlpFailureDetails {
    lines: VecDeque<String>,
    error: Option<JobFailure>,
}

impl YtDlpFailureDetails {
    fn observe(&mut self, line: &OutputLine) {
        if line.stream != crate::types::OutputStream::Stderr || self.error.is_some() {
            return;
        }
        if let ParsedLine::Error { category, message } = crate::parse::parse_line(line) {
            let message = if category == ErrorCategory::Unknown {
                failure_message(message, self.lines.iter().map(String::as_str))
            } else {
                message
            };
            self.error = Some(JobFailure { category, message });
            return;
        }
        if line.text.trim().is_empty()
            || line.text.starts_with(crate::types::DOWNLOAD_MARKER)
            || line.text.starts_with(crate::types::POSTPROCESS_MARKER)
            || line.text.starts_with(crate::types::PATH_MARKER)
        {
            return;
        }
        if self.lines.len() == FAILURE_DETAIL_LINES {
            self.lines.pop_front();
        }
        self.lines
            .push_back(cut_chars(&line.text, FAILURE_DETAIL_LINE_CHARS, true));
    }

    fn finish(self, exit_code: Option<i32>) -> JobFailure {
        self.error.unwrap_or_else(|| {
            let summary = exit_code.map_or_else(
                || "yt-dlp stopped without an exit code".to_owned(),
                |code| format!("yt-dlp exited with code {code}"),
            );
            JobFailure {
                category: ErrorCategory::Unknown,
                message: failure_message(summary, self.lines.iter().map(String::as_str)),
            }
        })
    }
}

fn failure_message<'a>(summary: String, lines: impl IntoIterator<Item = &'a str>) -> String {
    let mut message = summary;
    for line in lines {
        message.push('\n');
        message.push_str(line);
    }
    cut_chars(&message, FAILURE_MESSAGE_CHARS, false)
}

fn cut_chars(text: &str, limit: usize, ellipsis: bool) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept = limit.saturating_sub(if ellipsis { 1 } else { 0 });
    let mut cut = text.chars().take(kept).collect::<String>();
    if ellipsis && limit > 0 {
        cut.push('…');
    }
    cut
}

fn failure(message: impl Into<String>) -> JobFailure {
    JobFailure {
        category: ErrorCategory::Unknown,
        message: message.into(),
    }
}

fn no_video(message: impl Into<String>) -> JobFailure {
    JobFailure {
        category: ErrorCategory::NoVideo,
        message: message.into(),
    }
}

fn naming_failure(error: NamingError, destination: &Path) -> JobFailure {
    match error {
        NamingError::DestinationTooLong => failure(format!(
            "destination is too long: {}",
            destination.display()
        )),
        _ => failure(format!("could not fit output name: {error}")),
    }
}

fn io_failure(action: &str, path: &Path, error: std::io::Error) -> JobFailure {
    failure(format!("could not {action} {}: {error}", path.display()))
}

fn video_info(value: &Value) -> VideoInfo {
    VideoInfo {
        id: string_field(value, "id"),
        title: string_field(value, "title"),
        uploader: string_field(value, "uploader"),
        channel: string_field(value, "channel"),
        uploader_id: string_field(value, "uploader_id"),
        timestamp: i64_field(value, "timestamp"),
        upload_date: string_field(value, "upload_date"),
        playlist_title: string_field(value, "playlist_title"),
        playlist: string_field(value, "playlist"),
        playlist_index: u32_field(value, "playlist_index"),
        series: string_field(value, "series"),
        episode_number: u32_field(value, "episode_number"),
        episode: string_field(value, "episode"),
        ext: string_field(value, "ext"),
        extractor: string_field(value, "extractor"),
        duration: finite_f64_field(value, "duration"),
        live_status: string_field(value, "live_status"),
        vcodec: string_field(value, "vcodec"),
    }
}

fn page_headers(page_media: Option<&PageMedia>) -> Option<PageHeaders> {
    let media = page_media?;
    let page = url::Url::parse(&media.referer).ok()?;
    let origin = page.origin().ascii_serialization();
    (origin != "null").then(|| PageHeaders {
        referer: media.referer.clone(),
        origin,
    })
}

fn apply_page_media_facts(value: &mut Value, media: &PageMedia) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.insert(
        "webpage_url".to_owned(),
        Value::String(media.referer.clone()),
    );
    object.insert(
        "original_url".to_owned(),
        Value::String(media.referer.clone()),
    );
    for field in [
        "title",
        "fulltitle",
        "uploader",
        "channel",
        "uploader_id",
        "timestamp",
        "upload_date",
    ] {
        object.remove(field);
    }
    if let Some(title) = &media.title {
        object.insert("title".to_owned(), Value::String(title.clone()));
        object.insert("fulltitle".to_owned(), Value::String(title.clone()));
    }
    if let Some(uploader) = &media.uploader {
        object.insert("uploader".to_owned(), Value::String(uploader.clone()));
        object.insert("channel".to_owned(), Value::String(uploader.clone()));
    }
    if let Some(published_at) = media.published_at {
        object.insert("timestamp".to_owned(), Value::Number(published_at.into()));
    }
}

fn object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    object(value)?.get(key)?.as_str().map(ToOwned::to_owned)
}

fn finite_f64_field(value: &Value, key: &str) -> Option<f64> {
    let number = object(value)?.get(key)?.as_f64()?;
    number.is_finite().then_some(number)
}

fn i64_field(value: &Value, key: &str) -> Option<i64> {
    let value = object(value)?.get(key)?;
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    if let Some(number) = value.as_u64() {
        return i64::try_from(number).ok();
    }
    let number = value.as_f64()?;
    (number.is_finite() && number >= i64::MIN as f64 && number <= i64::MAX as f64)
        .then_some(number.trunc() as i64)
}

fn u32_field(value: &Value, key: &str) -> Option<u32> {
    u64_value(object(value)?.get(key)?).and_then(|number| u32::try_from(number).ok())
}

fn u64_value(value: &Value) -> Option<u64> {
    if let Some(number) = value.as_u64() {
        return Some(number);
    }
    let number = value.as_f64()?;
    (number.is_finite() && number >= 0.0 && number <= u64::MAX as f64)
        .then_some(number.trunc() as u64)
}

/// Build the expected stream list used by download progress aggregation.
pub fn expected_streams(value: &Value) -> Vec<ExpectedStream> {
    let Some(object) = object(value) else {
        return Vec::new();
    };
    let duration = object.get("duration").and_then(finite_positive_f64);
    if let Some(formats) = object.get("requested_formats").and_then(Value::as_array) {
        return formats
            .iter()
            .filter_map(|format| expected_stream(format, duration))
            .collect();
    }
    expected_stream(value, duration).into_iter().collect()
}

fn expected_stream(value: &Value, duration: Option<f64>) -> Option<ExpectedStream> {
    let object = object(value)?;
    let size = object.get("filesize").and_then(u64_value);
    let approx_size = size
        .or_else(|| object.get("filesize_approx").and_then(u64_value))
        .or_else(|| {
            let tbr = object.get("tbr").and_then(finite_positive_f64)?;
            let bytes = tbr * duration? * 125.0;
            (bytes.is_finite() && bytes > 0.0 && bytes <= u64::MAX as f64)
                .then_some(bytes.round() as u64)
        });
    Some(ExpectedStream {
        format_id: object.get("format_id")?.as_str()?.to_owned(),
        size,
        approx_size,
    })
}

fn finite_positive_f64(value: &Value) -> Option<f64> {
    let value = value.as_f64()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

/// The site name shown to the user for a yt-dlp extractor key (the library stores the key).
pub fn friendly_site(extractor_key: Option<&str>) -> String {
    let site = extractor_key.unwrap_or_default();
    match site.to_ascii_lowercase().as_str() {
        "youtube" => "YouTube".to_owned(),
        "niconico" => "ニコニコ動画".to_owned(),
        "bilibili" => "Bilibili".to_owned(),
        key if key.starts_with("twitch") => "Twitch".to_owned(),
        "twitter" => "X".to_owned(),
        _ => site.to_owned(),
    }
}

fn comment_source(enabled: bool, extractor_key: Option<&str>) -> CommentsSource {
    if !enabled {
        return CommentsSource::None;
    }
    match extractor_key {
        Some("Niconico") => CommentsSource::Niconico,
        Some(site) if site.starts_with("BiliBili") => CommentsSource::Bilibili,
        _ => CommentsSource::None,
    }
}

/// The UI translates these texts by exact pattern (`skipReasonText` in
/// apps/desktop/src/lib/jobNote.ts), also on rows already stored: change them together.
fn skip_reason(reason: &SkipReason) -> String {
    match reason {
        SkipReason::BelowMinDuration { minimum, .. } => format!("shorter than {minimum} s"),
        SkipReason::AboveMaxDuration { maximum, .. } => format!("longer than {maximum} s"),
        SkipReason::BeforeDate { from, .. } => format!("published before {from}"),
        SkipReason::AfterDate { to, .. } => format!("published after {to}"),
        SkipReason::TitleDoesNotIncludeAny(values) => {
            format!("title does not include any of: {}", values.join(", "))
        }
        SkipReason::TitleIncludesExcluded(value) => {
            format!("title includes excluded text: {value}")
        }
        SkipReason::NotVideo => "not a video".to_owned(),
        SkipReason::LiveOrUpcoming => "live or upcoming".to_owned(),
        SkipReason::AlreadyDownloaded => "already downloaded".to_owned(),
    }
}

fn make_comments(
    video_path: &Path,
    source: CommentsSource,
    settings: &AssSettings,
    probe: &Value,
) -> Result<Option<String>, JobFailure> {
    let raw_suffix = match source {
        CommentsSource::None => return Ok(None),
        CommentsSource::Niconico => ".comments.json",
        CommentsSource::Bilibili => ".danmaku.xml",
    };
    let raw_path = sibling_with_suffix(video_path, raw_suffix);
    if !raw_path.exists() {
        return Ok(None);
    }
    // The video is already saved: unreadable comments leave it without an .ass file
    // instead of failing the whole job.
    let raw = match fs::read_to_string(&raw_path) {
        Ok(raw) => raw,
        Err(error) => {
            eprintln!("could not read comments {}: {error}", raw_path.display());
            return Ok(None);
        }
    };
    let parsed = match source {
        CommentsSource::Niconico => comments::parse_niconico(&raw),
        CommentsSource::Bilibili => comments::parse_bilibili(&raw),
        CommentsSource::None => unreachable!(),
    };
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("could not parse comments {}: {error}", raw_path.display());
            return Ok(None);
        }
    };
    let video = video_size(probe);
    let ass = comments::to_ass(&parsed, settings, video);
    let ass_path = sibling_with_suffix(video_path, ".comments.ass");
    let mut bytes = Vec::with_capacity(3 + ass.text.len());
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(ass.text.as_bytes());
    fs::write(&ass_path, bytes)
        .map_err(|error| io_failure("write ASS comments", &ass_path, error))?;
    Ok(Some(ass_path.to_string_lossy().into_owned()))
}

fn video_size(probe: &Value) -> VideoSize {
    let duration_ms = finite_f64_field(probe, "duration").and_then(|seconds| {
        let milliseconds = seconds * 1000.0;
        (milliseconds.is_finite() && milliseconds >= 0.0 && milliseconds <= u64::MAX as f64)
            .then_some(milliseconds.trunc() as u64)
    });
    VideoSize {
        width: u32_field(probe, "width").unwrap_or(1280),
        height: u32_field(probe, "height").unwrap_or(720),
        duration_ms,
    }
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_stem().map_or_else(OsString::new, OsString::from);
    name.push(suffix);
    path.with_file_name(name)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        FAILURE_DETAIL_LINE_CHARS, FAILURE_MESSAGE_CHARS, LIST_PROBE_TIMEOUT, ListProbe,
        SourceItem, YtDlpFailureDetails, list_items, list_probe, list_probe_timed_out,
    };
    use crate::types::{ErrorCategory, OutputLine, OutputStream};

    const CHANNEL: &str = "https://www.youtube.com/@BlenderOfficial/videos";

    #[test]
    fn yt_dlp_failure_detail_lines_are_cut_to_300_characters() {
        let mut details = YtDlpFailureDetails::default();
        details.observe(&OutputLine {
            stream: OutputStream::Stderr,
            text: "x".repeat(FAILURE_DETAIL_LINE_CHARS + 20),
        });
        let failure = details.finish(Some(1));
        let line = failure.message.lines().nth(1).unwrap();
        assert_eq!(line.chars().count(), FAILURE_DETAIL_LINE_CHARS);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn yt_dlp_failure_messages_are_cut_to_2000_characters() {
        let mut details = YtDlpFailureDetails::default();
        for character in ['a', 'b', 'c', 'd', 'e'] {
            details.observe(&OutputLine {
                stream: OutputStream::Stderr,
                text: character.to_string().repeat(FAILURE_DETAIL_LINE_CHARS),
            });
        }
        details.observe(&OutputLine {
            stream: OutputStream::Stderr,
            text: format!("ERROR: {}", "z".repeat(1_000)),
        });
        let failure = details.finish(Some(1));
        assert_eq!(failure.category, ErrorCategory::Unknown);
        assert_eq!(failure.message.chars().count(), FAILURE_MESSAGE_CHARS);
    }

    /// The shape of `yt-dlp -J --flat-playlist` for a YouTube channel's videos tab.
    fn youtube_channel() -> serde_json::Value {
        json!({
            "id": "UCSMOQeBJ2RAnuFungnQOxLg",
            "channel": "Blender",
            "channel_id": "UCSMOQeBJ2RAnuFungnQOxLg",
            "title": "Blender - Videos",
            "uploader": "Blender",
            "uploader_id": "@BlenderOfficial",
            "_type": "playlist",
            "extractor": "youtube:tab",
            "extractor_key": "YoutubeTab",
            "webpage_url": CHANNEL,
            "entries": [
                {
                    "_type": "url",
                    "ie_key": "Youtube",
                    "id": "aqz-KE-bpKQ",
                    "url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
                    "title": "Big Buck Bunny 60fps 4K",
                    "duration": 635.0,
                    "live_status": null,
                    "webpage_url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
                    "extractor": "youtube",
                    "extractor_key": "Youtube",
                    "playlist_index": 1
                },
                {
                    "_type": "url",
                    "ie_key": "Youtube",
                    "id": "WhWc3b3KhnY",
                    "url": "https://www.youtube.com/shorts/WhWc3b3KhnY",
                    "title": "Spring",
                    "duration": 464.0,
                    "playlist_index": 2
                }
            ]
        })
    }

    #[test]
    fn youtube_channel_entries_are_keyed_by_extractor_and_id() {
        assert_eq!(
            list_items(CHANNEL, &youtube_channel()),
            vec![
                SourceItem {
                    key: "Youtube:aqz-KE-bpKQ".to_owned(),
                    url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ".to_owned(),
                    playlist_item: None,
                    title: Some("Big Buck Bunny 60fps 4K".to_owned()),
                },
                SourceItem {
                    key: "Youtube:WhWc3b3KhnY".to_owned(),
                    url: "https://www.youtube.com/shorts/WhWc3b3KhnY".to_owned(),
                    playlist_item: None,
                    title: Some("Spring".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn niconico_user_videos_use_the_niconico_key() {
        let list = "https://www.nicovideo.jp/user/4/video";
        let json = json!({
            "_type": "playlist",
            "id": "4",
            "title": "user 4",
            "extractor_key": "NiconicoUser",
            "entries": [
                {
                    "_type": "url",
                    "ie_key": "Niconico",
                    "id": "sm9",
                    "url": "https://www.nicovideo.jp/watch/sm9",
                    "title": "新・豪血寺一族 -煩悩解放 - レッツゴー！陰陽師"
                }
            ]
        });
        let items = list_items(list, &json);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].key, "Niconico:sm9");
        assert_eq!(items[0].url, "https://www.nicovideo.jp/watch/sm9");
        assert_eq!(items[0].playlist_item, None);
    }

    #[test]
    fn keys_fall_back_to_extractor_key_then_the_entrys_own_url() {
        let list = "https://example.test/list";
        let json = json!({
            "_type": "playlist",
            "entries": [
                // No ie_key: extractor_key and id.
                { "extractor_key": "Generic", "id": "a1", "url": "https://example.test/a1" },
                // No id: the entry's own URL.
                { "ie_key": "Generic", "webpage_url": "https://example.test/b" },
                // Only reachable through the list, but identified: kept with its number.
                { "ie_key": "Example", "id": "c", "url": list, "playlist_index": 9 },
                // Neither an id nor an own URL: no stable key, dropped.
                { "title": "No key", "url": "not-an-http-url" },
                // Empty strings count as absent.
                { "ie_key": "", "id": "", "url": "https://example.test/e" }
            ]
        });
        let items = list_items(list, &json);
        let summary: Vec<_> = items
            .iter()
            .map(|item| (item.key.as_str(), item.url.as_str(), item.playlist_item))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("Generic:a1", "https://example.test/a1", None),
                ("https://example.test/b", "https://example.test/b", None),
                ("Example:c", list, Some(9)),
                ("https://example.test/e", "https://example.test/e", None),
            ]
        );
    }

    #[test]
    fn list_items_are_not_capped() {
        let entries: Vec<_> = (1..=450)
            .map(|index| {
                json!({
                    "ie_key": "Youtube",
                    "id": format!("v{index}"),
                    "url": format!("https://www.youtube.com/watch?v=v{index}")
                })
            })
            .collect();
        let items = list_items(CHANNEL, &json!({ "_type": "playlist", "entries": entries }));
        assert_eq!(items.len(), 450);
        assert_eq!(items[449].key, "Youtube:v450");
        assert!(list_items(CHANNEL, &json!({ "_type": "playlist" })).is_empty());
    }

    #[test]
    fn a_list_probe_takes_the_title_then_channel_then_uploader_then_the_url() {
        let probe = |json: serde_json::Value| match list_probe(CHANNEL, json) {
            ListProbe::List { title, .. } => title,
            ListProbe::NotAList => panic!("expected a list"),
        };
        assert_eq!(probe(youtube_channel()), "Blender - Videos");
        assert_eq!(
            probe(
                json!({ "_type": "playlist", "title": " ", "channel": "Blender", "entries": [] })
            ),
            "Blender"
        );
        assert_eq!(
            probe(json!({ "_type": "playlist", "uploader": "Up", "entries": [] })),
            "Up"
        );
        assert_eq!(
            probe(json!({ "_type": "playlist", "entries": [] })),
            CHANNEL
        );
    }

    #[test]
    fn a_single_video_or_a_playlist_without_entries_is_not_a_list() {
        for json in [
            json!({ "id": "aqz-KE-bpKQ", "title": "Big Buck Bunny", "extractor_key": "Youtube" }),
            json!({ "_type": "video", "entries": [] }),
            json!({ "_type": "playlist", "entries": null }),
            json!({ "_type": "playlist" }),
        ] {
            assert_eq!(list_probe(CHANNEL, json), ListProbe::NotAList);
        }
    }

    #[test]
    fn a_timed_out_list_probe_is_a_network_failure_naming_the_limit() {
        let failure = list_probe_timed_out(LIST_PROBE_TIMEOUT);
        assert_eq!(failure.category, ErrorCategory::Network);
        assert_eq!(failure.message, "the list check timed out after 15 minutes");
        assert_eq!(
            list_probe_timed_out(std::time::Duration::from_secs(60)).message,
            "the list check timed out after 1 minute"
        );
        assert_eq!(
            list_probe_timed_out(std::time::Duration::from_secs(90)).message,
            "the list check timed out after 90 seconds"
        );
        assert_eq!(
            list_probe_timed_out(std::time::Duration::from_millis(1500)).message,
            "the list check timed out after 1500 ms"
        );
    }
}

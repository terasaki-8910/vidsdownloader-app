//! End-to-end executor tests with this test binary acting as yt-dlp.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio::time::{sleep, timeout};
use vids_core::args::{Cookies, Embed, Preset, ToolPaths};
use vids_core::comments::AssSettings;
use vids_core::executor::{
    ExecutorConfig, JobOptions, ListProbe, PageMedia, YtDlpExecutor, list_items,
};
use vids_core::filters::Filter;
use vids_core::naming::{NameTemplate, OptionalPart, Part, TargetOs, Token};
use vids_core::queue::{
    AlreadySaved, CancelToken, JobFailure, JobId, JobOutcome, ListEntry, Queue, QueueEvent,
};
use vids_core::site_probe::{ListVideos, SiteProbeReport, classify};
use vids_core::store::{LibraryQuery, NewLibraryItem, Store};
use vids_core::types::{ErrorCategory, JobState, PATH_MARKER};

const MODE_ENV: &str = "VIDS_EXECUTOR_TEST_MODE";
const RECORD_ENV: &str = "VIDS_EXECUTOR_TEST_RECORD";
const PID_ENV: &str = "VIDS_EXECUTOR_TEST_PID";
const ARGS_ENV: &str = "VIDS_EXECUTOR_TEST_ARGS";
const HELPER: &str = "fake_ytdlp";
const HANG: Duration = Duration::from_secs(10);

const DOWNLOAD_STDOUT: &str = include_str!("fixtures/ytdlp/download_bbb_240p.stdout");
const DOWNLOAD_STDERR: &str = include_str!("fixtures/ytdlp/download_bbb_240p.stderr");
const LOGIN_STDERR: &str = include_str!("fixtures/ytdlp/error_niconico_members_only.stderr");
const NICONICO_COMMENTS: &[u8] =
    include_bytes!("fixtures/comments/niconico_sm9_comments_sample.json");
const GALLERY_FIXTURE: &str = include_str!("fixtures/gallery-dl/x-two-photos.json");
const UNSUPPORTED_URL: &str = "https://example.test/image-post";

/// Entry point used when a child copy of this test binary acts as yt-dlp.
#[test]
fn fake_ytdlp() {
    let args: Vec<OsString> = std::env::args_os().collect();
    let is_probe = args.iter().any(|arg| arg == "-J");
    let is_download = args.iter().any(|arg| arg == "--load-info-json");
    let is_comments = args.iter().any(|arg| arg == "--skip-download");
    let is_gallery_metadata = args.iter().any(|arg| arg == "-j");
    let is_gallery_download = args.iter().any(|arg| arg == "-D");
    if !is_probe && !is_download && !is_comments && !is_gallery_metadata && !is_gallery_download {
        return;
    }
    let mode = std::env::var(MODE_ENV).expect("the parent supplies a fake yt-dlp mode");
    record_invocation(if is_probe {
        "probe"
    } else if is_download {
        "download"
    } else if is_comments {
        "comments"
    } else if is_gallery_metadata {
        "gallery-metadata"
    } else {
        "gallery-download"
    });
    if is_probe {
        fake_probe(&mode, &args);
    } else if is_download {
        fake_download(&mode, &args);
    } else if is_comments {
        fake_comments(&mode, &args);
    } else if is_gallery_metadata {
        fake_gallery_metadata(&mode);
    } else {
        fake_gallery_download(&mode, &args);
    }
}

fn fake_probe(mode: &str, args: &[OsString]) -> ! {
    if matches!(mode, "playlist-item" | "playlist-item-empty") {
        assert!(!args.iter().any(|arg| arg == "--flat-playlist"));
        assert_eq!(
            argument_value(args, "--playlist-items").as_deref(),
            Some(Path::new("2"))
        );
    } else {
        assert!(args.iter().any(|arg| arg == "--flat-playlist"));
    }
    match mode {
        "login" => {
            eprint!("{LOGIN_STDERR}");
            io::stderr().flush().unwrap();
            std::process::exit(1);
        }
        "gallery" => {
            eprintln!("ERROR: [Twitter] No video could be found in this tweet");
            io::stderr().flush().unwrap();
            std::process::exit(1);
        }
        mode if is_unsupported_mode(mode) => {
            eprintln!("ERROR: Unsupported URL: {UNSUPPORTED_URL}");
            io::stderr().flush().unwrap();
            std::process::exit(1);
        }
        "playlist" => {
            println!(
                "{}",
                json!({
                    "_type": "playlist",
                    "entries": [
                        {
                            "title": "Separate URL",
                            "playlist_index": 7,
                            "webpage_url": "https://example.test/watch/separate"
                        },
                        {
                            "title": "Same URL with index",
                            "playlist_index": 9,
                            "webpage_url": "https://example.test/list"
                        },
                        {
                            "title": "Same URL with position",
                            "url": "not-an-http-url"
                        }
                    ]
                })
            );
            io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        "playlist-cap" => {
            let entries = (1..=305)
                .map(|index| json!({"title": format!("Entry {index}")}))
                .collect::<Vec<_>>();
            println!("{}", json!({"_type": "playlist", "entries": entries}));
            io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        "list-flat" => {
            if let Some(path) = std::env::var_os(ARGS_ENV) {
                let lines: Vec<String> = args
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect();
                fs::write(path, lines.join("\n")).unwrap();
            }
            println!(
                "{}",
                json!({
                    "_type": "playlist",
                    "title": "Blender - Videos",
                    "entries": [
                        {
                            "_type": "url",
                            "ie_key": "Youtube",
                            "id": "aqz-KE-bpKQ",
                            "url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
                            "title": "Big Buck Bunny"
                        }
                    ]
                })
            );
            io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        "list-hang" => {
            if let Some(path) = std::env::var_os(PID_ENV) {
                fs::write(path, std::process::id().to_string()).unwrap();
            }
            std::thread::sleep(Duration::from_secs(60));
            std::process::exit(0);
        }
        "playlist-empty" | "playlist-item-empty" => {
            println!(r#"{{"_type":"playlist","entries":[]}}"#);
            io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        "playlist-item" => {
            let entry = json!({
                "id": "entry-2",
                "title": "Playlist Entry",
                "extractor_key": "Youtube",
                "extractor": "youtube",
                "uploader": "List Owner",
                "webpage_url": "https://example.test/watch/entry-2",
                "timestamp": 1212060266,
                "duration": 42,
                "thumbnail": "https://example.test/thumbnails/entry-2.jpg",
                "width": 426,
                "height": 240,
                "ext": "mp4",
                "requested_formats": [
                    {"format_id": "395", "filesize": 10},
                    {"format_id": "258", "filesize": 20}
                ]
            });
            println!("{}", json!({"_type": "playlist", "entries": [entry]}));
            io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        "ok"
        | "slow"
        | "ok-niconico"
        | "ok-comments-only"
        | "page-media"
        | "download-warning"
        | "download-unknown-error"
        | "download-classified-error" => {
            let niconico = matches!(mode, "ok-niconico" | "ok-comments-only");
            let page_media = mode == "page-media";
            let value = json!({
                "id": if page_media { "x36xhzz" } else { "YE7VzlLtp-4" },
                "title": if page_media { "x36xhzz" } else { "Big Buck Bunny" },
                "extractor_key": if page_media { "Generic" } else if niconico { "Niconico" } else { "Youtube" },
                "extractor": if page_media { "generic" } else if niconico { "niconico" } else { "youtube" },
                "uploader": if page_media { "Generic" } else { "Blender" },
                "channel": if page_media { Some("Generic channel") } else { None },
                "uploader_id": if page_media { Some("generic-id") } else { None },
                "webpage_url": if page_media { "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8" } else { "https://example.test/watch/YE7VzlLtp-4" },
                "timestamp": if page_media { 1586908800 } else { 1212060266 },
                "upload_date": if page_media { Some("20200415") } else { None },
                "duration": if niconico { 320 } else { 597 },
                "thumbnail": "https://example.test/thumbnails/YE7VzlLtp-4.jpg",
                "width": if niconico { 480 } else { 426 },
                "height": if niconico { 360 } else { 240 },
                "ext": "mp4",
                "requested_formats": [
                    {"format_id": "395", "filesize": 6540562},
                    {"format_id": "258", "filesize": 28921366}
                ]
            });
            println!("{}", serde_json::to_string(&value).unwrap());
            io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        other => panic!("unknown fake yt-dlp mode {other}"),
    }
}

fn fake_comments(mode: &str, args: &[OsString]) -> ! {
    assert_eq!(mode, "ok-comments-only");
    let template = argument_value(args, "-o").expect("comments output template");
    let path = PathBuf::from(
        template
            .to_string_lossy()
            .replace("%(ext)s", "comments.json"),
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, NICONICO_COMMENTS).unwrap();
    std::process::exit(0);
}

/// The modes where yt-dlp does not support the URL: `unsupported` has no gallery-dl, the
/// others differ in what gallery-dl does with the same URL.
fn is_unsupported_mode(mode: &str) -> bool {
    mode == "unsupported" || mode.starts_with("unsupported-gallery")
}

/// The modes where the executor is given a gallery-dl (the fake is this same binary).
fn uses_gallery_dl(mode: &str) -> bool {
    mode == "gallery" || mode.starts_with("unsupported-gallery")
}

/// What the fake gallery-dl prints for `-j`: the post's metadata, an empty list, a hang, or
/// one error line with a non-zero exit.
fn fake_gallery_metadata(mode: &str) -> ! {
    let (error_line, code) = match mode {
        "gallery" | "unsupported-gallery" => {
            print!("{GALLERY_FIXTURE}");
            ("", 0)
        }
        "unsupported-gallery-empty" => {
            println!("[]");
            ("", 0)
        }
        "unsupported-gallery-cancel" => {
            if let Some(path) = std::env::var_os(PID_ENV) {
                fs::write(path, std::process::id().to_string()).unwrap();
            }
            std::thread::sleep(Duration::from_secs(60));
            ("", 0)
        }
        "unsupported-gallery-unsupported" => (
            "[gallery-dl][error] Unsupported URL 'https://example.test/image-post'",
            64,
        ),
        "unsupported-gallery-404" => ("[twitter][error] NotFoundError: HTTP 404 Not Found", 4),
        "unsupported-gallery-auth" => ("[pixiv][error] AuthorizationError: HTTP 403 Forbidden", 16),
        other => panic!("unknown fake gallery-dl mode {other}"),
    };
    if !error_line.is_empty() {
        eprintln!("{error_line}");
    }
    io::stdout().flush().unwrap();
    io::stderr().flush().unwrap();
    std::process::exit(code);
}

fn fake_gallery_download(mode: &str, args: &[OsString]) -> ! {
    assert!(matches!(mode, "gallery" | "unsupported-gallery"));
    let folder = argument_value(args, "-D").expect("gallery destination");
    let template = argument_value(args, "-f")
        .expect("gallery filename template")
        .to_string_lossy()
        .into_owned();
    fs::create_dir_all(&folder).unwrap();
    for num in 1..=2 {
        let name = template
            .replace("{num}", &num.to_string())
            .replace("{extension}", "jpg")
            .replace("{{", "{")
            .replace("}}", "}");
        let path = folder.join(name);
        fs::write(&path, format!("fake image {num}")).unwrap();
        println!("{}", path.display());
    }
    io::stdout().flush().unwrap();
    std::process::exit(0);
}

fn fake_download(mode: &str, args: &[OsString]) -> ! {
    if mode == "slow" {
        if let Some(path) = std::env::var_os(PID_ENV) {
            fs::write(path, std::process::id().to_string()).unwrap();
        }
        println!(
            r#"[vdl-dl]{{"status":"downloading","downloaded_bytes":1,"total_bytes":100,"filename":"slow.f395.mp4"}}"#
        );
        io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(60));
        std::process::exit(0);
    }

    const RETRY_WARNING: &str =
        "WARNING: [download] Got error: HTTP Error 403: Forbidden. Retrying fragment 3 (1/10)...";
    match mode {
        "download-warning" => {
            eprintln!("{RETRY_WARNING}");
            io::stderr().flush().unwrap();
            std::process::exit(1);
        }
        "download-unknown-error" => {
            eprintln!("{RETRY_WARNING}");
            eprintln!("ERROR: fragment 3 not found, unable to continue");
            io::stderr().flush().unwrap();
            std::process::exit(1);
        }
        "download-classified-error" => {
            eprintln!(
                "ERROR: Unable to download webpage: <urlopen error [Errno 11001] getaddrinfo failed>"
            );
            io::stderr().flush().unwrap();
            std::process::exit(1);
        }
        _ => {}
    }
    assert!(matches!(
        mode,
        "ok" | "ok-niconico" | "playlist-item" | "page-media"
    ));
    if mode == "playlist-item" {
        assert_eq!(
            argument_value(args, "--playlist-items").as_deref(),
            Some(Path::new("2"))
        );
        let info_path = argument_value(args, "--load-info-json").expect("info JSON path");
        let info: Value = serde_json::from_slice(&fs::read(info_path).unwrap()).unwrap();
        assert_eq!(info["title"], "Playlist Entry");
        assert_eq!(info["id"], "entry-2");
        assert!(info.get("entries").is_none());
    } else if mode == "page-media" {
        let info_path = argument_value(args, "--load-info-json").expect("info JSON path");
        let info: Value = serde_json::from_slice(&fs::read(info_path).unwrap()).unwrap();
        assert_eq!(info["title"], "Sample player page");
        for field in [
            "uploader",
            "channel",
            "uploader_id",
            "timestamp",
            "upload_date",
        ] {
            assert!(info.get(field).is_none(), "{field} came from the manifest");
        }
    }
    let home = home_path(args);
    fs::create_dir_all(&home).unwrap();
    let final_path = match mode {
        "playlist-item" => home.join("Playlist Entry [entry-2].mp4"),
        "page-media" => home.join("Sample player page [x36xhzz].mp4"),
        _ => home.join("Big Buck Bunny [YE7VzlLtp-4].mp4"),
    };
    fs::write(&final_path, b"fake video").unwrap();
    if mode == "ok-niconico" {
        fs::write(
            home.join("Big Buck Bunny [YE7VzlLtp-4].comments.json"),
            NICONICO_COMMENTS,
        )
        .unwrap();
    }

    let mut stdout = io::stdout().lock();
    for line in DOWNLOAD_STDOUT.lines() {
        if line.starts_with(PATH_MARKER) {
            writeln!(
                stdout,
                "{PATH_MARKER}{}",
                serde_json::to_string(&final_path.to_string_lossy()).unwrap()
            )
            .unwrap();
        } else {
            writeln!(stdout, "{line}").unwrap();
        }
    }
    stdout.flush().unwrap();
    drop(stdout);
    std::thread::sleep(Duration::from_millis(25));
    eprint!("{DOWNLOAD_STDERR}");
    io::stderr().flush().unwrap();
    std::process::exit(0);
}

fn home_path(args: &[OsString]) -> PathBuf {
    args.windows(2)
        .find_map(|pair| {
            (pair[0] == "-P")
                .then(|| pair[1].to_string_lossy())
                .and_then(|value| value.strip_prefix("home:").map(PathBuf::from))
        })
        .expect("download arguments contain home:")
}

fn argument_value(args: &[OsString], name: &str) -> Option<PathBuf> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| PathBuf::from(&pair[1]))
}

fn record_invocation(kind: &str) {
    let Some(path) = std::env::var_os(RECORD_ENV) else {
        return;
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{kind}").unwrap();
}

struct Harness {
    temp: TempDir,
    store: Arc<Mutex<Store>>,
}

impl Harness {
    fn new() -> Self {
        Self {
            temp: tempfile::tempdir().unwrap(),
            store: Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
        }
    }

    fn queue(&self, mode: &str) -> Queue<YtDlpExecutor> {
        self.queue_with_env(mode, Vec::new())
    }

    fn queue_with_env(
        &self,
        mode: &str,
        extra_env: Vec<(OsString, OsString)>,
    ) -> Queue<YtDlpExecutor> {
        Queue::new(self.executor_with_env(mode, extra_env), 1)
    }

    fn persistent_queue(&self, mode: &str) -> Queue<YtDlpExecutor> {
        Queue::with_store(
            self.executor_with_env(mode, Vec::new()),
            1,
            Arc::clone(&self.store),
        )
    }

    fn executor_with_env(
        &self,
        mode: &str,
        mut extra_env: Vec<(OsString, OsString)>,
    ) -> YtDlpExecutor {
        extra_env.push((MODE_ENV.into(), mode.into()));
        let config = ExecutorConfig {
            tools: ToolPaths {
                yt_dlp: PathBuf::from("managed-yt-dlp"),
                ffmpeg_dir: PathBuf::from("managed-ffmpeg"),
                deno: PathBuf::from("managed-deno"),
            },
            work_dir: self.temp.path().join("work"),
            target_os: if cfg!(windows) {
                TargetOs::Windows
            } else {
                TargetOs::MacOs
            },
            gallery_dl: uses_gallery_dl(mode).then(|| PathBuf::from("managed-gallery-dl")),
        };
        let executor = YtDlpExecutor::new(config, Arc::clone(&self.store))
            .with_program_override_env(current_exe(), helper_args(), extra_env.clone());
        if uses_gallery_dl(mode) {
            executor.with_gallery_dl_program_override_env(current_exe(), helper_args(), extra_env)
        } else {
            executor
        }
    }

    fn options(&self, comments: bool, filter: Option<Filter>) -> Value {
        serde_json::to_value(JobOptions {
            dest_dir: self.temp.path().join("downloads"),
            template: NameTemplate {
                folders: Vec::new(),
                file: vec![
                    Part::Field {
                        field: Token::Title,
                    },
                    Part::Text {
                        text: " [".to_owned(),
                    },
                    Part::Field { field: Token::Id },
                    Part::Text {
                        text: "]".to_owned(),
                    },
                ],
            },
            tz_offset_seconds: 9 * 60 * 60,
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
            comments,
            ass: AssSettings::default(),
            cookies: Cookies::None,
            filter,
            playlist_item: None,
            force_save: false,
            page_images: None,
            page_media: None,
            page_media_choices: Vec::new(),
            cookie_domains: Vec::new(),
        })
        .unwrap()
    }

    fn attempt_path(&self, id: JobId) -> PathBuf {
        self.temp.path().join("work").join(format!("job-{id}-1"))
    }
}

#[test]
fn job_options_without_avoid_av1_default_to_false() {
    let harness = Harness::new();
    let mut json = harness.options(false, None);
    json.as_object_mut().unwrap().remove("avoid_av1");

    let options: JobOptions = serde_json::from_value(json).unwrap();
    assert!(!options.avoid_av1);
}

fn current_exe() -> PathBuf {
    std::env::current_exe().expect("path of this test binary")
}

fn helper_args() -> Vec<OsString> {
    [
        "--exact",
        HELPER,
        "--nocapture",
        "--quiet",
        "--test-threads=1",
        "--",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

struct Finished {
    state: JobState,
    outcome: Option<Result<JobOutcome, JobFailure>>,
    events: Vec<QueueEvent>,
}

async fn finished(mut events: broadcast::Receiver<QueueEvent>, id: JobId) -> Finished {
    timeout(HANG, async move {
        let mut seen = Vec::new();
        loop {
            let event = events.recv().await.expect("event receiver keeps up");
            let result = match &event {
                QueueEvent::Finished {
                    id: finished,
                    state,
                    outcome_or_error,
                } if *finished == id => Some((*state, outcome_or_error.clone())),
                _ => None,
            };
            seen.push(event);
            if let Some((state, outcome)) = result {
                return Finished {
                    state,
                    outcome,
                    events: seen,
                };
            }
        }
    })
    .await
    .expect("job finishes")
}

fn in_progress_states(events: &[QueueEvent], id: JobId) -> Vec<JobState> {
    events
        .iter()
        .filter_map(|event| match event {
            QueueEvent::StateChanged {
                id: changed, to, ..
            } if *changed == id
                && matches!(
                    to,
                    JobState::Probing
                        | JobState::Ready
                        | JobState::Downloading
                        | JobState::PostProcessing
                ) =>
            {
                Some(*to)
            }
            _ => None,
        })
        .collect()
}

fn library_query() -> LibraryQuery {
    LibraryQuery {
        text: None,
        site: None,
        published_from: None,
        published_to: None,
        include_missing: true,
        limit: 10,
        offset: 0,
    }
}

#[tokio::test]
async fn ok_runs_through_queue_and_records_progress_and_library() {
    let harness = Harness::new();
    let queue = harness.queue("ok");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", harness.options(false, None));
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Done);
    let outcome = done.outcome.unwrap().unwrap();
    let output = outcome.output_path.expect("saved path");
    assert_eq!(outcome.skipped, None);
    assert!(Path::new(&output).is_file());
    assert_eq!(
        in_progress_states(&done.events, id),
        [
            JobState::Probing,
            JobState::Ready,
            JobState::Downloading,
            JobState::PostProcessing
        ]
    );
    let progress: Vec<f64> = done
        .events
        .iter()
        .filter_map(|event| match event {
            QueueEvent::Progress {
                id: changed,
                fraction: Some(fraction),
            } if *changed == id => Some(*fraction),
            _ => None,
        })
        .collect();
    assert_eq!(progress.last(), Some(&1.0));
    let details = done
        .events
        .iter()
        .find_map(|event| match event {
            QueueEvent::Details {
                id: changed,
                details,
            } if *changed == id => Some(details),
            _ => None,
        })
        .expect("probe details event");
    assert_eq!(details.title.as_deref(), Some("Big Buck Bunny"));
    assert_eq!(details.site.as_deref(), Some("YouTube"));
    assert_eq!(
        details.thumbnail_url.as_deref(),
        Some("https://example.test/thumbnails/YE7VzlLtp-4.jpg")
    );
    assert!(done.events.iter().any(|event| matches!(
        event,
        QueueEvent::Transfer {
            id: changed,
            speed_bytes_per_s: Some(speed),
            ..
        } if *changed == id && *speed > 0.0
    )));

    let store = harness.store.lock().unwrap();
    let items = store.search_library(&library_query()).unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.site.as_deref(), Some("Youtube"));
    assert_eq!(item.video_id.as_deref(), Some("YE7VzlLtp-4"));
    assert_eq!(item.title.as_deref(), Some("Big Buck Bunny"));
    assert_eq!(item.uploader.as_deref(), Some("Blender"));
    assert_eq!(item.published_at, Some(1_212_060_266));
    assert_eq!(item.duration_s, Some(597.0));
    assert_eq!(item.path, output);
    assert_eq!(item.size_bytes, Some(10));
    assert_eq!(item.job_id, None);
    drop(store);
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn page_media_uses_only_page_facts_for_site_uploader_date_and_naming() {
    let harness = Harness::new();
    let queue = harness.queue("page-media");
    let events = queue.subscribe();
    let mut options: JobOptions = serde_json::from_value(harness.options(false, None)).unwrap();
    options.site_name = Some("Registered site name".to_owned());
    options.template.file = vec![
        Part::Optional {
            optional: OptionalPart {
                before: String::new(),
                field: Token::Date,
                after: " ".to_owned(),
            },
        },
        Part::Optional {
            optional: OptionalPart {
                before: String::new(),
                field: Token::Uploader,
                after: " ".to_owned(),
            },
        },
        Part::Field {
            field: Token::Title,
        },
        Part::Optional {
            optional: OptionalPart {
                before: " [".to_owned(),
                field: Token::Id,
                after: "]".to_owned(),
            },
        },
    ];
    options.page_media = Some(PageMedia {
        url: "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8".to_owned(),
        referer: "https://www.example.com/player".to_owned(),
        title: Some("Sample player page".to_owned()),
        uploader: None,
        published_at: None,
    });
    let id = queue.enqueue(
        "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8",
        serde_json::to_value(options).unwrap(),
    );
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Done, "{:#?}", done.outcome);
    let output = done.outcome.unwrap().unwrap().output_path.unwrap();
    assert!(output.ends_with("Sample player page [x36xhzz].mp4"));
    let details = done
        .events
        .iter()
        .find_map(|event| match event {
            QueueEvent::Details {
                id: changed,
                details,
            } if *changed == id => Some(details),
            _ => None,
        })
        .expect("page details event");
    assert_eq!(details.title.as_deref(), Some("Sample player page"));
    assert_eq!(details.site.as_deref(), Some("example.com"));
    assert_eq!(details.uploader, None);
    assert_eq!(details.published_at, None);

    let store = harness.store.lock().unwrap();
    let item = store.search_library(&library_query()).unwrap().remove(0);
    assert_eq!(item.site.as_deref(), Some("example.com"));
    assert_eq!(item.url.as_deref(), Some("https://www.example.com/player"));
    assert_eq!(item.title.as_deref(), Some("Sample player page"));
    assert_eq!(item.uploader, None);
    assert_eq!(item.published_at, None);
}

#[tokio::test]
async fn no_video_falls_back_to_gallery_dl_and_records_each_image() {
    let harness = Harness::new();
    let queue = harness.queue("gallery");
    let events = queue.subscribe();
    let id = queue.enqueue(
        "https://example.test/image-post",
        harness.options(false, None),
    );
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Done, "{:#?}", done.outcome);
    let outcome = done.outcome.unwrap().unwrap();
    let output = outcome.output_path.expect("first image path");
    assert!(output.ends_with(" [2020050253005918661] 1.jpg"));
    assert_eq!(
        in_progress_states(&done.events, id),
        [JobState::Probing, JobState::Ready, JobState::Downloading]
    );
    let store = harness.store.lock().unwrap();
    let items = store.search_library(&library_query()).unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .all(|item| item.site.as_deref() == Some("twitter"))
    );
    assert!(
        items
            .iter()
            .all(|item| item.video_id.as_deref() == Some("2020050253005918661"))
    );
    assert!(items.iter().all(|item| item.path.ends_with(".jpg")));
    assert!(items.iter().any(|item| item.path.ends_with(" 1.jpg")));
    assert!(items.iter().any(|item| item.path.ends_with(" 2.jpg")));
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn unsupported_url_tries_gallery_dl_and_records_each_image() {
    let harness = Harness::new();
    let (queue, record) = harness.recording_queue("unsupported-gallery");
    let events = queue.subscribe();
    let id = queue.enqueue(UNSUPPORTED_URL, harness.options(false, None));
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Done, "{:#?}", done.outcome);
    let outcome = done.outcome.unwrap().unwrap();
    let output = outcome.output_path.expect("first image path");
    assert!(output.ends_with(" [2020050253005918661] 1.jpg"));
    assert_eq!(
        in_progress_states(&done.events, id),
        [JobState::Probing, JobState::Ready, JobState::Downloading]
    );
    assert_eq!(
        fs::read_to_string(record).unwrap(),
        "probe
gallery-metadata
gallery-download
"
    );
    let store = harness.store.lock().unwrap();
    let items = store.search_library(&library_query()).unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .all(|item| item.site.as_deref() == Some("twitter")
                && item.video_id.as_deref() == Some("2020050253005918661")
                && item.path.ends_with(".jpg"))
    );
    assert!(items.iter().any(|item| item.path.ends_with(" 1.jpg")));
    assert!(items.iter().any(|item| item.path.ends_with(" 2.jpg")));
    assert!(!harness.attempt_path(id).exists());
}

/// Runs a job for a URL yt-dlp does not support and returns its failure and the fake
/// programs that ran, one per line.
async fn unsupported_failure(mode: &str) -> (JobFailure, String) {
    let harness = Harness::new();
    let (queue, record) = harness.recording_queue(mode);
    let events = queue.subscribe();
    let id = queue.enqueue(UNSUPPORTED_URL, harness.options(false, None));
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Failed, "{mode}: {:#?}", done.outcome);
    assert_eq!(harness.library_len(), 0, "{mode}");
    assert!(!harness.attempt_path(id).exists(), "{mode}");
    let error = done.outcome.unwrap().unwrap_err();
    (error, fs::read_to_string(record).unwrap())
}

fn yt_dlp_unsupported_failure() -> JobFailure {
    JobFailure {
        category: ErrorCategory::UnsupportedUrl,
        message: format!("Unsupported URL: {UNSUPPORTED_URL}"),
    }
}

#[tokio::test]
async fn unsupported_url_without_gallery_dl_is_unchanged() {
    let (error, invocations) = unsupported_failure("unsupported").await;

    assert_eq!(error, yt_dlp_unsupported_failure());
    assert_eq!(
        invocations,
        "probe
"
    );
}

#[tokio::test]
async fn unsupported_url_gallery_dl_cannot_read_keeps_yt_dlps_failure() {
    let (error, invocations) = unsupported_failure("unsupported-gallery-unsupported").await;

    assert_eq!(error, yt_dlp_unsupported_failure());
    assert_eq!(
        invocations,
        "probe
gallery-metadata
"
    );
}

#[tokio::test]
async fn unsupported_url_gallery_dl_failing_another_way_keeps_yt_dlps_failure() {
    for mode in [
        "unsupported-gallery-404",
        "unsupported-gallery-auth",
        "unsupported-gallery-empty",
    ] {
        let (error, invocations) = unsupported_failure(mode).await;

        assert_eq!(error, yt_dlp_unsupported_failure(), "{mode}");
        assert_eq!(
            invocations,
            "probe
gallery-metadata
",
            "{mode}"
        );
    }
}

#[tokio::test]
async fn cancelling_an_unsupported_url_during_the_gallery_dl_run_ends_it() {
    let harness = Harness::new();
    let pid_path = harness.temp.path().join("gallery.pid");
    let queue = harness.queue_with_env(
        "unsupported-gallery-cancel",
        vec![(PID_ENV.into(), pid_path.as_os_str().to_owned())],
    );
    let events = queue.subscribe();
    let id = queue.enqueue(UNSUPPORTED_URL, harness.options(false, None));
    let pid: u32 = timeout(HANG, async {
        loop {
            if let Some(pid) = fs::read_to_string(&pid_path)
                .ok()
                .and_then(|text| text.parse().ok())
            {
                return pid;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("gallery-dl starts");
    let canceled_at = Instant::now();
    queue.cancel(id).unwrap();
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Canceled);
    assert!(done.outcome.is_none());
    assert!(canceled_at.elapsed() < Duration::from_secs(5));
    assert_gone(pid).await;
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn persistent_queue_links_the_library_item_to_its_job() {
    let harness = Harness::new();
    let queue = harness.persistent_queue("ok");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", harness.options(false, None));
    assert_eq!(finished(events, id).await.state, JobState::Done);

    let item = harness
        .store
        .lock()
        .unwrap()
        .search_library(&library_query())
        .unwrap()
        .remove(0);
    assert_eq!(item.job_id, Some(id.0 as i64));
}

#[tokio::test]
async fn login_failure_is_classified() {
    let harness = Harness::new();
    let queue = harness.queue("login");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/private", harness.options(false, None));
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Failed);
    let error = done.outcome.unwrap().unwrap_err();
    assert_eq!(error.category, ErrorCategory::LoginRequired);
    assert!(error.message.contains("members only"));
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn download_failure_without_error_keeps_the_warning_tail() {
    let error = failed_download("download-warning").await;
    assert_eq!(error.category, ErrorCategory::Unknown);
    assert!(error.message.starts_with("yt-dlp exited with code 1"));
    assert!(error.message.contains(
        "WARNING: [download] Got error: HTTP Error 403: Forbidden. Retrying fragment 3 (1/10)..."
    ));
}

#[tokio::test]
async fn unknown_download_error_keeps_the_preceding_warning() {
    let error = failed_download("download-unknown-error").await;
    assert_eq!(error.category, ErrorCategory::Unknown);
    assert!(
        error
            .message
            .starts_with("fragment 3 not found, unable to continue")
    );
    assert!(
        error
            .message
            .lines()
            .skip(1)
            .any(|line| line.starts_with("WARNING:"))
    );
}

#[tokio::test]
async fn classified_download_error_keeps_its_category_and_message() {
    let error = failed_download("download-classified-error").await;
    assert_eq!(error.category, ErrorCategory::Network);
    assert_eq!(
        error.message,
        "Unable to download webpage: <urlopen error [Errno 11001] getaddrinfo failed>"
    );
}

async fn failed_download(mode: &str) -> JobFailure {
    let harness = Harness::new();
    let queue = harness.queue(mode);
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", harness.options(false, None));
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Failed);
    done.outcome.unwrap().unwrap_err()
}

#[tokio::test]
async fn playlist_probe_returns_ordered_child_jobs() {
    let harness = Harness::new();
    let record = harness.temp.path().join("invocations.txt");
    let queue = harness.queue_with_env(
        "playlist",
        vec![(RECORD_ENV.into(), record.as_os_str().to_owned())],
    );
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/list", harness.options(false, None));
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Done);
    assert_eq!(
        done.outcome.unwrap().unwrap(),
        JobOutcome {
            output_path: None,
            comments_count: None,
            skipped: None,
            entries: vec![
                ListEntry {
                    url: "https://example.test/watch/separate".to_owned(),
                    playlist_item: None,
                    title: Some("Separate URL".to_owned()),
                },
                ListEntry {
                    url: "https://example.test/list".to_owned(),
                    playlist_item: Some(9),
                    title: Some("Same URL with index".to_owned()),
                },
                ListEntry {
                    url: "https://example.test/list".to_owned(),
                    playlist_item: Some(3),
                    title: Some("Same URL with position".to_owned()),
                },
            ],
            list_total: Some(3),
            already_saved: None,
        }
    );
    assert_eq!(fs::read_to_string(record).unwrap(), "probe\n");
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn playlist_probe_caps_children_at_300_and_reports_total() {
    let harness = Harness::new();
    let queue = harness.queue("playlist-cap");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/list", harness.options(false, None));
    let outcome = finished(events, id).await.outcome.unwrap().unwrap();
    assert_eq!(outcome.entries.len(), 300);
    assert_eq!(outcome.list_total, Some(305));
    assert_eq!(outcome.entries[299].playlist_item, Some(300));
}

#[tokio::test]
async fn playlist_item_probe_unwraps_entry_for_naming_and_download() {
    let harness = Harness::new();
    let record = harness.temp.path().join("invocations.txt");
    let queue = harness.queue_with_env(
        "playlist-item",
        vec![(RECORD_ENV.into(), record.as_os_str().to_owned())],
    );
    let events = queue.subscribe();
    let mut options = harness.options(false, None);
    options["playlist_item"] = json!(2);
    let id = queue.enqueue("https://example.test/list", options);
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Done);
    let outcome = done.outcome.unwrap().unwrap();
    assert!(
        outcome
            .output_path
            .as_deref()
            .is_some_and(|path| path.ends_with("Playlist Entry [entry-2].mp4"))
    );
    let title = done.events.iter().find_map(|event| match event {
        QueueEvent::Details {
            id: changed,
            details,
        } if *changed == id => details.title.as_deref(),
        _ => None,
    });
    assert_eq!(title, Some("Playlist Entry"));
    assert_eq!(fs::read_to_string(record).unwrap(), "probe\ndownload\n");
}

#[tokio::test]
async fn empty_playlist_fails_as_no_video() {
    let harness = Harness::new();
    let queue = harness.queue("playlist-empty");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/list", harness.options(false, None));
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Failed);
    assert_eq!(
        done.outcome.unwrap().unwrap_err().category,
        ErrorCategory::NoVideo
    );
}

#[tokio::test]
async fn missing_playlist_item_fails_as_no_video() {
    let harness = Harness::new();
    let queue = harness.queue("playlist-item-empty");
    let events = queue.subscribe();
    let mut options = harness.options(false, None);
    options["playlist_item"] = json!(2);
    let id = queue.enqueue("https://example.test/list", options);
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Failed);
    assert_eq!(
        done.outcome.unwrap().unwrap_err().category,
        ErrorCategory::NoVideo
    );
}

#[tokio::test]
async fn app_filter_skips_without_starting_a_download() {
    let harness = Harness::new();
    let record = harness.temp.path().join("invocations.txt");
    let queue = harness.queue_with_env(
        "ok",
        vec![(RECORD_ENV.into(), record.as_os_str().to_owned())],
    );
    let events = queue.subscribe();
    let filter = Filter {
        min_duration_s: Some(1000),
        ..Filter::default()
    };
    let id = queue.enqueue(
        "https://example.test/video",
        harness.options(false, Some(filter)),
    );
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Done);
    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(outcome.output_path, None);
    assert_eq!(outcome.skipped.as_deref(), Some("shorter than 1000 s"));
    assert_eq!(fs::read_to_string(record).unwrap(), "probe\n");
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn niconico_comments_are_written_with_bom_and_recorded() {
    let harness = Harness::new();
    let queue = harness.queue("ok-niconico");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/sm9", harness.options(true, None));
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Done);
    let output = done.outcome.unwrap().unwrap().output_path.unwrap();
    let comments = Path::new(&output).with_file_name("Big Buck Bunny [YE7VzlLtp-4].comments.ass");
    assert!(
        fs::read(&comments)
            .unwrap()
            .starts_with(&[0xEF, 0xBB, 0xBF])
    );
    assert!(
        Path::new(&output)
            .with_file_name("Big Buck Bunny [YE7VzlLtp-4].comments.json")
            .is_file()
    );
    let store = harness.store.lock().unwrap();
    let item = store.search_library(&library_query()).unwrap().remove(0);
    assert_eq!(item.comments_path.as_deref(), comments.to_str());
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn comments_only_fetches_side_files_without_a_video_or_library_row() {
    let harness = Harness::new();
    let queue = harness.queue("ok-comments-only");
    let events = queue.subscribe();
    let mut options: JobOptions = serde_json::from_value(harness.options(false, None)).unwrap();
    options.preset = Preset::CommentsOnly;
    let id = queue.enqueue(
        "https://www.nicovideo.jp/watch/sm9",
        serde_json::to_value(options).unwrap(),
    );
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Done);
    let outcome = done.outcome.unwrap().unwrap();
    let raw = PathBuf::from(outcome.output_path.unwrap());
    assert_eq!(
        raw.file_name().and_then(|name| name.to_str()),
        Some("Big Buck Bunny [YE7VzlLtp-4].comments.json")
    );
    assert!(raw.is_file());
    assert!(
        raw.with_file_name("Big Buck Bunny [YE7VzlLtp-4].comments.ass")
            .is_file()
    );
    assert!(outcome.comments_count.is_some_and(|count| count > 0));
    assert!(
        harness
            .store
            .lock()
            .unwrap()
            .search_library(&library_query())
            .unwrap()
            .is_empty()
    );
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn cancel_ends_the_helper_and_queue_records_canceled() {
    let harness = Harness::new();
    let pid_path = harness.temp.path().join("helper.pid");
    let queue = harness.queue_with_env(
        "slow",
        vec![(PID_ENV.into(), pid_path.as_os_str().to_owned())],
    );
    let mut events = queue.subscribe();
    let id = queue.enqueue("https://example.test/slow", harness.options(false, None));
    timeout(HANG, async {
        loop {
            if matches!(
                events.recv().await.unwrap(),
                QueueEvent::StateChanged {
                    id: changed,
                    to: JobState::Downloading,
                    ..
                } if changed == id
            ) {
                break;
            }
        }
    })
    .await
    .expect("download starts");
    let pid: u32 = fs::read_to_string(&pid_path).unwrap().parse().unwrap();
    let canceled_at = Instant::now();
    queue.cancel(id).unwrap();
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Canceled);
    assert!(done.outcome.is_none());
    assert!(canceled_at.elapsed() < Duration::from_secs(5));
    assert_gone(pid).await;
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn unreadable_options_fail_as_unknown() {
    let harness = Harness::new();
    let queue = harness.queue("ok");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", json!({"dest_dir": 12}));
    let done = finished(events, id).await;
    assert_eq!(done.state, JobState::Failed);
    let error = done.outcome.unwrap().unwrap_err();
    assert_eq!(error.category, ErrorCategory::Unknown);
    assert!(error.message.contains("could not read job options"));
    assert!(!harness.attempt_path(id).exists());
}

async fn assert_gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while os::process_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "helper process {pid} is still alive"
        );
        sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(windows)]
mod os {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::Foundation::STILL_ACTIVE;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub fn process_alive(pid: u32) -> bool {
        // SAFETY: OpenProcess takes no pointers and returns an owned handle or null.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            return false;
        }
        // SAFETY: `raw` is a new non-null handle owned by this function.
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut code = 0_u32;
        // SAFETY: the handle is valid and `code` points to writable storage.
        let ok = unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) };
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(unix)]
mod os {
    pub fn process_alive(pid: u32) -> bool {
        let pid = libc::pid_t::try_from(pid).expect("process id fits pid_t");
        // SAFETY: signal 0 only checks whether the process exists.
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

const SAVED_AT: i64 = 1_700_000_000;

impl Harness {
    fn downloads(&self) -> PathBuf {
        self.temp.path().join("downloads")
    }

    /// Writes a file and returns its path.
    fn saved_file(&self, relative: &str) -> PathBuf {
        let path = relative
            .split('/')
            .fold(self.downloads(), |path, part| path.join(part));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"saved earlier").unwrap();
        path
    }

    fn record_in_library(&self, site: &str, video_id: &str, path: &Path, added_at: i64) {
        self.store
            .lock()
            .unwrap()
            .add_library_item(
                NewLibraryItem {
                    job_id: None,
                    site: Some(site.to_owned()),
                    video_id: Some(video_id.to_owned()),
                    url: None,
                    title: None,
                    uploader: None,
                    published_at: None,
                    duration_s: None,
                    path: path.to_string_lossy().into_owned(),
                    comments_path: None,
                    size_bytes: None,
                },
                added_at,
            )
            .unwrap();
    }

    fn library_len(&self) -> usize {
        self.store
            .lock()
            .unwrap()
            .search_library(&library_query())
            .unwrap()
            .len()
    }

    /// A queue whose fake yt-dlp and gallery-dl log each invocation, and where the log's
    /// path is returned.
    fn recording_queue(&self, mode: &str) -> (Queue<YtDlpExecutor>, PathBuf) {
        let record = self.temp.path().join("invocations.txt");
        let queue = self.queue_with_env(
            mode,
            vec![(RECORD_ENV.into(), record.as_os_str().to_owned())],
        );
        (queue, record)
    }
}

fn set_modified(path: &Path, unix_seconds: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(unix_seconds))
        .unwrap();
}

#[tokio::test]
async fn a_library_hit_skips_without_downloading() {
    let harness = Harness::new();
    let saved = harness.saved_file("Big Buck Bunny [YE7VzlLtp-4].mp4");
    harness.record_in_library("Youtube", "YE7VzlLtp-4", &saved, SAVED_AT);
    let (queue, record) = harness.recording_queue("ok");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", harness.options(false, None));
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Done);
    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(outcome.output_path, None);
    assert_eq!(outcome.skipped.as_deref(), Some("already saved"));
    assert_eq!(
        outcome.already_saved,
        Some(AlreadySaved {
            saved_at: Some(SAVED_AT),
            file_missing: false,
            path: saved.to_string_lossy().into_owned(),
        })
    );
    assert_eq!(
        fs::read_to_string(record).unwrap(),
        "probe\n",
        "no download process ran"
    );
    assert_eq!(harness.library_len(), 1, "nothing new was recorded");
    assert!(!harness.attempt_path(id).exists());
}

#[tokio::test]
async fn a_library_row_whose_file_is_gone_is_reported_missing() {
    let harness = Harness::new();
    let gone = harness.downloads().join("moved away [YE7VzlLtp-4].mp4");
    harness.record_in_library("Youtube", "YE7VzlLtp-4", &gone, SAVED_AT);
    let (queue, record) = harness.recording_queue("ok");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", harness.options(false, None));
    let done = finished(events, id).await;

    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(outcome.skipped.as_deref(), Some("already saved"));
    assert_eq!(
        outcome.already_saved,
        Some(AlreadySaved {
            saved_at: Some(SAVED_AT),
            file_missing: true,
            path: gone.to_string_lossy().into_owned(),
        })
    );
    assert_eq!(fs::read_to_string(record).unwrap(), "probe\n");
}

#[tokio::test]
async fn a_library_row_whose_file_moved_inside_the_folder_is_found() {
    let harness = Harness::new();
    let old = harness.downloads().join("old place [YE7VzlLtp-4].mp4");
    harness.record_in_library("Youtube", "YE7VzlLtp-4", &old, SAVED_AT);
    let moved = harness.saved_file("sorted/Big Buck Bunny [YE7VzlLtp-4].mp4");
    let (queue, record) = harness.recording_queue("ok");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", harness.options(false, None));
    let done = finished(events, id).await;

    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(
        outcome.already_saved,
        Some(AlreadySaved {
            saved_at: Some(SAVED_AT),
            file_missing: false,
            path: moved.to_string_lossy().into_owned(),
        })
    );
    assert_eq!(fs::read_to_string(record).unwrap(), "probe\n");
}

#[tokio::test]
async fn a_file_in_the_folder_counts_without_a_library_row() {
    let harness = Harness::new();
    let saved = harness.saved_file("Someone/Big Buck Bunny [YE7VzlLtp-4].mp4");
    set_modified(&saved, 1_650_000_000);
    harness.saved_file("Someone/Big Buck Bunny [YE7VzlLtp-4].mp4.part");
    let (queue, record) = harness.recording_queue("ok");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", harness.options(false, None));
    let done = finished(events, id).await;

    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(outcome.skipped.as_deref(), Some("already saved"));
    assert_eq!(
        outcome.already_saved,
        Some(AlreadySaved {
            saved_at: Some(1_650_000_000),
            file_missing: false,
            path: saved.to_string_lossy().into_owned(),
        })
    );
    assert_eq!(fs::read_to_string(record).unwrap(), "probe\n");
    assert_eq!(harness.library_len(), 0);
}

#[tokio::test]
async fn a_partial_download_in_the_folder_does_not_count() {
    let harness = Harness::new();
    harness.saved_file("Big Buck Bunny [YE7VzlLtp-4].mp4.part");
    let (queue, record) = harness.recording_queue("ok");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", harness.options(false, None));
    let done = finished(events, id).await;

    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(outcome.already_saved, None);
    assert!(outcome.output_path.is_some());
    assert_eq!(fs::read_to_string(record).unwrap(), "probe\ndownload\n");
}

#[tokio::test]
async fn force_save_downloads_a_video_that_is_already_saved() {
    let harness = Harness::new();
    let earlier = harness.saved_file("earlier/Big Buck Bunny [YE7VzlLtp-4].mp4");
    harness.record_in_library("Youtube", "YE7VzlLtp-4", &earlier, SAVED_AT);
    let (queue, record) = harness.recording_queue("ok");
    let events = queue.subscribe();
    // The filter would skip a video the library already holds; saving again must not
    // be undone by it.
    let filter = Filter {
        skip_downloaded: true,
        ..Filter::default()
    };
    let mut options = harness.options(false, Some(filter));
    options["force_save"] = json!(true);
    let id = queue.enqueue("https://example.test/video", options);
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Done);
    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(outcome.skipped, None);
    assert_eq!(outcome.already_saved, None);
    let output = outcome.output_path.expect("the video is downloaded again");
    assert!(Path::new(&output).is_file());
    assert_ne!(Path::new(&output), earlier);
    assert_eq!(fs::read_to_string(record).unwrap(), "probe\ndownload\n");
    assert_eq!(harness.library_len(), 2);
}

#[tokio::test]
async fn an_unsaved_video_is_downloaded_without_force_save() {
    let harness = Harness::new();
    let other = harness.saved_file("Another video [other-id-1].mp4");
    harness.record_in_library("Youtube", "other-id-1", &other, SAVED_AT);
    let (queue, record) = harness.recording_queue("ok");
    let events = queue.subscribe();
    let id = queue.enqueue("https://example.test/video", harness.options(false, None));
    let done = finished(events, id).await;

    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(outcome.already_saved, None);
    assert!(outcome.output_path.is_some());
    assert_eq!(fs::read_to_string(record).unwrap(), "probe\ndownload\n");
}

#[tokio::test]
async fn an_already_saved_image_post_skips_without_downloading() {
    let harness = Harness::new();
    let saved = harness.saved_file("Some post [2020050253005918661] 1.jpg");
    harness.record_in_library("twitter", "2020050253005918661", &saved, SAVED_AT);
    let (queue, record) = harness.recording_queue("gallery");
    let events = queue.subscribe();
    let id = queue.enqueue(
        "https://example.test/image-post",
        harness.options(false, None),
    );
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Done, "{:#?}", done.outcome);
    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(outcome.output_path, None);
    assert_eq!(outcome.skipped.as_deref(), Some("already saved"));
    assert_eq!(
        outcome.already_saved,
        Some(AlreadySaved {
            saved_at: Some(SAVED_AT),
            file_missing: false,
            path: saved.to_string_lossy().into_owned(),
        })
    );
    assert_eq!(
        fs::read_to_string(record).unwrap(),
        "probe\ngallery-metadata\n",
        "gallery-dl was asked for metadata only"
    );
    assert_eq!(harness.library_len(), 1);
}

#[tokio::test]
async fn force_save_downloads_an_image_post_that_is_already_saved() {
    let harness = Harness::new();
    let saved = harness.saved_file("Some post [2020050253005918661] 1.jpg");
    harness.record_in_library("twitter", "2020050253005918661", &saved, SAVED_AT);
    let (queue, record) = harness.recording_queue("gallery");
    let events = queue.subscribe();
    let mut options = harness.options(false, None);
    options["force_save"] = json!(true);
    let id = queue.enqueue("https://example.test/image-post", options);
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Done, "{:#?}", done.outcome);
    let outcome = done.outcome.unwrap().unwrap();
    assert_eq!(outcome.already_saved, None);
    assert!(outcome.output_path.is_some());
    assert_eq!(
        fs::read_to_string(record).unwrap(),
        "probe\ngallery-metadata\ngallery-download\n"
    );
}

#[test]
fn job_options_without_force_save_default_to_false() {
    let harness = Harness::new();
    let mut json = harness.options(false, None);
    json.as_object_mut().unwrap().remove("force_save");

    let options: JobOptions = serde_json::from_value(json).unwrap();
    assert!(!options.force_save);
}

const LIST_URL: &str = "https://www.youtube.com/@BlenderOfficial/videos";

#[tokio::test]
async fn a_list_probe_runs_one_flat_probe_with_the_cookies_and_no_format_selection() {
    let harness = Harness::new();
    let args_path = harness.temp.path().join("args.txt");
    let cookies = harness.temp.path().join("cookies-source-1-1.txt");
    let executor = harness.executor_with_env(
        "list-flat",
        vec![(ARGS_ENV.into(), args_path.as_os_str().to_owned())],
    );

    let probe = executor
        .probe_list(
            LIST_URL,
            Cookies::File(cookies.clone()),
            &CancelToken::new(),
        )
        .await
        .unwrap();

    let ListProbe::List { json, title } = probe else {
        panic!("expected a list");
    };
    assert_eq!(title, "Blender - Videos");
    let items = list_items(LIST_URL, &json);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].key, "Youtube:aqz-KE-bpKQ");

    let args = fs::read_to_string(&args_path).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert!(args.contains(&"-J"));
    assert!(args.contains(&"--flat-playlist"));
    for absent in ["-S", "-f", "--write-subs", "--playlist-items"] {
        assert!(!args.contains(&absent), "{absent} must not be passed");
    }
    let cookies_at = args.iter().position(|arg| *arg == "--cookies").unwrap();
    assert_eq!(args[cookies_at + 1], cookies.to_string_lossy());
    assert_eq!(args.last(), Some(&LIST_URL));
    // The scratch folder is gone afterwards.
    let work = harness.temp.path().join("work");
    assert!(!work.exists() || fs::read_dir(&work).unwrap().next().is_none());
}

#[tokio::test]
async fn a_list_probe_of_a_single_video_says_it_is_not_a_list() {
    let harness = Harness::new();
    let probe = harness
        .executor_with_env("ok", Vec::new())
        .probe_list(
            "https://www.youtube.com/watch?v=YE7VzlLtp-4",
            Cookies::None,
            &CancelToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(probe, ListProbe::NotAList);
}

#[tokio::test]
async fn a_failed_list_probe_is_classified_like_a_job_probe() {
    let harness = Harness::new();
    let error = harness
        .executor_with_env("login", Vec::new())
        .probe_list(LIST_URL, Cookies::None, &CancelToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.category, ErrorCategory::LoginRequired);
    assert!(error.message.contains("members only"));
}

#[tokio::test]
async fn the_add_site_report_reads_the_json_of_a_cookie_free_list_probe() {
    let harness = Harness::new();
    let mut reports = Vec::new();
    for (mode, url) in [
        ("list-flat", LIST_URL),
        ("ok", "https://www.youtube.com/watch?v=YE7VzlLtp-4"),
        ("login", LIST_URL),
    ] {
        let result = harness
            .executor_with_env(mode, Vec::new())
            .probe_list_json(url, Cookies::None, &CancelToken::new())
            .await;
        reports.push(classify(result, false));
    }
    assert_eq!(
        reports,
        vec![
            SiteProbeReport::List {
                count: 1,
                videos: ListVideos::Supported,
                logged_in: false
            },
            SiteProbeReport::Video { logged_in: false },
            SiteProbeReport::LoginRequired,
        ]
    );
}

#[tokio::test]
async fn cancelling_a_list_probe_ends_yt_dlp() {
    let harness = Harness::new();
    let pid_path = harness.temp.path().join("list.pid");
    let executor = harness.executor_with_env(
        "list-hang",
        vec![(PID_ENV.into(), pid_path.as_os_str().to_owned())],
    );
    let cancel = CancelToken::new();
    let canceller = cancel.clone();
    let watched = pid_path.clone();
    let stopper = tokio::spawn(async move {
        while !watched.exists() {
            sleep(Duration::from_millis(25)).await;
        }
        canceller.cancel();
    });

    let started = Instant::now();
    let error = timeout(HANG, executor.probe_list(LIST_URL, Cookies::None, &cancel))
        .await
        .expect("the probe ends once cancelled")
        .unwrap_err();
    stopper.await.unwrap();
    assert_eq!(error.message, "canceled");
    assert!(started.elapsed() < Duration::from_secs(8));
    let pid: u32 = fs::read_to_string(&pid_path).unwrap().parse().unwrap();
    assert_gone(pid).await;
}

#[tokio::test]
async fn a_list_probe_that_runs_too_long_ends_yt_dlp_and_fails_as_network() {
    let harness = Harness::new();
    let pid_path = harness.temp.path().join("list-timeout.pid");
    let executor = harness
        .executor_with_env(
            "list-hang",
            vec![(PID_ENV.into(), pid_path.as_os_str().to_owned())],
        )
        .with_list_probe_timeout(Duration::from_millis(1500));

    let started = Instant::now();
    let error = timeout(
        HANG,
        executor.probe_list(LIST_URL, Cookies::None, &CancelToken::new()),
    )
    .await
    .expect("the probe ends at its time limit")
    .unwrap_err();
    assert_eq!(error.category, ErrorCategory::Network);
    assert_eq!(error.message, "the list check timed out after 1500 ms");
    assert!(started.elapsed() < Duration::from_secs(8));
    let pid: u32 = fs::read_to_string(&pid_path).unwrap().parse().unwrap();
    assert_gone(pid).await;
}

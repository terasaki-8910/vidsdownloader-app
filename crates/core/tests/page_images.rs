//! Page-image jobs end to end, against a local HTTP server on a `TcpListener` (never the
//! network): naming and numbering, content types, the size cap, partial failure, the
//! already-saved check, cancel, and job options with and without `page_images`.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio::time::timeout;
use vids_core::args::{Cookies, Embed, Preset, ToolPaths};
use vids_core::comments::AssSettings;
use vids_core::executor::{ExecutorConfig, JobOptions, YtDlpExecutor};
use vids_core::naming::{NameTemplate, Part, TargetOs, Token};
use vids_core::page_images::{BROWSER_USER_AGENT, PageImage, PageImages};
use vids_core::queue::{AlreadySaved, JobFailure, JobId, JobOutcome, Queue, QueueEvent};
use vids_core::store::{LibraryItem, LibraryQuery, NewLibraryItem, Store};
use vids_core::types::{ErrorCategory, JobState};

const HANG: Duration = Duration::from_secs(20);
const PAGE_PATH: &str = "/notes/n1a2b3c4d5";
const PAGE_ID: &str = "n1a2b3c4d5";

/// How the test server answers one path.
#[derive(Clone)]
enum Reply {
    /// A complete response with a Content-Length.
    Full {
        status: u16,
        content_type: Option<&'static str>,
        body: Vec<u8>,
    },
    /// A redirect (`status` 301, 302, ...) to `location`, which may be relative.
    Redirect { status: u16, location: String },
    /// An image sent in chunked encoding, so no length is announced.
    Chunked { body: Vec<u8> },
    /// Announces `length` bytes, then sends a kilobyte every 20 ms until the client leaves.
    Drip { length: u64 },
    /// Announces `length` bytes, sends one kilobyte, then says nothing for `silence` and
    /// closes the connection.
    Stall { length: u64, silence: Duration },
}

#[derive(Debug, Clone)]
struct SeenRequest {
    path: String,
    headers: HashMap<String, String>,
}

struct TestServer {
    base: String,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl TestServer {
    fn start(routes: Vec<(&str, Reply)>) -> Self {
        Self::start_as("127.0.0.1", routes)
    }

    /// A server that listens on the loopback address and is reached as `host` (an address
    /// or `localhost`): a second host name for the same machine, so a test can tell the
    /// site from a CDN.
    fn start_as(host: &str, routes: Vec<(&str, Reply)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{host}:{}", listener.local_addr().unwrap().port());
        let routes: Arc<HashMap<String, Reply>> = Arc::new(
            routes
                .into_iter()
                .map(|(path, reply)| (path.to_owned(), reply))
                .collect(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (routes, recorder) = (Arc::clone(&routes), Arc::clone(&recorder));
                thread::spawn(move || serve(stream, &routes, &recorder));
            }
        });
        Self { base, seen }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn requests(&self) -> Vec<SeenRequest> {
        self.seen.lock().unwrap().clone()
    }
}

fn serve(mut stream: TcpStream, routes: &HashMap<String, Reply>, seen: &Mutex<Vec<SeenRequest>>) {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or("/")
        .to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    seen.lock().unwrap().push(SeenRequest {
        path: path.clone(),
        headers,
    });
    let reply = routes.get(&path).cloned().unwrap_or(Reply::Full {
        status: 404,
        content_type: Some("text/plain"),
        body: b"not found".to_vec(),
    });
    let _ = match reply {
        Reply::Full {
            status,
            content_type,
            body,
        } => {
            let content_type = content_type
                .map(|value| format!("Content-Type: {value}\r\n"))
                .unwrap_or_default();
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\n{content_type}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .and_then(|()| stream.write_all(&body))
        }
        Reply::Redirect { status, location } => write!(
            stream,
            "HTTP/1.1 {status} Moved\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        Reply::Chunked { body } => write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )
        .and_then(|()| {
            for chunk in body.chunks(512) {
                write!(stream, "{:x}\r\n", chunk.len())?;
                stream.write_all(chunk)?;
                stream.write_all(b"\r\n")?;
            }
            stream.write_all(b"0\r\n\r\n")
        }),
        Reply::Drip { length } => write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        )
        .and_then(|()| loop {
            stream.write_all(&[7_u8; 1024])?;
            stream.flush()?;
            thread::sleep(Duration::from_millis(20));
        }),
        Reply::Stall { length, silence } => write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        )
        .and_then(|()| {
            stream.write_all(&[7_u8; 1024])?;
            stream.flush()?;
            thread::sleep(silence);
            Ok(())
        }),
    };
}

fn image(content_type: &'static str, body: &[u8]) -> Reply {
    Reply::Full {
        status: 200,
        content_type: Some(content_type),
        body: body.to_vec(),
    }
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

    fn executor(&self) -> YtDlpExecutor {
        YtDlpExecutor::new(
            ExecutorConfig {
                // Never started: a page-image job needs neither yt-dlp nor gallery-dl.
                tools: ToolPaths {
                    yt_dlp: PathBuf::from("missing-yt-dlp"),
                    ffmpeg_dir: PathBuf::from("missing-ffmpeg"),
                    deno: PathBuf::from("missing-deno"),
                },
                work_dir: self.temp.path().join("work"),
                target_os: if cfg!(windows) {
                    TargetOs::Windows
                } else {
                    TargetOs::MacOs
                },
                gallery_dl: None,
            },
            Arc::clone(&self.store),
        )
    }

    fn dest(&self) -> PathBuf {
        self.temp.path().join("downloads")
    }

    /// Folder `<uploader>`, file `<title> [<id>]`.
    fn options(&self, site_name: Option<&str>, images: Vec<PageImage>) -> JobOptions {
        JobOptions {
            dest_dir: self.dest(),
            template: NameTemplate {
                folders: vec![vec![Part::Field {
                    field: Token::Uploader,
                }]],
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
            site_name: site_name.map(str::to_owned),
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
                title: Some("Autumn garden".to_owned()),
                uploader: Some("Kanade".to_owned()),
                published_at: Some(1_793_318_400),
                id: Some(PAGE_ID.to_owned()),
                images,
            }),
            page_media: None,
            page_media_choices: Vec::new(),
            cookie_domains: Vec::new(),
        }
    }

    /// `options` for an account of the site `domains`, whose cookies come from `cookies`.
    fn options_with_cookies(
        &self,
        images: Vec<PageImage>,
        cookies: Cookies,
        domains: &[&str],
    ) -> JobOptions {
        let mut options = self.options(None, images);
        options.cookies = cookies;
        options.cookie_domains = domains.iter().map(|domain| (*domain).to_owned()).collect();
        options
    }

    /// Writes a Netscape cookie file (the header line plus `lines`, tab-separated) and
    /// returns its path.
    fn cookie_file(&self, lines: &[&str]) -> PathBuf {
        let path = self.temp.path().join("cookies.txt");
        let mut text = "# Netscape HTTP Cookie File\n".to_owned();
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        fs::write(&path, text).unwrap();
        path
    }

    fn library(&self) -> Vec<LibraryItem> {
        self.store
            .lock()
            .unwrap()
            .search_library(&LibraryQuery {
                text: None,
                site: None,
                published_from: None,
                published_to: None,
                include_missing: true,
                limit: 50,
                offset: 0,
            })
            .unwrap()
    }

    fn folder(&self) -> PathBuf {
        self.dest().join("Kanade")
    }

    fn file_names(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.folder()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn partial_files(&self) -> Vec<String> {
        self.file_names()
            .into_iter()
            .filter(|name| name.ends_with(".part"))
            .collect()
    }
}

fn images(server: &TestServer, entries: &[(&str, u32)]) -> Vec<PageImage> {
    entries
        .iter()
        .map(|(path, index)| PageImage {
            url: server.url(path),
            index: *index,
        })
        .collect()
}

fn enqueue(queue: &Queue<YtDlpExecutor>, server: &TestServer, options: &JobOptions) -> JobId {
    queue.enqueue(
        server.url(PAGE_PATH),
        serde_json::to_value(options).unwrap(),
    )
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

fn done_outcome(done: Finished) -> JobOutcome {
    assert_eq!(done.state, JobState::Done, "{:#?}", done.outcome);
    done.outcome.unwrap().unwrap()
}

/// Polls until `condition` holds, for at most `limit`.
async fn eventually(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    condition()
}

#[tokio::test]
async fn images_are_named_by_their_index_with_the_extension_of_their_content_type() {
    let server = TestServer::start(vec![
        ("/img/a", image("image/jpeg", b"jpeg bytes")),
        (
            "/img/b.bin",
            image("image/png; charset=binary", b"png bytes"),
        ),
        ("/img/c", image("image/webp", b"webp bytes")),
    ]);
    let harness = Harness::new();
    let queue = Queue::new(harness.executor(), 1);
    let events = queue.subscribe();
    // Given out of page order: saved in page order, named by their own numbers.
    let options = harness.options(
        None,
        images(&server, &[("/img/c", 5), ("/img/a", 1), ("/img/b.bin", 3)]),
    );
    let id = enqueue(&queue, &server, &options);
    let done = finished(events, id).await;
    let events = done.events.clone();
    let outcome = done_outcome(done);

    let stem = format!("Autumn garden [{PAGE_ID}]");
    assert_eq!(
        harness.file_names(),
        vec![
            format!("{stem} 1.jpg"),
            format!("{stem} 3.png"),
            format!("{stem} 5.webp")
        ]
    );
    assert_eq!(
        fs::read(harness.folder().join(format!("{stem} 3.png"))).unwrap(),
        b"png bytes"
    );
    assert_eq!(
        outcome.output_path.as_deref(),
        Some(
            harness
                .folder()
                .join(format!("{stem} 1.jpg"))
                .to_string_lossy()
                .as_ref()
        )
    );
    assert_eq!(outcome.skipped, None);
    assert_eq!(outcome.already_saved, None);

    let states: Vec<JobState> = events
        .iter()
        .filter_map(|event| match event {
            QueueEvent::StateChanged {
                id: changed, to, ..
            } if *changed == id && !matches!(to, JobState::Queued | JobState::Done) => Some(*to),
            _ => None,
        })
        .collect();
    assert_eq!(states, [JobState::Ready, JobState::Downloading], "no probe");
    let progress: Vec<f64> = events
        .iter()
        .filter_map(|event| match event {
            QueueEvent::Progress {
                id: changed,
                fraction: Some(fraction),
            } if *changed == id => Some(*fraction),
            _ => None,
        })
        .collect();
    assert_eq!(progress, [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);
    let details = events
        .iter()
        .find_map(|event| match event {
            QueueEvent::Details {
                id: changed,
                details,
            } if *changed == id => Some(details.clone()),
            _ => None,
        })
        .expect("details");
    assert_eq!(details.title.as_deref(), Some("Autumn garden"));
    assert_eq!(details.uploader.as_deref(), Some("Kanade"));
    assert_eq!(
        details.site.as_deref(),
        Some("127.0.0.1"),
        "the host without a site"
    );
    assert_eq!(details.published_at, Some(1_793_318_400));
    assert_eq!(details.thumbnail_url, Some(server.url("/img/a")));

    let library = harness.library();
    assert_eq!(library.len(), 3);
    for item in &library {
        assert_eq!(item.site.as_deref(), Some("127.0.0.1"));
        assert_eq!(item.video_id.as_deref(), Some(PAGE_ID));
        assert_eq!(item.url, Some(server.url(PAGE_PATH)));
        assert_eq!(item.title.as_deref(), Some("Autumn garden"));
        assert_eq!(item.uploader.as_deref(), Some("Kanade"));
        assert_eq!(item.published_at, Some(1_793_318_400));
        assert_eq!(item.duration_s, None);
        assert!(Path::new(&item.path).is_file());
    }

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(
            request.headers.get("referer"),
            Some(&server.url(PAGE_PATH)),
            "{}",
            request.path
        );
        assert_eq!(
            request.headers.get("user-agent").map(String::as_str),
            Some(BROWSER_USER_AGENT)
        );
        assert!(
            !request.headers.contains_key("cookie"),
            "no cookies are sent"
        );
    }
    assert!(
        !harness
            .temp
            .path()
            .join("work")
            .join(format!("job-{id}-1"))
            .exists()
    );
}

#[tokio::test]
async fn a_registered_site_name_is_used_for_details_and_library_rows() {
    let server = TestServer::start(vec![("/img/a.jpg", image("image/jpeg", b"a"))]);
    let harness = Harness::new();
    let queue = Queue::new(harness.executor(), 1);
    let events = queue.subscribe();
    let options = harness.options(Some("My blog"), images(&server, &[("/img/a.jpg", 2)]));
    let id = enqueue(&queue, &server, &options);
    done_outcome(finished(events, id).await);

    assert_eq!(harness.library()[0].site.as_deref(), Some("My blog"));
}

#[tokio::test]
async fn images_over_the_size_cap_are_skipped_whether_or_not_their_length_is_announced() {
    let server = TestServer::start(vec![
        ("/img/announced.jpg", image("image/jpeg", &vec![1_u8; 2048])),
        (
            "/img/streamed.png",
            Reply::Chunked {
                body: vec![2_u8; 4096],
            },
        ),
        ("/img/small.gif", image("image/gif", &vec![3_u8; 512])),
    ]);
    let harness = Harness::new();
    let queue = Queue::new(harness.executor().with_max_image_bytes(1024), 1);
    let events = queue.subscribe();
    let options = harness.options(
        None,
        images(
            &server,
            &[
                ("/img/announced.jpg", 1),
                ("/img/streamed.png", 2),
                ("/img/small.gif", 3),
            ],
        ),
    );
    let id = enqueue(&queue, &server, &options);
    let outcome = done_outcome(finished(events, id).await);

    let stem = format!("Autumn garden [{PAGE_ID}]");
    assert_eq!(harness.file_names(), vec![format!("{stem} 3.gif")]);
    assert_eq!(
        outcome.skipped.as_deref(),
        Some("2 of 3 images could not be saved")
    );
    assert!(outcome.output_path.unwrap().ends_with(" 3.gif"));
    assert_eq!(harness.library().len(), 1);
}

#[tokio::test]
async fn a_failed_image_is_counted_and_the_rest_are_saved() {
    let server = TestServer::start(vec![
        (
            "/img/page.jpg",
            Reply::Full {
                status: 200,
                content_type: Some("text/html; charset=utf-8"),
                body: b"<html>login</html>".to_vec(),
            },
        ),
        ("/img/ok.jpg", image("image/jpeg", b"ok")),
    ]);
    let harness = Harness::new();
    let queue = Queue::new(harness.executor(), 1);
    let events = queue.subscribe();
    let options = harness.options(
        None,
        images(
            &server,
            &[
                ("/img/gone.jpg", 1),
                ("/img/page.jpg", 2),
                ("/img/ok.jpg", 3),
            ],
        ),
    );
    let id = enqueue(&queue, &server, &options);
    let done = finished(events, id).await;
    let last_progress = done.events.iter().rev().find_map(|event| match event {
        QueueEvent::Progress {
            fraction: Some(fraction),
            ..
        } => Some(*fraction),
        _ => None,
    });
    let outcome = done_outcome(done);

    assert_eq!(
        outcome.skipped.as_deref(),
        Some("2 of 3 images could not be saved")
    );
    assert!(outcome.output_path.unwrap().ends_with(" 3.jpg"));
    assert_eq!(last_progress, Some(1.0), "failed images count as finished");
    assert_eq!(
        harness.file_names(),
        vec![format!("Autumn garden [{PAGE_ID}] 3.jpg")]
    );
}

#[tokio::test]
async fn the_job_fails_only_when_no_image_was_saved() {
    let server = TestServer::start(Vec::new());
    let harness = Harness::new();
    let queue = Queue::new(harness.executor(), 1);
    let events = queue.subscribe();
    let options = harness.options(
        None,
        images(&server, &[("/img/a.jpg", 1), ("/img/b.jpg", 2)]),
    );
    let id = enqueue(&queue, &server, &options);
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Failed);
    let error = done.outcome.unwrap().unwrap_err();
    assert_eq!(error.category, ErrorCategory::Unavailable);
    assert_eq!(
        error.message,
        "none of the 2 images could be saved: HTTP 404"
    );
    assert!(harness.library().is_empty());
    assert!(harness.partial_files().is_empty());
}

#[tokio::test]
async fn a_page_already_in_the_library_is_skipped_without_any_request() {
    let server = TestServer::start(vec![("/img/a.jpg", image("image/jpeg", b"new"))]);
    let harness = Harness::new();
    fs::create_dir_all(harness.folder()).unwrap();
    let saved = harness
        .folder()
        .join(format!("Autumn garden [{PAGE_ID}] 1.jpg"));
    fs::write(&saved, b"old").unwrap();
    harness
        .store
        .lock()
        .unwrap()
        .add_library_item(
            NewLibraryItem {
                job_id: None,
                site: Some("127.0.0.1".to_owned()),
                video_id: Some(PAGE_ID.to_owned()),
                url: Some(server.url(PAGE_PATH)),
                title: Some("Autumn garden".to_owned()),
                uploader: Some("Kanade".to_owned()),
                published_at: Some(1_793_318_400),
                duration_s: None,
                path: saved.to_string_lossy().into_owned(),
                comments_path: None,
                size_bytes: Some(3),
            },
            1_000,
        )
        .unwrap();
    let queue = Queue::new(harness.executor(), 1);
    let events = queue.subscribe();
    let options = harness.options(None, images(&server, &[("/img/a.jpg", 1)]));
    let id = enqueue(&queue, &server, &options);
    let outcome = done_outcome(finished(events, id).await);

    assert_eq!(outcome.skipped.as_deref(), Some("already saved"));
    assert_eq!(
        outcome.already_saved,
        Some(AlreadySaved {
            saved_at: Some(1_000),
            file_missing: false,
            path: saved.to_string_lossy().into_owned(),
        })
    );
    assert!(server.requests().is_empty(), "nothing was downloaded");

    // Saving again downloads, and keeps a file already at the final name as it is.
    let events = queue.subscribe();
    let mut options = harness.options(None, images(&server, &[("/img/a.jpg", 1)]));
    options.force_save = true;
    let id = enqueue(&queue, &server, &options);
    let outcome = done_outcome(finished(events, id).await);

    assert_eq!(outcome.already_saved, None);
    assert_eq!(
        outcome.output_path,
        Some(saved.to_string_lossy().into_owned())
    );
    assert_eq!(server.requests().len(), 1);
    assert_eq!(fs::read(&saved).unwrap(), b"old");
    assert!(harness.partial_files().is_empty());
    assert_eq!(harness.library().len(), 1, "the path is recorded once");
}

#[tokio::test]
async fn a_page_without_a_library_row_is_found_by_its_id_in_the_folder() {
    let server = TestServer::start(vec![("/img/a.jpg", image("image/jpeg", b"new"))]);
    let harness = Harness::new();
    fs::create_dir_all(harness.folder()).unwrap();
    let saved = harness
        .folder()
        .join(format!("Earlier name [{PAGE_ID}] 4.png"));
    fs::write(&saved, b"old").unwrap();
    let queue = Queue::new(harness.executor(), 1);
    let events = queue.subscribe();
    let options = harness.options(None, images(&server, &[("/img/a.jpg", 1)]));
    let id = enqueue(&queue, &server, &options);
    let outcome = done_outcome(finished(events, id).await);

    assert_eq!(outcome.skipped.as_deref(), Some("already saved"));
    assert_eq!(
        outcome.already_saved.map(|saved| saved.path),
        Some(saved.to_string_lossy().into_owned())
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn cancel_during_a_download_keeps_finished_images_and_leaves_no_partial_file() {
    let server = TestServer::start(vec![
        ("/img/first.jpg", image("image/jpeg", b"first")),
        (
            "/img/slow.jpg",
            Reply::Drip {
                length: 50 * 1024 * 1024,
            },
        ),
    ]);
    let harness = Harness::new();
    let queue = Queue::new(harness.executor(), 1);
    let events = queue.subscribe();
    let options = harness.options(
        None,
        images(&server, &[("/img/first.jpg", 1), ("/img/slow.jpg", 2)]),
    );
    let id = enqueue(&queue, &server, &options);
    assert!(
        eventually(HANG, || !harness.partial_files().is_empty()).await,
        "the slow image starts"
    );
    queue.cancel(id).unwrap();
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Canceled);
    assert!(
        eventually(Duration::from_secs(3), || harness
            .partial_files()
            .is_empty())
        .await,
        "{:?}",
        harness.file_names()
    );
    assert_eq!(
        harness.file_names(),
        vec![format!("Autumn garden [{PAGE_ID}] 1.jpg")]
    );
    assert_eq!(harness.library().len(), 1, "the finished image is recorded");
}

#[tokio::test]
async fn cancel_while_a_read_is_stuck_returns_without_waiting_and_removes_the_partial_file() {
    let silence = Duration::from_secs(5);
    let server = TestServer::start(vec![(
        "/img/stuck.jpg",
        Reply::Stall {
            length: 1024 * 1024,
            silence,
        },
    )]);
    let harness = Harness::new();
    let queue = Queue::new(harness.executor(), 1);
    let events = queue.subscribe();
    let options = harness.options(None, images(&server, &[("/img/stuck.jpg", 1)]));
    let id = enqueue(&queue, &server, &options);
    assert!(
        eventually(HANG, || !harness.partial_files().is_empty()).await,
        "the image starts"
    );
    let canceled_at = Instant::now();
    queue.cancel(id).unwrap();
    let done = finished(events, id).await;

    assert_eq!(done.state, JobState::Canceled);
    assert!(
        canceled_at.elapsed() < silence,
        "the job ended before the stuck read did"
    );
    assert!(
        eventually(silence + Duration::from_secs(3), || harness
            .partial_files()
            .is_empty())
        .await,
        "{:?}",
        harness.file_names()
    );
    assert!(harness.library().is_empty());
}

#[test]
fn job_options_round_trip_with_and_without_page_images() {
    let harness = Harness::new();
    let with_images = harness.options(
        Some("My blog"),
        vec![PageImage {
            url: "https://example.com/a.jpg".to_owned(),
            index: 2,
        }],
    );
    let value = serde_json::to_value(&with_images).unwrap();
    assert_eq!(value["page_images"]["images"][0]["index"], json!(2));
    let read: JobOptions = serde_json::from_value(value).unwrap();
    assert_eq!(read.page_images, with_images.page_images);

    // A row written before page images existed has no such key and still loads.
    let mut old = serde_json::to_value(&with_images).unwrap();
    old.as_object_mut().unwrap().remove("page_images");
    let read: JobOptions = serde_json::from_value(old).unwrap();
    assert_eq!(read.page_images, None);

    // Jobs without page images keep the old shape.
    let mut plain = with_images.clone();
    plain.page_images = None;
    let value: Value = serde_json::to_value(&plain).unwrap();
    assert!(value.get("page_images").is_none());
    assert!(
        value.get("cookie_domains").is_none(),
        "no domains, no key: stored rows and old rows look alike"
    );

    // The domains the cookies belong to are written when given, and a row without them loads.
    let mut with_domains = with_images;
    with_domains.cookie_domains = vec!["example.com".to_owned()];
    let value = serde_json::to_value(&with_domains).unwrap();
    assert_eq!(value["cookie_domains"], json!(["example.com"]));
    let read: JobOptions = serde_json::from_value(value).unwrap();
    assert_eq!(read.cookie_domains, ["example.com"]);
}

// ---- The site account's cookies ----

fn image_at(url: String, index: u32) -> PageImage {
    PageImage { url, index }
}

/// Runs one page-images job to its end on a fresh single-job queue.
async fn run_job(executor: YtDlpExecutor, server: &TestServer, options: &JobOptions) -> Finished {
    let queue = Queue::new(executor, 1);
    let events = queue.subscribe();
    let id = enqueue(&queue, server, options);
    finished(events, id).await
}

/// The `Cookie` header of every request for `path`, in arrival order.
fn cookie_headers(server: &TestServer, path: &str) -> Vec<Option<String>> {
    server
        .requests()
        .into_iter()
        .filter(|request| request.path == path)
        .map(|request| request.headers.get("cookie").cloned())
        .collect()
}

#[tokio::test]
async fn a_cookies_file_is_sent_to_the_sites_host_and_never_to_another_host() {
    let site = TestServer::start(vec![("/img/a.jpg", image("image/jpeg", b"site"))]);
    // The CDN is another host name (localhost, not 127.0.0.1) on the same machine.
    let cdn = TestServer::start_as(
        "localhost",
        vec![("/img/b.jpg", image("image/jpeg", b"cdn"))],
    );
    let harness = Harness::new();
    let cookies = harness.cookie_file(&[
        ".127.0.0.1\tTRUE\t/\tFALSE\t0\tsid\tsecret",
        "127.0.0.1\tFALSE\t/\tFALSE\t0\thostonly\th1",
        ".127.0.0.1\tTRUE\t/img\tFALSE\t0\tdeep\td1",
        ".127.0.0.1\tTRUE\t/other\tFALSE\t0\toffpath\tx",
        ".127.0.0.1\tTRUE\t/\tTRUE\t0\tsecureonly\ts1",
        ".127.0.0.1\tTRUE\t/\tFALSE\t1\texpired\te1",
        "#HttpOnly_.127.0.0.1\tTRUE\t/\tFALSE\t4102444800\thttponly\thv",
        // Another host's cookie in the same file (a cookies.txt holds every site).
        ".localhost\tTRUE\t/\tFALSE\t0\tcdnsid\tcdnsecret",
    ]);
    let options = harness.options_with_cookies(
        vec![
            image_at(site.url("/img/a.jpg"), 1),
            image_at(cdn.url("/img/b.jpg"), 2),
        ],
        Cookies::File(cookies.clone()),
        &["127.0.0.1"],
    );
    let before = fs::read(&cookies).unwrap();
    done_outcome(run_job(harness.executor(), &site, &options).await);

    // Longest path first, then file order; no secure, expired or off-path cookie, and the
    // host-only cookie goes to its own host.
    assert_eq!(
        cookie_headers(&site, "/img/a.jpg"),
        [Some(
            "deep=d1; sid=secret; hostonly=h1; httponly=hv".to_owned()
        )]
    );
    let cdn_requests = cdn.requests();
    assert_eq!(cdn_requests.len(), 1);
    assert!(
        cdn_requests[0]
            .headers
            .get("host")
            .is_some_and(|host| host.starts_with("localhost:")),
        "the image really was asked for on the other host: {:?}",
        cdn_requests[0].headers
    );
    assert!(
        !cdn_requests[0].headers.contains_key("cookie"),
        "no cookies for another host, even ones that match it"
    );
    for request in site.requests().iter().chain(&cdn_requests) {
        assert_eq!(
            request.headers.get("referer"),
            Some(&site.url(PAGE_PATH)),
            "the page stays the referer"
        );
    }
    assert_eq!(
        fs::read(&cookies).unwrap(),
        before,
        "the file is not changed"
    );
    assert_eq!(harness.file_names().len(), 2);
}

#[tokio::test]
async fn a_redirect_off_the_site_drops_the_cookie_header_and_it_stays_dropped() {
    // The chain of image 3 goes site -> CDN -> site again; the last hop is on the site but
    // the request has been elsewhere, so a browser would treat it as cross-site.
    let site_again = TestServer::start(vec![("/img/final.jpg", image("image/jpeg", b"final"))]);
    let cdn = TestServer::start_as(
        "localhost",
        vec![
            ("/img/landed.jpg", image("image/jpeg", b"landed")),
            (
                "/img/back.jpg",
                Reply::Redirect {
                    status: 302,
                    location: site_again.url("/img/final.jpg"),
                },
            ),
        ],
    );
    let site = TestServer::start(vec![
        (
            "/img/off.jpg",
            Reply::Redirect {
                status: 302,
                location: cdn.url("/img/landed.jpg"),
            },
        ),
        (
            "/img/in.jpg",
            Reply::Redirect {
                status: 301,
                location: "/img/in-final.jpg".to_owned(),
            },
        ),
        ("/img/in-final.jpg", image("image/jpeg", b"in")),
        (
            "/img/via-cdn.jpg",
            Reply::Redirect {
                status: 307,
                location: cdn.url("/img/back.jpg"),
            },
        ),
    ]);
    let harness = Harness::new();
    let cookies = harness.cookie_file(&[
        ".127.0.0.1\tTRUE\t/\tFALSE\t0\tsid\tsecret",
        ".localhost\tTRUE\t/\tFALSE\t0\tcdnsid\tcdnsecret",
    ]);
    let options = harness.options_with_cookies(
        vec![
            image_at(site.url("/img/off.jpg"), 1),
            image_at(site.url("/img/in.jpg"), 2),
            image_at(site.url("/img/via-cdn.jpg"), 3),
            image_at(cdn.url("/img/back.jpg"), 4),
        ],
        Cookies::File(cookies),
        &["127.0.0.1"],
    );
    done_outcome(run_job(harness.executor(), &site, &options).await);

    let sent = || [Some("sid=secret".to_owned())];
    // Redirects within the site keep the cookies; the hop that leaves the site has none.
    assert_eq!(cookie_headers(&site, "/img/off.jpg"), sent());
    assert_eq!(cookie_headers(&site, "/img/in.jpg"), sent());
    assert_eq!(cookie_headers(&site, "/img/in-final.jpg"), sent());
    assert_eq!(cookie_headers(&site, "/img/via-cdn.jpg"), sent());
    assert_eq!(cookie_headers(&cdn, "/img/landed.jpg"), [None]);
    assert_eq!(cookie_headers(&cdn, "/img/back.jpg"), [None, None]);
    assert_eq!(
        cookie_headers(&site_again, "/img/final.jpg"),
        [None, None],
        "back on the site after leaving it: still none"
    );
    assert_eq!(harness.file_names().len(), 4);
}

#[tokio::test]
async fn too_many_redirects_fail_the_image() {
    let site = TestServer::start(vec![(
        "/img/loop.jpg",
        Reply::Redirect {
            status: 302,
            location: "/img/loop.jpg".to_owned(),
        },
    )]);
    let harness = Harness::new();
    let options = harness.options(None, images(&site, &[("/img/loop.jpg", 1)]));
    let done = run_job(harness.executor(), &site, &options).await;

    assert_eq!(done.state, JobState::Failed);
    assert_eq!(
        done.outcome.unwrap().unwrap_err().message,
        "none of the 1 images could be saved: too many redirects"
    );
    assert_eq!(
        site.requests().len(),
        6,
        "the first request and five redirects"
    );
}

#[tokio::test]
async fn without_a_cookie_source_or_without_domains_no_cookie_is_sent() {
    let site = TestServer::start(vec![
        ("/img/a.jpg", image("image/jpeg", b"a")),
        ("/img/b.jpg", image("image/jpeg", b"b")),
    ]);
    let line = ".127.0.0.1\tTRUE\t/\tFALSE\t0\tsid\tsecret";

    // No cookie source, though the job names the site's domains.
    let harness = Harness::new();
    let options = harness.options_with_cookies(
        images(&site, &[("/img/a.jpg", 1)]),
        Cookies::None,
        &["127.0.0.1"],
    );
    done_outcome(run_job(harness.executor(), &site, &options).await);

    // A cookie file, but no domain to send it to.
    let harness = Harness::new();
    let cookies = harness.cookie_file(&[line]);
    let options = harness.options_with_cookies(
        images(&site, &[("/img/b.jpg", 1)]),
        Cookies::File(cookies),
        &[],
    );
    done_outcome(run_job(harness.executor(), &site, &options).await);

    assert_eq!(cookie_headers(&site, "/img/a.jpg"), [None]);
    assert_eq!(cookie_headers(&site, "/img/b.jpg"), [None]);
}

#[tokio::test]
async fn a_cookies_file_that_cannot_be_read_fails_the_job_before_any_request() {
    let site = TestServer::start(vec![("/img/a.jpg", image("image/jpeg", b"a"))]);
    let harness = Harness::new();
    let options = harness.options_with_cookies(
        images(&site, &[("/img/a.jpg", 1)]),
        Cookies::File(harness.temp.path().join("gone.txt")),
        &["127.0.0.1"],
    );
    let done = run_job(harness.executor(), &site, &options).await;

    assert_eq!(done.state, JobState::Failed);
    let error = done.outcome.unwrap().unwrap_err();
    assert!(
        error.message.starts_with("could not read the cookies file"),
        "{}",
        error.message
    );
    assert!(site.requests().is_empty());
    assert!(harness.file_names().is_empty());
}

/// A Firefox profile folder with these cookies (host, name, value), all session-length
/// valid, and the `profiles.ini` listing it as `default-release`; returns the ini path.
fn fake_firefox(harness: &Harness, cookies: &[(&str, &str, &str)]) -> PathBuf {
    let root = harness.temp.path().join("firefox");
    let profile = root.join("Profiles").join("abc.default-release");
    fs::create_dir_all(&profile).unwrap();
    let ini = root.join("profiles.ini");
    fs::write(
        &ini,
        "[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/abc.default-release\nDefault=1\n",
    )
    .unwrap();
    let connection = rusqlite::Connection::open(profile.join("cookies.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE moz_cookies (
                id INTEGER PRIMARY KEY,
                originAttributes TEXT NOT NULL DEFAULT '',
                name TEXT, value TEXT, host TEXT, path TEXT,
                expiry INTEGER, lastAccessed INTEGER, creationTime INTEGER,
                isSecure INTEGER, isHttpOnly INTEGER,
                sameSite INTEGER DEFAULT 0
            )",
        )
        .unwrap();
    for (host, name, value) in cookies {
        connection
            .execute(
                "INSERT INTO moz_cookies \
                 (name, value, host, path, expiry, isSecure, isHttpOnly, sameSite) \
                 VALUES (?1, ?2, ?3, '/', 4102444800, 0, 1, 1)",
                rusqlite::params![name, value, host],
            )
            .unwrap();
    }
    drop(connection);
    ini
}

#[tokio::test]
async fn a_firefox_account_is_read_once_and_sent_to_the_sites_host_only() {
    let site = TestServer::start(vec![
        ("/img/a.jpg", image("image/jpeg", b"a")),
        ("/img/b.jpg", image("image/jpeg", b"b")),
    ]);
    let cdn = TestServer::start_as("localhost", vec![("/img/c.jpg", image("image/jpeg", b"c"))]);
    let harness = Harness::new();
    let ini = fake_firefox(
        &harness,
        &[
            (".127.0.0.1", "ffsid", "ffval"),
            (".localhost", "ffcdn", "cdnval"),
            (".other.test", "ffother", "otherval"),
        ],
    );
    let options = harness.options_with_cookies(
        vec![
            image_at(site.url("/img/a.jpg"), 1),
            image_at(site.url("/img/b.jpg"), 2),
            image_at(cdn.url("/img/c.jpg"), 3),
        ],
        Cookies::Firefox {
            profile: "default-release".to_owned(),
        },
        &["127.0.0.1"],
    );
    let executor = harness.executor().with_firefox_profiles_ini(ini);
    done_outcome(run_job(executor, &site, &options).await);

    assert_eq!(
        cookie_headers(&site, "/img/a.jpg"),
        [Some("ffsid=ffval".to_owned())]
    );
    assert_eq!(
        cookie_headers(&site, "/img/b.jpg"),
        [Some("ffsid=ffval".to_owned())]
    );
    assert_eq!(cookie_headers(&cdn, "/img/c.jpg"), [None]);
    // The temporary copy of the cookie database went with the attempt folder.
    let leftovers: Vec<_> = fs::read_dir(harness.temp.path().join("work"))
        .map(|entries| entries.map(|entry| entry.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn an_unknown_firefox_profile_fails_the_job_before_any_request() {
    let site = TestServer::start(vec![("/img/a.jpg", image("image/jpeg", b"a"))]);
    let harness = Harness::new();
    let ini = fake_firefox(&harness, &[(".127.0.0.1", "ffsid", "ffval")]);
    let options = harness.options_with_cookies(
        images(&site, &[("/img/a.jpg", 1)]),
        Cookies::Firefox {
            profile: "nosuch".to_owned(),
        },
        &["127.0.0.1"],
    );
    let executor = harness.executor().with_firefox_profiles_ini(ini);
    let done = run_job(executor, &site, &options).await;

    assert_eq!(done.state, JobState::Failed);
    assert_eq!(
        done.outcome.unwrap().unwrap_err().message,
        "Firefox profile nosuch was not found"
    );
    assert!(site.requests().is_empty());
}

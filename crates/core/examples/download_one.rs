//! Downloads one URL end to end with real tools, to check the executor by hand:
//!
//! ```text
//! cargo run -p vids-core --example download_one -- <tools dir> <dest dir> <url> [720|best|audio]
//! ```
//!
//! The tools directory holds yt-dlp, deno, ffmpeg and ffprobe (`.exe` on Windows). Uses
//! the default name template (uploader folder, then "date title [id]"), Japan time for
//! dates, thumbnails/metadata/chapters embedded, and comments written as .ass where the
//! site has them. Prints state changes, progress in 10 % steps, the outcome and the
//! library row.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use vids_core::args::{Cookies, Embed, Preset, ToolPaths};
use vids_core::comments::AssSettings;
use vids_core::executor::{ExecutorConfig, JobOptions, YtDlpExecutor};
use vids_core::naming::{NameTemplate, OptionalPart, Part, TargetOs, Token};
use vids_core::queue::{Queue, QueueEvent};
use vids_core::store::{LibraryQuery, Store};

#[tokio::main]
async fn main() {
    let mut cli = std::env::args().skip(1);
    let usage = "usage: download_one <tools dir> <dest dir> <url> [720|best|audio]";
    let tools_dir = PathBuf::from(cli.next().expect(usage));
    let dest_dir = PathBuf::from(cli.next().expect(usage));
    let url = cli.next().expect(usage);
    let preset = match cli.next().as_deref() {
        None | Some("720") => Preset::UpTo720,
        Some("best") => Preset::Best,
        Some("audio") => Preset::AudioM4a,
        Some(other) => panic!("unknown preset {other}; {usage}"),
    };

    let exe = |name: &str| {
        tools_dir.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        })
    };
    let config = ExecutorConfig {
        tools: ToolPaths {
            yt_dlp: exe("yt-dlp"),
            ffmpeg_dir: tools_dir.clone(),
            deno: exe("deno"),
        },
        work_dir: dest_dir.join(".work"),
        target_os: if cfg!(windows) {
            TargetOs::Windows
        } else {
            TargetOs::MacOs
        },
        gallery_dl: Some(exe("gallery-dl")),
    };
    let store = Arc::new(Mutex::new(Store::open_in_memory().expect("open store")));
    let queue = Queue::new(YtDlpExecutor::new(config, Arc::clone(&store)), 1);
    let mut events = queue.subscribe();

    let template = NameTemplate {
        folders: vec![vec![Part::Field {
            field: Token::Uploader,
        }]],
        file: vec![
            Part::Optional {
                optional: OptionalPart {
                    before: String::new(),
                    field: Token::Date,
                    after: " ".to_owned(),
                },
            },
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
    };
    let options = JobOptions {
        dest_dir,
        template,
        tz_offset_seconds: 9 * 3600,
        site_name: None,
        preset,
        avoid_av1: false,
        embed: Embed {
            subtitles: false,
            subtitle_langs: Vec::new(),
            keep_subtitle_files: false,
            thumbnail: true,
            metadata: true,
            chapters: true,
        },
        comments: true,
        ass: AssSettings::default(),
        cookies: Cookies::None,
        filter: None,
        playlist_item: None,
        force_save: false,
        page_images: None,
        page_media: None,
        page_media_choices: Vec::new(),
        cookie_domains: Vec::new(),
    };
    let id = queue.enqueue(
        url,
        serde_json::to_value(&options).expect("serialize options"),
    );

    let mut next_step = 0.0;
    loop {
        match events.recv().await.expect("queue events") {
            QueueEvent::StateChanged { id: event, to, .. } if event == id => {
                println!("state: {to:?}");
            }
            QueueEvent::Progress {
                id: event,
                fraction: Some(fraction),
            } if event == id && fraction >= next_step => {
                println!("progress: {:.0} %", fraction * 100.0);
                next_step = (fraction * 10.0).floor() / 10.0 + 0.1;
            }
            QueueEvent::Finished {
                id: event,
                state,
                outcome_or_error,
            } if event == id => {
                println!("finished: {state:?} {outcome_or_error:?}");
                break;
            }
            _ => {}
        }
    }

    let items = store
        .lock()
        .expect("store lock")
        .search_library(&LibraryQuery {
            text: None,
            site: None,
            published_from: None,
            published_to: None,
            include_missing: true,
            limit: 10,
            offset: 0,
        })
        .expect("library search");
    for item in items {
        println!("library: {item:?}");
    }
}

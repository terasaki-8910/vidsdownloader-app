use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager, UriSchemeResponder, Wry};
use vids_core::store::Store;
use vids_core::types::JobState;

use crate::is_main_webview;
use crate::state::{AppState, lock_recover};

const MAX_CHUNK_LEN: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

pub fn handle_request(
    app: &AppHandle<Wry>,
    webview_label: &str,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    if !is_main_webview(webview_label) {
        responder.respond(empty_response(StatusCode::FORBIDDEN));
        return;
    }

    let Some(id) = parse_job_path(request.uri().path()) else {
        responder.respond(empty_response(StatusCode::NOT_FOUND));
        return;
    };
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let store = Arc::clone(&app.state::<AppState>().store);

    tauri::async_runtime::spawn(async move {
        let response = tauri::async_runtime::spawn_blocking(move || read_chunk(store, id, range))
            .await
            .unwrap_or_else(|_| empty_response(StatusCode::INTERNAL_SERVER_ERROR));
        responder.respond(response);
    });
}

pub fn job_file_path(store: &Mutex<Store>, id: u64) -> Option<PathBuf> {
    let id = i64::try_from(id).ok()?;
    let job = lock_recover(store).get_job(id).ok()?;
    if job.state != JobState::Done {
        return None;
    }
    let path = PathBuf::from(job.output_path?);
    path.is_file().then_some(path)
}

fn read_chunk(
    store: Arc<Mutex<Store>>,
    id: u64,
    range_header: Option<String>,
) -> Response<Vec<u8>> {
    let Some(path) = job_file_path(&store, id) else {
        return empty_response(StatusCode::NOT_FOUND);
    };
    let Ok(mut file) = File::open(&path) else {
        return empty_response(StatusCode::NOT_FOUND);
    };
    let Ok(metadata) = file.metadata() else {
        return empty_response(StatusCode::NOT_FOUND);
    };
    let len = metadata.len();
    let range = match parse_range(range_header.as_deref(), len) {
        Ok(range) => range,
        Err(()) => return range_not_satisfiable(len),
    };
    let read_len = range.end - range.start + 1;
    let Ok(buffer_len) = usize::try_from(read_len) else {
        return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let mut body = vec![0; buffer_len];
    if file.seek(SeekFrom::Start(range.start)).is_err() || file.read_exact(&mut body).is_err() {
        return empty_response(StatusCode::NOT_FOUND);
    }

    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        // The same URL can later point at a different file (saved again, replaced).
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONTENT_TYPE, content_type(&path))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, read_len.to_string())
        .header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{len}", range.start, range.end),
        )
        .body(body)
        .expect("valid media response")
}

fn empty_response(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .body(Vec::new())
        .expect("valid empty response")
}

fn range_not_satisfiable(len: u64) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(header::CONTENT_RANGE, format!("bytes */{len}"))
        .body(Vec::new())
        .expect("valid range response")
}

fn parse_job_path(path: &str) -> Option<u64> {
    path.strip_prefix("/job/")
        .or_else(|| path.strip_prefix("/job%2F"))
        .or_else(|| path.strip_prefix("/job%2f"))?
        .parse()
        .ok()
}

fn parse_range(value: Option<&str>, len: u64) -> Result<ByteRange, ()> {
    if len == 0 {
        return Err(());
    }
    let first = value
        .unwrap_or("bytes=0-")
        .split(',')
        .next()
        .ok_or(())?
        .trim();
    let spec = first.strip_prefix("bytes=").ok_or(())?;
    let (start_text, end_text) = spec.split_once('-').ok_or(())?;
    let (start, requested_end) = if start_text.is_empty() {
        let suffix = end_text.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (len.saturating_sub(suffix), len - 1)
    } else {
        let start = start_text.parse::<u64>().map_err(|_| ())?;
        let end = if end_text.is_empty() {
            len - 1
        } else {
            end_text.parse::<u64>().map_err(|_| ())?.min(len - 1)
        };
        (start, end)
    };
    if start >= len || requested_end < start {
        return Err(());
    }
    Ok(ByteRange {
        start,
        end: requested_end.min(start.saturating_add(MAX_CHUNK_LEN - 1)),
    })
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mov") => "video/quicktime",
        Some("m4a") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("opus" | "ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("aac") => "audio/aac",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteRange, MAX_CHUNK_LEN, content_type, parse_job_path, parse_range};
    use std::path::Path;

    #[test]
    fn parses_job_paths() {
        assert_eq!(parse_job_path("/job/7"), Some(7));
        assert_eq!(parse_job_path("/job%2F7"), Some(7));
        assert_eq!(parse_job_path("/job%2f7"), Some(7));
        assert_eq!(parse_job_path("/job/nope"), None);
        assert_eq!(parse_job_path("/other/7"), None);
        assert_eq!(parse_job_path("/job/7/more"), None);
    }

    #[test]
    fn parses_range_forms_and_first_range() {
        assert_eq!(
            parse_range(Some("bytes=2-5"), 10),
            Ok(ByteRange { start: 2, end: 5 })
        );
        assert_eq!(
            parse_range(Some("bytes=7-"), 10),
            Ok(ByteRange { start: 7, end: 9 })
        );
        assert_eq!(
            parse_range(Some("bytes=-3"), 10),
            Ok(ByteRange { start: 7, end: 9 })
        );
        assert_eq!(
            parse_range(Some("bytes=1-2, 7-8"), 10),
            Ok(ByteRange { start: 1, end: 2 })
        );
        assert_eq!(parse_range(None, 10), Ok(ByteRange { start: 0, end: 9 }));
    }

    #[test]
    fn rejects_invalid_and_unsatisfiable_ranges() {
        for value in [
            "items=0-1",
            "bytes=x-1",
            "bytes=4-2",
            "bytes=10-",
            "bytes=-0",
        ] {
            assert_eq!(parse_range(Some(value), 10), Err(()), "{value}");
        }
        assert_eq!(parse_range(None, 0), Err(()));
    }

    #[test]
    fn clamps_suffix_and_chunk_length() {
        assert_eq!(
            parse_range(Some("bytes=-20"), 10),
            Ok(ByteRange { start: 0, end: 9 })
        );
        assert_eq!(
            parse_range(Some("bytes=2-"), MAX_CHUNK_LEN + 100),
            Ok(ByteRange {
                start: 2,
                end: MAX_CHUNK_LEN + 1
            })
        );
    }

    #[test]
    fn maps_content_types_case_insensitively() {
        let cases = [
            ("video.MP4", "video/mp4"),
            ("video.m4v", "video/mp4"),
            ("video.webm", "video/webm"),
            ("video.mkv", "video/x-matroska"),
            ("video.mov", "video/quicktime"),
            ("audio.m4a", "audio/mp4"),
            ("audio.mp3", "audio/mpeg"),
            ("audio.opus", "audio/ogg"),
            ("audio.ogg", "audio/ogg"),
            ("audio.wav", "audio/wav"),
            ("audio.flac", "audio/flac"),
            ("audio.aac", "audio/aac"),
            ("file.bin", "application/octet-stream"),
        ];
        for (path, expected) in cases {
            assert_eq!(content_type(Path::new(path)), expected, "{path}");
        }
    }
}

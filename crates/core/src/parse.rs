//! Parsing of yt-dlp output lines: progress, final path, errors and their categories.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::types::{
    DOWNLOAD_MARKER, DownloadProgress, DownloadStatus, ErrorCategory, OutputLine, PATH_MARKER,
    POSTPROCESS_MARKER, ParsedLine, PostprocessProgress, PostprocessStatus,
};

/// Parse one line from either yt-dlp output stream without panicking.
pub fn parse_line(line: &OutputLine) -> ParsedLine {
    if let Some(payload) = line.text.strip_prefix(DOWNLOAD_MARKER) {
        return parse_download(payload).unwrap_or_else(|| ParsedLine::Other(line.text.clone()));
    }
    if let Some(payload) = line.text.strip_prefix(POSTPROCESS_MARKER) {
        return parse_postprocess(payload).unwrap_or_else(|| ParsedLine::Other(line.text.clone()));
    }
    if let Some(payload) = line.text.strip_prefix(PATH_MARKER) {
        return serde_json::from_str::<String>(payload)
            .map(ParsedLine::FinalPath)
            .unwrap_or_else(|_| ParsedLine::Other(line.text.clone()));
    }
    if let Some(message) = line.text.strip_prefix("ERROR: ") {
        return ParsedLine::Error {
            category: classify_error(message),
            message: message.to_owned(),
        };
    }
    if let Some(message) = line.text.strip_prefix("WARNING: ") {
        return ParsedLine::Warning(message.to_owned());
    }
    ParsedLine::Other(line.text.clone())
}

fn parse_download(payload: &str) -> Option<ParsedLine> {
    let object = serde_json::from_str::<Value>(payload).ok()?;
    let object = object.as_object()?;
    let status = match object.get("status")?.as_str()? {
        "downloading" => DownloadStatus::Downloading,
        "finished" => DownloadStatus::Finished,
        "error" => DownloadStatus::Error,
        _ => return None,
    };
    Some(ParsedLine::Download(DownloadProgress {
        status,
        downloaded_bytes: optional_u64(object, "downloaded_bytes")?.unwrap_or(0),
        total_bytes: optional_u64(object, "total_bytes")?,
        total_bytes_estimate: optional_u64(object, "total_bytes_estimate")?,
        fragment_index: optional_u64(object, "fragment_index")?,
        fragment_count: optional_u64(object, "fragment_count")?,
        speed_bytes_per_s: optional_f64(object, "speed")?,
        eta_s: optional_f64(object, "eta")?,
        filename: object.get("filename")?.as_str()?.to_owned(),
    }))
}

fn parse_postprocess(payload: &str) -> Option<ParsedLine> {
    let object = serde_json::from_str::<Value>(payload).ok()?;
    let object = object.as_object()?;
    let status = match object.get("status")?.as_str()? {
        "started" => PostprocessStatus::Started,
        "processing" => PostprocessStatus::Processing,
        "finished" => PostprocessStatus::Finished,
        _ => return None,
    };
    Some(ParsedLine::Postprocess(PostprocessProgress {
        status,
        postprocessor: object.get("postprocessor")?.as_str()?.to_owned(),
    }))
}

fn optional_u64(object: &Map<String, Value>, key: &str) -> Option<Option<u64>> {
    let Some(value) = object.get(key) else {
        return Some(None);
    };
    if value.is_null() {
        return Some(None);
    }
    if let Some(value) = value.as_u64() {
        return Some(Some(value));
    }
    let value = value.as_f64()?;
    (value.is_finite() && value >= 0.0 && value <= u64::MAX as f64).then_some(Some(value as u64))
}

fn optional_f64(object: &Map<String, Value>, key: &str) -> Option<Option<f64>> {
    let Some(value) = object.get(key) else {
        return Some(None);
    };
    if value.is_null() {
        return Some(None);
    }
    let value = value.as_f64()?;
    value.is_finite().then_some(Some(value))
}

/// Classify a yt-dlp error message by stable, ordered substring rules.
pub fn classify_error(message: &str) -> ErrorCategory {
    let message = message.to_lowercase();
    let rules: &[(ErrorCategory, &[&str])] = &[
        (ErrorCategory::Drm, &["drm protected", "drm-protected"]),
        (
            ErrorCategory::GeoBlocked,
            &[
                "not available in your country",
                "geo restrict",
                "geo-restrict",
                "not made this video available in your country",
            ],
        ),
        (
            ErrorCategory::LoginRequired,
            &[
                "members only",
                "join this channel",
                "sign in to confirm",
                "age-restricted",
                "age restricted",
                "login required",
                "log in to",
                "--cookies",
                "account credentials",
                "premium members",
            ],
        ),
        (
            ErrorCategory::RateLimited,
            &[
                "http error 429",
                "too many requests",
                "rate-limit",
                "rate limit",
            ],
        ),
        (
            ErrorCategory::NoVideo,
            &[
                "no video could be found",
                "no video formats found",
                "there's no video in this",
            ],
        ),
        (ErrorCategory::UnsupportedUrl, &["unsupported url"]),
        (
            ErrorCategory::Unavailable,
            &[
                "video unavailable",
                "this video is unavailable",
                "private video",
                "has been removed",
                "has been deleted",
                "is not available",
                "does not exist",
            ],
        ),
        (
            ErrorCategory::Network,
            &[
                "unable to connect",
                "timed out",
                "connection reset",
                "connection refused",
                "name resolution",
                "getaddrinfo failed",
                "network is unreachable",
                "errno 11001",
            ],
        ),
        (
            ErrorCategory::Postprocess,
            &["postprocessing:", "conversion failed", "ffmpeg"],
        ),
        (
            ErrorCategory::ExtractorBroken,
            &[
                "unable to extract",
                "unable to download json metadata",
                "failed to parse json",
                "nsig extraction failed",
                "signature extraction failed",
                "please report this issue",
            ],
        ),
    ];

    rules
        .iter()
        .find_map(|(category, needles)| {
            needles
                .iter()
                .any(|needle| message.contains(needle))
                .then_some(*category)
        })
        .unwrap_or(ErrorCategory::Unknown)
}

/// One expected video or audio stream and its probed sizing information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedStream {
    /// yt-dlp format identifier, such as `395` or `399-sr`.
    pub format_id: String,
    /// Exact probed byte size from `filesize`, when supplied by the extractor.
    pub size: Option<u64>,
    /// Expected bytes used only to weigh this stream against the others.
    ///
    /// This is `filesize`, then `filesize_approx`, then bitrate times duration.
    pub approx_size: Option<u64>,
}

/// Aggregate progress for all streams in one download job.
#[derive(Debug, Clone, PartialEq)]
pub struct JobProgress {
    /// Monotonic completed fraction when every stream has a comparable basis.
    pub fraction: Option<f64>,
    /// Bytes completed across all streams.
    pub downloaded_bytes: u64,
    /// Total bytes across all streams when all totals are known.
    pub total_bytes: Option<u64>,
    /// Speed of the stream currently downloading.
    pub speed_bytes_per_s: Option<f64>,
    /// ETA of the stream currently downloading.
    pub eta_s: Option<f64>,
}

#[derive(Debug, Default)]
struct StreamProgress {
    expected_size: Option<u64>,
    approx_size: Option<u64>,
    observed_total: Option<u64>,
    observed_estimate: Option<u64>,
    fragment_index: Option<u64>,
    fragment_count: Option<u64>,
    downloaded: u64,
    finished: bool,
    seen_update: bool,
}

impl StreamProgress {
    fn fraction(&self) -> Option<f64> {
        let fraction = if self.finished {
            1.0
        } else if let Some(total) = self.observed_total.filter(|total| *total > 0) {
            self.downloaded as f64 / total as f64
        } else if let (Some(index), Some(count)) = (
            self.fragment_index,
            self.fragment_count.filter(|count| *count > 0),
        ) {
            index as f64 / count as f64
        } else if let Some(size) = self.expected_size.filter(|size| *size > 0) {
            self.downloaded as f64 / size as f64
        } else if let Some(estimate) = self.observed_estimate.filter(|estimate| *estimate > 0) {
            self.downloaded as f64 / estimate as f64
        } else if !self.seen_update {
            0.0
        } else {
            return None;
        };
        Some(fraction.clamp(0.0, 1.0))
    }

    fn weight(&self) -> Option<u64> {
        self.approx_size.or(self.observed_total)
    }

    fn exact_total(&self) -> Option<u64> {
        self.observed_total.or(self.expected_size)
    }
}

/// Combines sequential yt-dlp stream updates into monotonic job progress.
#[derive(Debug)]
pub struct ProgressAggregator {
    streams: HashMap<String, StreamProgress>,
    last_fraction: Option<f64>,
}

impl ProgressAggregator {
    /// Start aggregating a job with the formats selected during probing.
    pub fn new(expected: Vec<ExpectedStream>) -> Self {
        let streams = expected
            .into_iter()
            .map(|stream| {
                (
                    stream.format_id,
                    StreamProgress {
                        expected_size: stream.size,
                        approx_size: stream.approx_size,
                        ..StreamProgress::default()
                    },
                )
            })
            .collect();
        Self {
            streams,
            last_fraction: None,
        }
    }

    /// Incorporate one stream update and return current job progress.
    pub fn update(&mut self, progress: &DownloadProgress) -> JobProgress {
        let stream_id = format_id(&progress.filename)
            .or_else(|| {
                (self.streams.len() == 1)
                    .then(|| self.streams.keys().next().cloned())
                    .flatten()
            })
            .unwrap_or_else(|| "__single__".to_owned());
        let stream = self.streams.entry(stream_id).or_default();
        if let Some(total) = progress.total_bytes {
            stream.observed_total = Some(total);
        }
        if let Some(estimate) = progress.total_bytes_estimate {
            stream.observed_estimate = Some(estimate);
        }
        if let Some(index) = progress.fragment_index {
            stream.fragment_index = Some(index);
        }
        if let Some(count) = progress.fragment_count {
            stream.fragment_count = Some(count);
        }
        stream.downloaded = progress.downloaded_bytes;
        stream.seen_update = true;
        if progress.status == DownloadStatus::Finished {
            stream.finished = true;
            // A finished stream's size is what was downloaded, which also gives it a
            // weight when neither the probe nor yt-dlp supplied a total.
            if stream.observed_total.is_none() {
                stream.observed_total = Some(stream.downloaded);
            }
        }

        let downloaded_bytes = self
            .streams
            .values()
            .fold(0_u64, |sum, stream| sum.saturating_add(stream.downloaded));
        let totals: Option<Vec<u64>> = self
            .streams
            .values()
            .map(StreamProgress::exact_total)
            .collect();
        let total_bytes = totals.as_ref().map(|totals| {
            totals
                .iter()
                .fold(0_u64, |sum, total| sum.saturating_add(*total))
        });
        let all_finished =
            !self.streams.is_empty() && self.streams.values().all(|stream| stream.finished);
        let calculated = if all_finished {
            Some(1.0)
        } else if self.streams.len() == 1 {
            self.streams
                .values()
                .next()
                .and_then(StreamProgress::fraction)
        } else {
            let weighted: Option<Vec<(u64, f64)>> = self
                .streams
                .values()
                .map(|stream| Some((stream.weight()?, stream.fraction()?)))
                .collect();
            weighted.and_then(|weighted| {
                let weight_sum = weighted
                    .iter()
                    .fold(0_u64, |sum, (weight, _)| sum.saturating_add(*weight));
                (weight_sum > 0).then(|| {
                    weighted
                        .iter()
                        .map(|(weight, fraction)| *weight as f64 * fraction)
                        .sum::<f64>()
                        / weight_sum as f64
                })
            })
        };
        let fraction = match (self.last_fraction, calculated) {
            (Some(last), Some(calculated)) => Some(last.max(calculated)),
            (Some(last), None) => Some(last),
            (None, calculated) => calculated,
        };
        self.last_fraction = fraction;
        let downloading = progress.status == DownloadStatus::Downloading;

        JobProgress {
            fraction,
            downloaded_bytes,
            total_bytes,
            speed_bytes_per_s: downloading.then_some(progress.speed_bytes_per_s).flatten(),
            eta_s: downloading.then_some(progress.eta_s).flatten(),
        }
    }
}

/// The format id of a stream file named `<name>.f<format_id>.<ext>` by yt-dlp.
///
/// Only the part right before the extension counts, and it must look like a format id,
/// so titles such as "Mr.freeze" or "ver.final 2" are not mistaken for one.
fn format_id(filename: &str) -> Option<String> {
    let (stem, _extension) = filename.rsplit_once('.')?;
    let (_, id) = stem.rsplit_once(".f")?;
    let looks_like_id = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    looks_like_id.then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_id_reads_only_the_part_before_the_extension() {
        let cases = [
            ("Big Buck Bunny [YE7VzlLtp-4].f395.mp4", Some("395")),
            ("x.f399-sr.mp4", Some("399-sr")),
            (
                "sm9.fvideo-h264-360p-lowest-0.mp4",
                Some("video-h264-360p-lowest-0"),
            ),
            (r"C:\tmp\a.b\Title [id].f258.m4a", Some("258")),
            ("Mr.freeze [abc].mp4", None),
            ("Title ver.final 2 [abc].mp4", None),
            ("Title v.f2 [abc].mp4", None),
            ("Title [abc].mp4", None),
            ("noextension", None),
        ];
        for (filename, expected) in cases {
            assert_eq!(format_id(filename).as_deref(), expected, "{filename}");
        }
    }

    #[test]
    fn single_stream_title_with_dot_f_still_reaches_one() {
        let mut aggregator = ProgressAggregator::new(vec![ExpectedStream {
            format_id: "18".to_owned(),
            size: Some(1000),
            approx_size: Some(1000),
        }]);
        let line = |status, downloaded| DownloadProgress {
            status,
            downloaded_bytes: downloaded,
            total_bytes: Some(1000),
            total_bytes_estimate: None,
            fragment_index: None,
            fragment_count: None,
            speed_bytes_per_s: None,
            eta_s: None,
            filename: "Mr.freeze [abc].mp4".to_owned(),
        };
        let half = aggregator.update(&line(DownloadStatus::Downloading, 500));
        assert_eq!(half.fraction, Some(0.5));
        let done = aggregator.update(&line(DownloadStatus::Finished, 1000));
        assert_eq!(done.fraction, Some(1.0));
    }
    use crate::types::OutputStream;

    #[test]
    fn malformed_markers_and_unknown_statuses_remain_other() {
        for text in [
            "[vdl-dl]{",
            "[vdl-dl]{\"status\":\"paused\",\"filename\":\"x.f1.mp4\"}",
            "[vdl-pp]{\"status\":\"waiting\",\"postprocessor\":\"Merger\"}",
            "[vdl-path]not-json",
        ] {
            let line = OutputLine {
                stream: OutputStream::Stdout,
                text: text.to_owned(),
            };
            assert_eq!(parse_line(&line), ParsedLine::Other(text.to_owned()));
        }
    }

    #[test]
    fn numeric_nulls_floats_and_defaults_are_supported() {
        let line = OutputLine {
            stream: OutputStream::Stderr,
            text: concat!(
                "[vdl-dl]",
                r#"{"status":"downloading","downloaded_bytes":null,"total_bytes":12.9,"total_bytes_estimate":null,"speed":2,"eta":1.5,"filename":"x.f1.mp4"}"#
            )
            .to_owned(),
        };
        let ParsedLine::Download(progress) = parse_line(&line) else {
            panic!("expected download progress");
        };
        assert_eq!(progress.downloaded_bytes, 0);
        assert_eq!(progress.total_bytes, Some(12));
        assert_eq!(progress.fragment_index, None);
        assert_eq!(progress.fragment_count, None);
        assert_eq!(progress.speed_bytes_per_s, Some(2.0));
        assert_eq!(progress.eta_s, Some(1.5));
    }

    #[test]
    fn fragment_counters_are_parsed_when_present() {
        let line = OutputLine {
            stream: OutputStream::Stdout,
            text: concat!(
                "[vdl-dl]",
                r#"{"status":"downloading","downloaded_bytes":10,"fragment_index":27,"fragment_count":54,"filename":"x.f1.mp4"}"#
            )
            .to_owned(),
        };
        let ParsedLine::Download(progress) = parse_line(&line) else {
            panic!("expected download progress");
        };
        assert_eq!(progress.fragment_index, Some(27));
        assert_eq!(progress.fragment_count, Some(54));
    }

    #[test]
    fn single_stream_can_use_fragment_counters_without_sizes() {
        let mut aggregator = ProgressAggregator::new(vec![ExpectedStream {
            format_id: "hls".to_owned(),
            size: None,
            approx_size: None,
        }]);
        let progress = DownloadProgress {
            status: DownloadStatus::Downloading,
            downloaded_bytes: 10,
            total_bytes: None,
            total_bytes_estimate: None,
            fragment_index: Some(27),
            fragment_count: Some(54),
            speed_bytes_per_s: None,
            eta_s: None,
            filename: "title.fhls.mp4".to_owned(),
        };
        assert_eq!(aggregator.update(&progress).fraction, Some(0.5));
    }

    #[test]
    fn error_rules_cover_every_category_and_preserve_order() {
        let cases = [
            ("DRM-Protected stream", ErrorCategory::Drm),
            (
                "This uploader has not made this video available in your country",
                ErrorCategory::GeoBlocked,
            ),
            (
                "Channel members only; use --cookies",
                ErrorCategory::LoginRequired,
            ),
            (
                "HTTP Error 429: Too Many Requests",
                ErrorCategory::RateLimited,
            ),
            ("No video formats found", ErrorCategory::NoVideo),
            ("Unsupported URL", ErrorCategory::UnsupportedUrl),
            ("Private video", ErrorCategory::Unavailable),
            ("getaddrinfo failed", ErrorCategory::Network),
            ("Postprocessing: ffmpeg failed", ErrorCategory::Postprocess),
            ("Unable to extract title", ErrorCategory::ExtractorBroken),
            ("something unprecedented", ErrorCategory::Unknown),
        ];
        for (message, expected) in cases {
            assert_eq!(classify_error(message), expected, "{message}");
        }
        assert_eq!(
            classify_error("DRM protected and not available in your country"),
            ErrorCategory::Drm
        );
    }

    #[test]
    fn extracts_format_id_from_the_last_format_segment() {
        assert_eq!(format_id("title.f395.mp4").as_deref(), Some("395"));
        assert_eq!(format_id("x.f399-sr.mp4").as_deref(), Some("399-sr"));
        assert_eq!(format_id("plain.mp4"), None);
    }
}

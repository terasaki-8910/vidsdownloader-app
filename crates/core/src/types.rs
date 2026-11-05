//! Types shared by the yt-dlp output parser (`parse`), the argument builder (`args`),
//! the process runner (`runner`) and the job queue (`queue`).
//!
//! These shapes are the contract between modules built in parallel; change them only
//! together with every module that uses them.

use serde::{Deserialize, Serialize};

/// Prefix of a download progress line, printed by `--progress-template "download:..."`.
/// The rest of the line is a JSON object (yt-dlp's progress dict). Arrives on stdout.
pub const DOWNLOAD_MARKER: &str = "[vdl-dl]";

/// Prefix of a postprocessor progress line (`--progress-template "postprocess:..."`).
/// The rest of the line is a JSON object. Arrives on stderr.
pub const POSTPROCESS_MARKER: &str = "[vdl-pp]";

/// Prefix of the final file path line (`--print "after_move:..."`). The rest of the line
/// is a JSON string holding the absolute path. Arrives on stdout.
pub const PATH_MARKER: &str = "[vdl-path]";

/// Which pipe a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// One line of child process output, without its line terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    pub stream: OutputStream,
    pub text: String,
}

/// State reported in a download progress line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadStatus {
    Downloading,
    Finished,
    Error,
}

/// One download progress update for one stream (video or audio) of a job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub status: DownloadStatus,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub total_bytes_estimate: Option<u64>,
    /// yt-dlp's current fragment counter for fragmented downloads such as HLS/DASH.
    /// The index can exceed the count by one at the end.
    pub fragment_index: Option<u64>,
    /// yt-dlp's total fragment counter for fragmented downloads such as HLS/DASH.
    pub fragment_count: Option<u64>,
    pub speed_bytes_per_s: Option<f64>,
    pub eta_s: Option<f64>,
    /// Final name of the stream being downloaded, e.g. `Title [id].f395.mp4`.
    pub filename: String,
}

/// State reported in a postprocessor line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PostprocessStatus {
    Started,
    Processing,
    Finished,
}

/// One postprocessor update (merging, embedding, moving files, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostprocessProgress {
    pub status: PostprocessStatus,
    /// yt-dlp's postprocessor name, e.g. `Merger`, `MoveFiles`.
    pub postprocessor: String,
}

/// What kind of failure an `ERROR:` line describes; drives the next action shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCategory {
    UnsupportedUrl,
    PageMediaNotFound,
    PageMediaChoice,
    LoginRequired,
    Unavailable,
    GeoBlocked,
    RateLimited,
    NoVideo,
    Drm,
    ExtractorBroken,
    Network,
    Postprocess,
    Unknown,
}

/// Extra work within a lifecycle state that the UI should name explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobPhase {
    PageSearch,
}

/// A parsed line of yt-dlp output.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedLine {
    Download(DownloadProgress),
    Postprocess(PostprocessProgress),
    FinalPath(String),
    Error {
        category: ErrorCategory,
        message: String,
    },
    Warning(String),
    /// Anything else, kept for the job log.
    Other(String),
}

/// Lifecycle of a job. `Done`, `Failed` and `Canceled` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobState {
    Queued,
    Probing,
    Ready,
    Downloading,
    PostProcessing,
    Done,
    Failed,
    Canceled,
}

impl JobState {
    /// Whether the job has finished for good (until retried).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Canceled)
    }
}

use std::path::Path;

use serde::{Deserialize, Serialize};
use vids_core::executor::PageMediaChoice;
use vids_core::queue::{AlreadySaved, JobView};
use vids_core::store::LibraryItem;
use vids_core::types::{ErrorCategory, JobPhase, JobState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Ja,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    #[serde(rename = "best")]
    Best,
    #[serde(rename = "upTo1080")]
    UpTo1080,
    #[serde(rename = "upTo720")]
    UpTo720,
    #[serde(rename = "compatibilityH264")]
    CompatibilityH264,
    #[serde(rename = "audioM4a")]
    AudioM4a,
    #[serde(rename = "audioMp3")]
    AudioMp3,
    #[serde(rename = "commentsOnly")]
    CommentsOnly,
}

impl From<Preset> for vids_core::args::Preset {
    fn from(value: Preset) -> Self {
        match value {
            Preset::Best => Self::Best,
            Preset::UpTo1080 => Self::UpTo1080,
            Preset::UpTo720 => Self::UpTo720,
            Preset::CompatibilityH264 => Self::CompatibilityH264,
            Preset::AudioM4a => Self::AudioM4a,
            Preset::AudioMp3 => Self::AudioMp3,
            Preset::CommentsOnly => Self::CommentsOnly,
        }
    }
}

impl From<vids_core::args::Preset> for Preset {
    fn from(value: vids_core::args::Preset) -> Self {
        match value {
            vids_core::args::Preset::Best => Self::Best,
            vids_core::args::Preset::UpTo1080 => Self::UpTo1080,
            vids_core::args::Preset::UpTo720 => Self::UpTo720,
            vids_core::args::Preset::CompatibilityH264 => Self::CompatibilityH264,
            vids_core::args::Preset::AudioM4a => Self::AudioM4a,
            vids_core::args::Preset::AudioMp3 => Self::AudioMp3,
            vids_core::args::Preset::CommentsOnly => Self::CommentsOnly,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub language: Language,
    pub theme: Theme,
    pub concurrency: i64,
    pub clipboard_banner: bool,
    #[serde(default)]
    pub collect_include_ads: bool,
    #[serde(default)]
    pub keep_subtitle_files: bool,
    #[serde(default)]
    pub avoid_av1: bool,
    pub download_dir: String,
    /// Folders chosen as the download folder, most recent first (at most
    /// `MAX_RECENT_DOWNLOAD_DIRS`). Feeds the URL bar's folder menu.
    #[serde(default)]
    pub recent_download_dirs: Vec<String>,
    pub default_preset: Preset,
    /// The in-app browser's start page no longer offers importing logins from Firefox
    /// (the user chose 「今はしない」 or ran the import).
    #[serde(default)]
    pub import_offer_dismissed: bool,
    /// How often sources are checked for new items while the app runs (Settings > 一般
    /// 「新着を自動で確認」); off unless the user turns it on.
    #[serde(default)]
    pub source_auto_check: SourceAutoCheck,
    /// Which desktop notifications are shown; all on unless the user turns them off.
    #[serde(default)]
    pub notify: NotifySettings,
    /// The app version the user chose 「この版はスキップ」 for: the launch check does not offer
    /// it again (a newer one is offered). None until the user skips one.
    #[serde(default)]
    pub skipped_update_version: Option<String>,
}

/// The notification switches. `enabled = false` silences every notification; the three
/// kinds keep their own values meanwhile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotifySettings {
    pub enabled: bool,
    /// A download finished and saved a file.
    pub done: bool,
    /// A download failed.
    pub failed: bool,
    /// A source check queued new items.
    pub found: bool,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            done: true,
            failed: true,
            found: true,
        }
    }
}

/// How often every source is checked for new items while the app is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceAutoCheck {
    #[default]
    Off,
    Hourly,
    SixHours,
    Daily,
}

impl SourceAutoCheck {
    /// Seconds between two automatic checks of a source; `None` when off.
    pub const fn interval_seconds(self) -> Option<i64> {
        match self {
            Self::Off => None,
            Self::Hourly => Some(60 * 60),
            Self::SixHours => Some(6 * 60 * 60),
            Self::Daily => Some(24 * 60 * 60),
        }
    }
}

/// How many recently used download folders are kept.
pub const MAX_RECENT_DOWNLOAD_DIRS: usize = 5;

impl Settings {
    pub fn defaults(download_dir: String) -> Self {
        Self {
            language: Language::Ja,
            theme: Theme::System,
            concurrency: 2,
            clipboard_banner: true,
            collect_include_ads: false,
            keep_subtitle_files: false,
            avoid_av1: false,
            download_dir,
            recent_download_dirs: Vec::new(),
            default_preset: Preset::Best,
            import_offer_dismissed: false,
            source_auto_check: SourceAutoCheck::Off,
            notify: NotifySettings::default(),
            skipped_update_version: None,
        }
    }

    pub fn validated(mut self) -> Result<Self, String> {
        if !Path::new(&self.download_dir).is_absolute() {
            return Err("downloadDir must be an absolute path".to_owned());
        }
        self.concurrency = self.concurrency.clamp(1, 5);
        // A bad recent list is repaired, never a reason to reject (and so reset) the
        // user's other settings.
        let mut kept: Vec<String> = Vec::new();
        for dir in std::mem::take(&mut self.recent_download_dirs) {
            if kept.len() == MAX_RECENT_DOWNLOAD_DIRS {
                break;
            }
            if Path::new(&dir).is_absolute() && !kept.contains(&dir) {
                kept.push(dir);
            }
        }
        self.recent_download_dirs = kept;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDetails {
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub site: Option<String>,
    pub published_at: Option<i64>,
    pub duration_s: Option<f64>,
    pub thumbnail_url: Option<String>,
}

impl From<vids_core::queue::JobDetails> for JobDetails {
    fn from(value: vids_core::queue::JobDetails) -> Self {
        Self {
            title: value.title,
            uploader: value.uploader,
            site: value.site,
            published_at: value.published_at,
            duration_s: value.duration_s,
            thumbnail_url: value.thumbnail_url,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobError {
    pub category: ErrorCategory,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRow {
    pub id: u64,
    pub url: String,
    pub state: JobState,
    pub attempt: u32,
    pub progress: Option<f64>,
    pub phase: Option<JobPhase>,
    pub speed_bytes_per_s: Option<f64>,
    pub eta_s: Option<f64>,
    pub error: Option<JobError>,
    pub details: Option<JobDetails>,
    pub output_path: Option<String>,
    pub comments_count: Option<u64>,
    /// The newest saved-library item made by this job, if any.
    pub library_id: Option<i64>,
    pub skipped: Option<String>,
    pub list_count: Option<u32>,
    pub list_total: Option<u32>,
    /// Set when the job was skipped because the video is already saved. The core type
    /// already serializes as `{ savedAt, fileMissing, path }`.
    pub already_saved: Option<AlreadySaved>,
    pub page_media_choices: Vec<PageMediaChoice>,
}

impl JobRow {
    pub fn from_view(value: JobView, library_id: Option<i64>) -> Self {
        let outcome = value.outcome.unwrap_or_default();
        let list_count = (!outcome.entries.is_empty()).then(|| {
            u32::try_from(outcome.entries.len()).expect("list outcomes are capped at 300")
        });
        Self {
            id: value.id.0,
            url: value.url,
            state: value.state,
            attempt: value.attempt,
            progress: value.progress,
            phase: value.phase,
            speed_bytes_per_s: value.speed_bytes_per_s,
            eta_s: value.eta_s,
            error: value.error.map(|error| JobError {
                category: error.category,
                message: error.message,
            }),
            details: value.details.map(Into::into),
            output_path: outcome.output_path,
            comments_count: outcome.comments_count,
            library_id,
            skipped: outcome.skipped,
            list_count,
            list_total: outcome.list_total,
            already_saved: outcome.already_saved,
            page_media_choices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRow {
    pub id: i64,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub site: Option<String>,
    pub published_at: Option<i64>,
    pub duration_s: Option<f64>,
    pub path: String,
    pub comments_path: Option<String>,
    pub missing: bool,
    pub added_at: i64,
}

impl From<LibraryItem> for LibraryRow {
    fn from(value: LibraryItem) -> Self {
        Self {
            id: value.id,
            title: value.title,
            uploader: value.uploader,
            site: value
                .site
                .map(|key| vids_core::executor::friendly_site(Some(&key))),
            published_at: value.published_at,
            duration_s: value.duration_s,
            path: value.path,
            comments_path: value.comments_path,
            missing: value.missing,
            added_at: value.added_at,
        }
    }
}

/// What erasing a saved item's records did (`library_trash`, `library_forget`), so the UI
/// can tell the user what could not be erased.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EraseResult {
    /// The database's write-ahead log was copied into the database and emptied.
    pub checkpointed: bool,
    /// The app UI's HTTP disk cache (thumbnails) was cleared.
    pub cache_cleared: bool,
    /// Side files that could not be moved to the Recycle Bin / Trash and were left where
    /// they are.
    pub side_files_failed: Vec<String>,
}

/// Payload of the `job-removed` event: a job left the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct JobRemoved {
    pub id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ToolId {
    #[serde(rename = "yt-dlp")]
    YtDlp,
    #[serde(rename = "ffmpeg")]
    Ffmpeg,
    #[serde(rename = "deno")]
    Deno,
    #[serde(rename = "gallery-dl")]
    GalleryDl,
    /// Windows only (the in-app browser's ad blocker).
    #[serde(rename = "ublock-lite")]
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    UblockLite,
}

impl ToolId {
    pub const fn core(self) -> vids_core::tools::Tool {
        match self {
            Self::YtDlp => vids_core::tools::Tool::YtDlp,
            Self::Ffmpeg => vids_core::tools::Tool::Ffmpeg,
            Self::Deno => vids_core::tools::Tool::Deno,
            Self::GalleryDl => vids_core::tools::Tool::GalleryDl,
            Self::UblockLite => vids_core::tools::Tool::UblockLite,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub tool: ToolId,
    pub installed: bool,
    pub version: Option<String>,
    pub expected_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolPhase {
    Downloading,
    // Kept for exact IPC contract parity; this installer reports download bytes and
    // then its terminal phase because the core install API is one blocking operation.
    #[allow(dead_code)]
    Installing,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolProgress {
    pub tool: ToolId,
    pub phase: ToolPhase,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vids_core::queue::{JobDetails as CoreJobDetails, JobFailure, JobId, JobOutcome};

    #[test]
    fn job_view_maps_details_outcome_and_error() {
        let view = JobView {
            id: JobId(7),
            url: "https://example.com/video".to_owned(),
            state: JobState::Failed,
            attempt: 2,
            progress: Some(0.75),
            phase: Some(JobPhase::PageSearch),
            details: Some(CoreJobDetails {
                title: Some("Title".to_owned()),
                uploader: Some("Uploader".to_owned()),
                site: Some("Site".to_owned()),
                published_at: Some(123),
                duration_s: Some(45.5),
                thumbnail_url: Some("https://example.com/thumb.jpg".to_owned()),
            }),
            speed_bytes_per_s: Some(1000.0),
            eta_s: Some(4.0),
            outcome: Some(JobOutcome {
                output_path: Some("C:\\Videos\\file.mp4".to_owned()),
                comments_count: None,
                skipped: Some("reason".to_owned()),
                entries: vec![vids_core::queue::ListEntry {
                    url: "https://example.com/child".to_owned(),
                    playlist_item: None,
                    title: Some("Child".to_owned()),
                }],
                list_total: Some(4),
                already_saved: Some(AlreadySaved {
                    saved_at: Some(1_700_000_000),
                    file_missing: true,
                    path: "C:\\Videos\\saved.mp4".to_owned(),
                }),
            }),
            error: Some(JobFailure {
                category: ErrorCategory::Network,
                message: "offline".to_owned(),
            }),
        };

        let row = JobRow::from_view(view, Some(12));
        assert_eq!(row.id, 7);
        assert_eq!(row.details.as_ref().unwrap().published_at, Some(123));
        assert_eq!(row.output_path.as_deref(), Some("C:\\Videos\\file.mp4"));
        assert_eq!(row.skipped.as_deref(), Some("reason"));
        assert_eq!(row.list_count, Some(1));
        assert_eq!(row.list_total, Some(4));
        assert_eq!(row.error.as_ref().unwrap().category, ErrorCategory::Network);
        let json = serde_json::to_value(row).unwrap();
        assert_eq!(json["state"], "Failed");
        assert_eq!(json["speedBytesPerS"], 1000.0);
        assert_eq!(json["phase"], "pageSearch");
        assert_eq!(
            json["details"]["thumbnailUrl"],
            "https://example.com/thumb.jpg"
        );
        assert_eq!(json["error"]["category"], "Network");
        assert_eq!(json["error"]["message"], "offline");
        assert_eq!(json["outputPath"], "C:\\Videos\\file.mp4");
        assert_eq!(json["libraryId"], 12);
        assert_eq!(json["listCount"], 1);
        assert_eq!(json["listTotal"], 4);
        assert_eq!(
            json["alreadySaved"],
            serde_json::json!({
                "savedAt": 1_700_000_000,
                "fileMissing": true,
                "path": "C:\\Videos\\saved.mp4"
            })
        );
    }

    #[test]
    fn job_view_without_an_outcome_has_no_already_saved() {
        let view = JobView {
            id: JobId(8),
            url: "https://example.com/video".to_owned(),
            state: JobState::Queued,
            attempt: 1,
            progress: None,
            phase: None,
            details: None,
            speed_bytes_per_s: None,
            eta_s: None,
            outcome: None,
            error: None,
        };

        let json = serde_json::to_value(JobRow::from_view(view, None)).unwrap();
        assert_eq!(json["libraryId"], serde_json::Value::Null);
        assert_eq!(json["alreadySaved"], serde_json::Value::Null);
        assert_eq!(json["skipped"], serde_json::Value::Null);
        assert_eq!(json["listCount"], serde_json::Value::Null);
    }

    #[test]
    fn library_row_shows_the_friendly_site_name() {
        let item = LibraryItem {
            id: 1,
            job_id: None,
            site: Some("Youtube".to_owned()),
            video_id: Some("YE7VzlLtp-4".to_owned()),
            url: None,
            title: Some("Big Buck Bunny".to_owned()),
            uploader: Some("Blender".to_owned()),
            published_at: None,
            duration_s: None,
            path: r"C:\Videos\a.mp4".to_owned(),
            comments_path: None,
            size_bytes: None,
            added_at: 0,
            missing: false,
        };
        let json = serde_json::to_value(LibraryRow::from(item)).unwrap();
        assert_eq!(json["site"], "YouTube");
        assert_eq!(json["commentsPath"], serde_json::Value::Null);
    }

    #[test]
    fn erase_result_and_job_removed_match_the_contract() {
        let result = EraseResult {
            checkpointed: true,
            cache_cleared: false,
            side_files_failed: vec!["/videos/a.ja.vtt".to_owned()],
        };
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({
                "checkpointed": true,
                "cacheCleared": false,
                "sideFilesFailed": ["/videos/a.ja.vtt"]
            })
        );
        assert_eq!(
            serde_json::to_value(JobRemoved { id: 7 }).unwrap(),
            serde_json::json!({ "id": 7 })
        );
    }

    #[test]
    fn settings_defaults_match_the_contract() {
        let settings = Settings::defaults("/absolute/downloads".to_owned());
        assert_eq!(settings.language, Language::Ja);
        assert_eq!(settings.theme, Theme::System);
        assert_eq!(settings.concurrency, 2);
        assert!(settings.clipboard_banner);
        assert!(!settings.collect_include_ads);
        assert!(!settings.keep_subtitle_files);
        assert!(!settings.avoid_av1);
        assert_eq!(settings.download_dir, "/absolute/downloads");
        assert!(settings.recent_download_dirs.is_empty());
        assert_eq!(settings.default_preset, Preset::Best);
        assert!(!settings.import_offer_dismissed);
    }

    #[test]
    fn settings_serde_round_trip_uses_camel_case() {
        let settings = Settings::defaults("/absolute/downloads".to_owned());
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["language"], "ja");
        assert_eq!(json["theme"], "system");
        assert_eq!(json["clipboardBanner"], true);
        assert_eq!(json["collectIncludeAds"], false);
        assert_eq!(json["keepSubtitleFiles"], false);
        assert_eq!(json["avoidAv1"], false);
        assert_eq!(json["downloadDir"], "/absolute/downloads");
        assert_eq!(json["recentDownloadDirs"], serde_json::json!([]));
        assert_eq!(json["defaultPreset"], "best");
        assert_eq!(json["importOfferDismissed"], false);
        assert!(json.get("download_dir").is_none());
        assert_eq!(serde_json::from_value::<Settings>(json).unwrap(), settings);
    }

    #[test]
    fn settings_without_keep_subtitle_files_defaults_to_false() {
        let mut json =
            serde_json::to_value(Settings::defaults("/absolute/downloads".to_owned())).unwrap();
        json.as_object_mut().unwrap().remove("keepSubtitleFiles");

        let settings = serde_json::from_value::<Settings>(json).unwrap();
        assert!(!settings.keep_subtitle_files);
    }

    #[test]
    fn settings_without_avoid_av1_defaults_to_false() {
        let mut json =
            serde_json::to_value(Settings::defaults("/absolute/downloads".to_owned())).unwrap();
        json.as_object_mut().unwrap().remove("avoidAv1");

        let settings = serde_json::from_value::<Settings>(json).unwrap();
        assert!(!settings.avoid_av1);
    }

    #[test]
    fn settings_without_import_offer_dismissed_defaults_to_false() {
        let mut json =
            serde_json::to_value(Settings::defaults("/absolute/downloads".to_owned())).unwrap();
        json.as_object_mut().unwrap().remove("importOfferDismissed");

        let settings = serde_json::from_value::<Settings>(json).unwrap();
        assert!(!settings.import_offer_dismissed);

        let mut json =
            serde_json::to_value(Settings::defaults("/absolute/downloads".to_owned())).unwrap();
        json["importOfferDismissed"] = serde_json::json!(true);
        assert!(
            serde_json::from_value::<Settings>(json)
                .unwrap()
                .import_offer_dismissed
        );
    }

    #[test]
    fn source_auto_check_is_off_by_default_and_uses_the_contract_names() {
        let mut json =
            serde_json::to_value(Settings::defaults("/absolute/downloads".to_owned())).unwrap();
        assert_eq!(json["sourceAutoCheck"], "off");
        json.as_object_mut().unwrap().remove("sourceAutoCheck");
        let settings = serde_json::from_value::<Settings>(json).unwrap();
        assert_eq!(settings.source_auto_check, SourceAutoCheck::Off);

        for (value, expected, seconds) in [
            ("off", SourceAutoCheck::Off, None),
            ("hourly", SourceAutoCheck::Hourly, Some(3_600)),
            ("sixHours", SourceAutoCheck::SixHours, Some(21_600)),
            ("daily", SourceAutoCheck::Daily, Some(86_400)),
        ] {
            let parsed: SourceAutoCheck = serde_json::from_value(serde_json::json!(value)).unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(serde_json::to_value(parsed).unwrap(), value);
            assert_eq!(parsed.interval_seconds(), seconds);
        }
    }

    #[test]
    fn notifications_are_all_on_by_default_and_when_stored_settings_lack_them() {
        let all_on = NotifySettings {
            enabled: true,
            done: true,
            failed: true,
            found: true,
        };
        let mut json =
            serde_json::to_value(Settings::defaults("/absolute/downloads".to_owned())).unwrap();
        assert_eq!(
            json["notify"],
            serde_json::json!({ "enabled": true, "done": true, "failed": true, "found": true })
        );
        json.as_object_mut().unwrap().remove("notify");
        let settings = serde_json::from_value::<Settings>(json.clone()).unwrap();
        assert_eq!(settings.notify, all_on);

        // A switch missing from a stored object is on; the others keep their values.
        json["notify"] = serde_json::json!({ "enabled": false, "failed": false });
        let settings = serde_json::from_value::<Settings>(json).unwrap();
        assert_eq!(
            settings.notify,
            NotifySettings {
                enabled: false,
                failed: false,
                ..all_on
            }
        );
    }

    #[test]
    fn skipped_update_version_is_none_by_default_and_when_stored_settings_lack_it() {
        let mut json =
            serde_json::to_value(Settings::defaults("/absolute/downloads".to_owned())).unwrap();
        assert_eq!(json["skippedUpdateVersion"], serde_json::Value::Null);
        json.as_object_mut().unwrap().remove("skippedUpdateVersion");
        let settings = serde_json::from_value::<Settings>(json.clone()).unwrap();
        assert_eq!(settings.skipped_update_version, None);

        json["skippedUpdateVersion"] = serde_json::json!("0.2.0");
        let settings = serde_json::from_value::<Settings>(json).unwrap();
        assert_eq!(settings.skipped_update_version.as_deref(), Some("0.2.0"));
        assert_eq!(
            serde_json::to_value(&settings).unwrap()["skippedUpdateVersion"],
            "0.2.0"
        );
    }

    #[test]
    fn settings_without_collect_include_ads_defaults_to_false() {
        let mut json =
            serde_json::to_value(Settings::defaults("/absolute/downloads".to_owned())).unwrap();
        json.as_object_mut().unwrap().remove("collectIncludeAds");

        let settings = serde_json::from_value::<Settings>(json).unwrap();
        assert!(!settings.collect_include_ads);
    }

    #[test]
    fn settings_without_recent_download_dirs_loads_with_an_empty_list() {
        let mut json =
            serde_json::to_value(Settings::defaults("/absolute/downloads".to_owned())).unwrap();
        json.as_object_mut().unwrap().remove("recentDownloadDirs");

        let settings = serde_json::from_value::<Settings>(json).unwrap();
        assert!(settings.recent_download_dirs.is_empty());
    }

    #[test]
    fn settings_validation_sanitizes_recent_download_dirs_without_failing() {
        let root = std::env::current_dir().unwrap();
        let abs = |name: &str| root.join(name).to_string_lossy().into_owned();

        let mut settings = Settings::defaults(abs("current"));
        settings.recent_download_dirs = vec![
            abs("a"),
            "relative/dir".to_owned(),
            abs("b"),
            abs("a"),
            String::new(),
            abs("c"),
            abs("d"),
            abs("e"),
            abs("f"),
            abs("g"),
        ];
        let validated = settings.validated().unwrap();
        assert_eq!(
            validated.recent_download_dirs,
            vec![abs("a"), abs("b"), abs("c"), abs("d"), abs("e")]
        );
        assert_eq!(validated.download_dir, abs("current"));

        let mut settings = Settings::defaults(abs("current"));
        settings.recent_download_dirs = vec!["relative".to_owned(), String::new()];
        assert!(
            settings
                .validated()
                .unwrap()
                .recent_download_dirs
                .is_empty()
        );
    }

    #[test]
    fn tool_status_serializes_expected_version_in_camel_case() {
        let status = ToolStatus {
            tool: ToolId::Ffmpeg,
            installed: true,
            version: Some("latest".to_owned()),
            expected_version: "2026-10-31".to_owned(),
        };

        let json = serde_json::to_value(status).unwrap();
        assert_eq!(json["tool"], "ffmpeg");
        assert_eq!(json["installed"], true);
        assert_eq!(json["version"], "latest");
        assert_eq!(json["expectedVersion"], "2026-10-31");
    }

    #[test]
    fn settings_validation_clamps_concurrency_and_rejects_relative_paths() {
        let mut settings = Settings::defaults(
            std::env::current_dir()
                .unwrap()
                .join("downloads")
                .to_string_lossy()
                .into_owned(),
        );
        settings.concurrency = 99;
        assert_eq!(settings.validated().unwrap().concurrency, 5);

        let mut settings = Settings::defaults(
            std::env::current_dir()
                .unwrap()
                .join("downloads")
                .to_string_lossy()
                .into_owned(),
        );
        settings.concurrency = -10;
        assert_eq!(settings.validated().unwrap().concurrency, 1);

        let relative = Settings::defaults("downloads".to_owned());
        assert_eq!(
            relative.validated().unwrap_err(),
            "downloadDir must be an absolute path"
        );
    }

    #[test]
    fn preset_strings_and_core_mapping_round_trip() {
        let cases = [
            (Preset::Best, "best"),
            (Preset::UpTo1080, "upTo1080"),
            (Preset::UpTo720, "upTo720"),
            (Preset::CompatibilityH264, "compatibilityH264"),
            (Preset::AudioM4a, "audioM4a"),
            (Preset::AudioMp3, "audioMp3"),
            (Preset::CommentsOnly, "commentsOnly"),
        ];
        for (preset, wire) in cases {
            assert_eq!(
                serde_json::to_string(&preset).unwrap(),
                format!("\"{wire}\"")
            );
            assert_eq!(
                serde_json::from_str::<Preset>(&format!("\"{wire}\"")).unwrap(),
                preset
            );
            assert_eq!(Preset::from(vids_core::args::Preset::from(preset)), preset);
        }
    }
}

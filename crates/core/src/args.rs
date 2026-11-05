//! Building yt-dlp command lines (argv) for probing, downloading and fetching comments.

use std::{ffi::OsString, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{DOWNLOAD_MARKER, PATH_MARKER, POSTPROCESS_MARKER};

/// Managed executable locations used in every yt-dlp invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPaths {
    /// Path to the managed yt-dlp executable; callers use it as the program.
    pub yt_dlp: PathBuf,
    /// Directory containing managed ffmpeg and ffprobe executables.
    pub ffmpeg_dir: PathBuf,
    /// Path to the managed Deno executable.
    pub deno: PathBuf,
}

/// Download quality and conversion preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Best available quality in an MP4 container.
    Best,
    /// Best quality no higher than 1080p.
    UpTo1080,
    /// Best quality no higher than 720p.
    UpTo720,
    /// Prefer H.264 video for player compatibility.
    CompatibilityH264,
    /// Extract audio as M4A.
    AudioM4a,
    /// Extract audio as highest-quality MP3.
    AudioMp3,
    /// Fetch supported scrolling comments without downloading the video.
    CommentsOnly,
}

impl Preset {
    fn is_audio(self) -> bool {
        matches!(self, Self::AudioM4a | Self::AudioMp3)
    }
}

/// Optional content embedded by yt-dlp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Embed {
    /// Download and embed subtitles.
    pub subtitles: bool,
    /// Comma-separated subtitle languages passed to yt-dlp.
    pub subtitle_langs: Vec<String>,
    /// Keep subtitle files after yt-dlp embeds them into the media file.
    #[serde(default)]
    pub keep_subtitle_files: bool,
    /// Embed the thumbnail.
    pub thumbnail: bool,
    /// Embed media metadata.
    pub metadata: bool,
    /// Embed chapter markers.
    pub chapters: bool,
}

/// Comment subtitle source requested for a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentsSource {
    /// Do not fetch comments.
    None,
    /// Fetch niconico `comments` subtitles.
    Niconico,
    /// Fetch Bilibili `danmaku` subtitles.
    Bilibili,
}

/// Cookie source passed to yt-dlp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cookies {
    /// Run without cookies.
    None,
    /// Read a Netscape cookie file.
    File(PathBuf),
    /// Read cookies from a named Firefox profile.
    Firefox {
        /// Firefox profile name or path understood by yt-dlp.
        profile: String,
    },
}

/// Options for probing a URL as JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOptions {
    /// Avoid resolving every item in a playlist.
    pub flat_playlist: bool,
    /// yt-dlp playlist item selection expression.
    pub playlist_items: Option<String>,
    /// Cookie source for extraction.
    pub cookies: Cookies,
    /// The preset whose format selection the probe applies, so that the probe's
    /// `requested_formats` are the formats the download will fetch.
    pub format_selection: Option<Preset>,
    /// Prefer VP9, then H.264, over AV1 at the selected resolution.
    pub avoid_av1: bool,
    /// Extract niconico comments and Bilibili danmaku into the probe JSON. Those
    /// extractors only fetch them when subtitles are requested while extracting, and a
    /// download from the saved JSON (`--load-info-json`) cannot fetch them later.
    pub comment_subtitles: bool,
}

/// Options for downloading from a previously saved info JSON document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOptions {
    /// Saved yt-dlp info JSON used as the download input.
    pub info_json: PathBuf,
    /// Per-job yt-dlp output template.
    pub output_template: String,
    /// Final output directory.
    pub home_dir: PathBuf,
    /// Temporary download directory.
    pub temp_dir: PathBuf,
    /// Quality or audio preset.
    pub preset: Preset,
    /// Prefer VP9, then H.264, over AV1 at the selected resolution.
    pub avoid_av1: bool,
    /// Optional embedded content.
    pub embed: Embed,
    /// Optional scrolling-comment source.
    pub comments: CommentsSource,
    /// Cookie source for extraction and download.
    pub cookies: Cookies,
    /// Optional yt-dlp match-filter expression.
    pub match_filter: Option<String>,
    /// Optional yt-dlp download archive.
    pub download_archive: Option<PathBuf>,
    /// yt-dlp playlist item selection expression.
    pub playlist_items: Option<String>,
}

/// Page headers needed when yt-dlp is handed a stream URL that was discovered while the
/// page was running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageHeaders {
    pub referer: String,
    pub origin: String,
}

/// Invalid argument-builder request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ArgsError {
    /// Fetching comments requires a concrete supported source.
    #[error("comments source must be niconico or bilibili")]
    MissingCommentsSource,
}

/// Build arguments for probing one URL.
pub fn probe_args(tools: &ToolPaths, url: &str, options: &ProbeOptions) -> Vec<OsString> {
    probe_args_with_headers(tools, url, options, None)
}

pub fn probe_args_with_headers(
    tools: &ToolPaths,
    url: &str,
    options: &ProbeOptions,
    headers: Option<&PageHeaders>,
) -> Vec<OsString> {
    let mut args = common_args(tools);
    push(&mut args, &["-J"]);
    if options.flat_playlist {
        push(&mut args, &["--flat-playlist"]);
    }
    if let Some(items) = &options.playlist_items {
        push(&mut args, &["--playlist-items"]);
        args.push(items.into());
    }
    if let Some(preset) = options.format_selection {
        push(&mut args, &format_selection(preset, options.avoid_av1));
    }
    if options.comment_subtitles {
        push(
            &mut args,
            &["--write-subs", "--sub-langs", "comments,danmaku"],
        );
    }
    append_cookies(&mut args, &options.cookies);
    append_page_headers(&mut args, headers);
    push(&mut args, &["--"]);
    args.push(url.into());
    args
}

/// The format selection part of a preset, shared by probe and download. Probing without
/// it picks other formats (yt-dlp's default best, e.g. 1080p) than the download, and the
/// job's progress, summed over the probe's formats, then never reaches 100 %.
fn format_selection(preset: Preset, avoid_av1: bool) -> [&'static str; 2] {
    match (preset, avoid_av1) {
        (Preset::Best, false) => ["-S", "res,ext:mp4:m4a"],
        (Preset::Best, true) => ["-S", "res,vcodec:vp9,ext:mp4:m4a"],
        (Preset::UpTo1080, false) => ["-S", "res:1080,ext:mp4:m4a"],
        (Preset::UpTo1080, true) => ["-S", "res:1080,vcodec:vp9,ext:mp4:m4a"],
        (Preset::UpTo720, false) => ["-S", "res:720,ext:mp4:m4a"],
        (Preset::UpTo720, true) => ["-S", "res:720,vcodec:vp9,ext:mp4:m4a"],
        (Preset::CompatibilityH264, _) => ["-S", "vcodec:h264,res,ext:mp4:m4a"],
        (Preset::AudioM4a, _) => ["-f", "ba[ext=m4a]/ba"],
        (Preset::AudioMp3, _) => ["-f", "ba"],
        // The probe still resolves the video normally; the executor branches before download.
        (Preset::CommentsOnly, false) => ["-S", "res,ext:mp4:m4a"],
        (Preset::CommentsOnly, true) => ["-S", "res,vcodec:vp9,ext:mp4:m4a"],
    }
}

/// Build arguments for downloading from saved info JSON.
pub fn download_args(tools: &ToolPaths, options: &DownloadOptions) -> Vec<OsString> {
    download_args_with_headers(tools, options, None)
}

pub fn download_args_with_headers(
    tools: &ToolPaths,
    options: &DownloadOptions,
    headers: Option<&PageHeaders>,
) -> Vec<OsString> {
    let mut args = common_args(tools);
    push(
        &mut args,
        &["--newline", "--progress", "--progress-template"],
    );
    args.push(format!("download:{DOWNLOAD_MARKER}%(progress)j").into());
    push(&mut args, &["--progress-template"]);
    args.push(format!("postprocess:{POSTPROCESS_MARKER}%(progress)j").into());
    push(&mut args, &["--print"]);
    args.push(format!("after_move:{PATH_MARKER}%(filepath)j").into());
    push(&mut args, &["--load-info-json"]);
    args.push(options.info_json.as_os_str().to_owned());
    push(&mut args, &["-P"]);
    args.push(prefixed_path("home:", &options.home_dir));
    push(&mut args, &["-P"]);
    args.push(prefixed_path("temp:", &options.temp_dir));
    push(&mut args, &["-o"]);
    args.push(options.output_template.as_str().into());
    append_preset(&mut args, options.preset, options.avoid_av1);
    append_embed(&mut args, options);
    append_comments(&mut args, options.comments);
    append_cookies(&mut args, &options.cookies);
    append_page_headers(&mut args, headers);
    if let Some(filter) = &options.match_filter {
        push(&mut args, &["--match-filters"]);
        args.push(filter.into());
    }
    if let Some(archive) = &options.download_archive {
        push(&mut args, &["--download-archive"]);
        args.push(archive.as_os_str().to_owned());
    }
    if let Some(items) = &options.playlist_items {
        push(&mut args, &["--playlist-items"]);
        args.push(items.into());
    }
    args
}

/// Build arguments for fetching comments without downloading the video.
pub fn comments_args(
    tools: &ToolPaths,
    url: &str,
    output_template: &str,
    source: CommentsSource,
    cookies: &Cookies,
) -> Result<Vec<OsString>, ArgsError> {
    let language = match source {
        CommentsSource::Niconico => "comments",
        CommentsSource::Bilibili => "danmaku",
        CommentsSource::None => return Err(ArgsError::MissingCommentsSource),
    };
    let mut args = common_args(tools);
    push(
        &mut args,
        &[
            "--skip-download",
            "--write-subs",
            "--sub-langs",
            language,
            "-o",
        ],
    );
    args.push(output_template.into());
    append_cookies(&mut args, cookies);
    push(&mut args, &["--"]);
    args.push(url.into());
    Ok(args)
}

fn common_args(tools: &ToolPaths) -> Vec<OsString> {
    let mut args = Vec::new();
    push(
        &mut args,
        &[
            "--ignore-config",
            "--no-update",
            "--encoding",
            "utf-8",
            "--js-runtimes",
        ],
    );
    args.push(prefixed_path("deno:", &tools.deno));
    push(&mut args, &["--ffmpeg-location"]);
    args.push(tools.ffmpeg_dir.as_os_str().to_owned());
    args
}

fn append_preset(args: &mut Vec<OsString>, preset: Preset, avoid_av1: bool) {
    push(args, &format_selection(preset, avoid_av1));
    match preset {
        Preset::Best | Preset::UpTo1080 | Preset::UpTo720 | Preset::CompatibilityH264 => {
            push(args, &["--merge-output-format", "mp4"]);
        }
        Preset::AudioM4a => push(args, &["-x", "--audio-format", "m4a"]),
        Preset::AudioMp3 => push(
            args,
            &["-x", "--audio-format", "mp3", "--audio-quality", "0"],
        ),
        Preset::CommentsOnly => {}
    }
}

fn append_embed(args: &mut Vec<OsString>, options: &DownloadOptions) {
    if options.embed.subtitles
        && options.comments == CommentsSource::None
        && !options.preset.is_audio()
    {
        push(args, &["--write-subs", "--embed-subs", "--sub-langs"]);
        args.push(options.embed.subtitle_langs.join(",").into());
        if !options.embed.keep_subtitle_files {
            push(args, &["--compat-options", "no-keep-subs"]);
        }
    }
    if options.embed.thumbnail {
        push(args, &["--embed-thumbnail"]);
    }
    if options.embed.metadata {
        push(args, &["--embed-metadata"]);
    }
    if options.embed.chapters {
        push(args, &["--embed-chapters"]);
    }
}

fn append_comments(args: &mut Vec<OsString>, source: CommentsSource) {
    let language = match source {
        CommentsSource::None => return,
        CommentsSource::Niconico => "comments",
        CommentsSource::Bilibili => "danmaku",
    };
    push(args, &["--write-subs", "--sub-langs", language]);
}

fn append_cookies(args: &mut Vec<OsString>, cookies: &Cookies) {
    match cookies {
        Cookies::None => {}
        Cookies::File(path) => {
            push(args, &["--cookies"]);
            args.push(path.as_os_str().to_owned());
        }
        Cookies::Firefox { profile } => {
            push(args, &["--cookies-from-browser"]);
            args.push(format!("firefox:{profile}").into());
        }
    }
}

fn append_page_headers(args: &mut Vec<OsString>, headers: Option<&PageHeaders>) {
    let Some(headers) = headers else {
        return;
    };
    push(args, &["--referer"]);
    args.push(headers.referer.as_str().into());
    push(args, &["--add-header"]);
    args.push(format!("Origin:{}", headers.origin).into());
}

fn prefixed_path(prefix: &str, path: &PathBuf) -> OsString {
    let mut value = OsString::from(prefix);
    value.push(path);
    value
}

fn push(args: &mut Vec<OsString>, values: &[&str]) {
    args.extend(values.iter().map(OsString::from));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> ToolPaths {
        ToolPaths {
            yt_dlp: PathBuf::from(r"C:\Program Files\動画 tools\yt-dlp.exe"),
            ffmpeg_dir: PathBuf::from(r"C:\Program Files\動画 tools\ffmpeg"),
            deno: PathBuf::from(r"C:\Program Files\動画 tools\deno.exe"),
        }
    }

    fn strings(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn common() -> Vec<OsString> {
        strings(&[
            "--ignore-config",
            "--no-update",
            "--encoding",
            "utf-8",
            "--js-runtimes",
            r"deno:C:\Program Files\動画 tools\deno.exe",
            "--ffmpeg-location",
            r"C:\Program Files\動画 tools\ffmpeg",
        ])
    }

    fn base_options(preset: Preset) -> DownloadOptions {
        DownloadOptions {
            info_json: PathBuf::from(r"C:\作業 folder\video.info.json"),
            output_template: "%(title)s [%(id)s].%(ext)s".to_owned(),
            home_dir: PathBuf::from(r"C:\動画 output"),
            temp_dir: PathBuf::from(r"C:\動画 temp"),
            preset,
            avoid_av1: false,
            embed: Embed {
                subtitles: false,
                subtitle_langs: Vec::new(),
                keep_subtitle_files: false,
                thumbnail: false,
                metadata: false,
                chapters: false,
            },
            comments: CommentsSource::None,
            cookies: Cookies::None,
            match_filter: None,
            download_archive: None,
            playlist_items: None,
        }
    }

    fn download_prefix() -> Vec<OsString> {
        let mut expected = common();
        expected.extend(strings(&[
            "--newline",
            "--progress",
            "--progress-template",
            "download:[vdl-dl]%(progress)j",
            "--progress-template",
            "postprocess:[vdl-pp]%(progress)j",
            "--print",
            "after_move:[vdl-path]%(filepath)j",
            "--load-info-json",
            r"C:\作業 folder\video.info.json",
            "-P",
            r"home:C:\動画 output",
            "-P",
            r"temp:C:\動画 temp",
            "-o",
            "%(title)s [%(id)s].%(ext)s",
        ]));
        expected
    }

    fn format_selection_cases() -> [(Preset, bool, &'static [&'static str]); 12] {
        [
            (Preset::Best, false, &["-S", "res,ext:mp4:m4a"]),
            (Preset::Best, true, &["-S", "res,vcodec:vp9,ext:mp4:m4a"]),
            (Preset::UpTo1080, false, &["-S", "res:1080,ext:mp4:m4a"]),
            (
                Preset::UpTo1080,
                true,
                &["-S", "res:1080,vcodec:vp9,ext:mp4:m4a"],
            ),
            (Preset::UpTo720, false, &["-S", "res:720,ext:mp4:m4a"]),
            (
                Preset::UpTo720,
                true,
                &["-S", "res:720,vcodec:vp9,ext:mp4:m4a"],
            ),
            (
                Preset::CompatibilityH264,
                false,
                &["-S", "vcodec:h264,res,ext:mp4:m4a"],
            ),
            (
                Preset::CompatibilityH264,
                true,
                &["-S", "vcodec:h264,res,ext:mp4:m4a"],
            ),
            (Preset::AudioM4a, false, &["-f", "ba[ext=m4a]/ba"]),
            (Preset::AudioM4a, true, &["-f", "ba[ext=m4a]/ba"]),
            (Preset::AudioMp3, false, &["-f", "ba"]),
            (Preset::AudioMp3, true, &["-f", "ba"]),
        ]
    }

    #[test]
    fn probe_plain_has_exact_arguments() {
        let options = ProbeOptions {
            flat_playlist: false,
            playlist_items: None,
            cookies: Cookies::None,
            format_selection: None,
            avoid_av1: false,
            comment_subtitles: false,
        };
        let mut expected = common();
        expected.extend(strings(&["-J", "--", "https://example.test/動画"]));
        assert_eq!(
            probe_args(&tools(), "https://example.test/動画", &options),
            expected
        );
    }

    #[test]
    fn probe_flat_with_items_and_firefox_has_exact_arguments() {
        let options = ProbeOptions {
            flat_playlist: true,
            playlist_items: Some("1:20:2".to_owned()),
            cookies: Cookies::Firefox {
                profile: "利用者 profile".to_owned(),
            },
            format_selection: None,
            avoid_av1: false,
            comment_subtitles: false,
        };
        let mut expected = common();
        expected.extend(strings(&[
            "-J",
            "--flat-playlist",
            "--playlist-items",
            "1:20:2",
            "--cookies-from-browser",
            "firefox:利用者 profile",
            "--",
            "https://example.test/channel",
        ]));
        assert_eq!(
            probe_args(&tools(), "https://example.test/channel", &options),
            expected
        );
    }

    #[test]
    fn probe_requests_comment_subtitles_when_asked() {
        let options = ProbeOptions {
            flat_playlist: false,
            playlist_items: None,
            cookies: Cookies::None,
            format_selection: Some(Preset::Best),
            avoid_av1: false,
            comment_subtitles: true,
        };
        let mut expected = common();
        expected.extend(strings(&[
            "-J",
            "-S",
            "res,ext:mp4:m4a",
            "--write-subs",
            "--sub-langs",
            "comments,danmaku",
            "--",
            "https://www.nicovideo.jp/watch/sm9",
        ]));
        assert_eq!(
            probe_args(&tools(), "https://www.nicovideo.jp/watch/sm9", &options),
            expected
        );
    }

    #[test]
    fn probe_has_exact_arguments_for_every_preset_with_and_without_avoiding_av1() {
        for (preset, avoid_av1, selection) in format_selection_cases() {
            let options = ProbeOptions {
                flat_playlist: false,
                playlist_items: None,
                cookies: Cookies::None,
                format_selection: Some(preset),
                avoid_av1,
                comment_subtitles: false,
            };
            let mut expected = common();
            expected.extend(strings(&["-J"]));
            expected.extend(strings(selection));
            expected.extend(strings(&["--", "https://example.test/v"]));
            assert_eq!(
                probe_args(&tools(), "https://example.test/v", &options),
                expected,
                "{preset:?}, avoid_av1={avoid_av1}"
            );
        }
    }

    #[test]
    fn download_has_exact_arguments_for_every_preset_with_and_without_avoiding_av1() {
        for (preset, avoid_av1, selection) in format_selection_cases() {
            let mut expected = download_prefix();
            expected.extend(strings(selection));
            match preset {
                Preset::Best | Preset::UpTo1080 | Preset::UpTo720 | Preset::CompatibilityH264 => {
                    expected.extend(strings(&["--merge-output-format", "mp4"]));
                }
                Preset::AudioM4a => {
                    expected.extend(strings(&["-x", "--audio-format", "m4a"]));
                }
                Preset::AudioMp3 => {
                    expected.extend(strings(&[
                        "-x",
                        "--audio-format",
                        "mp3",
                        "--audio-quality",
                        "0",
                    ]));
                }
                Preset::CommentsOnly => {}
            }
            let mut options = base_options(preset);
            options.avoid_av1 = avoid_av1;
            assert_eq!(
                download_args(&tools(), &options),
                expected,
                "{preset:?}, avoid_av1={avoid_av1}"
            );
        }
    }

    #[test]
    fn download_selects_one_playlist_item_when_requested() {
        let mut options = base_options(Preset::Best);
        options.playlist_items = Some("12".to_owned());
        let mut expected = download_prefix();
        expected.extend(strings(&[
            "-S",
            "res,ext:mp4:m4a",
            "--merge-output-format",
            "mp4",
            "--playlist-items",
            "12",
        ]));
        assert_eq!(download_args(&tools(), &options), expected);
    }

    #[test]
    fn embedded_subtitles_are_deleted_by_default() {
        let mut options = base_options(Preset::Best);
        options.embed = Embed {
            subtitles: true,
            subtitle_langs: vec!["ja".to_owned(), "en.*".to_owned()],
            keep_subtitle_files: false,
            thumbnail: true,
            metadata: true,
            chapters: true,
        };
        let mut expected = download_prefix();
        expected.extend(strings(&[
            "-S",
            "res,ext:mp4:m4a",
            "--merge-output-format",
            "mp4",
            "--write-subs",
            "--embed-subs",
            "--sub-langs",
            "ja,en.*",
            "--compat-options",
            "no-keep-subs",
            "--embed-thumbnail",
            "--embed-metadata",
            "--embed-chapters",
        ]));
        assert_eq!(download_args(&tools(), &options), expected);
    }

    #[test]
    fn embedded_subtitles_can_be_kept() {
        let mut options = base_options(Preset::Best);
        options.embed = Embed {
            subtitles: true,
            subtitle_langs: vec!["ja".to_owned(), "en.*".to_owned()],
            keep_subtitle_files: true,
            thumbnail: false,
            metadata: false,
            chapters: false,
        };
        let mut expected = download_prefix();
        expected.extend(strings(&[
            "-S",
            "res,ext:mp4:m4a",
            "--merge-output-format",
            "mp4",
            "--write-subs",
            "--embed-subs",
            "--sub-langs",
            "ja,en.*",
        ]));
        assert_eq!(download_args(&tools(), &options), expected);
    }

    #[test]
    fn comments_disable_subtitle_embedding() {
        let mut options = base_options(Preset::Best);
        options.embed.subtitles = true;
        options.embed.subtitle_langs = vec!["ja".to_owned()];
        options.comments = CommentsSource::Niconico;
        let mut expected = download_prefix();
        expected.extend(strings(&[
            "-S",
            "res,ext:mp4:m4a",
            "--merge-output-format",
            "mp4",
            "--write-subs",
            "--sub-langs",
            "comments",
        ]));
        assert_eq!(download_args(&tools(), &options), expected);
        assert!(!expected.contains(&OsString::from("--embed-subs")));
    }

    #[test]
    fn audio_presets_never_embed_subtitles() {
        for preset in [Preset::AudioM4a, Preset::AudioMp3] {
            let mut options = base_options(preset);
            options.embed.subtitles = true;
            options.embed.subtitle_langs = vec!["ja".to_owned()];
            assert!(!download_args(&tools(), &options).contains(&OsString::from("--embed-subs")));
        }
    }

    #[test]
    fn comments_args_are_exact_and_none_is_an_error() {
        let mut expected = common();
        expected.extend(strings(&[
            "--skip-download",
            "--write-subs",
            "--sub-langs",
            "danmaku",
            "-o",
            r"C:\動画 output\%(title)s.%(ext)s",
            "--cookies",
            r"C:\秘密 folder\cookies.txt",
            "--",
            "https://example.test/video",
        ]));
        assert_eq!(
            comments_args(
                &tools(),
                "https://example.test/video",
                r"C:\動画 output\%(title)s.%(ext)s",
                CommentsSource::Bilibili,
                &Cookies::File(PathBuf::from(r"C:\秘密 folder\cookies.txt")),
            ),
            Ok(expected)
        );
        assert_eq!(
            comments_args(
                &tools(),
                "https://example.test/video",
                "%(title)s",
                CommentsSource::None,
                &Cookies::None,
            ),
            Err(ArgsError::MissingCommentsSource)
        );
    }

    #[test]
    fn comments_only_preset_has_the_stable_serde_name() {
        assert_eq!(
            serde_json::to_string(&Preset::CommentsOnly).unwrap(),
            "\"comments_only\""
        );
        assert_eq!(
            serde_json::from_str::<Preset>("\"comments_only\"").unwrap(),
            Preset::CommentsOnly
        );
    }

    #[test]
    fn page_media_probe_and_download_send_referer_and_origin() {
        let headers = PageHeaders {
            referer: "https://page.example/watch/1".to_owned(),
            origin: "https://page.example".to_owned(),
        };
        let probe = ProbeOptions {
            flat_playlist: false,
            playlist_items: None,
            cookies: Cookies::None,
            format_selection: None,
            avoid_av1: false,
            comment_subtitles: false,
        };
        let probe = probe_args_with_headers(
            &tools(),
            "https://cdn.example/master.m3u8",
            &probe,
            Some(&headers),
        );
        assert!(
            probe
                .windows(2)
                .any(|pair| pair == strings(&["--referer", "https://page.example/watch/1"]))
        );
        assert!(
            probe
                .windows(2)
                .any(|pair| pair == strings(&["--add-header", "Origin:https://page.example"]))
        );

        let download =
            download_args_with_headers(&tools(), &base_options(Preset::Best), Some(&headers));
        assert!(
            download
                .windows(2)
                .any(|pair| pair == strings(&["--referer", "https://page.example/watch/1"]))
        );
        assert!(
            download
                .windows(2)
                .any(|pair| pair == strings(&["--add-header", "Origin:https://page.example"]))
        );
    }
}

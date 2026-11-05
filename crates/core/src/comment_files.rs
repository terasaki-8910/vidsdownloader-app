//! Fetch-independent comment file naming, staging, counting and conflict handling.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use regex::Regex;

use crate::args::CommentsSource;
use crate::comments::{self, AssSettings, VideoSize};

/// The queue's comments-only preset reports this exact text for an unsupported site. The UI
/// translates it, just as it does the executor's filter skip reasons.
pub const COMMENTS_ONLY_UNSUPPORTED: &str = "Only niconico and Bilibili have comments to extract";

/// The comment source of a supported video page URL.
pub fn source_for_url(url: &str) -> Option<CommentsSource> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host == "nicovideo.jp" || host.ends_with(".nicovideo.jp") {
        Some(CommentsSource::Niconico)
    } else if host == "bilibili.com" || host.ends_with(".bilibili.com") {
        Some(CommentsSource::Bilibili)
    } else {
        None
    }
}

/// The comment source named by a library site's extractor key or friendly name.
pub fn source_for_site(site: Option<&str>) -> Option<CommentsSource> {
    let site = site?.to_ascii_lowercase();
    if site == "niconico" || site.contains("ニコニコ") {
        Some(CommentsSource::Niconico)
    } else if site.starts_with("bilibili") {
        Some(CommentsSource::Bilibili)
    } else {
        None
    }
}

/// Turn a supported id found in a video file name into its watch-page URL.
///
/// niconico ids are `sm`, `nm` or `so` followed by digits and Bilibili ids are `BV` followed
/// by exactly ten ASCII alphanumerics. Either must be a whole word (`[sm9]`, `sm9`, and
/// `..._BV1xx411c7mD...` are typical); a longer alphanumeric run is not an id.
pub fn video_url_from_file_name(name: &str) -> Option<String> {
    static NICONICO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:^|[^a-z0-9])((?:sm|nm|so)[0-9]+)(?:[^a-z0-9]|$)")
            .expect("static niconico id regex")
    });
    static BILIBILI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:^|[^A-Za-z0-9])(BV[A-Za-z0-9]{10})(?:[^A-Za-z0-9]|$)")
            .expect("static Bilibili id regex")
    });
    if let Some(captures) = NICONICO.captures(name) {
        return Some(format!(
            "https://www.nicovideo.jp/watch/{}",
            captures[1].to_ascii_lowercase()
        ));
    }
    BILIBILI
        .captures(name)
        .map(|captures| format!("https://www.bilibili.com/video/{}", captures[1].to_owned()))
}

/// The suffix of the raw comments document.
pub fn raw_suffix(source: CommentsSource) -> Option<&'static str> {
    match source {
        CommentsSource::None => None,
        CommentsSource::Niconico => Some(".comments.json"),
        CommentsSource::Bilibili => Some(".danmaku.xml"),
    }
}

/// Where the raw comments and rendered ASS belong beside `video_path`.
pub fn destination_paths(video_path: &Path, source: CommentsSource) -> Option<(PathBuf, PathBuf)> {
    Some((
        sibling_with_suffix(video_path, raw_suffix(source)?),
        sibling_with_suffix(video_path, ".comments.ass"),
    ))
}

/// A newly fetched raw document and its ASS, still in a disposable staging folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedComments {
    pub raw_path: PathBuf,
    pub ass_path: PathBuf,
    /// Number parsed from the raw document, before ASS limits or filters.
    pub count: usize,
}

/// Parse a raw file yt-dlp wrote as `fetch.comments.json` / `fetch.danmaku.xml` in
/// `scratch`, and write `fetch.comments.ass` there with a UTF-8 BOM.
pub fn stage_raw_file(
    scratch: &Path,
    source: CommentsSource,
    settings: &AssSettings,
    video: VideoSize,
) -> Result<FetchedComments, String> {
    let raw_path = scratch.join(format!(
        "fetch{}",
        raw_suffix(source).ok_or_else(|| "comments source is not supported".to_owned())?
    ));
    let raw = fs::read_to_string(&raw_path)
        .map_err(|error| format!("could not read comments {}: {error}", raw_path.display()))?;
    let parsed = match source {
        CommentsSource::Niconico => comments::parse_niconico(&raw),
        CommentsSource::Bilibili => comments::parse_bilibili(&raw),
        CommentsSource::None => unreachable!("the source was checked above"),
    }
    .map_err(|error| format!("could not parse comments {}: {error}", raw_path.display()))?;
    let ass = comments::to_ass(&parsed, settings, video);
    let ass_path = scratch.join("fetch.comments.ass");
    let mut bytes = Vec::with_capacity(3 + ass.text.len());
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(ass.text.as_bytes());
    fs::write(&ass_path, bytes).map_err(|error| {
        format!(
            "could not write ASS comments {}: {error}",
            ass_path.display()
        )
    })?;
    Ok(FetchedComments {
        raw_path,
        ass_path,
        count: parsed.len(),
    })
}

/// A same-name destination that needs the user's decision before it is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentConflict {
    pub existing_count: usize,
    /// Unix seconds of the raw file's modification time, else the ASS's, when available.
    pub existing_modified: Option<i64>,
    pub new_count: usize,
}

/// Detect either destination file already existing. An unreadable raw document counts zero.
pub fn conflict(
    video_path: &Path,
    source: CommentsSource,
    new_count: usize,
) -> Option<CommentConflict> {
    let (raw, ass) = destination_paths(video_path, source)?;
    if !raw.exists() && !ass.exists() {
        return None;
    }
    let existing_modified = [&raw, &ass].into_iter().find_map(|path| {
        fs::metadata(path)
            .ok()?
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_secs()).ok())
    });
    Some(CommentConflict {
        existing_count: count_raw_file(&raw, source),
        existing_modified,
        new_count,
    })
}

/// Count comments in a raw JSON / XML document. Missing, unreadable and malformed files all
/// count zero, which is also the conflict dialog's required behavior.
pub fn count_raw_file(path: &Path, source: CommentsSource) -> usize {
    let Ok(raw) = fs::read_to_string(path) else {
        return 0;
    };
    match source {
        CommentsSource::Niconico => comments::parse_niconico(&raw),
        CommentsSource::Bilibili => comments::parse_bilibili(&raw),
        CommentsSource::None => return 0,
    }
    .map_or(0, |parsed| parsed.len())
}

/// Copy staged files beside the video under their final names. With `replace == false`, any
/// existing raw or ASS destination refuses the whole operation. With `replace == true`, both
/// are overwritten. Copying rather than renaming also works when the app's work folder and
/// the video are on different volumes.
pub fn install(
    fetched: &FetchedComments,
    video_path: &Path,
    source: CommentsSource,
    replace: bool,
) -> io::Result<(PathBuf, PathBuf)> {
    let (raw, ass) = destination_paths(video_path, source).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "comments source is not supported",
        )
    })?;
    if !replace && (raw.exists() || ass.exists()) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a same-name comments file exists",
        ));
    }
    if let Some(parent) = raw.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&fetched.raw_path, &raw)?;
    fs::copy(&fetched.ass_path, &ass)?;
    Ok((raw, ass))
}

/// Remove staged files. The caller removes their now-empty token folder.
pub fn discard(fetched: &FetchedComments) {
    for path in [&fetched.raw_path, &fetched.ass_path] {
        if let Err(error) = fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "could not remove staged comments {}: {error}",
                path.display()
            );
        }
    }
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_stem().map_or_else(OsString::new, OsString::from);
    name.push(suffix);
    path.with_file_name(name)
}

/// Whether a path has one of the video extensions offered by the file picker.
pub fn is_supported_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            ["mp4", "m4v", "webm", "mkv", "mov", "flv"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(source: CommentsSource, count: usize) -> String {
        match source {
            CommentsSource::Niconico => (0..count)
                .map(|index| format!(r#"{{"vposMs":{index},"body":"comment {index}"}}"#))
                .collect::<Vec<_>>()
                .join(",")
                .pipe(|items| format!("[{items}]")),
            CommentsSource::Bilibili => format!(
                "<i>{}</i>",
                (0..count)
                    .map(|index| format!(r#"<d p="0,1,25,16777215">comment {index}</d>"#))
                    .collect::<Vec<_>>()
                    .join("")
            ),
            CommentsSource::None => unreachable!(),
        }
    }

    trait Pipe: Sized {
        fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
            f(self)
        }
    }
    impl<T> Pipe for T {}

    #[test]
    fn ids_are_read_from_file_names_as_whole_words() {
        for (name, id) in [
            ("video [sm9].mp4", "sm9"),
            ("sm9.mkv", "sm9"),
            ("prefix_nm123-suffix.webm", "nm123"),
            ("SO456.mov", "so456"),
        ] {
            assert_eq!(
                video_url_from_file_name(name).as_deref(),
                Some(format!("https://www.nicovideo.jp/watch/{id}").as_str()),
                "{name}"
            );
        }
        assert_eq!(
            video_url_from_file_name("clip [BV1xx411c7mD].mp4").as_deref(),
            Some("https://www.bilibili.com/video/BV1xx411c7mD")
        );
        for name in [
            "clip.mp4",
            "xsm9.mp4",
            "sm9x.mp4",
            "BV1xx411c7m.mp4",
            "bv1xx411c7mD.mp4",
        ] {
            assert_eq!(video_url_from_file_name(name), None, "{name}");
        }
    }

    #[test]
    fn niconico_and_bilibili_files_are_placed_under_the_video_stem() {
        for (source, raw_name) in [
            (CommentsSource::Niconico, "video.comments.json"),
            (CommentsSource::Bilibili, "video.danmaku.xml"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let scratch = temp.path().join("scratch");
            fs::create_dir(&scratch).unwrap();
            let video = temp.path().join("video.mp4");
            fs::write(&video, b"video").unwrap();
            fs::write(
                scratch.join(format!("fetch{}", raw_suffix(source).unwrap())),
                raw(source, 2),
            )
            .unwrap();
            let fetched = stage_raw_file(
                &scratch,
                source,
                &AssSettings::default(),
                VideoSize {
                    width: 640,
                    height: 360,
                    duration_ms: None,
                },
            )
            .unwrap();
            let (raw_path, ass_path) = install(&fetched, &video, source, false).unwrap();
            assert_eq!(raw_path.file_name().and_then(OsStr::to_str), Some(raw_name));
            assert_eq!(
                ass_path.file_name().and_then(OsStr::to_str),
                Some("video.comments.ass")
            );
            assert_eq!(count_raw_file(&raw_path, source), 2);
            assert!(fs::read(ass_path).unwrap().starts_with(&[0xEF, 0xBB, 0xBF]));
        }
    }

    #[test]
    fn conflicts_leave_existing_files_untouched_and_resolve_both_ways() {
        let temp = tempfile::tempdir().unwrap();
        let video = temp.path().join("video.mp4");
        fs::write(&video, b"video").unwrap();
        let (existing_raw, existing_ass) =
            destination_paths(&video, CommentsSource::Niconico).unwrap();
        fs::write(&existing_raw, raw(CommentsSource::Niconico, 3)).unwrap();
        fs::write(&existing_ass, b"old ass").unwrap();

        let scratch = temp.path().join("scratch");
        fs::create_dir(&scratch).unwrap();
        fs::write(
            scratch.join("fetch.comments.json"),
            raw(CommentsSource::Niconico, 5),
        )
        .unwrap();
        let fetched = stage_raw_file(
            &scratch,
            CommentsSource::Niconico,
            &AssSettings::default(),
            VideoSize {
                width: 640,
                height: 360,
                duration_ms: None,
            },
        )
        .unwrap();
        let found = conflict(&video, CommentsSource::Niconico, fetched.count).unwrap();
        assert_eq!(found.existing_count, 3);
        assert_eq!(found.new_count, 5);
        assert!(found.existing_modified.is_some());
        assert_eq!(
            fs::read_to_string(&existing_raw).unwrap(),
            raw(CommentsSource::Niconico, 3)
        );
        assert!(install(&fetched, &video, CommentsSource::Niconico, false).is_err());

        install(&fetched, &video, CommentsSource::Niconico, true).unwrap();
        assert_eq!(count_raw_file(&existing_raw, CommentsSource::Niconico), 5);
        assert_ne!(fs::read(existing_ass).unwrap(), b"old ass");

        // Keeping the existing pair simply leaves both files as they are; the caller discards
        // the staged pair. Check that behavior with another newly staged document.
        fs::write(
            scratch.join("fetch.comments.json"),
            raw(CommentsSource::Niconico, 1),
        )
        .unwrap();
        let newer = stage_raw_file(
            &scratch,
            CommentsSource::Niconico,
            &AssSettings::default(),
            VideoSize {
                width: 640,
                height: 360,
                duration_ms: None,
            },
        )
        .unwrap();
        let before = fs::read(&existing_raw).unwrap();
        discard(&newer);
        assert_eq!(fs::read(&existing_raw).unwrap(), before);
        assert!(!newer.raw_path.exists());
        assert!(!newer.ass_path.exists());
    }

    #[test]
    fn an_unreadable_existing_raw_file_counts_zero() {
        let temp = tempfile::tempdir().unwrap();
        let video = temp.path().join("video.mp4");
        let (raw_path, _) = destination_paths(&video, CommentsSource::Niconico).unwrap();
        fs::write(&raw_path, b"not json").unwrap();
        assert_eq!(count_raw_file(&raw_path, CommentsSource::Niconico), 0);
        assert_eq!(
            conflict(&video, CommentsSource::Niconico, 2)
                .unwrap()
                .existing_count,
            0
        );
    }
}

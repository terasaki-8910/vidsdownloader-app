//! The files that make up a saved item: the main file and the side files saved next to it
//! (comments, subtitles, thumbnail), and renaming them together.
//!
//! Side files are found by name only: a regular file in the same folder whose name is the
//! main file's exact stem followed by a known suffix. Nothing here reads the library; the
//! caller adds the library row's own comments file and leaves out files that are another
//! library row's main file.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::naming::TargetOs;

/// Extensions of video and audio files; only these get a thumbnail side file.
const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "webm", "mkv", "mov", "flv", "m4a", "mp3", "opus", "ogg", "wav", "flac", "aac",
];
/// Extensions of subtitle files written as `<stem>.<lang>.<ext>`.
const SUBTITLE_EXTENSIONS: &[&str] = &[
    "vtt", "srt", "ass", "ssa", "ttml", "srv1", "srv2", "srv3", "json3", "lrc",
];
/// Extensions of thumbnail files written as `<stem>.<ext>`.
const THUMBNAIL_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
/// Comment files written next to a video (raw niconico / Bilibili comments and the ASS
/// made from them).
const COMMENT_SUFFIXES: &[&str] = &[".comments.json", ".comments.ass", ".danmaku.xml"];
/// Longest subtitle language tag accepted in `<stem>.<lang>.<ext>`.
const MAX_LANGUAGE_LEN: usize = 20;
/// Longest file name, in UTF-8 bytes.
const MAX_NAME_BYTES: usize = 255;
/// Longest full path on Windows, in UTF-16 units (MAX_PATH without the terminating NUL).
const MAX_WINDOWS_PATH_UNITS: usize = 259;
/// Device names Windows reserves, with or without an extension, in any case.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
/// Characters no file name may hold (Windows' set, refused on every OS so names stay
/// portable).
const FORBIDDEN_CHARACTERS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// The side files of the saved file at `path`, sorted: existing regular files in the same
/// folder (links and folders are never included) named the main file's exact stem followed
/// by `.comments.json`, `.comments.ass` or `.danmaku.xml`; a subtitle `.<lang>.<ext>`
/// (`<lang>` 1 to 20 of `A-Z a-z 0-9 _ -`, `<ext>` one of vtt, srt, ass, ssa, ttml, srv1,
/// srv2, srv3, json3, lrc); or, when the main file is a video or audio file, a thumbnail
/// `.<ext>` (jpg, jpeg, png, webp). Extensions match in any case, the stem exactly. The
/// main file itself is never included. An unreadable folder has no side files.
pub fn side_files(path: &Path) -> Vec<PathBuf> {
    let (Some(folder), Some(main_name), Some(stem)) = (
        path.parent(),
        path.file_name().and_then(OsStr::to_str),
        path.file_stem().and_then(OsStr::to_str),
    ) else {
        return Vec::new();
    };
    let media = path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| contains_ignore_case(MEDIA_EXTENSIONS, extension));
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        // Does not follow links, and reports a Windows junction as a link too.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == main_name {
            continue;
        }
        if name
            .strip_prefix(stem)
            .is_some_and(|suffix| is_side_suffix(suffix, media))
        {
            found.push(folder.join(name));
        }
    }
    found.sort();
    found
}

/// [`side_files`] plus the library row's own comments file when it exists as a regular
/// file, even if its name does not follow the side-file pattern. Never the main file.
pub fn item_side_files(path: &Path, comments_path: Option<&Path>) -> Vec<PathBuf> {
    let mut files = side_files(path);
    if let Some(comments) = comments_path
        && fs::symlink_metadata(comments).is_ok_and(|metadata| metadata.is_file())
        && !same_path(comments, path)
        && !files.iter().any(|file| same_path(file, comments))
    {
        files.push(comments.to_path_buf());
        files.sort();
    }
    files
}

/// Whether two paths name the same file as this OS spells paths: on Windows separators
/// (`/` and `\`) and ASCII/Unicode case do not matter; elsewhere the components must be
/// equal. The file system is not consulted.
pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let normalize = |path: &Path| path.to_string_lossy().replace('/', "\\").to_lowercase();
        normalize(a) == normalize(b)
    } else {
        a == b
    }
}

fn is_side_suffix(suffix: &str, media: bool) -> bool {
    let lower = suffix.to_ascii_lowercase();
    if COMMENT_SUFFIXES.contains(&lower.as_str()) {
        return true;
    }
    let Some(rest) = lower.strip_prefix('.') else {
        return false;
    };
    match rest.split_once('.') {
        Some((language, extension)) => {
            is_language_tag(language) && SUBTITLE_EXTENSIONS.contains(&extension)
        }
        None => media && THUMBNAIL_EXTENSIONS.contains(&rest),
    }
}

fn is_language_tag(value: &str) -> bool {
    (1..=MAX_LANGUAGE_LEN).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn contains_ignore_case(list: &[&str], value: &str) -> bool {
    list.iter().any(|item| item.eq_ignore_ascii_case(value))
}

/// Why a saved item could not be renamed. Nothing was left renamed.
#[derive(Debug, thiserror::Error)]
pub enum RenameError {
    /// The new name is empty once surrounding spaces are trimmed.
    #[error("the new name is empty")]
    Empty,
    /// `.` and `..` name folders, not files.
    #[error("\"{0}\" cannot be used as a file name")]
    DotName(String),
    /// A character no file name may hold.
    #[error("the new name contains a character file names cannot hold: {0:?}")]
    ForbiddenCharacter(char),
    /// Windows drops a trailing dot or space from file names.
    #[error("the new name ends with a dot or a space")]
    TrailingDotOrSpace,
    /// A device name Windows reserves (CON, PRN, AUX, NUL, COM1-9, LPT1-9).
    #[error("\"{0}\" is a device name Windows reserves")]
    ReservedName(String),
    /// A new file name (the main file's or a side file's) is longer than 255 bytes.
    #[error("the new file name {0:?} is longer than 255 bytes")]
    NameTooLong(String),
    /// A new full path is longer than Windows' 259 characters.
    #[error("the new path {} is longer than 259 characters", .0.display())]
    PathTooLong(PathBuf),
    /// The new name is the current one.
    #[error("the new name is the same as the current one")]
    Unchanged,
    /// The file to rename has no usable name.
    #[error("{} has no file name that can be renamed", .0.display())]
    NoFileName(PathBuf),
    /// A file the rename would create already exists.
    #[error("a file named {} already exists", .0.display())]
    TargetExists(PathBuf),
    /// A rename failed; the ones already done were undone (`undo_failed` lists those that
    /// could not be, as "new -> old: error").
    #[error("{}", rename_failure_message(.from, .to, .source, .undo_failed))]
    Io {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
        undo_failed: Vec<String>,
    },
}

fn rename_failure_message(
    from: &Path,
    to: &Path,
    source: &io::Error,
    undo_failed: &[String],
) -> String {
    let mut message = format!(
        "could not rename {} to {}: {source}",
        from.display(),
        to.display()
    );
    if !undo_failed.is_empty() {
        message.push_str("; could not rename back: ");
        message.push_str(&undo_failed.join("; "));
    }
    message
}

/// One file to rename, from `from` to `to` (same folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMove {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Check a new file name typed by the user (without extension) and return it trimmed of
/// surrounding whitespace. Refused: empty, `.` or `..`, any of `/ \ : * ? " < > |` or a
/// control character, a trailing dot or space, and a Windows reserved device name (CON,
/// PRN, AUX, NUL, COM1-9, LPT1-9, with or without an extension, any case).
pub fn validate_new_stem(name: &str) -> Result<&str, RenameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RenameError::Empty);
    }
    if name == "." || name == ".." {
        return Err(RenameError::DotName(name.to_owned()));
    }
    if let Some(character) = name
        .chars()
        .find(|character| FORBIDDEN_CHARACTERS.contains(character) || character.is_control())
    {
        return Err(RenameError::ForbiddenCharacter(character));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(RenameError::TrailingDotOrSpace);
    }
    // Windows also treats "CON .txt" as CON: spaces before the first dot do not count.
    let base = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    if contains_ignore_case(RESERVED_NAMES, base) {
        return Err(RenameError::ReservedName(base.to_owned()));
    }
    Ok(name)
}

/// Plan renaming the saved file `main` to the stem `new_name` (no extension; the extension
/// and folder stay) together with its `side_files`, which keep their suffix after the new
/// stem. A side file whose name does not start with the main file's stem keeps its name and
/// is left out of the plan. The main file's move comes first.
///
/// Refused when `new_name` fails [`validate_new_stem`] or equals the current stem, when a
/// new file name is longer than 255 bytes (UTF-8) or, for [`TargetOs::Windows`], a new full
/// path longer than 259 UTF-16 units, or when any target already exists (a change of case
/// only, the same file on a case-insensitive disk, is allowed).
pub fn plan_rename(
    main: &Path,
    side_files: &[PathBuf],
    new_name: &str,
    target: TargetOs,
) -> Result<Vec<FileMove>, RenameError> {
    let new_stem = validate_new_stem(new_name)?;
    let old_stem = main
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| RenameError::NoFileName(main.to_path_buf()))?;
    if new_stem == old_stem {
        return Err(RenameError::Unchanged);
    }
    let new_main_name = match main.extension() {
        Some(extension) => {
            let extension = extension
                .to_str()
                .ok_or_else(|| RenameError::NoFileName(main.to_path_buf()))?;
            format!("{new_stem}.{extension}")
        }
        None => new_stem.to_owned(),
    };
    let mut moves = vec![FileMove {
        from: main.to_path_buf(),
        to: main.with_file_name(new_main_name),
    }];
    for side in side_files {
        let Some(suffix) = side
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|name| name.strip_prefix(old_stem))
        else {
            continue;
        };
        moves.push(FileMove {
            from: side.clone(),
            to: side.with_file_name(format!("{new_stem}{suffix}")),
        });
    }

    for planned in &moves {
        let name = planned
            .to
            .file_name()
            .map(OsStr::to_string_lossy)
            .unwrap_or_default();
        if name.len() > MAX_NAME_BYTES {
            return Err(RenameError::NameTooLong(name.into_owned()));
        }
        if target == TargetOs::Windows
            && planned.to.to_string_lossy().encode_utf16().count() > MAX_WINDOWS_PATH_UNITS
        {
            return Err(RenameError::PathTooLong(planned.to.clone()));
        }
    }
    for planned in &moves {
        if target_taken(planned) {
            return Err(RenameError::TargetExists(planned.to.clone()));
        }
    }
    Ok(moves)
}

/// Carry out a plan from [`plan_rename`] in order. Each target is checked again right
/// before its rename. When a rename fails, the ones already done are renamed back (last
/// first) and the error says which, if any, could not be.
pub fn apply_renames(moves: &[FileMove]) -> Result<(), RenameError> {
    for (index, planned) in moves.iter().enumerate() {
        let result = if target_taken(planned) {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the target already exists",
            ))
        } else {
            fs::rename(&planned.from, &planned.to)
        };
        if let Err(source) = result {
            return Err(RenameError::Io {
                from: planned.from.clone(),
                to: planned.to.clone(),
                source,
                undo_failed: undo_renames(&moves[..index]),
            });
        }
    }
    Ok(())
}

/// Rename `done` back, last first. Returns the ones that could not be, as
/// "new -> old: error".
pub fn undo_renames(done: &[FileMove]) -> Vec<String> {
    let mut failed = Vec::new();
    for planned in done.iter().rev() {
        if let Err(error) = fs::rename(&planned.to, &planned.from) {
            failed.push(format!(
                "{} -> {}: {error}",
                planned.to.display(),
                planned.from.display()
            ));
        }
    }
    failed
}

/// Whether the target of `planned` exists as a different file than its source.
fn target_taken(planned: &FileMove) -> bool {
    if fs::symlink_metadata(&planned.to).is_err() {
        return false;
    }
    // A change of case only: on a case-insensitive disk the "target" is the source itself.
    match (
        fs::canonicalize(&planned.from),
        fs::canonicalize(&planned.to),
    ) {
        (Ok(from), Ok(to)) => from != to,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_suffixes() {
        for suffix in [
            ".comments.json",
            ".comments.ass",
            ".danmaku.xml",
            ".COMMENTS.ASS",
            ".ja.vtt",
            ".en-US.srt",
            ".live_chat.json3",
            ".zh-Hans.ass",
            ".a1234567890123456789.lrc",
            ".jpg",
            ".WEBP",
        ] {
            assert!(is_side_suffix(suffix, true), "{suffix}");
        }
        for suffix in [
            "",
            ".mp4",
            ".part2.mp4",
            " bar.comments.ass",
            ".en.txt",
            ".a12345678901234567890.vtt",
            "..vtt",
            ".ja.en.vtt",
            ".日本語.vtt",
            ".info.json",
            ".gif",
        ] {
            assert!(!is_side_suffix(suffix, true), "{suffix}");
        }
        assert!(!is_side_suffix(".jpg", false));
        assert!(is_side_suffix(".ja.vtt", false));
    }

    #[test]
    fn stems_are_validated_and_trimmed() {
        assert_eq!(validate_new_stem("  新しい名前  ").unwrap(), "新しい名前");
        assert_eq!(validate_new_stem("a.b").unwrap(), "a.b");
        assert_eq!(validate_new_stem("CONSOLE").unwrap(), "CONSOLE");
        assert_eq!(validate_new_stem("COM0").unwrap(), "COM0");
        assert!(matches!(validate_new_stem("   "), Err(RenameError::Empty)));
        assert!(matches!(
            validate_new_stem("."),
            Err(RenameError::DotName(_))
        ));
        assert!(matches!(
            validate_new_stem(".."),
            Err(RenameError::DotName(_))
        ));
        for bad in [
            "a/b", "a\\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "a\u{7}b",
        ] {
            assert!(
                matches!(
                    validate_new_stem(bad),
                    Err(RenameError::ForbiddenCharacter(_))
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            validate_new_stem("name."),
            Err(RenameError::TrailingDotOrSpace)
        ));
        for reserved in [
            "CON",
            "con",
            "Nul",
            "aux.txt",
            "COM1",
            "lpt9.tar.gz",
            "PRN .x",
        ] {
            assert!(
                matches!(
                    validate_new_stem(reserved),
                    Err(RenameError::ReservedName(_))
                ),
                "{reserved}"
            );
        }
    }

    #[test]
    fn same_path_follows_the_os() {
        assert!(same_path(Path::new("/a/b.mp4"), Path::new("/a/b.mp4")));
        assert!(!same_path(Path::new("/a/b.mp4"), Path::new("/a/c.mp4")));
        assert_eq!(
            same_path(Path::new(r"C:\A\B.mp4"), Path::new("c:/a/b.MP4")),
            cfg!(windows)
        );
    }
}

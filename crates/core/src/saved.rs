//! Finding a video that is already saved in a folder, by the `[ID]` its file name carries.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Deepest folder level searched; files directly in the destination are level 1.
const MAX_DEPTH: usize = 4;
/// Directory entries visited in one search, so a huge folder tree cannot stall a job.
const MAX_ENTRIES: usize = 50_000;
/// Extensions of finished media files. Partial downloads (`.part`, `.ytdl`, `.temp`,
/// `.tmp`) are deliberately absent.
const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "webm", "mkv", "mov", "flv", "m4a", "mp3", "opus", "ogg", "wav", "flac", "aac",
    "jpg", "jpeg", "png", "gif", "webp", "avif",
];

/// Search `dest_dir` for a finished media file whose name contains `[<id>]`.
///
/// Walks breadth first to four levels, visiting at most 50,000 directory entries. Symlinks
/// and junctions are never followed and unreadable folders are skipped. Returns the first
/// match with its modification time as Unix seconds. An `id` shorter than three characters,
/// or one made only of digits and shorter than six, matches too many unrelated files and
/// finds nothing.
pub fn find_saved_file(dest_dir: &Path, id: &str) -> Option<(PathBuf, Option<i64>)> {
    search(dest_dir, id, MAX_ENTRIES)
}

fn search(dest_dir: &Path, id: &str, max_entries: usize) -> Option<(PathBuf, Option<i64>)> {
    if id.chars().count() < 3 || (id.len() < 6 && id.bytes().all(|byte| byte.is_ascii_digit())) {
        return None;
    }
    let needle = format!("[{id}]");
    let mut visited = 0_usize;
    let mut folders = VecDeque::from([(dest_dir.to_owned(), 1_usize)]);
    while let Some((folder, depth)) = folders.pop_front() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        // Sorted so the first match is the same on every run.
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            visited += 1;
            if visited > max_entries {
                return None;
            }
            // Does not follow links, and reports a Windows junction as a link too.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    folders.push_back((entry.path(), depth + 1));
                }
            } else if kind.is_file()
                && is_saved_media(&entry.file_name().to_string_lossy(), &needle)
            {
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .and_then(|age| i64::try_from(age.as_secs()).ok());
                return Some((entry.path(), modified));
            }
        }
    }
    None
}

fn is_saved_media(name: &str, needle: &str) -> bool {
    name.contains(needle)
        && Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                MEDIA_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_stops_after_the_entry_limit() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.txt", "z [abc123].mp4"] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }

        assert!(search(dir.path(), "abc123", 4).is_some());
        assert!(
            search(dir.path(), "abc123", 3).is_none(),
            "the match is the fourth entry visited"
        );
    }
}

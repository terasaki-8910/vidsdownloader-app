//! Finding an already-saved video by the `[ID]` in its file name.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use tempfile::TempDir;
use vids_core::saved::find_saved_file;

fn touch(root: &Path, relative: &str) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"x").unwrap();
    path
}

fn found(dir: &TempDir, id: &str) -> Option<PathBuf> {
    find_saved_file(dir.path(), id).map(|(path, _)| path)
}

#[test]
fn finds_a_file_directly_in_the_folder_and_four_levels_down() {
    let dir = tempfile::tempdir().unwrap();
    let shallow = touch(dir.path(), "Title [abc123].mp4");
    let deep = touch(dir.path(), "a/b/c/Deep [deep456].mkv");

    assert_eq!(found(&dir, "abc123"), Some(shallow));
    assert_eq!(found(&dir, "deep456"), Some(deep));
}

#[test]
fn does_not_look_below_the_fourth_level() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "a/b/c/d/Too deep [deep456].mp4");

    assert_eq!(found(&dir, "deep456"), None);
}

#[test]
fn prefers_the_shallower_match() {
    let dir = tempfile::tempdir().unwrap();
    let deep = touch(dir.path(), "a/Same [abc123].mp4");
    assert_eq!(found(&dir, "abc123"), Some(deep));

    let shallow = touch(dir.path(), "Same [abc123].mp4");
    assert_eq!(found(&dir, "abc123"), Some(shallow));
}

#[test]
fn ignores_partial_downloads_and_other_file_types() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "Title [abc123].mp4.part");
    touch(dir.path(), "Title [abc123].f137.mp4.ytdl");
    touch(dir.path(), "Title [abc123].temp");
    touch(dir.path(), "Title [abc123].tmp");
    touch(dir.path(), "Title [abc123].info.json");
    touch(dir.path(), "Title [abc123]");
    assert_eq!(found(&dir, "abc123"), None);

    let finished = touch(dir.path(), "Title [abc123].MP4");
    assert_eq!(found(&dir, "abc123"), Some(finished));
}

#[test]
fn a_folder_named_like_a_video_is_not_a_match() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("Title [abc123].mp4")).unwrap();

    assert_eq!(found(&dir, "abc123"), None);
}

#[test]
fn the_id_must_be_the_whole_bracketed_text() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "Title [xabc].mp4");
    touch(dir.path(), "Title [abcx].mp4");
    touch(dir.path(), "Title abc.mp4");
    assert_eq!(found(&dir, "abc"), None);

    let exact = touch(dir.path(), "Title [abc].mp4");
    assert_eq!(found(&dir, "abc"), Some(exact));
}

#[test]
fn short_and_short_numeric_ids_find_nothing() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "Title [ab].mp4");
    touch(dir.path(), "Title [12345].mp4");
    touch(dir.path(), "Title [123456].mp4");
    touch(dir.path(), "Title [12a45].mp4");

    assert_eq!(found(&dir, "ab"), None);
    assert_eq!(found(&dir, ""), None);
    assert_eq!(
        found(&dir, "12345"),
        None,
        "five digits match unrelated files"
    );
    assert!(found(&dir, "123456").is_some());
    assert!(found(&dir, "12a45").is_some());
}

#[test]
fn returns_the_modification_time_in_unix_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let path = touch(dir.path(), "Title [abc123].mp4");
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        .unwrap();

    assert_eq!(
        find_saved_file(dir.path(), "abc123"),
        Some((path, Some(1_700_000_000)))
    );
}

#[test]
fn a_missing_folder_finds_nothing() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(find_saved_file(&dir.path().join("absent"), "abc123"), None);
}

#[cfg(unix)]
#[test]
fn does_not_follow_symlinked_folders() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    touch(outside.path(), "Title [abc123].mp4");
    std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();

    assert_eq!(found(&dir, "abc123"), None);
}

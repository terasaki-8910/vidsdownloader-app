//! A saved item's side files on disk, and renaming the item together with them.

use std::fs;
use std::path::{Path, PathBuf};

use vids_core::item_files::{
    FileMove, RenameError, apply_renames, item_side_files, plan_rename, side_files,
};
use vids_core::naming::TargetOs;

fn touch(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, name.as_bytes()).unwrap();
    path
}

fn names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn side_files_follow_the_exact_stem_and_known_suffixes() {
    let dir = tempfile::tempdir().unwrap();
    let main = touch(dir.path(), "Foo.mp4");
    for name in [
        "Foo.comments.json",
        "Foo.comments.ass",
        "Foo.danmaku.xml",
        "Foo.ja.vtt",
        "Foo.en-US.SRT",
        "Foo.live_chat.json3",
        "Foo.jpg",
        "Foo.webp",
        // Not side files.
        "Foo bar.comments.ass",
        "Foo.part2.mp4",
        "Foo.mkv",
        "Foo.en.txt",
        "Foo.info.json",
        "Foobar.jpg",
        "Other.comments.ass",
    ] {
        touch(dir.path(), name);
    }
    fs::create_dir(dir.path().join("Foo.en.vtt")).unwrap();

    assert_eq!(
        names(&side_files(&main)),
        vec![
            "Foo.comments.ass",
            "Foo.comments.json",
            "Foo.danmaku.xml",
            "Foo.en-US.SRT",
            "Foo.ja.vtt",
            "Foo.jpg",
            "Foo.live_chat.json3",
            "Foo.webp",
        ]
    );
}

#[test]
fn only_video_and_audio_files_get_a_thumbnail() {
    let dir = tempfile::tempdir().unwrap();
    let image = touch(dir.path(), "Post 1.png");
    touch(dir.path(), "Post 1.jpg");
    touch(dir.path(), "Post 1.ja.vtt");
    assert_eq!(names(&side_files(&image)), vec!["Post 1.ja.vtt"]);

    let audio = touch(dir.path(), "Song.M4A");
    touch(dir.path(), "Song.jpg");
    assert_eq!(names(&side_files(&audio)), vec!["Song.jpg"]);
}

#[test]
fn a_missing_folder_has_no_side_files() {
    let dir = tempfile::tempdir().unwrap();
    assert!(side_files(&dir.path().join("gone").join("Foo.mp4")).is_empty());
}

#[test]
fn the_library_comments_file_is_added_once() {
    let dir = tempfile::tempdir().unwrap();
    let main = touch(dir.path(), "Foo.mp4");
    let ass = touch(dir.path(), "Foo.comments.ass");
    let odd = touch(dir.path(), "comments for foo.ass");

    assert_eq!(
        names(&item_side_files(&main, Some(&ass))),
        vec!["Foo.comments.ass"]
    );
    assert_eq!(
        names(&item_side_files(&main, Some(&odd))),
        vec!["Foo.comments.ass", "comments for foo.ass"]
    );
    assert_eq!(
        names(&item_side_files(&main, Some(&dir.path().join("gone.ass")))),
        vec!["Foo.comments.ass"]
    );
    assert_eq!(
        names(&item_side_files(&main, Some(&main))),
        vec!["Foo.comments.ass"]
    );
}

#[test]
fn rename_moves_the_main_file_and_its_side_files() {
    let dir = tempfile::tempdir().unwrap();
    let main = touch(dir.path(), "Old name.mp4");
    touch(dir.path(), "Old name.comments.ass");
    touch(dir.path(), "Old name.ja.vtt");
    let odd = touch(dir.path(), "comments.ass");
    let mut sides = side_files(&main);
    sides.push(odd.clone());

    let moves = plan_rename(&main, &sides, "  新しい名前  ", TargetOs::Windows).unwrap();
    assert_eq!(
        moves[0],
        FileMove {
            from: main.clone(),
            to: dir.path().join("新しい名前.mp4"),
        }
    );
    assert_eq!(moves.len(), 3, "a side file not named after the stem stays");
    apply_renames(&moves).unwrap();

    let mut left: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        vec![
            "comments.ass",
            "新しい名前.comments.ass",
            "新しい名前.ja.vtt",
            "新しい名前.mp4",
        ]
    );
    assert_eq!(
        fs::read(dir.path().join("新しい名前.mp4")).unwrap(),
        b"Old name.mp4"
    );
}

#[test]
fn rename_refuses_bad_unchanged_and_taken_names() {
    let dir = tempfile::tempdir().unwrap();
    let main = touch(dir.path(), "Video.mp4");
    let ass = touch(dir.path(), "Video.comments.ass");
    touch(dir.path(), "Taken.mp4");
    touch(dir.path(), "Side taken.comments.ass");
    let sides = vec![ass];

    let refused = |name: &str| plan_rename(&main, &sides, name, TargetOs::Windows).unwrap_err();
    assert!(matches!(refused(""), RenameError::Empty));
    assert!(matches!(refused(".."), RenameError::DotName(_)));
    assert!(matches!(
        refused("a:b"),
        RenameError::ForbiddenCharacter(':')
    ));
    assert!(matches!(refused("name."), RenameError::TrailingDotOrSpace));
    assert!(matches!(refused("nul"), RenameError::ReservedName(_)));
    assert!(matches!(refused(" Video "), RenameError::Unchanged));
    assert!(matches!(refused("Taken"), RenameError::TargetExists(_)));
    assert!(matches!(
        refused("Side taken"),
        RenameError::TargetExists(_)
    ));
    assert!(matches!(
        refused(&"あ".repeat(85)),
        RenameError::NameTooLong(_)
    ));
    // Nothing was renamed.
    assert!(main.exists());
}

#[test]
fn windows_paths_are_limited_to_259_units() {
    let dir = tempfile::tempdir().unwrap();
    let main = touch(dir.path(), "v.mp4");
    let folder_units = dir.path().to_string_lossy().encode_utf16().count();
    // Folder + separator + stem + ".mp4" = 260 units: one too many on Windows only.
    let stem = "a".repeat(260 - folder_units - 1 - 4);
    assert!(matches!(
        plan_rename(&main, &[], &stem, TargetOs::Windows),
        Err(RenameError::PathTooLong(_))
    ));
    assert!(plan_rename(&main, &[], &stem[1..], TargetOs::Windows).is_ok());
    assert!(plan_rename(&main, &[], &stem, TargetOs::MacOs).is_ok());
}

#[test]
fn a_case_only_rename_is_allowed_where_the_disk_ignores_case() {
    let dir = tempfile::tempdir().unwrap();
    let main = touch(dir.path(), "video.mp4");
    let case_insensitive = dir.path().join("VIDEO.mp4").exists();

    let planned = plan_rename(&main, &[], "Video", TargetOs::Windows);
    assert!(planned.is_ok(), "{planned:?}");
    apply_renames(&planned.unwrap()).unwrap();
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["Video.mp4"],
        "case-insensitive disk: {case_insensitive}"
    );
}

#[test]
fn a_failed_side_rename_undoes_the_main_rename() {
    let dir = tempfile::tempdir().unwrap();
    let main = touch(dir.path(), "Clip.mp4");
    let ass = touch(dir.path(), "Clip.comments.ass");
    let vtt = touch(dir.path(), "Clip.ja.vtt");
    let moves = plan_rename(
        &main,
        &[ass.clone(), vtt.clone()],
        "Renamed",
        TargetOs::Windows,
    )
    .unwrap();
    // The last side file disappears between the plan and the renames.
    fs::remove_file(&vtt).unwrap();

    let error = apply_renames(&moves).unwrap_err();
    assert!(
        matches!(&error, RenameError::Io { from, undo_failed, .. }
            if *from == vtt && undo_failed.is_empty()),
        "{error:?}"
    );
    assert!(main.exists());
    assert!(ass.exists());
    assert!(!dir.path().join("Renamed.mp4").exists());
    assert!(!dir.path().join("Renamed.comments.ass").exists());
}

#[test]
fn a_target_created_after_the_plan_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let main = touch(dir.path(), "Clip.mp4");
    let ass = touch(dir.path(), "Clip.comments.ass");
    let moves = plan_rename(
        &main,
        std::slice::from_ref(&ass),
        "Renamed",
        TargetOs::Windows,
    )
    .unwrap();
    let late = touch(dir.path(), "Renamed.comments.ass");

    assert!(matches!(apply_renames(&moves), Err(RenameError::Io { .. })));
    assert_eq!(fs::read(&late).unwrap(), b"Renamed.comments.ass");
    assert!(main.exists());
    assert!(ass.exists());
}

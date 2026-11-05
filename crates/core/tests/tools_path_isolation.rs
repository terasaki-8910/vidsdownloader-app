//! Kept in its own test binary: it changes PATH, and no other test may run in the same
//! process while it does (environment changes race with every thread reading it).

use std::fs;

use tempfile::tempdir;
use vids_core::naming::TargetOs;
use vids_core::tools::{Tool, ToolManager};

#[test]
fn resolve_ignores_path() {
    let root = tempdir().unwrap();
    let path_tool = tempdir().unwrap();
    fs::write(path_tool.path().join("yt-dlp.exe"), b"from PATH").unwrap();
    let original = std::env::var_os("PATH");
    let joined = std::env::join_paths(std::iter::once(path_tool.path().to_path_buf()).chain(
        std::env::split_paths(original.as_deref().unwrap_or_default()),
    ))
    .unwrap();
    // SAFETY: this binary holds this single test, so no other thread reads or writes the
    // environment while PATH is changed and restored.
    unsafe { std::env::set_var("PATH", joined) };
    let resolved =
        ToolManager::new(root.path().to_path_buf()).resolve(Tool::YtDlp, TargetOs::Windows);
    match original {
        // SAFETY: as above.
        Some(value) => unsafe { std::env::set_var("PATH", value) },
        // SAFETY: as above.
        None => unsafe { std::env::remove_var("PATH") },
    }
    assert!(resolved.is_none());
}

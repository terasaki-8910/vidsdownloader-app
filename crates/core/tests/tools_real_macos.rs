//! Installs the pinned macOS ffmpeg from the network and runs it. Ignored by default; the
//! macOS CI job runs it with `--ignored` (this PC cannot run macOS binaries).
#![cfg(target_os = "macos")]

use std::process::{Command, Output};
use tempfile::tempdir;
use vids_core::naming::TargetOs;
use vids_core::tools::{Arch, Tool, ToolManager, ffmpeg_macos_source};

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
#[ignore = "downloads about 60 MB; run on macOS CI with --ignored"]
fn pinned_macos_ffmpeg_installs_runs_and_has_libass() {
    let arch = if cfg!(target_arch = "aarch64") {
        Arch::Aarch64
    } else {
        Arch::X86_64
    };
    let root = tempdir().unwrap();
    let manager = ToolManager::new(root.path().to_path_buf());
    manager
        .install(&ffmpeg_macos_source(arch), &mut |_| {})
        .unwrap();
    let dir = manager
        .resolve(Tool::Ffmpeg, TargetOs::MacOs)
        .expect("ffmpeg and ffprobe are installed")
        .path;

    let buildconf = Command::new(dir.join("ffmpeg"))
        .args(["-hide_banner", "-buildconf"])
        .output()
        .unwrap();
    let text = combined(&buildconf);
    assert!(buildconf.status.success(), "{text}");
    assert!(text.contains("--enable-libass"), "{text}");

    let version = Command::new(dir.join("ffprobe"))
        .args(["-hide_banner", "-version"])
        .output()
        .unwrap();
    assert!(version.status.success(), "{}", combined(&version));
}

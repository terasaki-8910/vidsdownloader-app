use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;
use tiny_http::{Response, Server};
use vids_core::naming::TargetOs;
use vids_core::tools::{
    Arch, Channel, ChecksumSource, FfmpegBuild, HttpBody, HttpClient, InstallProgress, Package,
    Tool, ToolAsset, ToolManager, ToolSource, ToolsError, UreqClient, ZipEntry, deno_source,
    ffmpeg_source, gallery_dl_source, ublock_lite_source, yt_dlp_source,
};
use zip::write::SimpleFileOptions;

struct MemoryClient {
    responses: HashMap<String, Vec<u8>>,
}

impl MemoryClient {
    fn new(responses: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        Self {
            responses: responses.into_iter().collect(),
        }
    }
}

impl HttpClient for MemoryClient {
    fn get(&self, url: &str) -> Result<HttpBody, ToolsError> {
        self.responses
            .get(url)
            .cloned()
            .map(|bytes| HttpBody {
                content_length: Some(bytes.len() as u64),
                reader: Box::new(Cursor::new(bytes)) as Box<dyn Read + Send>,
            })
            .ok_or_else(|| ToolsError::Http(format!("no fixture for {url}")))
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn fake_binary_source(tool: Tool, version: &str, url: &str, bytes: &[u8]) -> ToolSource {
    ToolSource {
        extra: Vec::new(),
        tool,
        version: version.to_owned(),
        url: url.to_owned(),
        checksum: ChecksumSource::Inline(sha256(bytes)),
        package: Package::Binary {
            file_name: match tool {
                Tool::YtDlp => "yt-dlp.exe",
                Tool::Deno => "deno.exe",
                Tool::Ffmpeg => "ffmpeg.exe",
                Tool::GalleryDl => "gallery-dl.exe",
                Tool::UblockLite => "manifest.json",
            }
            .to_owned(),
        },
    }
}

fn make_ffmpeg_zip() -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default();
    for (name, contents) in [
        ("ffmpeg-x/bin/ffmpeg.exe", b"ffmpeg".as_slice()),
        ("ffmpeg-x/bin/ffprobe.exe", b"ffprobe".as_slice()),
        ("ffmpeg-x/bin/a.dll", b"dll".as_slice()),
        ("ffmpeg-x/README.txt", b"readme".as_slice()),
    ] {
        archive.start_file(name, options).unwrap();
        archive.write_all(contents).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    for (name, contents) in entries {
        archive
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        archive.write_all(contents).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

fn ublock_fixture_source(version: &str, url: &str, archive: &[u8]) -> ToolSource {
    ToolSource {
        extra: Vec::new(),
        tool: Tool::UblockLite,
        version: version.to_owned(),
        url: url.to_owned(),
        checksum: ChecksumSource::Inline(sha256(archive)),
        package: Package::ZipTree {
            subdir: "ubol".to_owned(),
        },
    }
}

fn start_server(
    routes: Vec<(String, Vec<u8>)>,
) -> (String, thread::JoinHandle<Result<(), String>>) {
    let server = Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        let mut routes: HashMap<_, _> = routes.into_iter().collect();
        while !routes.is_empty() {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "timed out waiting for a request".to_owned())?;
            let user_agent = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("User-Agent"))
                .map(|header| header.value.as_str().to_string());
            if user_agent.as_deref() != Some("vidsdownloader/0.1") {
                return Err(format!("unexpected User-Agent: {user_agent:?}"));
            }
            let path = request.url().to_owned();
            let Some(body) = routes.remove(&path) else {
                request.respond(Response::empty(404)).unwrap();
                return Err(format!("unexpected request path: {path}"));
            };
            request.respond(Response::from_data(body)).unwrap();
        }
        Ok(())
    });
    (base, handle)
}

fn assert_progress(progress: &[InstallProgress], expected_length: usize) {
    assert!(!progress.is_empty());
    assert!(
        progress
            .windows(2)
            .all(|pair| pair[0].downloaded_bytes <= pair[1].downloaded_bytes)
    );
    // The server sends Content-Length, so every event carries the total.
    assert!(
        progress
            .iter()
            .all(|event| event.total_bytes == Some(expected_length as u64))
    );
    assert_eq!(
        progress.last().unwrap().downloaded_bytes,
        expected_length as u64
    );
}

#[test]
fn installs_binary_and_zip_with_real_http_client() {
    let root = tempdir().unwrap();
    let binary = b"fake executable".to_vec();
    let zip = make_ffmpeg_zip();
    let binary_checksum = format!("{}  yt-dlp.exe\n", sha256(&binary));
    let zip_checksum = format!("{} *ffmpeg.zip\n", sha256(&zip));
    let (base, server) = start_server(vec![
        ("/yt-dlp.exe".to_owned(), binary.clone()),
        ("/SHA2-256SUMS".to_owned(), binary_checksum.into_bytes()),
        ("/ffmpeg.zip".to_owned(), zip.clone()),
        ("/checksums.sha256".to_owned(), zip_checksum.into_bytes()),
    ]);
    let manager = ToolManager::with_client(root.path().to_path_buf(), Box::new(UreqClient));

    let binary_source = ToolSource {
        extra: Vec::new(),
        tool: Tool::YtDlp,
        version: "v1".to_owned(),
        url: format!("{base}/yt-dlp.exe"),
        checksum: ChecksumSource::Url {
            url: format!("{base}/SHA2-256SUMS"),
            entry_name: "yt-dlp.exe".to_owned(),
        },
        package: Package::Binary {
            file_name: "yt-dlp.exe".to_owned(),
        },
    };
    let mut binary_progress = Vec::new();
    manager
        .install(&binary_source, &mut |event| binary_progress.push(event))
        .unwrap();
    assert_progress(&binary_progress, binary.len());
    assert_eq!(
        fs::read(root.path().join("yt-dlp/v1/yt-dlp.exe")).unwrap(),
        binary
    );

    let zip_source = ToolSource {
        extra: Vec::new(),
        tool: Tool::Ffmpeg,
        version: "build-1".to_owned(),
        url: format!("{base}/ffmpeg.zip"),
        checksum: ChecksumSource::Url {
            url: format!("{base}/checksums.sha256"),
            entry_name: "ffmpeg.zip".to_owned(),
        },
        package: Package::Zip {
            entries: vec![
                ZipEntry {
                    pattern: "*/bin/ffmpeg.exe".to_owned(),
                },
                ZipEntry {
                    pattern: "*/bin/ffprobe.exe".to_owned(),
                },
                ZipEntry {
                    pattern: "*/bin/*.dll".to_owned(),
                },
            ],
        },
    };
    let mut zip_progress = Vec::new();
    manager
        .install(&zip_source, &mut |event| zip_progress.push(event))
        .unwrap();
    assert_progress(&zip_progress, zip.len());

    let ffmpeg_dir = root.path().join("ffmpeg/build-1");
    assert_eq!(fs::read(ffmpeg_dir.join("ffmpeg.exe")).unwrap(), b"ffmpeg");
    assert_eq!(
        fs::read(ffmpeg_dir.join("ffprobe.exe")).unwrap(),
        b"ffprobe"
    );
    assert_eq!(fs::read(ffmpeg_dir.join("a.dll")).unwrap(), b"dll");
    assert!(!ffmpeg_dir.join("README.txt").exists());

    let state: Value =
        serde_json::from_slice(&fs::read(root.path().join("tools.json")).unwrap()).unwrap();
    assert_eq!(state["tools"]["yt-dlp"]["current"]["version"], "v1");
    assert!(state["tools"]["yt-dlp"]["previous"].is_null());
    assert_eq!(state["tools"]["ffmpeg"]["current"]["version"], "build-1");
    assert!(state["tools"]["ffmpeg"]["previous"].is_null());
    server.join().unwrap().unwrap();
}

#[test]
fn checksum_comparison_is_case_insensitive() {
    let root = tempdir().unwrap();
    let bytes = b"binary".to_vec();
    let url = "memory://binary";
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        Box::new(MemoryClient::new([(url.to_owned(), bytes.clone())])),
    );
    let mut source = fake_binary_source(Tool::YtDlp, "v1", url, &bytes);
    source.checksum = ChecksumSource::Inline(sha256(&bytes).to_ascii_uppercase());
    manager.install(&source, &mut |_| {}).unwrap();
}

#[test]
fn entries_for_tools_this_build_does_not_know_are_kept_and_ignored() {
    let root = tempdir().unwrap();
    let url = "memory://yt-dlp";
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        Box::new(MemoryClient::new([(url.to_owned(), b"binary".to_vec())])),
    );
    manager
        .install(
            &fake_binary_source(Tool::YtDlp, "v1", url, b"binary"),
            &mut |_| {},
        )
        .unwrap();
    // A newer version of the app recorded a tool this build has never heard of.
    let path = root.path().join("tools.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["tools"]["future-tool"] = state["tools"]["yt-dlp"].clone();
    fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();

    assert!(manager.resolve(Tool::YtDlp, TargetOs::Windows).is_some());
    manager
        .install(
            &fake_binary_source(Tool::YtDlp, "v2", url, b"binary"),
            &mut |_| {},
        )
        .unwrap();
    let state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(state["tools"].get("future-tool").is_some());
    assert_eq!(state["tools"]["yt-dlp"]["current"]["version"], "v2");
}

#[test]
fn checksum_mismatch_leaves_no_install_or_temporary_download() {
    let root = tempdir().unwrap();
    let bytes = b"wrong bytes".to_vec();
    let (base, server) = start_server(vec![
        ("/tool".to_owned(), bytes),
        (
            "/checksum".to_owned(),
            format!("{}  yt-dlp.exe\n", "0".repeat(64)).into_bytes(),
        ),
    ]);
    let source = ToolSource {
        extra: Vec::new(),
        tool: Tool::YtDlp,
        version: "bad".to_owned(),
        url: format!("{base}/tool"),
        checksum: ChecksumSource::Url {
            url: format!("{base}/checksum"),
            entry_name: "yt-dlp.exe".to_owned(),
        },
        package: Package::Binary {
            file_name: "yt-dlp.exe".to_owned(),
        },
    };
    let manager = ToolManager::with_client(root.path().to_path_buf(), Box::new(UreqClient));
    let error = manager.install(&source, &mut |_| {}).unwrap_err();
    assert!(matches!(error, ToolsError::ChecksumMismatch { .. }));
    assert!(!root.path().join("tools.json").exists());
    assert!(!root.path().join("yt-dlp/bad").exists());
    let temporary_entries: Vec<_> = fs::read_dir(root.path().join(".tmp"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(temporary_entries.is_empty(), "{temporary_entries:?}");
    server.join().unwrap().unwrap();
}

#[test]
fn unmatched_zip_pattern_is_an_error_and_cleans_temporary_files() {
    let root = tempdir().unwrap();
    let zip = make_ffmpeg_zip();
    let url = "memory://archive";
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        Box::new(MemoryClient::new([(url.to_owned(), zip.clone())])),
    );
    let source = ToolSource {
        extra: Vec::new(),
        tool: Tool::Ffmpeg,
        version: "bad-pattern".to_owned(),
        url: url.to_owned(),
        checksum: ChecksumSource::Inline(sha256(&zip)),
        package: Package::Zip {
            entries: vec![ZipEntry {
                pattern: "*/bin/missing.exe".to_owned(),
            }],
        },
    };

    let error = manager.install(&source, &mut |_| {}).unwrap_err();
    assert!(matches!(
        error,
        ToolsError::ZipPatternNotFound { pattern } if pattern == "*/bin/missing.exe"
    ));
    assert!(!root.path().join("ffmpeg/bad-pattern").exists());
    assert!(!root.path().join("tools.json").exists());
    assert_eq!(fs::read_dir(root.path().join(".tmp")).unwrap().count(), 0);
}

#[test]
fn zip_tree_install_preserves_nested_paths() {
    let root = tempdir().unwrap();
    let archive = make_zip(&[
        ("manifest.json", br#"{"version":"test"}"#),
        ("_locales/en/messages.json", b"{}"),
        ("js/background.js", b"export {}"),
    ]);
    let url = "memory://ubol.zip";
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        Box::new(MemoryClient::new([(url.to_owned(), archive.clone())])),
    );
    let source = ublock_fixture_source("test", url, &archive);

    manager.install(&source, &mut |_| {}).unwrap();

    let installed = root.path().join("ublock-lite/test/ubol");
    assert_eq!(
        fs::read(installed.join("manifest.json")).unwrap(),
        br#"{"version":"test"}"#
    );
    assert_eq!(
        fs::read(installed.join("_locales/en/messages.json")).unwrap(),
        b"{}"
    );
    assert_eq!(
        fs::read(installed.join("js/background.js")).unwrap(),
        b"export {}"
    );
}

#[test]
fn zip_tree_refuses_parent_paths_and_cleans_staging() {
    let root = tempdir().unwrap();
    let archive = make_zip(&[("../outside.txt", b"escape")]);
    let url = "memory://unsafe-ubol.zip";
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        Box::new(MemoryClient::new([(url.to_owned(), archive.clone())])),
    );
    let source = ublock_fixture_source("unsafe", url, &archive);

    let error = manager.install(&source, &mut |_| {}).unwrap_err();

    assert!(matches!(error, ToolsError::UnsafeZipPath { .. }));
    assert!(!root.path().join("outside.txt").exists());
    assert!(!root.path().join("ublock-lite/unsafe").exists());
    assert_eq!(fs::read_dir(root.path().join(".tmp")).unwrap().count(), 0);
}

#[test]
fn zip_tree_wrong_digest_deletes_download_and_errors() {
    let root = tempdir().unwrap();
    let archive = make_zip(&[("manifest.json", b"{}")]);
    let url = "memory://bad-digest-ubol.zip";
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        Box::new(MemoryClient::new([(url.to_owned(), archive.clone())])),
    );
    let mut source = ublock_fixture_source("bad-digest", url, &archive);
    source.checksum = ChecksumSource::Inline("0".repeat(64));

    let error = manager.install(&source, &mut |_| {}).unwrap_err();

    assert!(matches!(error, ToolsError::ChecksumMismatch { .. }));
    assert!(!root.path().join("ublock-lite/bad-digest").exists());
    assert_eq!(fs::read_dir(root.path().join(".tmp")).unwrap().count(), 0);
}

#[test]
fn resolve_requires_each_tools_expected_managed_files() {
    let root = tempdir().unwrap();
    let yt_dir = root.path().join("yt-dlp/v1");
    let ffmpeg_dir = root.path().join("ffmpeg/v1");
    let deno_dir = root.path().join("deno/v1");
    let ublock_dir = root.path().join("ublock-lite/v1");
    fs::create_dir_all(&yt_dir).unwrap();
    fs::create_dir_all(&ffmpeg_dir).unwrap();
    fs::create_dir_all(&deno_dir).unwrap();
    fs::create_dir_all(ublock_dir.join("ubol")).unwrap();
    fs::write(yt_dir.join("yt-dlp.exe"), b"yt").unwrap();
    fs::write(ffmpeg_dir.join("ffmpeg.exe"), b"ffmpeg").unwrap();
    fs::write(ffmpeg_dir.join("ffprobe.exe"), b"ffprobe").unwrap();
    fs::write(deno_dir.join("deno.exe"), b"deno").unwrap();
    fs::write(ublock_dir.join("ubol/manifest.json"), b"{}").unwrap();
    let installed = |version_dir: &Path| {
        serde_json::json!({
            "version": "v1",
            "dir": version_dir,
            "sha256": "0".repeat(64),
            "source_url": "memory://fixture",
            "installed_at": 1
        })
    };
    let state = serde_json::json!({
        "tools": {
            "yt-dlp": { "current": installed(&yt_dir), "previous": null },
            "ffmpeg": { "current": installed(&ffmpeg_dir), "previous": null },
            "deno": { "current": installed(&deno_dir), "previous": null },
            "ublock-lite": { "current": installed(&ublock_dir), "previous": null }
        }
    });
    fs::write(
        root.path().join("tools.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let manager = ToolManager::new(root.path().to_path_buf());

    assert_eq!(
        manager
            .resolve(Tool::YtDlp, TargetOs::Windows)
            .unwrap()
            .path,
        yt_dir.join("yt-dlp.exe")
    );
    assert_eq!(
        manager
            .resolve(Tool::Ffmpeg, TargetOs::Windows)
            .unwrap()
            .path,
        ffmpeg_dir
    );
    assert_eq!(
        manager.resolve(Tool::Deno, TargetOs::Windows).unwrap().path,
        deno_dir.join("deno.exe")
    );
    assert_eq!(
        manager
            .resolve(Tool::UblockLite, TargetOs::Windows)
            .unwrap()
            .path,
        ublock_dir
    );
    assert!(manager.resolve(Tool::UblockLite, TargetOs::MacOs).is_none());
    assert!(manager.resolve(Tool::YtDlp, TargetOs::MacOs).is_none());
    fs::remove_file(deno_dir.join("deno.exe")).unwrap();
    fs::remove_file(ffmpeg_dir.join("ffprobe.exe")).unwrap();
    assert!(manager.resolve(Tool::Deno, TargetOs::Windows).is_none());
    assert!(manager.resolve(Tool::Ffmpeg, TargetOs::Windows).is_none());
    fs::remove_file(root.path().join("ublock-lite/v1/ubol/manifest.json")).unwrap();
    assert!(
        manager
            .resolve(Tool::UblockLite, TargetOs::Windows)
            .is_none()
    );
}

#[test]
fn rollback_and_new_install_keep_exactly_one_previous_version() {
    let root = tempdir().unwrap();
    let fixtures = [
        ("memory://v1".to_owned(), b"one".to_vec()),
        ("memory://v2".to_owned(), b"two".to_vec()),
        ("memory://v3".to_owned(), b"three".to_vec()),
    ];
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        Box::new(MemoryClient::new(fixtures.clone())),
    );

    let v1 = fake_binary_source(Tool::YtDlp, "v1", &fixtures[0].0, &fixtures[0].1);
    let v2 = fake_binary_source(Tool::YtDlp, "v2", &fixtures[1].0, &fixtures[1].1);
    let v3 = fake_binary_source(Tool::YtDlp, "v3", &fixtures[2].0, &fixtures[2].1);
    manager.install(&v1, &mut |_| {}).unwrap();
    assert_versions(root.path(), "v1", None);
    manager.install(&v2, &mut |_| {}).unwrap();
    assert_versions(root.path(), "v2", Some("v1"));
    assert!(root.path().join("yt-dlp/v1").is_dir());
    assert!(root.path().join("yt-dlp/v2").is_dir());

    let rolled_back = manager.rollback(Tool::YtDlp).unwrap();
    assert_eq!(rolled_back.version, "v1");
    assert_versions(root.path(), "v1", Some("v2"));

    manager.install(&v3, &mut |_| {}).unwrap();
    assert_versions(root.path(), "v3", Some("v1"));
    assert!(root.path().join("yt-dlp/v1").is_dir());
    assert!(!root.path().join("yt-dlp/v2").exists());
    assert!(root.path().join("yt-dlp/v3").is_dir());
}

fn assert_versions(root: &Path, current: &str, previous: Option<&str>) {
    let state: Value = serde_json::from_slice(&fs::read(root.join("tools.json")).unwrap()).unwrap();
    assert_eq!(state["tools"]["yt-dlp"]["current"]["version"], current);
    match previous {
        Some(version) => assert_eq!(state["tools"]["yt-dlp"]["previous"]["version"], version),
        None => assert!(state["tools"]["yt-dlp"]["previous"].is_null()),
    }
}

#[test]
fn official_yt_dlp_sources_are_exact() {
    for (channel, repository) in [
        (Channel::Stable, "yt-dlp/yt-dlp"),
        (Channel::Nightly, "yt-dlp/yt-dlp-nightly-builds"),
    ] {
        for (os, asset) in [
            (TargetOs::Windows, "yt-dlp.exe"),
            (TargetOs::MacOs, "yt-dlp_macos"),
        ] {
            let source = yt_dlp_source(channel, "2026.10.26", os);
            let base = format!("https://github.com/{repository}/releases/download/2026.10.26/");
            assert_eq!(source.tool, Tool::YtDlp);
            assert_eq!(source.version, "2026.10.26");
            assert_eq!(source.url, format!("{base}{asset}"));
            assert_eq!(
                source.checksum,
                ChecksumSource::Url {
                    url: format!("{base}SHA2-256SUMS"),
                    entry_name: asset.to_owned(),
                }
            );
            assert_eq!(
                source.package,
                Package::Binary {
                    file_name: asset.to_owned(),
                }
            );
        }
    }
}

#[test]
fn official_deno_sources_are_exact() {
    for (os, arch, triple, executable) in [
        (
            TargetOs::Windows,
            Arch::X86_64,
            "x86_64-pc-windows-msvc",
            "deno.exe",
        ),
        (
            TargetOs::Windows,
            Arch::Aarch64,
            "aarch64-pc-windows-msvc",
            "deno.exe",
        ),
        (TargetOs::MacOs, Arch::X86_64, "x86_64-apple-darwin", "deno"),
        (
            TargetOs::MacOs,
            Arch::Aarch64,
            "aarch64-apple-darwin",
            "deno",
        ),
    ] {
        let source = deno_source("2.5.0", os, arch);
        let asset = format!("deno-{triple}.zip");
        let url = format!("https://github.com/denoland/deno/releases/download/v2.5.0/{asset}");
        assert_eq!(source.tool, Tool::Deno);
        assert_eq!(source.version, "2.5.0");
        assert_eq!(source.url, url);
        assert_eq!(
            source.checksum,
            ChecksumSource::Url {
                url: format!("{url}.sha256sum"),
                entry_name: asset,
            }
        );
        assert_eq!(
            source.package,
            Package::Zip {
                entries: vec![ZipEntry {
                    pattern: executable.to_owned(),
                }],
            }
        );
    }
}

#[test]
fn official_ffmpeg_sources_are_exact() {
    let build = FfmpegBuild {
        release_tag: "autobuild-2026-10-31-18-18",
        asset: "ffmpeg-N-128251-g6d1bfd7609-win64-gpl-shared.zip",
        version: "2026-10-31",
    };
    let source = ffmpeg_source(&build, TargetOs::Windows).unwrap();
    let base =
        "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/autobuild-2026-10-31-18-18/";
    let asset = build.asset;
    assert_eq!(source.tool, Tool::Ffmpeg);
    assert_eq!(source.version, "2026-10-31");
    assert_eq!(source.url, format!("{base}{asset}"));
    assert_eq!(
        source.checksum,
        ChecksumSource::Url {
            url: format!("{base}checksums.sha256"),
            entry_name: asset.to_owned(),
        }
    );
    assert_eq!(
        source.package,
        Package::Zip {
            entries: vec![
                ZipEntry {
                    pattern: "*/bin/ffmpeg.exe".to_owned(),
                },
                ZipEntry {
                    pattern: "*/bin/ffprobe.exe".to_owned(),
                },
                ZipEntry {
                    pattern: "*/bin/*.dll".to_owned(),
                },
            ],
        }
    );
    assert!(matches!(
        ffmpeg_source(&build, TargetOs::MacOs),
        Err(ToolsError::Unsupported)
    ));
}

#[test]
fn ublock_lite_source_is_exactly_pinned() {
    let source = ublock_lite_source();
    assert_eq!(source.tool, Tool::UblockLite);
    assert_eq!(source.version, "2026.1026.1812");
    assert_eq!(
        source.url,
        "https://github.com/uBlockOrigin/uBOL-home/releases/download/2026.1026.1812/uBOLite_2026.1026.1812.edge.zip"
    );
    assert_eq!(
        source.checksum,
        ChecksumSource::Inline(
            "12a3be6e4c24c932677208bd0b8fff0689676ef2b0db689cfe794b02e0ed69fc".to_owned()
        )
    );
    assert_eq!(
        source.package,
        Package::ZipTree {
            subdir: "ubol".to_owned()
        }
    );
}

#[test]
fn gallery_dl_sources_are_exactly_pinned() {
    for (os, asset, file_name, checksum) in [
        (
            TargetOs::Windows,
            "gallery-dl_windows.exe",
            "gallery-dl.exe",
            "baf1777cc27fce50ad5adbe77962b37c71334a701393d2c2029e2dc62cd3306f",
        ),
        (
            TargetOs::MacOs,
            "gallery-dl_macos",
            "gallery-dl",
            "2150d83db28e9bf07ddce491153961e14f4ff26f85b6e2ccf4909a8c350f7878",
        ),
    ] {
        let source = gallery_dl_source(os);
        assert_eq!(source.tool, Tool::GalleryDl);
        assert_eq!(source.version, "2026.11.03");
        assert_eq!(
            source.url,
            format!("https://github.com/gdl-org/builds/releases/download/2026.11.03/{asset}")
        );
        assert_eq!(source.checksum, ChecksumSource::Inline(checksum.to_owned()));
        assert_eq!(
            source.package,
            Package::Binary {
                file_name: file_name.to_owned()
            }
        );
    }
}

#[cfg(windows)]
#[test]
fn zip_entry_names_cannot_escape_the_version_directory() {
    let root = tempdir().unwrap();
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    archive
        .start_file(
            r"ffmpeg-x/bin/..\..\..\evil.dll",
            SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(b"evil").unwrap();
    let zip = archive.finish().unwrap().into_inner();
    let source = ToolSource {
        extra: Vec::new(),
        tool: Tool::Ffmpeg,
        version: "build-x".to_owned(),
        url: "memory://ffmpeg.zip".to_owned(),
        checksum: ChecksumSource::Inline(sha256(&zip)),
        package: Package::Zip {
            entries: vec![ZipEntry {
                pattern: "*/bin/*.dll".to_owned(),
            }],
        },
    };
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        Box::new(MemoryClient::new([("memory://ffmpeg.zip".to_owned(), zip)])),
    );
    let error = manager.install(&source, &mut |_| {}).unwrap_err();
    assert!(matches!(error, ToolsError::InvalidFileName(_)), "{error:?}");
    assert!(!root.path().join("evil.dll").exists());
    assert!(!root.path().parent().unwrap().join("evil.dll").exists());
    assert!(!root.path().join("tools.json").exists());
}

#[test]
fn concurrent_installs_of_different_tools_keep_both_entries() {
    let root = tempdir().unwrap();
    let yt_dlp = b"yt-dlp bytes".to_vec();
    let deno = b"deno bytes".to_vec();
    let manager = std::sync::Arc::new(ToolManager::with_client(
        root.path().to_path_buf(),
        Box::new(MemoryClient::new([
            ("memory://yt-dlp".to_owned(), yt_dlp.clone()),
            ("memory://deno".to_owned(), deno.clone()),
        ])),
    ));
    let sources = [
        fake_binary_source(Tool::YtDlp, "y1", "memory://yt-dlp", &yt_dlp),
        fake_binary_source(Tool::Deno, "d1", "memory://deno", &deno),
    ];
    let handles: Vec<_> = sources
        .into_iter()
        .map(|source| {
            let manager = std::sync::Arc::clone(&manager);
            thread::spawn(move || manager.install(&source, &mut |_| {}).unwrap())
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let state: Value =
        serde_json::from_slice(&fs::read(root.path().join("tools.json")).unwrap()).unwrap();
    assert_eq!(state["tools"]["yt-dlp"]["current"]["version"], "y1");
    assert_eq!(state["tools"]["deno"]["current"]["version"], "d1");
}

fn single_file_zip(name: &str, contents: &[u8]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file(name, SimpleFileOptions::default())
        .unwrap();
    archive.write_all(contents).unwrap();
    archive.finish().unwrap().into_inner()
}

/// The macOS ffmpeg shape: ffmpeg in the main archive, ffprobe in a second one.
fn two_archive_ffmpeg_source(ffmpeg: &[u8], ffprobe_digest: String) -> ToolSource {
    let zip_of = |name: &str| Package::Zip {
        entries: vec![ZipEntry {
            pattern: name.to_owned(),
        }],
    };
    ToolSource {
        tool: Tool::Ffmpeg,
        version: "9.0.2".to_owned(),
        url: "memory://ffmpeg.zip".to_owned(),
        checksum: ChecksumSource::Inline(sha256(ffmpeg)),
        package: zip_of("ffmpeg"),
        extra: vec![ToolAsset {
            url: "memory://ffprobe.zip".to_owned(),
            checksum: ChecksumSource::Inline(ffprobe_digest),
            package: zip_of("ffprobe"),
        }],
    }
}

fn ffmpeg_archive_client(ffmpeg: &[u8], ffprobe: &[u8]) -> Box<MemoryClient> {
    Box::new(MemoryClient::new([
        ("memory://ffmpeg.zip".to_owned(), ffmpeg.to_vec()),
        ("memory://ffprobe.zip".to_owned(), ffprobe.to_vec()),
    ]))
}

#[test]
fn extra_assets_install_into_the_same_version_directory() {
    let root = tempdir().unwrap();
    let ffmpeg = single_file_zip("ffmpeg", b"ffmpeg");
    let ffprobe = single_file_zip("ffprobe", b"ffprobe");
    let source = two_archive_ffmpeg_source(&ffmpeg, sha256(&ffprobe));
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        ffmpeg_archive_client(&ffmpeg, &ffprobe),
    );

    let mut progress = Vec::new();
    let installed = manager
        .install(&source, &mut |event| progress.push(event))
        .unwrap();

    let dir = root.path().join("ffmpeg").join("9.0.2");
    assert_eq!(fs::read(dir.join("ffmpeg")).unwrap(), b"ffmpeg");
    assert_eq!(fs::read(dir.join("ffprobe")).unwrap(), b"ffprobe");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["ffmpeg", "ffprobe"] {
            let mode = fs::metadata(dir.join(name)).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "{name} is not executable");
        }
    }
    assert_eq!(installed.sha256, sha256(&ffmpeg));
    assert_eq!(installed.source_url, "memory://ffmpeg.zip");
    assert_eq!(
        progress.last().unwrap().downloaded_bytes,
        (ffmpeg.len() + ffprobe.len()) as u64
    );
    assert!(progress.iter().all(|event| event.total_bytes.is_none()));
    assert!(
        progress
            .windows(2)
            .all(|pair| pair[0].downloaded_bytes <= pair[1].downloaded_bytes)
    );
    let resolved = manager.resolve(Tool::Ffmpeg, TargetOs::MacOs).unwrap();
    assert_eq!(resolved.path, dir);
    assert_eq!(resolved.version, "9.0.2");
}

#[test]
fn a_bad_extra_asset_installs_nothing_and_cleans_up() {
    let root = tempdir().unwrap();
    let ffmpeg = single_file_zip("ffmpeg", b"ffmpeg");
    let ffprobe = single_file_zip("ffprobe", b"ffprobe");
    let source = two_archive_ffmpeg_source(&ffmpeg, sha256(b"another ffprobe"));
    let manager = ToolManager::with_client(
        root.path().to_path_buf(),
        ffmpeg_archive_client(&ffmpeg, &ffprobe),
    );

    let error = manager.install(&source, &mut |_| {}).unwrap_err();

    assert!(matches!(error, ToolsError::ChecksumMismatch { .. }));
    assert!(!root.path().join("tools.json").exists());
    assert!(!root.path().join("ffmpeg").join("9.0.2").exists());
    let temporary_entries: Vec<_> = fs::read_dir(root.path().join(".tmp"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(temporary_entries.is_empty(), "{temporary_entries:?}");
    assert!(manager.resolve(Tool::Ffmpeg, TargetOs::MacOs).is_none());
}

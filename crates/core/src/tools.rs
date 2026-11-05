//! Managed tools: installing, verifying, resolving and rolling back yt-dlp, ffmpeg and Deno.

use crate::naming::TargetOs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const PROGRESS_INTERVAL: u64 = 256 * 1024;
const USER_AGENT: &str = "vidsdownloader/0.1";
static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A managed external tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tool {
    YtDlp,
    Ffmpeg,
    Deno,
    GalleryDl,
    UblockLite,
}

impl Tool {
    /// The stable identifier used in paths and `tools.json`.
    pub const fn id(self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
            Self::Deno => "deno",
            Self::GalleryDl => "gallery-dl",
            Self::UblockLite => "ublock-lite",
        }
    }

    /// Convert a stable identifier from `tools.json` back to a tool.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "yt-dlp" => Some(Self::YtDlp),
            "ffmpeg" => Some(Self::Ffmpeg),
            "deno" => Some(Self::Deno),
            "gallery-dl" => Some(Self::GalleryDl),
            "ublock-lite" => Some(Self::UblockLite),
            _ => None,
        }
    }
}

/// A complete description of one tool download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSource {
    pub tool: Tool,
    pub version: String,
    pub url: String,
    pub checksum: ChecksumSource,
    pub package: Package,
    /// Further downloads installed into the same version directory, each verified on its own.
    pub extra: Vec<ToolAsset>,
}

/// One further download of a tool (for builds that ship each executable separately).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolAsset {
    pub url: String,
    pub checksum: ChecksumSource,
    pub package: Package,
}

/// A pinned yt-dlp/FFmpeg-Builds release selected by the desktop shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfmpegBuild {
    pub release_tag: &'static str,
    pub asset: &'static str,
    pub version: &'static str,
}

/// Where the expected SHA-256 digest comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChecksumSource {
    Url { url: String, entry_name: String },
    Inline(String),
}

/// How downloaded bytes are installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Package {
    Binary { file_name: String },
    Zip { entries: Vec<ZipEntry> },
    ZipTree { subdir: String },
}

/// One archive path pattern to extract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    pub pattern: String,
}

/// Persisted metadata for an installed version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledTool {
    pub version: String,
    pub dir: PathBuf,
    pub sha256: String,
    pub source_url: String,
    pub installed_at: u64,
}

/// Download progress. The current HTTP abstraction does not expose a total size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

/// A tool path that has been checked against the current installation metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    pub path: PathBuf,
    pub version: String,
}

/// The yt-dlp release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Nightly,
}

/// A tool asset's CPU architecture.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

/// Failures while downloading, verifying, installing, or resolving managed tools.
#[derive(Debug, thiserror::Error)]
pub enum ToolsError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("invalid tools.json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid zip archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("checksum entry not found: {entry_name}")]
    ChecksumEntryNotFound { entry_name: String },
    #[error("checksum file is not in a recognized format")]
    UnparseableChecksum,
    #[error("checksum is not a 64-character hexadecimal SHA-256 digest")]
    InvalidChecksum,
    #[error("checksum file is not valid UTF-8")]
    InvalidChecksumEncoding,
    #[error("zip pattern matched no files: {pattern}")]
    ZipPatternNotFound { pattern: String },
    #[error("unsafe path in zip archive: {path}")]
    UnsafeZipPath { path: String },
    #[error("no previous installed version is available")]
    NoPreviousVersion,
    #[error("this tool source is unsupported on the target platform")]
    Unsupported,
    #[error("invalid version path component: {0}")]
    InvalidVersion(String),
    #[error("invalid installed file name: {0}")]
    InvalidFileName(String),
    #[error("system time is before the Unix epoch")]
    InvalidSystemTime,
}

/// A response body and its announced length, when the server sent one.
pub struct HttpBody {
    /// The body as a readable stream.
    pub reader: Box<dyn Read + Send>,
    /// `Content-Length`, used to show download progress as a percentage.
    pub content_length: Option<u64>,
}

/// Minimal streaming HTTP abstraction used by the tool manager.
pub trait HttpClient: Send + Sync {
    /// Fetch `url` and return its body as a readable stream.
    fn get(&self, url: &str) -> Result<HttpBody, ToolsError>;
}

/// Synchronous HTTP client used in production.
#[derive(Debug, Default, Clone, Copy)]
pub struct UreqClient;

impl HttpClient for UreqClient {
    fn get(&self, url: &str) -> Result<HttpBody, ToolsError> {
        let response = ureq::get(url)
            .header("User-Agent", USER_AGENT)
            .call()
            .map_err(|error| ToolsError::Http(error.to_string()))?;
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse().ok());
        let (_, body) = response.into_parts();
        Ok(HttpBody {
            reader: Box::new(body.into_reader()),
            content_length,
        })
    }
}

/// Installs and resolves tools below a single managed root directory.
pub struct ToolManager {
    root: PathBuf,
    client: Box<dyn HttpClient>,
    /// Serializes changes to tools.json and the version directories, so two installs
    /// running at once (say yt-dlp and ffmpeg on first run) cannot lose each other's entry.
    state_lock: Mutex<()>,
}

impl ToolManager {
    /// Create a tool manager rooted at `root`, downloading with the real network client.
    pub fn new(root: PathBuf) -> Self {
        Self::with_client(root, Box::new(UreqClient))
    }

    /// Create a tool manager rooted at `root`, using a caller-supplied HTTP client.
    pub fn with_client(root: PathBuf, client: Box<dyn HttpClient>) -> Self {
        Self {
            root,
            client,
            state_lock: Mutex::new(()),
        }
    }

    /// Download, verify, and install `source`, reporting progress as bytes arrive.
    pub fn install(
        &self,
        source: &ToolSource,
        on_progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<InstalledTool, ToolsError> {
        validate_component(&source.version)
            .map_err(|_| ToolsError::InvalidVersion(source.version.clone()))?;
        fs::create_dir_all(self.root.join(".tmp"))?;
        let unique = unique_name();
        let part_paths: Vec<PathBuf> = (0..=source.extra.len())
            .map(|index| {
                let name = match index {
                    0 => format!("{unique}.part"),
                    _ => format!("{unique}-{index}.part"),
                };
                self.root.join(".tmp").join(name)
            })
            .collect();
        let staging_path = self.root.join(".tmp").join(format!(
            "{}-{}-{unique}",
            source.tool.id(),
            source.version
        ));

        let result = self.install_inner(source, on_progress, &part_paths, &staging_path);
        let part_cleanup = part_paths
            .iter()
            .try_for_each(|path| remove_file_if_exists(path));
        let staging_cleanup = remove_dir_if_exists(&staging_path);

        match result {
            Err(error) => Err(error),
            Ok(installed) => {
                part_cleanup?;
                staging_cleanup?;
                Ok(installed)
            }
        }
    }

    fn install_inner(
        &self,
        source: &ToolSource,
        on_progress: &mut dyn FnMut(InstallProgress),
        part_paths: &[PathBuf],
        staging_path: &Path,
    ) -> Result<InstalledTool, ToolsError> {
        let assets = std::iter::once((&source.url, &source.checksum, &source.package)).chain(
            source
                .extra
                .iter()
                .map(|asset| (&asset.url, &asset.checksum, &asset.package)),
        );
        let several = !source.extra.is_empty();
        let mut done_bytes = 0_u64;
        let mut actual = String::new();
        for (index, (url, checksum, package)) in assets.enumerate() {
            let part_path = &part_paths[index];
            // Progress keeps rising across assets; a single asset's size is no total then.
            let digest = self.download(url, part_path, &mut |progress| {
                on_progress(InstallProgress {
                    downloaded_bytes: done_bytes + progress.downloaded_bytes,
                    total_bytes: if several { None } else { progress.total_bytes },
                })
            })?;
            let expected = self.expected_checksum(checksum)?;
            if expected != digest {
                return Err(ToolsError::ChecksumMismatch {
                    expected,
                    actual: digest,
                });
            }
            done_bytes += fs::metadata(part_path)?.len();
            if index == 0 {
                fs::create_dir(staging_path)?;
                actual = digest;
            }
            self.stage(package, part_path, staging_path)?;
        }
        make_staged_files_executable(staging_path)?;

        // Downloading and staging run unlocked; moving into place and updating tools.json
        // happen one install at a time.
        let _state_guard = self
            .state_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self.read_state()?;
        let tool_root = self.root.join(source.tool.id());
        let destination = tool_root.join(&source.version);
        fs::create_dir_all(&tool_root)?;
        remove_dir_if_exists(&destination)?;
        fs::rename(staging_path, &destination)?;

        let installed = InstalledTool {
            version: source.version.clone(),
            dir: destination,
            sha256: actual,
            source_url: source.url.clone(),
            installed_at: unix_seconds()?,
        };
        let key = source.tool.id().to_owned();
        match state.tools.get_mut(&key) {
            None => {
                state.tools.insert(
                    key,
                    InstalledVersions {
                        current: installed.clone(),
                        previous: None,
                    },
                );
            }
            Some(versions) if versions.current.version == source.version => {
                versions.current = installed.clone();
            }
            Some(versions) => {
                let old_current = std::mem::replace(&mut versions.current, installed.clone());
                versions.previous = Some(old_current);
            }
        }

        self.write_state(&state)?;
        let retained = state
            .tools
            .get(source.tool.id())
            .expect("entry just inserted");
        cleanup_old_versions(&tool_root, retained)?;
        Ok(installed)
    }

    /// Move or extract one verified download into the staging directory.
    fn stage(
        &self,
        package: &Package,
        part_path: &Path,
        staging_path: &Path,
    ) -> Result<(), ToolsError> {
        match package {
            Package::Binary { file_name } => {
                validate_component(file_name)
                    .map_err(|_| ToolsError::InvalidFileName(file_name.clone()))?;
                fs::rename(part_path, staging_path.join(file_name))?;
            }
            Package::Zip { entries } => {
                self.extract_zip(part_path, staging_path, entries)?;
                fs::remove_file(part_path)?;
            }
            Package::ZipTree { subdir } => {
                validate_component(subdir)
                    .map_err(|_| ToolsError::InvalidFileName(subdir.clone()))?;
                self.extract_zip_tree(part_path, &staging_path.join(subdir))?;
                fs::remove_file(part_path)?;
            }
        }
        Ok(())
    }

    fn download(
        &self,
        url: &str,
        path: &Path,
        on_progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<String, ToolsError> {
        let HttpBody {
            reader: mut response,
            content_length,
        } = self.client.get(url)?;
        let mut output = File::create(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut downloaded = 0_u64;
        let mut last_reported = 0_u64;
        loop {
            let read = response.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
            hasher.update(&buffer[..read]);
            downloaded += read as u64;
            if downloaded - last_reported >= PROGRESS_INTERVAL {
                on_progress(InstallProgress {
                    downloaded_bytes: downloaded,
                    total_bytes: content_length,
                });
                last_reported = downloaded;
            }
        }
        output.flush()?;
        on_progress(InstallProgress {
            downloaded_bytes: downloaded,
            total_bytes: content_length,
        });
        Ok(hex::encode(hasher.finalize()))
    }

    fn expected_checksum(&self, source: &ChecksumSource) -> Result<String, ToolsError> {
        match source {
            ChecksumSource::Inline(value) => normalize_checksum(value),
            ChecksumSource::Url { url, entry_name } => {
                let mut reader = self.client.get(url)?.reader;
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes)?;
                let text =
                    std::str::from_utf8(&bytes).map_err(|_| ToolsError::InvalidChecksumEncoding)?;
                parse_checksum(text, entry_name)
            }
        }
    }

    fn extract_zip(
        &self,
        archive_path: &Path,
        staging_path: &Path,
        entries: &[ZipEntry],
    ) -> Result<(), ToolsError> {
        let file = File::open(archive_path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        for requested in entries {
            let mut matches = 0_usize;
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index)?;
                let name = entry.name()?.into_owned();
                if entry.is_dir() || !glob_matches(&requested.pattern, &name) {
                    continue;
                }
                let Some(file_name) = name.rsplit('/').next().filter(|name| !name.is_empty())
                else {
                    continue;
                };
                // The archive's checksum is verified, but a name such as "..\..\x.dll"
                // must still never write outside the staging directory.
                validate_component(file_name)
                    .map_err(|_| ToolsError::InvalidFileName(file_name.to_owned()))?;
                let mut output = File::create(staging_path.join(file_name))?;
                io::copy(&mut entry, &mut output)?;
                matches += 1;
            }
            if matches == 0 {
                return Err(ToolsError::ZipPatternNotFound {
                    pattern: requested.pattern.clone(),
                });
            }
        }
        Ok(())
    }

    fn extract_zip_tree(&self, archive_path: &Path, destination: &Path) -> Result<(), ToolsError> {
        let file = File::open(archive_path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        fs::create_dir_all(destination)?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name()?.into_owned();
            let relative = Path::new(&name);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
            {
                return Err(ToolsError::UnsafeZipPath { path: name });
            }
            let output_path = destination.join(relative);
            if !output_path.starts_with(destination) {
                return Err(ToolsError::UnsafeZipPath { path: name });
            }
            if entry.is_dir() {
                fs::create_dir_all(&output_path)?;
            } else {
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut output = File::create(output_path)?;
                io::copy(&mut entry, &mut output)?;
            }
        }
        Ok(())
    }

    /// Resolve only the current managed copy. The process environment is never consulted.
    pub fn resolve(&self, tool: Tool, os: TargetOs) -> Option<ResolvedTool> {
        let state = self.read_state().ok()?;
        let current = &state.tools.get(tool.id())?.current;
        let path = match tool {
            Tool::YtDlp => current.dir.join(match os {
                TargetOs::Windows => "yt-dlp.exe",
                TargetOs::MacOs => "yt-dlp_macos",
            }),
            Tool::Deno => current.dir.join(match os {
                TargetOs::Windows => "deno.exe",
                TargetOs::MacOs => "deno",
            }),
            Tool::GalleryDl => current.dir.join(match os {
                TargetOs::Windows => "gallery-dl.exe",
                TargetOs::MacOs => "gallery-dl",
            }),
            Tool::Ffmpeg => {
                let (ffmpeg, ffprobe) = match os {
                    TargetOs::Windows => ("ffmpeg.exe", "ffprobe.exe"),
                    TargetOs::MacOs => ("ffmpeg", "ffprobe"),
                };
                if !current.dir.join(ffmpeg).is_file() || !current.dir.join(ffprobe).is_file() {
                    return None;
                }
                current.dir.clone()
            }
            Tool::UblockLite => {
                if os == TargetOs::MacOs
                    || !current.dir.join("ubol").join("manifest.json").is_file()
                {
                    return None;
                }
                current.dir.clone()
            }
        };
        if matches!(tool, Tool::YtDlp | Tool::Deno | Tool::GalleryDl) && !path.is_file() {
            return None;
        }
        Some(ResolvedTool {
            path,
            version: current.version.clone(),
        })
    }

    /// Roll back `tool` by swapping its current and previous installed versions.
    pub fn rollback(&self, tool: Tool) -> Result<InstalledTool, ToolsError> {
        let _state_guard = self
            .state_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self.read_state()?;
        let versions = state
            .tools
            .get_mut(tool.id())
            .ok_or(ToolsError::NoPreviousVersion)?;
        let previous = versions
            .previous
            .take()
            .filter(|installed| installed.dir.is_dir())
            .ok_or(ToolsError::NoPreviousVersion)?;
        let old_current = std::mem::replace(&mut versions.current, previous);
        versions.previous = Some(old_current);
        let current = versions.current.clone();
        self.write_state(&state)?;
        Ok(current)
    }

    fn read_state(&self) -> Result<ToolsFile, ToolsError> {
        let path = self.root.join("tools.json");
        if !path.exists() {
            return Ok(ToolsFile::default());
        }
        // Entries for tools this build does not know (written by a newer version of the
        // app) are kept as they are and never looked up, so an older build still works.
        let state: ToolsFile = serde_json::from_reader(File::open(path)?)?;
        Ok(state)
    }

    fn write_state(&self, state: &ToolsFile) -> Result<(), ToolsError> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(self.root.join(".tmp"))?;
        let temporary = self
            .root
            .join(".tmp")
            .join(format!("tools-{}.json.tmp", unique_name()));
        let write_result = (|| -> Result<(), ToolsError> {
            let mut file = File::create(&temporary)?;
            serde_json::to_writer_pretty(&mut file, state)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, self.root.join("tools.json"))?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = remove_file_if_exists(&temporary);
        }
        write_result
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ToolsFile {
    #[serde(default)]
    tools: BTreeMap<String, InstalledVersions>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstalledVersions {
    current: InstalledTool,
    previous: Option<InstalledTool>,
}

/// Construct an official yt-dlp release source.
pub fn yt_dlp_source(channel: Channel, version: &str, os: TargetOs) -> ToolSource {
    let repository = match channel {
        Channel::Stable => "yt-dlp/yt-dlp",
        Channel::Nightly => "yt-dlp/yt-dlp-nightly-builds",
    };
    let asset = match os {
        TargetOs::Windows => "yt-dlp.exe",
        TargetOs::MacOs => "yt-dlp_macos",
    };
    let base = format!("https://github.com/{repository}/releases/download/{version}/");
    ToolSource {
        extra: Vec::new(),
        tool: Tool::YtDlp,
        version: version.to_owned(),
        url: format!("{base}{asset}"),
        checksum: ChecksumSource::Url {
            url: format!("{base}SHA2-256SUMS"),
            entry_name: asset.to_owned(),
        },
        package: Package::Binary {
            file_name: asset.to_owned(),
        },
    }
}

/// Construct an official Deno release source.
pub fn deno_source(version: &str, os: TargetOs, arch: Arch) -> ToolSource {
    let triple = match (os, arch) {
        (TargetOs::Windows, Arch::X86_64) => "x86_64-pc-windows-msvc",
        (TargetOs::Windows, Arch::Aarch64) => "aarch64-pc-windows-msvc",
        (TargetOs::MacOs, Arch::X86_64) => "x86_64-apple-darwin",
        (TargetOs::MacOs, Arch::Aarch64) => "aarch64-apple-darwin",
    };
    let asset = format!("deno-{triple}.zip");
    let url = format!("https://github.com/denoland/deno/releases/download/v{version}/{asset}");
    ToolSource {
        extra: Vec::new(),
        tool: Tool::Deno,
        version: version.to_owned(),
        checksum: ChecksumSource::Url {
            url: format!("{url}.sha256sum"),
            entry_name: asset,
        },
        url,
        package: Package::Zip {
            entries: vec![ZipEntry {
                pattern: match os {
                    TargetOs::Windows => "deno.exe",
                    TargetOs::MacOs => "deno",
                }
                .to_owned(),
            }],
        },
    }
}

/// The macOS ffmpeg release pinned to this app release.
pub const FFMPEG_MACOS_VERSION: &str = "9.0.2";

/// Construct the macOS ffmpeg source: Martin Riedl's static release build, which includes
/// libass. ffmpeg and ffprobe come as separate archives, each holding the one executable.
pub fn ffmpeg_macos_source(arch: Arch) -> ToolSource {
    let (build, ffmpeg_digest, ffprobe_digest) = match arch {
        Arch::X86_64 => (
            "amd64/1789931006_9.0.2",
            "7c6b4125b191cbf773832dc51f424cf2b6bb7da43007d1e066f95909e47cacd4",
            "2322438ed2f6319a691291b247d09c69dcaa3a982460d1f269a7e1af335cfdfd",
        ),
        Arch::Aarch64 => (
            "arm64/1789931890_9.0.2",
            "c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924",
            "fcbe839537485eaee7a7a8bc5cbc0f90d53617e80943e8a5b2e31cb851197ea6",
        ),
    };
    let base = format!("https://ffmpeg.martin-riedl.de/download/macos/{build}/");
    let zip_of = |name: &str| Package::Zip {
        entries: vec![ZipEntry {
            pattern: name.to_owned(),
        }],
    };
    ToolSource {
        tool: Tool::Ffmpeg,
        version: FFMPEG_MACOS_VERSION.to_owned(),
        url: format!("{base}ffmpeg.zip"),
        checksum: ChecksumSource::Inline(ffmpeg_digest.to_owned()),
        package: zip_of("ffmpeg"),
        extra: vec![ToolAsset {
            url: format!("{base}ffprobe.zip"),
            checksum: ChecksumSource::Inline(ffprobe_digest.to_owned()),
            package: zip_of("ffprobe"),
        }],
    }
}

/// Construct the chosen Windows ffmpeg source.
pub fn ffmpeg_source(build: &FfmpegBuild, os: TargetOs) -> Result<ToolSource, ToolsError> {
    if os == TargetOs::MacOs {
        return Err(ToolsError::Unsupported);
    }
    let base = format!(
        "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/{}/",
        build.release_tag
    );
    Ok(ToolSource {
        extra: Vec::new(),
        tool: Tool::Ffmpeg,
        version: build.version.to_owned(),
        url: format!("{base}{}", build.asset),
        checksum: ChecksumSource::Url {
            url: format!("{base}checksums.sha256"),
            entry_name: build.asset.to_owned(),
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
    })
}

/// Construct the uBlock Origin Lite release pinned to this app release.
pub fn ublock_lite_source() -> ToolSource {
    const VERSION: &str = "2026.1026.1812";
    const ASSET: &str = "uBOLite_2026.1026.1812.edge.zip";
    ToolSource {
        extra: Vec::new(),
        tool: Tool::UblockLite,
        version: VERSION.to_owned(),
        url: format!(
            "https://github.com/uBlockOrigin/uBOL-home/releases/download/{VERSION}/{ASSET}"
        ),
        checksum: ChecksumSource::Inline(
            "12a3be6e4c24c932677208bd0b8fff0689676ef2b0db689cfe794b02e0ed69fc".to_owned(),
        ),
        package: Package::ZipTree {
            subdir: "ubol".to_owned(),
        },
    }
}

/// Construct the gallery-dl standalone build pinned to this app release.
pub fn gallery_dl_source(os: TargetOs) -> ToolSource {
    const VERSION: &str = "2026.11.03";
    let (asset, file_name, checksum) = match os {
        TargetOs::Windows => (
            "gallery-dl_windows.exe",
            "gallery-dl.exe",
            "baf1777cc27fce50ad5adbe77962b37c71334a701393d2c2029e2dc62cd3306f",
        ),
        TargetOs::MacOs => (
            "gallery-dl_macos",
            "gallery-dl",
            "2150d83db28e9bf07ddce491153961e14f4ff26f85b6e2ccf4909a8c350f7878",
        ),
    };
    ToolSource {
        extra: Vec::new(),
        tool: Tool::GalleryDl,
        version: VERSION.to_owned(),
        url: format!("https://github.com/gdl-org/builds/releases/download/{VERSION}/{asset}"),
        checksum: ChecksumSource::Inline(checksum.to_owned()),
        package: Package::Binary {
            file_name: file_name.to_owned(),
        },
    }
}

fn parse_checksum(text: &str, entry_name: &str) -> Result<String, ToolsError> {
    let mut gnu_lines = Vec::new();
    for line in text.lines() {
        if let Some((digest, name)) = parse_gnu_line(line.trim_end_matches('\r')) {
            gnu_lines.push((digest, name));
        }
    }
    if !gnu_lines.is_empty() {
        return gnu_lines
            .into_iter()
            .find_map(|(digest, name)| (name == entry_name).then_some(digest))
            .ok_or_else(|| ToolsError::ChecksumEntryNotFound {
                entry_name: entry_name.to_owned(),
            });
    }

    for line in text.lines() {
        if let Some(digest) = parse_powershell_hash_line(line.trim_end_matches('\r')) {
            return Ok(digest);
        }
    }
    normalize_checksum(text.trim()).map_err(|_| ToolsError::UnparseableChecksum)
}

fn parse_gnu_line(line: &str) -> Option<(String, String)> {
    let bytes = line.as_bytes();
    if bytes.len() < 66 || !bytes[..64].iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut index = 64;
    let whitespace_start = index;
    while index < bytes.len() && matches!(bytes[index], b' ' | b'\t') {
        index += 1;
    }
    if index == whitespace_start {
        return None;
    }
    if bytes.get(index) == Some(&b'*') {
        index += 1;
    }
    let mut name = line[index..].trim();
    if let Some(stripped) = name.strip_prefix("./") {
        name = stripped;
    }
    Some((line[..64].to_ascii_lowercase(), name.to_owned()))
}

fn parse_powershell_hash_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let (label, value) = trimmed.split_once(':')?;
    if !label.trim().eq_ignore_ascii_case("hash") {
        return None;
    }
    normalize_checksum(value.trim()).ok()
}

fn normalize_checksum(value: &str) -> Result<String, ToolsError> {
    let trimmed = value.trim();
    if trimmed.len() == 64 && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(trimmed.to_ascii_lowercase())
    } else {
        Err(ToolsError::InvalidChecksum)
    }
}

fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let pattern_segments: Vec<_> = pattern.split('/').collect();
    let candidate_segments: Vec<_> = candidate.split('/').collect();
    pattern_segments.len() == candidate_segments.len()
        && pattern_segments
            .iter()
            .zip(candidate_segments)
            .all(|(pattern, candidate)| segment_matches(pattern, candidate))
}

fn segment_matches(pattern: &str, candidate: &str) -> bool {
    let pattern = pattern.as_bytes();
    let candidate = candidate.as_bytes();
    let mut pattern_index = 0;
    let mut candidate_index = 0;
    let mut star = None;
    let mut star_candidate = 0;
    while candidate_index < candidate.len() {
        if pattern.get(pattern_index) == Some(&candidate[candidate_index]) {
            pattern_index += 1;
            candidate_index += 1;
        } else if pattern.get(pattern_index) == Some(&b'*') {
            star = Some(pattern_index);
            pattern_index += 1;
            star_candidate = candidate_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            star_candidate += 1;
            candidate_index = star_candidate;
        } else {
            return false;
        }
    }
    while pattern.get(pattern_index) == Some(&b'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn validate_component(value: &str) -> Result<(), ()> {
    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) if value != "." && value != ".." => Ok(()),
        _ => Err(()),
    }
}

fn unique_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{count}", std::process::id())
}

fn unix_seconds() -> Result<u64, ToolsError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ToolsError::InvalidSystemTime)
}

fn remove_file_if_exists(path: &Path) -> Result<(), io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_dir_if_exists(path: &Path) -> Result<(), io::Error> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn cleanup_old_versions(tool_root: &Path, retained: &InstalledVersions) -> Result<(), io::Error> {
    let current = retained.current.version.as_str();
    let previous = retained.previous.as_ref().map(|tool| tool.version.as_str());
    for entry in fs::read_dir(tool_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name != current && previous.is_none_or(|version| name != version) {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn make_staged_files_executable(staging_path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;

    for entry in fs::read_dir(staging_path)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_staged_files_executable(_staging_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOWER: &str = "cba84b2a1d9f4e5c6b7a8091234567890abcdef1234567890abcdef123456789";
    const UPPER: &str = "CBA84B2A1D9F4E5C6B7A8091234567890ABCDEF1234567890ABCDEF123456789";

    #[test]
    fn parses_gnu_text_and_binary_formats() {
        assert_eq!(
            parse_checksum(&format!("{UPPER}  ./asset.zip\r\n"), "asset.zip").unwrap(),
            LOWER
        );
        assert_eq!(
            parse_checksum(&format!("{UPPER} *asset.zip\n"), "asset.zip").unwrap(),
            LOWER
        );
    }

    #[test]
    fn parses_powershell_output() {
        let text = format!(
            "Algorithm : SHA256\r\n  Hash   :   {UPPER}  \r\nPath : C:\\tmp\\asset.zip\r\n"
        );
        assert_eq!(parse_checksum(&text, "ignored").unwrap(), LOWER);
    }

    #[test]
    fn parses_bare_digest() {
        assert_eq!(parse_checksum(UPPER, "ignored").unwrap(), LOWER);
    }

    #[test]
    fn gnu_file_does_not_fall_back_when_entry_is_absent() {
        let error = parse_checksum(&format!("{UPPER}  other.zip\n"), "asset.zip")
            .expect_err("the requested GNU entry is absent");
        assert!(matches!(
            error,
            ToolsError::ChecksumEntryNotFound { entry_name } if entry_name == "asset.zip"
        ));
    }

    #[test]
    fn glob_star_stays_within_one_segment() {
        assert!(glob_matches("*/bin/*.dll", "ffmpeg-x/bin/a.dll"));
        assert!(!glob_matches("*/bin/*.dll", "ffmpeg/x/bin/a.dll"));
        assert!(!glob_matches("*/bin/*.dll", "ffmpeg-x/lib/a.dll"));
    }

    #[test]
    fn ffmpeg_source_uses_the_pinned_build_description() {
        let build = FfmpegBuild {
            release_tag: "autobuild-2026-10-31-18-18",
            asset: "ffmpeg-N-128251-g6d1bfd7609-win64-gpl-shared.zip",
            version: "2026-10-31",
        };
        let source = ffmpeg_source(&build, TargetOs::Windows).unwrap();
        let base =
            "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/autobuild-2026-10-31-18-18/";

        assert_eq!(source.version, "2026-10-31");
        assert_eq!(source.url, format!("{base}{}", build.asset));
        assert_eq!(
            source.checksum,
            ChecksumSource::Url {
                url: format!("{base}checksums.sha256"),
                entry_name: build.asset.to_owned(),
            }
        );
        assert!(matches!(source.package, Package::Zip { .. }));
    }

    #[test]
    fn macos_ffmpeg_source_fetches_ffmpeg_and_ffprobe_for_each_arch() {
        let mut digests = Vec::new();
        for (arch, base) in [
            (
                Arch::X86_64,
                "https://ffmpeg.martin-riedl.de/download/macos/amd64/1789931006_9.0.2/",
            ),
            (
                Arch::Aarch64,
                "https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/",
            ),
        ] {
            let source = ffmpeg_macos_source(arch);
            assert_eq!(source.tool, Tool::Ffmpeg);
            assert_eq!(source.url, format!("{base}ffmpeg.zip"));
            assert_eq!(
                source.package,
                Package::Zip {
                    entries: vec![ZipEntry {
                        pattern: "ffmpeg".to_owned()
                    }]
                }
            );
            let [ffprobe] = source.extra.as_slice() else {
                panic!("expected ffprobe as the one extra asset");
            };
            assert_eq!(ffprobe.url, format!("{base}ffprobe.zip"));
            assert_eq!(
                ffprobe.package,
                Package::Zip {
                    entries: vec![ZipEntry {
                        pattern: "ffprobe".to_owned()
                    }]
                }
            );
            for checksum in [&source.checksum, &ffprobe.checksum] {
                let ChecksumSource::Inline(digest) = checksum else {
                    panic!("macOS ffmpeg digests are pinned inline");
                };
                assert_eq!(normalize_checksum(digest).unwrap(), *digest);
                digests.push(digest.clone());
            }
        }
        digests.sort();
        digests.dedup();
        assert_eq!(digests.len(), 4);
    }
}

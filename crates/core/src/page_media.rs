//! Pure filtering and selection for media URLs observed in a rendered page, plus the
//! bounded HTTP reads needed to compare manifests.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::time::Duration;

use url::Url;

use crate::args::Preset;

pub const MAX_CANDIDATES: usize = 20;
pub const MAX_MANIFEST_FETCHES: usize = 8;
pub const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MANIFEST_TYPES: &[&str] = &[
    "application/vnd.apple.mpegurl",
    "application/x-mpegurl",
    "audio/mpegurl",
    "application/dash+xml",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaCandidate {
    pub url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateKind {
    Hls,
    Dash,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Hls,
    Dash,
    Mp4,
    WebM,
    Mov,
}

impl MediaKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hls => "HLS",
            Self::Dash => "DASH",
            Self::Mp4 => "MP4",
            Self::WebM => "WebM",
            Self::Mov => "MOV",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedMedia {
    pub url: String,
    pub kind: MediaKind,
    pub duration_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityLimit {
    Unlimited,
    Height(u32),
}

impl From<Preset> for QualityLimit {
    fn from(preset: Preset) -> Self {
        match preset {
            Preset::UpTo720 => Self::Height(720),
            Preset::UpTo1080 => Self::Height(1080),
            Preset::Best
            | Preset::CompatibilityH264
            | Preset::AudioM4a
            | Preset::AudioMp3
            | Preset::CommentsOnly => Self::Unlimited,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerHint {
    pub src: String,
    pub area: u64,
    pub playing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaChoice {
    One(String),
    Ask(Vec<ReducedMedia>),
    None,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestDocument {
    pub body: String,
    /// The first variant named by a master HLS playlist, if it could be fetched.
    pub first_variant_body: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ManifestCache {
    pub documents: HashMap<String, ManifestDocument>,
    attempted: HashSet<String>,
    fetches: usize,
}

/// Adds one valid, unique candidate, merging response metadata into an earlier URL and
/// retaining at most [`MAX_CANDIDATES`].
pub fn add_candidate(candidates: &mut Vec<MediaCandidate>, mut candidate: MediaCandidate) {
    if candidate_kind(&candidate).is_none() {
        return;
    }
    if let Some(existing) = candidates.iter_mut().find(|item| item.url == candidate.url) {
        if existing.content_type.is_none() {
            existing.content_type = candidate.content_type.take();
        }
        existing.content_length = existing.content_length.max(candidate.content_length);
        if (200..400).contains(&candidate.status) {
            existing.status = candidate.status;
        }
    } else if candidates.len() < MAX_CANDIDATES {
        candidates.push(candidate);
    }
}

/// Selects from already-fetched evidence. Manifests beat files; HLS masters and MPDs
/// beat HLS media playlists, then duration decides. Files use Content-Length.
pub fn choose_candidate(
    candidates: &[MediaCandidate],
    manifests: &HashMap<String, ManifestDocument>,
) -> Option<String> {
    match choose_media(candidates, manifests, QualityLimit::Unlimited, &[]) {
        MediaChoice::One(url) => Some(url),
        MediaChoice::Ask(choices) => choices.into_iter().next().map(|choice| choice.url),
        MediaChoice::None => None,
    }
}

#[derive(Clone)]
struct CandidateInfo<'a> {
    candidate: &'a MediaCandidate,
    reduced: ReducedMedia,
    class: u8,
    duration_millis: Option<u64>,
    master: bool,
}

#[derive(Clone)]
struct MediaGroup<'a> {
    members: Vec<CandidateInfo<'a>>,
    representative: CandidateInfo<'a>,
    class: u8,
    duration_millis: Option<u64>,
    bytes: u64,
    order: usize,
}

/// Chooses one distinct video, or returns the equally plausible videos that need a
/// user's decision. Quality variants are collapsed before videos are ranked.
pub fn choose_media(
    candidates: &[MediaCandidate],
    manifests: &HashMap<String, ManifestDocument>,
    limit: QualityLimit,
    players: &[PlayerHint],
) -> MediaChoice {
    let infos = candidates
        .iter()
        .filter_map(|candidate| candidate_info(candidate, manifests))
        .collect::<Vec<_>>();
    if infos.is_empty() {
        return MediaChoice::None;
    }

    let mut parents = (0..infos.len()).collect::<Vec<_>>();
    let normalized = infos
        .iter()
        .map(|info| normalized_quality_url(&info.candidate.url))
        .collect::<Vec<_>>();
    for left in 0..infos.len() {
        for right in (left + 1)..infos.len() {
            let same_quality_shape = normalized[left].is_some()
                && normalized[left] == normalized[right]
                && durations_compatible(infos[left].duration_millis, infos[right].duration_millis);
            let master_child = (infos[left].master
                && hls_variants(
                    &infos[left].candidate.url,
                    &manifests[&infos[left].candidate.url].body,
                )
                .contains(&infos[right].candidate.url))
                || (infos[right].master
                    && hls_variants(
                        &infos[right].candidate.url,
                        &manifests[&infos[right].candidate.url].body,
                    )
                    .contains(&infos[left].candidate.url));
            if same_quality_shape || master_child {
                union(&mut parents, left, right);
            }
        }
    }

    let mut grouped: Vec<(usize, Vec<CandidateInfo<'_>>)> = Vec::new();
    for (index, info) in infos.into_iter().enumerate() {
        let root = find(&mut parents, index);
        if let Some((_, members)) = grouped.iter_mut().find(|(group, _)| *group == root) {
            members.push(info);
        } else {
            grouped.push((root, vec![info]));
        }
    }
    let mut groups = grouped
        .into_iter()
        .enumerate()
        .map(|(order, (_, members))| make_group(members, limit, order))
        .collect::<Vec<_>>();

    let hinted = players
        .iter()
        .filter(|player| player.playing)
        .max_by_key(|player| player.area)
        .or_else(|| players.iter().max_by_key(|player| player.area));
    if let Some(player) = hinted.filter(|player| is_http_url(&player.src))
        && let Some(group) = groups.iter().find(|group| {
            group
                .members
                .iter()
                .any(|member| member.candidate.url == player.src)
        })
    {
        return MediaChoice::One(group.representative.candidate.url.clone());
    }

    groups.sort_by(|left, right| {
        (right.class, right.duration_millis.unwrap_or(0), right.bytes)
            .cmp(&(left.class, left.duration_millis.unwrap_or(0), left.bytes))
            .then_with(|| left.order.cmp(&right.order))
    });
    let top = &groups[0];
    let tied = top.duration_millis.map_or_else(Vec::new, |top_duration| {
        groups
            .iter()
            .filter(|group| {
                group.class == top.class
                    && group
                        .duration_millis
                        .is_some_and(|duration| duration.abs_diff(top_duration) <= 1_000)
            })
            .take(8)
            .collect::<Vec<_>>()
    });
    if tied.len() > 1 {
        MediaChoice::Ask(
            tied.into_iter()
                .map(|group| group.representative.reduced.clone())
                .collect(),
        )
    } else {
        MediaChoice::One(top.representative.candidate.url.clone())
    }
}

fn candidate_info<'a>(
    candidate: &'a MediaCandidate,
    manifests: &HashMap<String, ManifestDocument>,
) -> Option<CandidateInfo<'a>> {
    let kind = media_kind(candidate)?;
    let document = manifests.get(&candidate.url);
    let master =
        kind == MediaKind::Hls && document.is_some_and(|document| is_hls_master(&document.body));
    let duration_millis = match kind {
        MediaKind::Hls => document.map(|document| {
            document
                .first_variant_body
                .as_deref()
                .map(hls_duration_millis)
                .unwrap_or_else(|| hls_duration_millis(&document.body))
        }),
        MediaKind::Dash => document.and_then(|document| mpd_duration_millis(&document.body)),
        MediaKind::Mp4 | MediaKind::WebM | MediaKind::Mov => None,
    }
    .filter(|duration| *duration > 0);
    Some(CandidateInfo {
        candidate,
        reduced: ReducedMedia {
            url: candidate.url.clone(),
            kind,
            duration_seconds: duration_millis.map(|duration| (duration + 500) / 1_000),
        },
        class: match kind {
            MediaKind::Hls if master => 3,
            MediaKind::Dash => 3,
            MediaKind::Hls => 2,
            MediaKind::Mp4 | MediaKind::WebM | MediaKind::Mov => 1,
        },
        duration_millis,
        master,
    })
}

fn make_group<'a>(
    members: Vec<CandidateInfo<'a>>,
    limit: QualityLimit,
    order: usize,
) -> MediaGroup<'a> {
    let representative = members
        .iter()
        .filter(|member| member.master || member.reduced.kind == MediaKind::Dash)
        .max_by_key(|member| {
            (
                member.class,
                member.duration_millis.unwrap_or(0),
                member.candidate.content_length.unwrap_or(0),
            )
        })
        .cloned()
        .unwrap_or_else(|| quality_representative(&members, limit));
    let class = members.iter().map(|member| member.class).max().unwrap_or(0);
    let duration_millis = members
        .iter()
        .filter_map(|member| member.duration_millis)
        .max();
    let bytes = members
        .iter()
        .filter_map(|member| member.candidate.content_length)
        .max()
        .unwrap_or(0);
    MediaGroup {
        members,
        representative,
        class,
        duration_millis,
        bytes,
        order,
    }
}

fn quality_representative<'a>(
    members: &[CandidateInfo<'a>],
    limit: QualityLimit,
) -> CandidateInfo<'a> {
    let known = members
        .iter()
        .filter_map(|member| quality_height(&member.candidate.url).map(|height| (member, height)))
        .collect::<Vec<_>>();
    let selected = match limit {
        QualityLimit::Unlimited => known
            .iter()
            .max_by_key(|(member, height)| (*height, member.candidate.content_length.unwrap_or(0))),
        QualityLimit::Height(maximum) => known
            .iter()
            .filter(|(_, height)| *height <= maximum)
            .max_by_key(|(member, height)| (*height, member.candidate.content_length.unwrap_or(0)))
            .or_else(|| {
                known.iter().min_by_key(|(member, height)| {
                    (
                        *height,
                        std::cmp::Reverse(member.candidate.content_length.unwrap_or(0)),
                    )
                })
            }),
    };
    selected
        .map(|(member, _)| (*member).clone())
        .unwrap_or_else(|| {
            members
                .iter()
                .max_by_key(|member| member.candidate.content_length.unwrap_or(0))
                .expect("media groups are non-empty")
                .clone()
        })
}

fn durations_compatible(left: Option<u64>, right: Option<u64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.abs_diff(right) <= 1_000,
        _ => true,
    }
}

fn find(parents: &mut [usize], index: usize) -> usize {
    if parents[index] != index {
        parents[index] = find(parents, parents[index]);
    }
    parents[index]
}

fn union(parents: &mut [usize], left: usize, right: usize) {
    let left = find(parents, left);
    let right = find(parents, right);
    if left != right {
        parents[right] = left;
    }
}

/// The named video height in a URL path. Hosts and query strings are ignored.
pub fn quality_height(url: &str) -> Option<u32> {
    quality_token(url).map(|token| token.height)
}

struct QualityToken {
    start: usize,
    end: usize,
    height: u32,
}

fn normalized_quality_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let token = quality_token(url)?;
    let path = parsed.path();
    let mut normalized_path = String::with_capacity(path.len());
    normalized_path.push_str(&path[..token.start]);
    normalized_path.push_str("{quality}");
    normalized_path.push_str(&path[token.end..]);
    let mut normalized = parsed;
    normalized.set_path(&normalized_path);
    Some(normalized.to_string())
}

fn quality_token(url: &str) -> Option<QualityToken> {
    let parsed = Url::parse(url).ok()?;
    let path = parsed.path();
    let lower = path.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    const HEIGHTS: &[u32] = &[144, 240, 360, 480, 540, 576, 720, 1080, 1440, 2160];
    for start in 0..bytes.len() {
        for height in HEIGHTS {
            let text = height.to_string();
            if !lower[start..].starts_with(&text) {
                continue;
            }
            let number_end = start + text.len();
            let before_digit = start > 0 && bytes[start - 1].is_ascii_digit();
            let after_digit = number_end < bytes.len() && bytes[number_end].is_ascii_digit();
            if before_digit || after_digit {
                continue;
            }
            if bytes.get(number_end) == Some(&b'p')
                && bytes
                    .get(number_end + 1)
                    .is_none_or(|byte| is_quality_delimiter(*byte))
            {
                return Some(QualityToken {
                    start,
                    end: number_end + 1,
                    height: *height,
                });
            }
            if boundary_before(bytes, start) && boundary_after(bytes, number_end) {
                return Some(QualityToken {
                    start,
                    end: number_end,
                    height: *height,
                });
            }
        }
    }
    for (word, height) in [
        ("fhd", 1080),
        ("uhd", 2160),
        ("4k", 2160),
        ("sd", 480),
        ("hd", 720),
    ] {
        for (start, _) in lower.match_indices(word) {
            let end = start + word.len();
            if boundary_before(bytes, start) && boundary_after(bytes, end) {
                return Some(QualityToken { start, end, height });
            }
        }
    }
    None
}

fn boundary_before(bytes: &[u8], index: usize) -> bool {
    index == 0 || is_quality_delimiter(bytes[index - 1])
}

fn boundary_after(bytes: &[u8], index: usize) -> bool {
    index == bytes.len() || is_quality_delimiter(bytes[index])
}

fn is_quality_delimiter(byte: u8) -> bool {
    matches!(byte, b'_' | b'-' | b'.' | b'/')
}

fn is_http_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// Reduces response candidates to distinct videos. An HLS master represents the media
/// playlists it names, so those child candidates are omitted. Duration is rounded to the
/// nearest second and comes from the first master variant, a media playlist, or the MPD.
pub fn reduce_candidates(
    candidates: &[MediaCandidate],
    manifests: &HashMap<String, ManifestDocument>,
) -> Vec<ReducedMedia> {
    let hidden_playlists = candidates
        .iter()
        .filter_map(|candidate| {
            let document = manifests.get(&candidate.url)?;
            is_hls_master(&document.body).then_some((candidate.url.as_str(), &document.body))
        })
        .flat_map(|(master_url, body)| hls_variants(master_url, body))
        .collect::<HashSet<_>>();

    candidates
        .iter()
        .filter_map(|candidate| {
            let kind = media_kind(candidate)?;
            if hidden_playlists.contains(&candidate.url) {
                return None;
            }
            let duration_millis = match kind {
                MediaKind::Hls => manifests.get(&candidate.url).map(|document| {
                    document
                        .first_variant_body
                        .as_deref()
                        .map(hls_duration_millis)
                        .unwrap_or_else(|| hls_duration_millis(&document.body))
                }),
                MediaKind::Dash => manifests
                    .get(&candidate.url)
                    .and_then(|document| mpd_duration_millis(&document.body)),
                MediaKind::Mp4 | MediaKind::WebM | MediaKind::Mov => None,
            };
            Some(ReducedMedia {
                url: candidate.url.clone(),
                kind,
                duration_seconds: duration_millis
                    .filter(|duration| *duration > 0)
                    .map(|duration| (duration + 500) / 1_000),
            })
        })
        .collect()
}

/// Fetches only candidate manifests with a browser-like client and the page as Referer,
/// then chooses with [`choose_candidate`]. Each response is capped at 2 MiB.
pub fn choose_remote(candidates: &[MediaCandidate], referer: &str) -> Option<String> {
    let mut cache = ManifestCache::default();
    fetch_manifests(candidates, referer, &mut cache);
    choose_candidate(candidates, &cache.documents)
}

/// Fetches bounded manifest evidence, then chooses with page-player and quality hints.
pub fn choose_remote_media(
    candidates: &[MediaCandidate],
    referer: &str,
    limit: QualityLimit,
    players: &[PlayerHint],
) -> MediaChoice {
    let mut cache = ManifestCache::default();
    fetch_manifests(candidates, referer, &mut cache);
    choose_media(candidates, &cache.documents, limit, players)
}

/// Fetches bounded manifest evidence and returns every remaining video candidate.
pub fn reduce_remote(candidates: &[MediaCandidate], referer: &str) -> Vec<ReducedMedia> {
    let mut cache = ManifestCache::default();
    reduce_remote_cached(candidates, referer, &mut cache)
}

/// Adds newly seen manifests to a page cache, never making more than eight manifest
/// requests over that cache's lifetime, then reduces all candidates with the evidence.
pub fn reduce_remote_cached(
    candidates: &[MediaCandidate],
    referer: &str,
    cache: &mut ManifestCache,
) -> Vec<ReducedMedia> {
    fetch_manifests(candidates, referer, cache);
    reduce_candidates(candidates, &cache.documents)
}

fn fetch_manifests(candidates: &[MediaCandidate], referer: &str, cache: &mut ManifestCache) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(FETCH_TIMEOUT))
        .timeout_recv_response(Some(FETCH_TIMEOUT))
        .timeout_per_read(Some(FETCH_TIMEOUT))
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
        )
        .build()
        .into();
    for candidate in candidates.iter().take(MAX_CANDIDATES) {
        if cache.fetches >= MAX_MANIFEST_FETCHES {
            break;
        }
        let Some(kind) = candidate_kind(candidate) else {
            continue;
        };
        if kind == CandidateKind::File {
            continue;
        }
        if !cache.attempted.insert(candidate.url.clone()) {
            continue;
        }
        cache.fetches += 1;
        let Some(body) = fetch_text(&agent, &candidate.url, referer) else {
            continue;
        };
        let first_variant_url = (kind == CandidateKind::Hls && is_hls_master(&body))
            .then(|| first_hls_variant(&body))
            .flatten()
            .and_then(|variant| Url::parse(&candidate.url).ok()?.join(variant).ok());
        let first_variant_body = if let Some(url) = first_variant_url {
            let url = url.to_string();
            if let Some(document) = cache.documents.get(&url) {
                Some(document.body.clone())
            } else if cache.fetches < MAX_MANIFEST_FETCHES && cache.attempted.insert(url.clone()) {
                cache.fetches += 1;
                fetch_text(&agent, &url, referer).inspect(|body| {
                    cache.documents.insert(
                        url,
                        ManifestDocument {
                            body: body.clone(),
                            first_variant_body: None,
                        },
                    );
                })
            } else {
                None
            }
        } else {
            None
        };
        cache.documents.insert(
            candidate.url.clone(),
            ManifestDocument {
                body,
                first_variant_body,
            },
        );
    }
}

fn fetch_text(agent: &ureq::Agent, url: &str, referer: &str) -> Option<String> {
    let response = agent.get(url).header("Referer", referer).call().ok()?;
    if response
        .body()
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES)
    {
        return None;
    }
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_MANIFEST_BYTES).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

fn candidate_kind(candidate: &MediaCandidate) -> Option<CandidateKind> {
    match media_kind(candidate)? {
        MediaKind::Hls => Some(CandidateKind::Hls),
        MediaKind::Dash => Some(CandidateKind::Dash),
        MediaKind::Mp4 | MediaKind::WebM | MediaKind::Mov => Some(CandidateKind::File),
    }
}

fn media_kind(candidate: &MediaCandidate) -> Option<MediaKind> {
    if !(200..400).contains(&candidate.status) {
        return None;
    }
    let parsed = Url::parse(&candidate.url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let path = parsed.path().to_ascii_lowercase();
    let file = path.rsplit('/').next().unwrap_or_default();
    if [".ts", ".m4s", ".aac", ".key"]
        .iter()
        .any(|extension| path.ends_with(extension))
        || file.starts_with("init.")
        || file.starts_with("init-")
        || file.contains("initsegment")
    {
        return None;
    }
    let media_type = candidate
        .content_type
        .as_deref()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if path.ends_with(".m3u8") || MANIFEST_TYPES[..3].contains(&media_type.as_str()) {
        Some(MediaKind::Hls)
    } else if path.ends_with(".mpd") || media_type == MANIFEST_TYPES[3] {
        Some(MediaKind::Dash)
    } else if path.ends_with(".webm") || media_type == "video/webm" {
        Some(MediaKind::WebM)
    } else if path.ends_with(".mov") || media_type == "video/quicktime" {
        Some(MediaKind::Mov)
    } else if path.ends_with(".mp4") || path.ends_with(".m4v") || media_type.starts_with("video/") {
        Some(MediaKind::Mp4)
    } else {
        None
    }
}

fn hls_variants(master_url: &str, body: &str) -> Vec<String> {
    let Ok(base) = Url::parse(master_url) else {
        return Vec::new();
    };
    let mut wants_variant = false;
    let mut variants = Vec::new();
    for line in body.lines().map(str::trim) {
        if wants_variant && !line.is_empty() && !line.starts_with('#') {
            if let Ok(url) = base.join(line) {
                variants.push(url.to_string());
            }
            wants_variant = false;
        } else if !line.is_empty() {
            wants_variant = line.starts_with("#EXT-X-STREAM-INF");
        }
    }
    variants
}

fn is_hls_master(body: &str) -> bool {
    body.lines()
        .any(|line| line.trim_start().starts_with("#EXT-X-STREAM-INF"))
}

fn first_hls_variant(body: &str) -> Option<&str> {
    let mut wants_variant = false;
    for line in body.lines().map(str::trim) {
        if wants_variant && !line.is_empty() && !line.starts_with('#') {
            return Some(line);
        }
        wants_variant = line.starts_with("#EXT-X-STREAM-INF");
    }
    None
}

fn hls_duration_millis(body: &str) -> u64 {
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("#EXTINF:"))
        .filter_map(|value| value.split(',').next()?.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(|seconds| (seconds * 1000.0).round() as u64)
        .sum()
}

fn mpd_duration_millis(body: &str) -> Option<u64> {
    let marker = "mediaPresentationDuration";
    let rest = body.split_once(marker)?.1.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let value = rest[quote.len_utf8()..].split(quote).next()?;
    iso_duration_millis(value)
}

fn iso_duration_millis(value: &str) -> Option<u64> {
    let chars = value.strip_prefix("PT")?.chars();
    let mut number = String::new();
    let mut seconds = 0.0;
    for character in chars {
        if character.is_ascii_digit() || character == '.' {
            number.push(character);
            continue;
        }
        let amount: f64 = number.parse().ok()?;
        number.clear();
        seconds += match character {
            'H' => amount * 3600.0,
            'M' => amount * 60.0,
            'S' => amount,
            _ => return None,
        };
    }
    (number.is_empty() && seconds.is_finite() && seconds >= 0.0)
        .then(|| (seconds * 1000.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(url: &str, content_type: Option<&str>, length: Option<u64>) -> MediaCandidate {
        MediaCandidate {
            url: url.to_owned(),
            status: 200,
            content_type: content_type.map(str::to_owned),
            content_length: length,
        }
    }

    #[test]
    fn filters_extensions_types_segments_and_non_http_urls() {
        let mut found = Vec::new();
        for item in [
            candidate("https://cdn.test/master.m3u8?token=x", None, None),
            candidate(
                "https://cdn.test/manifest",
                Some("application/dash+xml"),
                None,
            ),
            candidate(
                "https://cdn.test/movie",
                Some("video/mp4; charset=binary"),
                Some(9),
            ),
            candidate("https://cdn.test/seg.ts", Some("video/mp2t"), Some(9)),
            candidate("https://cdn.test/seg.m4s", Some("video/mp4"), Some(9)),
            candidate("https://cdn.test/audio.aac", Some("video/mp4"), Some(9)),
            candidate("https://cdn.test/key.key", Some("video/mp4"), Some(9)),
            candidate("https://cdn.test/init.mp4", Some("video/mp4"), Some(9)),
            candidate("blob:https://page.test/id", Some("video/mp4"), Some(9)),
            candidate("data:video/mp4;base64,AA", Some("video/mp4"), Some(9)),
        ] {
            add_candidate(&mut found, item);
        }
        assert_eq!(
            found
                .iter()
                .map(|item| item.url.as_str())
                .collect::<Vec<_>>(),
            [
                "https://cdn.test/master.m3u8?token=x",
                "https://cdn.test/manifest",
                "https://cdn.test/movie"
            ]
        );
    }

    #[test]
    fn chooses_master_over_its_media_playlist_and_longest_manifest() {
        let candidates = vec![
            candidate("https://cdn.test/media.m3u8", None, None),
            candidate("https://cdn.test/master.m3u8", None, None),
            candidate("https://cdn.test/long.mpd", None, None),
        ];
        let manifests = HashMap::from([
            (
                candidates[0].url.clone(),
                ManifestDocument {
                    body: "#EXTM3U\n#EXTINF:90,\na.ts\n".to_owned(),
                    first_variant_body: None,
                },
            ),
            (
                candidates[1].url.clone(),
                ManifestDocument {
                    body: "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nmedia.m3u8\n".to_owned(),
                    first_variant_body: Some("#EXTM3U\n#EXTINF:120,\na.ts\n".to_owned()),
                },
            ),
            (
                candidates[2].url.clone(),
                ManifestDocument {
                    body: "<MPD mediaPresentationDuration=\"PT2M1S\">".to_owned(),
                    first_variant_body: None,
                },
            ),
        ]);
        assert_eq!(
            choose_candidate(&candidates, &manifests),
            Some(candidates[2].url.clone())
        );
    }

    #[test]
    fn chooses_largest_file_when_there_is_no_manifest() {
        let candidates = vec![
            candidate("https://cdn.test/small.mp4", None, Some(100)),
            candidate("https://cdn.test/large.webm", None, Some(200)),
        ];
        assert_eq!(
            choose_candidate(&candidates, &HashMap::new()),
            Some(candidates[1].url.clone())
        );
    }

    #[test]
    fn master_hides_its_media_playlists_and_keeps_kind_and_duration() {
        let candidates = vec![
            candidate("https://cdn.test/master.m3u8", None, None),
            candidate("https://cdn.test/low/index.m3u8", None, None),
            candidate("https://cdn.test/movie.mpd", None, None),
            candidate("https://cdn.test/clip.webm", None, Some(200)),
        ];
        let manifests = HashMap::from([
            (
                candidates[0].url.clone(),
                ManifestDocument {
                    body: "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nlow/index.m3u8\n".to_owned(),
                    first_variant_body: Some(
                        "#EXTM3U\n#EXTINF:600.4,\na.ts\n#EXTINF:33.2,\nb.ts\n".to_owned(),
                    ),
                },
            ),
            (
                candidates[1].url.clone(),
                ManifestDocument {
                    body: "#EXTM3U\n#EXTINF:633.6,\na.ts\n".to_owned(),
                    first_variant_body: None,
                },
            ),
            (
                candidates[2].url.clone(),
                ManifestDocument {
                    body: "<MPD mediaPresentationDuration=\"PT1H2M3S\">".to_owned(),
                    first_variant_body: None,
                },
            ),
        ]);

        assert_eq!(
            reduce_candidates(&candidates, &manifests),
            vec![
                ReducedMedia {
                    url: candidates[0].url.clone(),
                    kind: MediaKind::Hls,
                    duration_seconds: Some(634),
                },
                ReducedMedia {
                    url: candidates[2].url.clone(),
                    kind: MediaKind::Dash,
                    duration_seconds: Some(3_723),
                },
                ReducedMedia {
                    url: candidates[3].url.clone(),
                    kind: MediaKind::WebM,
                    duration_seconds: None,
                },
            ]
        );
        assert_eq!(MediaKind::Hls.label(), "HLS");
        assert_eq!(MediaKind::Dash.label(), "DASH");
        assert_eq!(MediaKind::Mp4.label(), "MP4");
        assert_eq!(MediaKind::WebM.label(), "WebM");
        assert_eq!(MediaKind::Mov.label(), "MOV");
    }

    #[test]
    fn reads_quality_only_from_delimited_path_tokens() {
        for (url, expected) in [
            ("https://cdn.test/ep01_720.mp4", Some(720)),
            ("https://cdn.test/1080/index.m3u8", Some(1080)),
            ("https://cdn.test/video-480-x.mp4", Some(480)),
            ("https://cdn.test/video720p.mp4", Some(720)),
            ("https://cdn.test/movie.fhd.mp4", Some(1080)),
            ("https://cdn.test/movie_uhd.mp4", Some(2160)),
            ("https://cdn.test/movie-4k.mp4", Some(2160)),
            ("https://cdn.test/movie-sd.mp4", Some(480)),
            ("https://cdn.test/movie-hd.mp4", Some(720)),
            ("https://720.test/movie.mp4", None),
            ("https://cdn.test/movie.mp4?quality=1080p", None),
            ("https://cdn.test/12720/index.mp4", None),
            ("https://cdn.test/movie_hdready.mp4", None),
            ("https://cdn.test/episode720.mp4", None),
        ] {
            assert_eq!(quality_height(url), expected, "{url}");
        }
    }

    #[test]
    fn quality_variants_follow_the_limit() {
        let candidates = [360, 720, 1080]
            .map(|height| {
                candidate(
                    &format!("https://cdn.test/episode_{height}.mp4"),
                    None,
                    Some(height as u64),
                )
            })
            .to_vec();
        assert_eq!(
            choose_media(&candidates, &HashMap::new(), QualityLimit::Unlimited, &[]),
            MediaChoice::One(candidates[2].url.clone())
        );
        assert_eq!(
            choose_media(
                &candidates,
                &HashMap::new(),
                QualityLimit::from(Preset::UpTo720),
                &[]
            ),
            MediaChoice::One(candidates[1].url.clone())
        );
    }

    fn hls_document(seconds: u64) -> ManifestDocument {
        ManifestDocument {
            body: format!("#EXTM3U\n#EXTINF:{seconds},\nsegment.ts\n"),
            first_variant_body: None,
        }
    }

    #[test]
    fn equal_length_videos_ask_unless_the_largest_playing_player_identifies_one() {
        let candidates = vec![
            candidate("https://cdn.test/ep01.m3u8", None, None),
            candidate("https://cdn.test/ep02.m3u8", None, None),
        ];
        let manifests = HashMap::from([
            (candidates[0].url.clone(), hls_document(1_420)),
            (candidates[1].url.clone(), hls_document(1_420)),
        ]);
        assert_eq!(
            choose_media(&candidates, &manifests, QualityLimit::Unlimited, &[]),
            MediaChoice::Ask(vec![
                ReducedMedia {
                    url: candidates[0].url.clone(),
                    kind: MediaKind::Hls,
                    duration_seconds: Some(1_420),
                },
                ReducedMedia {
                    url: candidates[1].url.clone(),
                    kind: MediaKind::Hls,
                    duration_seconds: Some(1_420),
                },
            ])
        );
        assert_eq!(
            choose_media(
                &candidates,
                &manifests,
                QualityLimit::Unlimited,
                &[
                    PlayerHint {
                        src: candidates[0].url.clone(),
                        area: 10_000,
                        playing: false,
                    },
                    PlayerHint {
                        src: candidates[1].url.clone(),
                        area: 20_000,
                        playing: true,
                    },
                ]
            ),
            MediaChoice::One(candidates[1].url.clone())
        );
    }

    #[test]
    fn a_long_main_video_beats_a_preview() {
        let candidates = vec![
            candidate("https://cdn.test/preview.m3u8", None, None),
            candidate("https://cdn.test/main.m3u8", None, None),
        ];
        let manifests = HashMap::from([
            (candidates[0].url.clone(), hls_document(10)),
            (candidates[1].url.clone(), hls_document(24 * 60)),
        ]);
        assert_eq!(
            choose_media(&candidates, &manifests, QualityLimit::Unlimited, &[]),
            MediaChoice::One(candidates[1].url.clone())
        );
    }

    #[test]
    fn a_master_represents_its_media_playlists() {
        let candidates = vec![
            candidate("https://cdn.test/master.m3u8", None, None),
            candidate("https://cdn.test/360/index.m3u8", None, None),
            candidate("https://cdn.test/720/index.m3u8", None, None),
        ];
        let manifests = HashMap::from([
            (
                candidates[0].url.clone(),
                ManifestDocument {
                    body: "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n360/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2\n720/index.m3u8\n".to_owned(),
                    first_variant_body: Some(hls_document(600).body),
                },
            ),
            (candidates[1].url.clone(), hls_document(600)),
            (candidates[2].url.clone(), hls_document(600)),
        ]);
        assert_eq!(
            choose_media(
                &candidates,
                &manifests,
                QualityLimit::from(Preset::UpTo720),
                &[]
            ),
            MediaChoice::One(candidates[0].url.clone())
        );
    }
}

//! Parsing and ASS rendering for scrolling video comments.

use std::collections::HashSet;

use quick_xml::{Reader, events::Event, name::QName};
use serde::{Deserialize, Serialize};

const FIXED_SECONDS: f64 = 3.0;
const GAP_PX: f64 = 24.0;

/// A normalized timed comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub at_ms: u64,
    pub text: String,
    pub position: Position,
    pub size: Size,
    pub color: u32,
}

/// How a comment moves or stays fixed on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Scroll,
    Top,
    Bottom,
}

/// The source comment's relative font size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    Small,
    Medium,
    Big,
}

/// Settings for ASS comment layout and filtering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssSettings {
    pub size_factor: f64,
    pub opacity_percent: u8,
    pub max_on_screen: usize,
    pub font: String,
    pub scroll_seconds: f64,
    pub display_area: f64,
    pub max_total: usize,
    pub ng_words: Vec<String>,
}

impl Default for AssSettings {
    fn default() -> Self {
        Self {
            size_factor: 1.0,
            opacity_percent: 100,
            max_on_screen: 0,
            font: "BIZ UDPGothic".to_owned(),
            scroll_seconds: 4.0,
            display_area: 1.0,
            max_total: 0,
            ng_words: Vec::new(),
        }
    }
}

/// Pixel dimensions used for ASS layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSize {
    /// Video width in pixels.
    pub width: u32,
    /// Video height in pixels.
    pub height: u32,
    /// Video duration used to discard comments after playback ends.
    pub duration_ms: Option<u64>,
}

/// Rendered ASS text and placement statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssOutput {
    /// Complete rendered ASS document.
    pub text: String,
    /// Number of comments included in the ASS document.
    pub placed: usize,
    /// Number dropped by layout or maximum-count limits.
    pub dropped: usize,
    /// Number dropped by the NG-word filter.
    pub dropped_by_ng: usize,
    /// Number starting after the video duration.
    pub dropped_after_end: usize,
}

/// Invalid comment input.
#[derive(Debug, thiserror::Error)]
pub enum CommentsError {
    #[error("invalid niconico comment JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid Bilibili comment XML: {0}")]
    Xml(String),
}

#[derive(Deserialize)]
struct NiconicoComment {
    #[serde(rename = "vposMs")]
    at_ms: u64,
    body: String,
    #[serde(default)]
    commands: Vec<String>,
}

/// Parse niconico's JSON comment array.
pub fn parse_niconico(input: &str) -> Result<Vec<Comment>, CommentsError> {
    let source: Vec<NiconicoComment> = serde_json::from_str(input)?;
    Ok(source
        .into_iter()
        .map(|comment| {
            let commands: HashSet<&str> = comment.commands.iter().map(String::as_str).collect();
            let position = if commands.contains("ue") {
                Position::Top
            } else if commands.contains("shita") {
                Position::Bottom
            } else {
                Position::Scroll
            };
            let size = if commands.contains("small") {
                Size::Small
            } else if commands.contains("medium") {
                Size::Medium
            } else if commands.contains("big") {
                Size::Big
            } else {
                Size::Medium
            };
            let color = named_color(&commands)
                .or_else(|| {
                    comment
                        .commands
                        .iter()
                        .find_map(|value| parse_hex_color(value))
                })
                .unwrap_or(0xFFFFFF);
            Comment {
                at_ms: comment.at_ms,
                text: comment.body,
                position,
                size,
                color,
            }
        })
        .collect())
}

/// Parse Bilibili XML danmaku, skipping unsupported positioned modes.
pub fn parse_bilibili(input: &str) -> Result<Vec<Comment>, CommentsError> {
    let mut reader = Reader::from_str(input);
    let mut comments = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) if start.name() == QName("d") => {
                let parameters = start
                    .attributes()
                    .with_checks(false)
                    .find_map(|attribute| match attribute {
                        Ok(attribute) if attribute.key == QName("p") => Some(
                            attribute
                                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .map(|value| value.into_owned())
                                .map_err(|error| CommentsError::Xml(error.to_string())),
                        ),
                        Ok(_) => None,
                        Err(error) => Some(Err(CommentsError::Xml(error.to_string()))),
                    })
                    .transpose()?
                    .ok_or_else(|| {
                        CommentsError::Xml("<d> is missing its p attribute".to_owned())
                    })?;
                let raw_text = reader
                    .read_text(QName("d"))
                    .map_err(|error| CommentsError::Xml(error.to_string()))?;
                let text = quick_xml::escape::unescape(&raw_text)
                    .map_err(|error| CommentsError::Xml(error.to_string()))?
                    .into_owned();
                if let Some(comment) = parse_bilibili_comment(&parameters, text)? {
                    comments.push(comment);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(CommentsError::Xml(error.to_string())),
        }
    }
    Ok(comments)
}

/// Convert normalized comments to an Advanced SubStation Alpha subtitle.
pub fn to_ass(comments: &[Comment], settings: &AssSettings, video: VideoSize) -> AssOutput {
    let mut sorted: Vec<&Comment> = comments.iter().collect();
    sorted.sort_by_key(|comment| comment.at_ms);

    let before_duration = sorted.len();
    if let Some(duration_ms) = video.duration_ms {
        sorted.retain(|comment| comment.at_ms <= duration_ms);
    }
    let dropped_after_end = before_duration - sorted.len();

    let folded_ng: Vec<String> = settings
        .ng_words
        .iter()
        .map(|word| word.to_lowercase())
        .collect();
    let before_ng = sorted.len();
    sorted.retain(|comment| {
        let text = comment.text.to_lowercase();
        !folded_ng.iter().any(|word| text.contains(word))
    });
    let dropped_by_ng = before_ng - sorted.len();
    let mut dropped = 0;

    if settings.max_total > 0 && sorted.len() > settings.max_total {
        let original_len = sorted.len();
        sorted = evenly_spread(&sorted, settings.max_total);
        dropped += original_len - sorted.len();
    }

    let width = f64::from(video.width);
    let height = f64::from(video.height);
    let base_px = height * 0.0625 * settings.size_factor;
    let line_height = base_px * 1.25;
    let fixed_lane_count = ((height / line_height).floor() as usize).max(1);
    let display_height = height * settings.display_area.clamp(0.25, 1.0);
    let scroll_lane_count = ((display_height / line_height).floor() as usize).max(1);
    let scroll_seconds = if settings.scroll_seconds > 0.0 {
        settings.scroll_seconds
    } else {
        4.0
    };
    let mut scroll_lanes: Vec<Option<(f64, f64, f64)>> = vec![None; scroll_lane_count];
    let mut top_free = vec![0.0_f64; fixed_lane_count];
    let mut bottom_free = vec![0.0_f64; fixed_lane_count];
    let opacity = settings.opacity_percent.min(100);
    // Ties go to even, as with the Python prototype's round() (opacity 30 gives B2, not B3).
    let alpha = ((f64::from(100 - opacity) * 255.0 / 100.0).round_ties_even()) as u8;
    let mut shown = Vec::<f64>::new();
    let mut events = Vec::<AssEvent>::new();

    for comment in sorted {
        let start = comment.at_ms as f64 / 1000.0;
        if settings.max_on_screen > 0 {
            shown.retain(|end| *end > start);
            if shown.len() >= settings.max_on_screen {
                dropped += 1;
                continue;
            }
        }

        let font_px = base_px * size_factor(comment.size);
        let tags = format!(
            "\\fs{font_px:.0}\\c{}\\alpha&H{alpha:02X}&",
            ass_color(comment.color)
        );
        let escaped = escape_ass(&comment.text);
        let (end, text, layer) = match comment.position {
            Position::Top | Position::Bottom => {
                let top = comment.position == Position::Top;
                let free = if top { &mut top_free } else { &mut bottom_free };
                let Some(lane) = free.iter().position(|free_at| *free_at <= start) else {
                    dropped += 1;
                    continue;
                };
                free[lane] = start + FIXED_SECONDS;
                let y = if top {
                    lane as f64 * line_height
                } else {
                    height - lane as f64 * line_height
                };
                let anchor = if top { 8 } else { 2 };
                (
                    start + FIXED_SECONDS,
                    format!(
                        "{{\\an{anchor}\\pos({},{y:.0}){tags}}}{escaped}",
                        video.width / 2
                    ),
                    1,
                )
            }
            Position::Scroll => {
                let text_width = estimated_text_width(&comment.text, font_px);
                let speed = (width + text_width) / scroll_seconds;
                let mut lane = None;
                for (index, previous) in scroll_lanes.iter().enumerate() {
                    if fixed_lane_is_busy(
                        index,
                        start,
                        &top_free,
                        &bottom_free,
                        scroll_lane_count,
                        fixed_lane_count,
                    ) {
                        continue;
                    }
                    match previous {
                        None => {
                            lane = Some(index);
                            break;
                        }
                        Some((previous_start, previous_width, previous_speed)) => {
                            let entered =
                                previous_start + (previous_width + GAP_PX) / previous_speed;
                            let no_catch_up = previous_start + scroll_seconds - width / speed;
                            if start >= entered.max(no_catch_up) {
                                lane = Some(index);
                                break;
                            }
                        }
                    }
                }
                let Some(lane) = lane else {
                    dropped += 1;
                    continue;
                };
                scroll_lanes[lane] = Some((start, text_width, speed));
                let y = lane as f64 * line_height;
                (
                    start + scroll_seconds,
                    format!(
                        "{{\\an7\\move({},{y:.0},-{},{y:.0}){tags}}}{escaped}",
                        video.width,
                        text_width.ceil() as u64
                    ),
                    0,
                )
            }
        };
        shown.push(end);
        events.push(AssEvent {
            start,
            end,
            text,
            layer,
        });
    }

    let header = format!(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {}\nPlayResY: {}\nWrapStyle: 2\nScaledBorderAndShadow: yes\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Danmaku,{},{base_px:.0},&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,2,0,7,0,0,0,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text",
        video.width, video.height, settings.font
    );
    let mut text = header;
    text.push('\n');
    for event in &events {
        text.push_str(&format!(
            "Dialogue: {},{},{},Danmaku,,0,0,0,,{}\n",
            event.layer,
            ass_time(event.start),
            ass_time(event.end),
            event.text
        ));
    }
    AssOutput {
        text,
        placed: events.len(),
        dropped,
        dropped_by_ng,
        dropped_after_end,
    }
}

struct AssEvent {
    start: f64,
    end: f64,
    text: String,
    layer: u8,
}

fn parse_bilibili_comment(
    parameters: &str,
    text: String,
) -> Result<Option<Comment>, CommentsError> {
    let fields: Vec<&str> = parameters.split(',').collect();
    if fields.len() < 4 {
        return Err(CommentsError::Xml(
            "Bilibili p attribute has fewer than four fields".to_owned(),
        ));
    }
    let seconds: f64 = fields[0]
        .parse()
        .map_err(|_| CommentsError::Xml("invalid Bilibili comment time".to_owned()))?;
    let mode: u8 = fields[1]
        .parse()
        .map_err(|_| CommentsError::Xml("invalid Bilibili comment mode".to_owned()))?;
    let position = match mode {
        1..=3 => Position::Scroll,
        5 => Position::Top,
        4 => Position::Bottom,
        _ => return Ok(None),
    };
    let source_size: u32 = fields[2]
        .parse()
        .map_err(|_| CommentsError::Xml("invalid Bilibili comment size".to_owned()))?;
    let size = if source_size <= 18 {
        Size::Small
    } else if source_size >= 36 {
        Size::Big
    } else {
        Size::Medium
    };
    let color = fields[3]
        .parse::<u32>()
        .map_err(|_| CommentsError::Xml("invalid Bilibili comment color".to_owned()))?
        & 0xFFFFFF;
    Ok(Some(Comment {
        at_ms: (seconds * 1000.0).round().max(0.0) as u64,
        text,
        position,
        size,
        color,
    }))
}

fn named_color(commands: &HashSet<&str>) -> Option<u32> {
    [
        ("white", 0xFFFFFF),
        ("red", 0xFF0000),
        ("pink", 0xFF8080),
        ("orange", 0xFFC000),
        ("yellow", 0xFFFF00),
        ("green", 0x00FF00),
        ("cyan", 0x00FFFF),
        ("blue", 0x0000FF),
        ("purple", 0xC000FF),
        ("black", 0x000000),
    ]
    .into_iter()
    .find_map(|(name, color)| commands.contains(name).then_some(color))
}

fn parse_hex_color(value: &str) -> Option<u32> {
    let digits = value.strip_prefix('#')?;
    (digits.len() == 6)
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

fn evenly_spread<'a>(comments: &[&'a Comment], count: usize) -> Vec<&'a Comment> {
    if count == 1 {
        return vec![comments[comments.len() / 2]];
    }
    (0..count)
        .map(|index| {
            let numerator = index * (comments.len() - 1);
            let selected = (numerator + (count - 1) / 2) / (count - 1);
            comments[selected]
        })
        .collect()
}

fn fixed_lane_is_busy(
    scroll_lane: usize,
    start: f64,
    top_free: &[f64],
    bottom_free: &[f64],
    scroll_lane_count: usize,
    fixed_lane_count: usize,
) -> bool {
    if top_free.get(scroll_lane).is_some_and(|free| *free > start) {
        return true;
    }
    scroll_lane_count == fixed_lane_count
        && bottom_free
            .get(scroll_lane_count - 1 - scroll_lane)
            .is_some_and(|free| *free > start)
}

fn size_factor(size: Size) -> f64 {
    match size {
        Size::Small => 0.65,
        Size::Medium => 1.0,
        Size::Big => 1.6,
    }
}

fn estimated_text_width(text: &str, font_px: f64) -> f64 {
    text.chars()
        .map(|character| {
            if u32::from(character) >= 0x2E80 {
                1.0
            } else {
                0.55
            }
        })
        .sum::<f64>()
        * font_px
}

fn ass_time(seconds: f64) -> String {
    let centiseconds = (seconds * 100.0).round() as u64;
    let hours = centiseconds / 360_000;
    let minutes = centiseconds % 360_000 / 6_000;
    let seconds = centiseconds % 6_000 / 100;
    let fraction = centiseconds % 100;
    format!("{hours}:{minutes:02}:{seconds:02}.{fraction:02}")
}

fn ass_color(rgb: u32) -> String {
    let red = (rgb >> 16) & 0xFF;
    let green = (rgb >> 8) & 0xFF;
    let blue = rgb & 0xFF;
    format!("&H{blue:02X}{green:02X}{red:02X}&")
}

fn escape_ass(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('\n', "\\N")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIDEO: VideoSize = VideoSize {
        width: 1280,
        height: 720,
        duration_ms: None,
    };

    #[test]
    fn niconico_default_matches_reference_ass() {
        let comments = parse_niconico(include_str!(
            "../../tests/fixtures/comments/niconico_sample.json"
        ))
        .unwrap();
        let output = to_ass(&comments, &AssSettings::default(), VIDEO);
        assert_eq!(
            output.text,
            include_str!("../../tests/fixtures/comments/expected_default.ass")
        );
        assert_eq!((output.placed, output.dropped), (39, 2));
    }

    #[test]
    fn niconico_tuned_matches_reference_ass() {
        let comments = parse_niconico(include_str!(
            "../../tests/fixtures/comments/niconico_sample.json"
        ))
        .unwrap();
        let settings = AssSettings {
            size_factor: 0.8,
            opacity_percent: 60,
            max_on_screen: 12,
            ..AssSettings::default()
        };
        let output = to_ass(&comments, &settings, VIDEO);
        assert_eq!(
            output.text,
            include_str!("../../tests/fixtures/comments/expected_tuned.ass")
        );
        assert_eq!((output.placed, output.dropped), (16, 25));
    }

    #[test]
    fn bilibili_fixture_parses_supported_modes_and_entities() {
        let comments = parse_bilibili(include_str!(
            "../../tests/fixtures/comments/bilibili_sample.xml"
        ))
        .unwrap();
        assert_eq!(comments.len(), 6);
        assert_eq!(comments[0].position, Position::Scroll);
        assert_eq!(comments[1].position, Position::Top);
        assert_eq!(comments[2].position, Position::Bottom);
        assert_eq!(comments[3].size, Size::Big);
        assert_eq!(comments[4].size, Size::Small);
        assert_eq!(comments[1].color, 0xFF0000);
        assert_eq!(comments[2].color, 0x00FF00);
        assert_eq!(comments[5].text, "A & B <tag>");
    }

    fn simple_comments(count: usize) -> Vec<Comment> {
        (0..count)
            .map(|index| Comment {
                at_ms: index as u64 * 1_000,
                text: format!("comment {index}"),
                position: Position::Scroll,
                size: Size::Medium,
                color: 0xFFFFFF,
            })
            .collect()
    }

    #[test]
    fn scroll_seconds_changes_event_duration() {
        let comments = simple_comments(1);
        let output = to_ass(
            &comments,
            &AssSettings {
                scroll_seconds: 6.0,
                ..AssSettings::default()
            },
            VIDEO,
        );
        assert!(output.text.contains("0:00:00.00,0:00:06.00"));
    }

    #[test]
    fn display_area_limits_scrolling_lane_positions() {
        let comments: Vec<Comment> = (0..20)
            .map(|_| Comment {
                at_ms: 0,
                ..simple_comments(1).remove(0)
            })
            .collect();
        let output = to_ass(
            &comments,
            &AssSettings {
                display_area: 0.25,
                ..AssSettings::default()
            },
            VIDEO,
        );
        assert!(
            output
                .text
                .lines()
                .filter(|line| line.starts_with("Dialogue:"))
                .all(|line| {
                    let marker = line.split("\\move(1280,").nth(1).unwrap();
                    let y: u32 = marker.split(',').next().unwrap().parse().unwrap();
                    y < 180
                })
        );
    }

    #[test]
    fn max_total_keeps_an_even_deterministic_subset() {
        let output = to_ass(
            &simple_comments(10),
            &AssSettings {
                max_total: 3,
                ..AssSettings::default()
            },
            VIDEO,
        );
        assert_eq!((output.placed, output.dropped), (3, 7));
        assert!(output.text.contains("comment 0"));
        assert!(output.text.contains("comment 5"));
        assert!(output.text.contains("comment 9"));
    }

    #[test]
    fn ng_words_are_case_insensitive_and_counted_separately() {
        let mut comments = simple_comments(3);
        comments[1].text = "Spoiler HERE".to_owned();
        let output = to_ass(
            &comments,
            &AssSettings {
                ng_words: vec!["spoiler".to_owned()],
                ..AssSettings::default()
            },
            VIDEO,
        );
        assert_eq!(output.dropped_by_ng, 1);
        assert_eq!(output.dropped, 0);
        assert!(!output.text.contains("Spoiler HERE"));
    }

    #[test]
    fn real_niconico_sample_drops_comments_after_video_end() {
        let comments = parse_niconico(include_str!(
            "../../tests/fixtures/comments/niconico_sm9_comments_sample.json"
        ))
        .unwrap();
        let output = to_ass(
            &comments,
            &AssSettings::default(),
            VideoSize {
                width: 480,
                height: 360,
                duration_ms: Some(320_000),
            },
        );
        assert!(output.placed > 0);
        assert_eq!(
            output.placed + output.dropped + output.dropped_by_ng + output.dropped_after_end,
            comments.len()
        );
        assert!(output.text.contains("PlayResX: 480"));
        assert!(output.text.contains("PlayResY: 360"));
    }
}

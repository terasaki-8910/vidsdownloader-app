//! gallery-dl metadata, naming inputs, arguments, and error classification.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use time::{Date, Month, PrimitiveDateTime, Time};

use crate::args::Cookies;
use crate::executor::friendly_site;
use crate::naming::VideoInfo;
use crate::types::ErrorCategory;

// Generic fallback fields. Site profiles may override these lists later.
const FIELD_CANDIDATES: &[(&str, &[&str])] = &[
    (
        "uploader",
        &[
            "author.nick",
            "author.name",
            "user.nick",
            "user.name",
            "uploader",
            "artist",
            "username",
        ],
    ),
    ("title", &["title", "content", "description"]),
    ("id", &["tweet_id", "post_id", "id"]),
    ("date", &["date"]),
    ("site", &["category"]),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GalleryFile {
    pub num: u32,
    pub extension: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GalleryPost {
    pub info: VideoInfo,
    pub category: Option<String>,
    pub site_name: String,
    pub files: Vec<GalleryFile>,
}

/// Parse gallery-dl's `-j` message array without converting integer IDs through `f64`.
pub fn parse_metadata(raw: &str) -> Result<GalleryPost, String> {
    let messages: Vec<Value> =
        serde_json::from_str(raw).map_err(|error| format!("invalid gallery-dl JSON: {error}"))?;
    let mut post_fields = None;
    let mut files = Vec::new();
    for message in &messages {
        let Some(parts) = message.as_array() else {
            continue;
        };
        let kind = parts.first().and_then(Value::as_i64);
        let fields = match kind {
            Some(2) => parts.get(1),
            Some(3) => parts.get(2),
            _ => None,
        };
        let Some(fields) = fields.filter(|value| value.is_object()) else {
            continue;
        };
        if post_fields.is_none() {
            post_fields = Some(fields);
        }
        if kind == Some(3)
            && let (Some(num), Some(extension)) = (
                field_string(fields, "num").and_then(|value| value.parse::<u32>().ok()),
                field_string(fields, "extension"),
            )
        {
            files.push(GalleryFile { num, extension });
        }
    }
    let empty_fields = Value::Object(Map::new());
    let fields = post_fields.unwrap_or(&empty_fields);
    let uploader = first_field(fields, candidates("uploader"));
    let title = first_field(fields, candidates("title")).map(|value| collapse_first_line(&value));
    let id = first_field(fields, candidates("id"));
    let timestamp = first_field(fields, candidates("date"))
        .as_deref()
        .and_then(parse_utc_timestamp);
    let category = first_field(fields, candidates("site"));
    let site_name = friendly_site(category.as_deref());
    Ok(GalleryPost {
        info: VideoInfo {
            id,
            title,
            uploader,
            timestamp,
            extractor: category.clone(),
            ..VideoInfo::default()
        },
        category,
        site_name,
        files,
    })
}

fn candidates(name: &str) -> &'static [&'static str] {
    FIELD_CANDIDATES
        .iter()
        .find_map(|(candidate_name, fields)| (*candidate_name == name).then_some(*fields))
        .expect("every mapped gallery field has candidates")
}

fn first_field(value: &Value, paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .filter_map(|path| field_string(value, path))
        .find(|value| !value.trim().is_empty())
}

fn field_string(value: &Value, path: &str) -> Option<String> {
    let mut current = value;
    for component in path.split('.') {
        current = current.as_object()?.get(component)?;
    }
    match current {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn collapse_first_line(value: &str) -> String {
    value
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_utc_timestamp(value: &str) -> Option<i64> {
    if value.len() != 19
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(10) != Some(&b' ')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| value.get(range)?.parse::<u8>().ok();
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = Month::try_from(number(5..7)?).ok()?;
    let date = Date::from_calendar_date(year, month, number(8..10)?).ok()?;
    let time = Time::from_hms(number(11..13)?, number(14..16)?, number(17..19)?).ok()?;
    Some(
        PrimitiveDateTime::new(date, time)
            .assume_utc()
            .unix_timestamp(),
    )
}

/// Options every run gets. Without the output options gallery-dl writes paths and messages in
/// the system code page (cp932 on Japanese Windows) when its output is a pipe.
fn base_args() -> Vec<OsString> {
    vec![
        "--config-ignore".into(),
        "-o".into(),
        "output.stdout=utf-8".into(),
        "-o".into(),
        "output.stderr=utf-8".into(),
    ]
}

pub fn metadata_args(url: &str, cookies: &Cookies) -> Vec<OsString> {
    let mut args = base_args();
    args.push("-j".into());
    push_cookie_file(&mut args, cookies);
    args.push(url.into());
    args
}

pub fn download_args(folder: &Path, stem: &str, url: &str, cookies: &Cookies) -> Vec<OsString> {
    let mut args = base_args();
    push_cookie_file(&mut args, cookies);
    args.extend([
        OsString::from("-D"),
        folder.as_os_str().to_owned(),
        OsString::from("-f"),
        OsString::from(format!("{} {{num}}.{{extension}}", escape_format(stem))),
        OsString::from(url),
    ]);
    args
}

fn push_cookie_file(args: &mut Vec<OsString>, cookies: &Cookies) {
    if let Cookies::File(path) = cookies {
        args.push("--cookies".into());
        args.push(path.as_os_str().to_owned());
    }
}

pub fn escape_format(value: &str) -> String {
    value.replace('{', "{{").replace('}', "}}")
}

pub fn classify_error_line(line: &str) -> Option<(ErrorCategory, String)> {
    let marker = line.to_ascii_lowercase().find("[error]")?;
    let message = line[marker + "[error]".len()..].trim().to_owned();
    let lower = message.to_ascii_lowercase();
    let category = if lower.contains("unsupported url") {
        ErrorCategory::UnsupportedUrl
    } else if lower.contains("401")
        || lower.contains("403")
        || lower.contains("authorizationerror")
        || lower.contains("login")
    {
        ErrorCategory::LoginRequired
    } else if lower.contains("404") || lower.contains("notfounderror") {
        ErrorCategory::Unavailable
    } else if lower.contains("429") {
        ErrorCategory::RateLimited
    } else {
        ErrorCategory::Unknown
    };
    Some((category, message))
}

pub fn written_path(line: &str, folder: &Path) -> Option<PathBuf> {
    // gallery-dl marks a file that already existed with "# ".
    let value = line.trim();
    let value = value.strip_prefix("# ").unwrap_or(value);
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        folder.join(path)
    };
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::{NameTemplate, NamingContext, OptionalPart, Part, TargetOs, Token};

    const FIXTURE: &str = include_str!("../tests/fixtures/gallery-dl/x-two-photos.json");

    fn template() -> NameTemplate {
        NameTemplate {
            folders: vec![vec![Part::Field {
                field: Token::Uploader,
            }]],
            file: vec![
                Part::Optional {
                    optional: OptionalPart {
                        before: String::new(),
                        field: Token::Date,
                        after: " ".to_owned(),
                    },
                },
                Part::Field {
                    field: Token::Title,
                },
                Part::Optional {
                    optional: OptionalPart {
                        before: " [".to_owned(),
                        field: Token::Id,
                        after: "]".to_owned(),
                    },
                },
            ],
        }
    }

    #[test]
    fn fixture_preserves_large_id_and_maps_generic_fields() {
        let post = parse_metadata(FIXTURE).unwrap();
        assert_eq!(
            post.files,
            [
                GalleryFile {
                    num: 1,
                    extension: "jpg".into()
                },
                GalleryFile {
                    num: 2,
                    extension: "jpg".into()
                }
            ]
        );
        assert_eq!(post.info.id.as_deref(), Some("2020050253005918661"));
        assert_eq!(post.info.uploader.as_deref(), Some("猫神めたも"));
        assert_eq!(
            post.info.title.as_deref(),
            Some("雀聖2に昇段しました🍀たくさんの応援ありがとうございました🎵 #猫神めたも #雀魂")
        );
        assert_eq!(post.site_name, "X");
    }

    #[test]
    fn title_uses_only_the_collapsed_first_line_and_missing_optional_fields_drop() {
        let raw = r#"[[2,{"author":{"name":"handle"},"content":" first   line \nsecond"}],[3,"url",{"num":1,"extension":"png"}]]"#;
        let post = parse_metadata(raw).unwrap();
        assert_eq!(post.info.uploader.as_deref(), Some("handle"));
        assert_eq!(post.info.title.as_deref(), Some("first line"));
        let rendered = crate::naming::render_stem(
            &template(),
            &NamingContext {
                tz_offset_seconds: 0,
                site_name: post.site_name,
                title_budget_bytes: None,
            },
            &post.info,
            TargetOs::MacOs,
        )
        .unwrap();
        assert_eq!(rendered, ["handle", "first line"]);
    }

    #[test]
    fn fixture_renders_numbered_names_with_local_date() {
        let post = parse_metadata(FIXTURE).unwrap();
        let context = NamingContext {
            tz_offset_seconds: 9 * 3600,
            site_name: post.site_name,
            title_budget_bytes: None,
        };
        let rendered =
            crate::naming::render_stem(&template(), &context, &post.info, TargetOs::MacOs).unwrap();
        assert_eq!(rendered[0], "猫神めたも");
        assert!(format!("{} 1.jpg", rendered[1]).ends_with(" [2020050253005918661] 1.jpg"));
        assert!(format!("{} 2.jpg", rendered[1]).starts_with("2026-02-07 "));
    }

    #[test]
    fn braces_and_arguments_are_exact() {
        assert_eq!(escape_format("a{b}c"), "a{{b}}c");
        let cookies = Cookies::File(PathBuf::from("C:/ログ in/cookies.txt"));
        assert_eq!(
            metadata_args("https://example.test/post", &cookies),
            [
                "--config-ignore".into(),
                "-o".into(),
                "output.stdout=utf-8".into(),
                "-o".into(),
                "output.stderr=utf-8".into(),
                "-j".into(),
                "--cookies".into(),
                OsString::from("C:/ログ in/cookies.txt"),
                "https://example.test/post".into()
            ]
        );
        assert_eq!(
            download_args(
                Path::new("C:/保存 先/猫"),
                "題名 {ID}",
                "https://example.test/post",
                &cookies
            ),
            [
                "--config-ignore".into(),
                "-o".into(),
                "output.stdout=utf-8".into(),
                "-o".into(),
                "output.stderr=utf-8".into(),
                "--cookies".into(),
                OsString::from("C:/ログ in/cookies.txt"),
                "-D".into(),
                OsString::from("C:/保存 先/猫"),
                "-f".into(),
                "題名 {{ID}} {num}.{extension}".into(),
                "https://example.test/post".into()
            ]
        );
    }

    #[test]
    fn error_lines_map_to_existing_categories() {
        for (line, expected) in [
            (
                "[gallery-dl][error] Unsupported URL",
                ErrorCategory::UnsupportedUrl,
            ),
            (
                "[x][error] AuthorizationError: HTTP 403",
                ErrorCategory::LoginRequired,
            ),
            ("[x][error] NotFoundError: 404", ErrorCategory::Unavailable),
            ("[x][error] HTTP 429", ErrorCategory::RateLimited),
            ("[x][error] odd failure", ErrorCategory::Unknown),
        ] {
            assert_eq!(classify_error_line(line).unwrap().0, expected);
        }
    }
}

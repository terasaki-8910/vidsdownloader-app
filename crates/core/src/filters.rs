//! App-side filtering and equivalent yt-dlp match-filter generation.

use serde::{Deserialize, Serialize};

use crate::naming::{VideoInfo, local_date};

/// Cross-site filters applied to probed video metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub min_duration_s: Option<u32>,
    pub max_duration_s: Option<u32>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    #[serde(default)]
    pub title_includes: Vec<String>,
    #[serde(default)]
    pub title_excludes: Vec<String>,
    #[serde(default)]
    pub videos_only: bool,
    #[serde(default)]
    pub skip_live: bool,
    #[serde(default)]
    pub skip_downloaded: bool,
}

/// Result of evaluating all applicable filter conditions.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Keep,
    Skip(Vec<SkipReason>),
    Unknown(Vec<MissingField>),
}

/// A filter condition that rejected an item.
#[derive(Debug, Clone, PartialEq)]
pub enum SkipReason {
    BelowMinDuration { actual: f64, minimum: u32 },
    AboveMaxDuration { actual: f64, maximum: u32 },
    BeforeDate { actual: String, from: String },
    AfterDate { actual: String, to: String },
    TitleDoesNotIncludeAny(Vec<String>),
    TitleIncludesExcluded(String),
    NotVideo,
    LiveOrUpcoming,
    AlreadyDownloaded,
}

/// Metadata required by a condition but absent from the probe result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingField {
    Duration,
    Date,
    Title,
}

/// Evaluate every app-side condition, preferring concrete failures over unknowns.
pub fn evaluate(
    filter: &Filter,
    info: &VideoInfo,
    tz_offset_seconds: i32,
    is_downloaded: bool,
) -> Decision {
    let mut failures = Vec::new();
    let mut missing = Vec::new();

    if let Some(minimum) = filter.min_duration_s {
        match info.duration {
            Some(actual) if actual < f64::from(minimum) => {
                failures.push(SkipReason::BelowMinDuration { actual, minimum });
            }
            Some(_) => {}
            None => push_missing(&mut missing, MissingField::Duration),
        }
    }
    if let Some(maximum) = filter.max_duration_s {
        match info.duration {
            Some(actual) if actual > f64::from(maximum) => {
                failures.push(SkipReason::AboveMaxDuration { actual, maximum });
            }
            Some(_) => {}
            None => push_missing(&mut missing, MissingField::Duration),
        }
    }

    if filter.date_from.is_some() || filter.date_to.is_some() {
        let date = local_date(info, tz_offset_seconds)
            .ok()
            .flatten()
            .map(|value| value.replace('-', ""));
        match date {
            Some(actual) => {
                if let Some(from) = &filter.date_from
                    && actual < *from
                {
                    failures.push(SkipReason::BeforeDate {
                        actual: actual.clone(),
                        from: from.clone(),
                    });
                }
                if let Some(to) = &filter.date_to
                    && actual > *to
                {
                    failures.push(SkipReason::AfterDate {
                        actual,
                        to: to.clone(),
                    });
                }
            }
            None => push_missing(&mut missing, MissingField::Date),
        }
    }

    if !filter.title_includes.is_empty() || !filter.title_excludes.is_empty() {
        if let Some(title) = info.title.as_deref().filter(|title| !title.is_empty()) {
            let folded_title = title.to_lowercase();
            if !filter.title_includes.is_empty()
                && !filter
                    .title_includes
                    .iter()
                    .any(|keyword| folded_title.contains(&keyword.to_lowercase()))
            {
                failures.push(SkipReason::TitleDoesNotIncludeAny(
                    filter.title_includes.clone(),
                ));
            }
            for keyword in &filter.title_excludes {
                if folded_title.contains(&keyword.to_lowercase()) {
                    failures.push(SkipReason::TitleIncludesExcluded(keyword.clone()));
                }
            }
        } else {
            push_missing(&mut missing, MissingField::Title);
        }
    }

    if filter.videos_only && info.vcodec.as_deref() == Some("none") {
        failures.push(SkipReason::NotVideo);
    }
    if filter.skip_live && matches!(info.live_status.as_deref(), Some("is_live" | "is_upcoming")) {
        failures.push(SkipReason::LiveOrUpcoming);
    }
    if filter.skip_downloaded && is_downloaded {
        failures.push(SkipReason::AlreadyDownloaded);
    }

    if !failures.is_empty() {
        Decision::Skip(failures)
    } else if !missing.is_empty() {
        Decision::Unknown(missing)
    } else {
        Decision::Keep
    }
}

/// Build one yt-dlp `--match-filters` expression.
///
/// yt-dlp's `upload_date` is UTC, so local-date app-side evaluation is authoritative.
pub fn to_match_filter(filter: &Filter) -> Option<String> {
    let mut conditions = Vec::new();
    if let Some(minimum) = filter.min_duration_s {
        conditions.push(format!("duration >=? {minimum}"));
    }
    if let Some(maximum) = filter.max_duration_s {
        conditions.push(format!("duration <=? {maximum}"));
    }
    if let Some(from) = &filter.date_from {
        conditions.push(format!("upload_date >=? {from}"));
    }
    if let Some(to) = &filter.date_to {
        conditions.push(format!("upload_date <=? {to}"));
    }
    if !filter.title_includes.is_empty() {
        let alternatives = filter
            .title_includes
            .iter()
            .map(|keyword| escape_match_regex(keyword))
            .collect::<Vec<_>>()
            .join("|");
        let expression = if filter.title_includes.len() == 1 {
            alternatives
        } else {
            format!("(?:{alternatives})")
        };
        conditions.push(format!("title ~= '(?i){expression}'"));
    }
    for keyword in &filter.title_excludes {
        conditions.push(format!("title !~= '(?i){}'", escape_match_regex(keyword)));
    }
    if filter.videos_only {
        conditions.push("vcodec !=? 'none'".to_owned());
    }
    if filter.skip_live {
        conditions.push("live_status !=? 'is_live'".to_owned());
        conditions.push("live_status !=? 'is_upcoming'".to_owned());
    }
    (!conditions.is_empty()).then(|| conditions.join(" & "))
}

fn push_missing(missing: &mut Vec<MissingField>, field: MissingField) {
    if !missing.contains(&field) {
        missing.push(field);
    }
}

fn escape_match_regex(keyword: &str) -> String {
    regex::escape(keyword).replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> VideoInfo {
        VideoInfo {
            title: Some("Rust Video Tutorial".to_owned()),
            duration: Some(450.0),
            timestamp: Some(1_704_067_200),
            vcodec: Some("h264".to_owned()),
            live_status: Some("not_live".to_owned()),
            ..VideoInfo::default()
        }
    }

    #[test]
    fn evaluate_table() {
        struct Case {
            name: &'static str,
            filter: Filter,
            info: VideoInfo,
            downloaded: bool,
            expected: Decision,
        }
        let cases = vec![
            Case {
                name: "all conditions pass at inclusive bounds",
                filter: Filter {
                    min_duration_s: Some(450),
                    max_duration_s: Some(450),
                    date_from: Some("20240101".to_owned()),
                    date_to: Some("20240101".to_owned()),
                    title_includes: vec!["video".to_owned(), "other".to_owned()],
                    title_excludes: vec!["spoiler".to_owned()],
                    videos_only: true,
                    skip_live: true,
                    skip_downloaded: true,
                },
                info: info(),
                downloaded: false,
                expected: Decision::Keep,
            },
            Case {
                name: "every concrete failure is returned",
                filter: Filter {
                    min_duration_s: Some(500),
                    max_duration_s: Some(400),
                    date_from: Some("20240102".to_owned()),
                    date_to: Some("20231231".to_owned()),
                    title_includes: vec!["missing".to_owned()],
                    title_excludes: vec!["TUTORIAL".to_owned()],
                    videos_only: true,
                    skip_live: true,
                    skip_downloaded: true,
                },
                info: VideoInfo {
                    vcodec: Some("none".to_owned()),
                    live_status: Some("is_upcoming".to_owned()),
                    ..info()
                },
                downloaded: true,
                expected: Decision::Skip(vec![
                    SkipReason::BelowMinDuration {
                        actual: 450.0,
                        minimum: 500,
                    },
                    SkipReason::AboveMaxDuration {
                        actual: 450.0,
                        maximum: 400,
                    },
                    SkipReason::BeforeDate {
                        actual: "20240101".to_owned(),
                        from: "20240102".to_owned(),
                    },
                    SkipReason::AfterDate {
                        actual: "20240101".to_owned(),
                        to: "20231231".to_owned(),
                    },
                    SkipReason::TitleDoesNotIncludeAny(vec!["missing".to_owned()]),
                    SkipReason::TitleIncludesExcluded("TUTORIAL".to_owned()),
                    SkipReason::NotVideo,
                    SkipReason::LiveOrUpcoming,
                    SkipReason::AlreadyDownloaded,
                ]),
            },
            Case {
                name: "missing metadata is unknown",
                filter: Filter {
                    min_duration_s: Some(1),
                    date_from: Some("20200101".to_owned()),
                    title_includes: vec!["x".to_owned()],
                    videos_only: true,
                    skip_live: true,
                    ..Filter::default()
                },
                info: VideoInfo::default(),
                downloaded: false,
                expected: Decision::Unknown(vec![
                    MissingField::Duration,
                    MissingField::Date,
                    MissingField::Title,
                ]),
            },
            Case {
                name: "failure takes precedence over unknown",
                filter: Filter {
                    min_duration_s: Some(1),
                    skip_downloaded: true,
                    ..Filter::default()
                },
                info: VideoInfo::default(),
                downloaded: true,
                expected: Decision::Skip(vec![SkipReason::AlreadyDownloaded]),
            },
        ];
        for case in cases {
            assert_eq!(
                evaluate(&case.filter, &case.info, 0, case.downloaded),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn match_filter_table() {
        let cases = vec![
            ("empty", Filter::default(), None),
            (
                "numeric and dates",
                Filter {
                    min_duration_s: Some(300),
                    max_duration_s: Some(600),
                    date_from: Some("20240101".to_owned()),
                    date_to: Some("20241231".to_owned()),
                    ..Filter::default()
                },
                Some(
                    "duration >=? 300 & duration <=? 600 & upload_date >=? 20240101 & upload_date <=? 20241231",
                ),
            ),
            (
                "title escaping and any include",
                Filter {
                    title_includes: vec!["cats & dogs".to_owned(), "it's (fine)".to_owned()],
                    title_excludes: vec!["[spoiler]".to_owned()],
                    ..Filter::default()
                },
                Some(
                    "title ~= '(?i)(?:cats \\& dogs|it\\'s \\(fine\\))' & title !~= '(?i)\\[spoiler\\]'",
                ),
            ),
            (
                "video and live",
                Filter {
                    videos_only: true,
                    skip_live: true,
                    skip_downloaded: true,
                    ..Filter::default()
                },
                Some(
                    "vcodec !=? 'none' & live_status !=? 'is_live' & live_status !=? 'is_upcoming'",
                ),
            ),
        ];
        for (name, filter, expected) in cases {
            assert_eq!(to_match_filter(&filter).as_deref(), expected, "{name}");
        }
    }
}

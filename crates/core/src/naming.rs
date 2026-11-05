//! Pure compilation and rendering of download name templates.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A user-editable naming template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameTemplate {
    pub folders: Vec<Vec<Part>>,
    pub file: Vec<Part>,
}

/// One literal, field, or conditional part of a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Part {
    Text { text: String },
    Field { field: Token },
    Optional { optional: OptionalPart },
}

/// A field with surrounding text that disappears when the field is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionalPart {
    pub before: String,
    pub field: Token,
    pub after: String,
}

/// Metadata fields available to naming templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Token {
    Uploader,
    Channel,
    Site,
    Date,
    Title,
    Id,
    Playlist,
    Index,
    Series,
    EpisodeNumber,
    Episode,
}

/// Per-job values used while compiling a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamingContext {
    pub tz_offset_seconds: i32,
    pub site_name: String,
    pub title_budget_bytes: Option<usize>,
}

/// Probed metadata shared by naming and filtering.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoInfo {
    pub id: Option<String>,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub channel: Option<String>,
    pub uploader_id: Option<String>,
    pub timestamp: Option<i64>,
    pub upload_date: Option<String>,
    pub playlist_title: Option<String>,
    pub playlist: Option<String>,
    pub playlist_index: Option<u32>,
    pub series: Option<String>,
    pub episode_number: Option<u32>,
    pub episode: Option<String>,
    pub ext: Option<String>,
    pub extractor: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub live_status: Option<String>,
    #[serde(default)]
    pub vcodec: Option<String>,
}

/// Operating-system path rules used for rendering and length checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    Windows,
    MacOs,
}

/// Failures while compiling or fitting a name.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NamingError {
    #[error("the destination path is too long even with an empty title")]
    DestinationTooLong,
    #[error("timestamp is outside the supported calendar range")]
    InvalidTimestamp,
}

/// A structural problem that prevents a template from being used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateProblem {
    TextContainsPathSeparator,
    OptionalContainsReservedCharacter,
    TitleCannotBeOptional,
    FileMissingTitle,
}

/// A non-fatal naming-template warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateWarning {
    MissingId,
}

/// Compile a template to the per-job yt-dlp output-template syntax.
pub fn compile(
    template: &NameTemplate,
    context: &NamingContext,
    info: &VideoInfo,
) -> Result<String, NamingError> {
    let mut components = Vec::new();
    for folder in &template.folders {
        if !folder_is_empty(folder, info) {
            components.push(compile_parts(folder, context));
        }
    }
    components.push(format!(
        "{}.%({})s",
        compile_parts(&template.file, context),
        "ext"
    ));
    Ok(components.join("/"))
}

/// Render a template to sanitized relative path components.
pub fn render(
    template: &NameTemplate,
    context: &NamingContext,
    info: &VideoInfo,
    target: TargetOs,
) -> Result<Vec<String>, NamingError> {
    let ext = sanitize_filename(info.ext.as_deref().unwrap_or_default());
    render_with_suffix(template, context, info, target, &format!(".{ext}"))
}

/// Render the template's folders and file stem without an extension.
pub fn render_stem(
    template: &NameTemplate,
    context: &NamingContext,
    info: &VideoInfo,
    target: TargetOs,
) -> Result<Vec<String>, NamingError> {
    render_with_suffix(template, context, info, target, "")
}

fn render_with_suffix(
    template: &NameTemplate,
    context: &NamingContext,
    info: &VideoInfo,
    target: TargetOs,
    file_suffix: &str,
) -> Result<Vec<String>, NamingError> {
    let mut components = Vec::new();
    for folder in &template.folders {
        if !folder_is_empty(folder, info) {
            components.push(render_parts(folder, context, info)?);
        }
    }
    components.push(format!(
        "{}{}",
        render_parts(&template.file, context, info)?,
        file_suffix
    ));
    if target == TargetOs::Windows {
        Ok(sanitize_path_parts(&components))
    } else {
        Ok(components)
    }
}

/// Find the largest raw title byte budget that fits the target filesystem.
pub fn title_budget(
    template: &NameTemplate,
    context: &NamingContext,
    info: &VideoInfo,
    dest_dir: &str,
    target: TargetOs,
) -> Result<Option<usize>, NamingError> {
    let ext = sanitize_filename(info.ext.as_deref().unwrap_or_default());
    title_budget_with_suffix(
        template,
        context,
        info,
        dest_dir,
        target,
        &format!(".{ext}"),
    )
}

/// Find the largest title budget when the rendered file stem has a caller-defined suffix.
pub fn title_budget_with_suffix(
    template: &NameTemplate,
    context: &NamingContext,
    info: &VideoInfo,
    dest_dir: &str,
    target: TargetOs,
    file_suffix: &str,
) -> Result<Option<usize>, NamingError> {
    let title_len = info.title.as_deref().unwrap_or_default().len();
    let mut candidate = context.clone();
    candidate.title_budget_bytes = None;
    if path_fits_with_suffix(template, &candidate, info, dest_dir, target, file_suffix)? {
        return Ok(None);
    }
    for budget in (0..=title_len).rev() {
        candidate.title_budget_bytes = Some(budget);
        if path_fits_with_suffix(template, &candidate, info, dest_dir, target, file_suffix)? {
            return Ok(Some(budget));
        }
    }
    Err(NamingError::DestinationTooLong)
}

/// Validate path separators, optional syntax, and the required title field.
pub fn validate(template: &NameTemplate) -> Result<(), Vec<TemplateProblem>> {
    let mut problems = Vec::new();
    for part in template.folders.iter().flatten().chain(&template.file) {
        match part {
            Part::Text { text } if text.contains(['/', '\\']) => {
                problems.push(TemplateProblem::TextContainsPathSeparator);
            }
            Part::Optional { optional } => {
                if optional.field == Token::Title {
                    problems.push(TemplateProblem::TitleCannotBeOptional);
                }
                if optional
                    .before
                    .chars()
                    .chain(optional.after.chars())
                    .any(|c| "(){}|&%/\\".contains(c))
                {
                    problems.push(TemplateProblem::OptionalContainsReservedCharacter);
                }
            }
            _ => {}
        }
    }
    if !template.file.iter().any(|part| {
        matches!(
            part,
            Part::Field {
                field: Token::Title
            }
        )
    }) {
        problems.push(TemplateProblem::FileMissingTitle);
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Return warnings for collision-prone but valid templates.
pub fn lint(template: &NameTemplate) -> Vec<TemplateWarning> {
    let has_id = template.file.iter().any(|part| match part {
        Part::Field { field } => *field == Token::Id,
        Part::Optional { optional } => optional.field == Token::Id,
        Part::Text { .. } => false,
    });
    if has_id {
        Vec::new()
    } else {
        vec![TemplateWarning::MissingId]
    }
}

fn compile_parts(parts: &[Part], context: &NamingContext) -> String {
    parts
        .iter()
        .map(|part| compile_part(part, context))
        .collect()
}

fn compile_part(part: &Part, context: &NamingContext) -> String {
    match part {
        Part::Text { text } => text.replace('%', "%%"),
        Part::Field { field: Token::Site } => {
            sanitize_filename(&context.site_name).replace('%', "%%")
        }
        Part::Field {
            field: Token::Title,
        } if context.title_budget_bytes.is_some() => {
            format!(
                "%({}|).{}B",
                expression(Token::Title, context),
                context.title_budget_bytes.unwrap_or_default()
            )
        }
        Part::Field { field } => format!("%({}|)s", expression(*field, context)),
        Part::Optional { optional } => format!(
            "%({}&{}{{}}{}|)s",
            expression(optional.field, context),
            optional.before,
            optional.after
        ),
    }
}

fn expression(token: Token, context: &NamingContext) -> String {
    match token {
        Token::Uploader => "uploader,channel,uploader_id".into(),
        Token::Channel => "channel,uploader".into(),
        Token::Site => "extractor".into(),
        Token::Date => {
            let timestamp = match context.tz_offset_seconds.cmp(&0) {
                std::cmp::Ordering::Equal => "timestamp".into(),
                std::cmp::Ordering::Greater => {
                    format!("timestamp+{}", context.tz_offset_seconds)
                }
                std::cmp::Ordering::Less => {
                    format!("timestamp-{}", context.tz_offset_seconds.unsigned_abs())
                }
            };
            format!("{timestamp}>%Y-%m-%d,upload_date>%Y-%m-%d")
        }
        Token::Title => "title".into(),
        Token::Id => "id".into(),
        Token::Playlist => "playlist_title,playlist".into(),
        Token::Index => "playlist_index".into(),
        Token::Series => "series".into(),
        Token::EpisodeNumber => "episode_number".into(),
        Token::Episode => "episode".into(),
    }
}

fn folder_is_empty(parts: &[Part], info: &VideoInfo) -> bool {
    parts.iter().all(|part| match part {
        Part::Text { text } => text.is_empty(),
        Part::Field { field } => !has_value(*field, info),
        Part::Optional { optional } => !has_value(optional.field, info),
    })
}

fn has_value(token: Token, info: &VideoInfo) -> bool {
    match token {
        Token::Site => true,
        Token::Date => {
            info.timestamp.is_some()
                || info
                    .upload_date
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
        }
        _ => field_value(token, info).is_some(),
    }
}

fn field_value(token: Token, info: &VideoInfo) -> Option<String> {
    let nonempty = |value: &Option<String>| value.as_ref().filter(|s| !s.is_empty()).cloned();
    match token {
        Token::Uploader => nonempty(&info.uploader)
            .or_else(|| nonempty(&info.channel))
            .or_else(|| nonempty(&info.uploader_id)),
        Token::Channel => nonempty(&info.channel).or_else(|| nonempty(&info.uploader)),
        Token::Site => Some(String::new()),
        Token::Date => None,
        Token::Title => nonempty(&info.title),
        Token::Id => nonempty(&info.id),
        Token::Playlist => nonempty(&info.playlist_title).or_else(|| nonempty(&info.playlist)),
        Token::Index => info.playlist_index.map(|v| v.to_string()),
        Token::Series => nonempty(&info.series),
        Token::EpisodeNumber => info.episode_number.map(|v| v.to_string()),
        Token::Episode => nonempty(&info.episode),
    }
}

fn render_parts(
    parts: &[Part],
    context: &NamingContext,
    info: &VideoInfo,
) -> Result<String, NamingError> {
    let mut rendered = String::new();
    for part in parts {
        match part {
            Part::Text { text } => rendered.push_str(text),
            Part::Field { field } => {
                let value = evaluated_value(*field, context, info)?;
                let value = if *field == Token::Title {
                    truncate_utf8(&value.unwrap_or_default(), context.title_budget_bytes)
                } else {
                    value.unwrap_or_default()
                };
                rendered.push_str(&sanitize_filename(&value));
            }
            Part::Optional { optional } => {
                if let Some(value) = evaluated_value(optional.field, context, info)? {
                    let replacement = format!("{}{}{}", optional.before, value, optional.after);
                    rendered.push_str(&sanitize_filename(&replacement));
                }
            }
        }
    }
    Ok(rendered)
}

fn evaluated_value(
    token: Token,
    context: &NamingContext,
    info: &VideoInfo,
) -> Result<Option<String>, NamingError> {
    match token {
        Token::Site => Ok(Some(context.site_name.clone())),
        Token::Date => local_date(info, context.tz_offset_seconds),
        _ => Ok(field_value(token, info)),
    }
}

pub(crate) fn local_date(
    info: &VideoInfo,
    tz_offset_seconds: i32,
) -> Result<Option<String>, NamingError> {
    if let Some(timestamp) = info.timestamp {
        let shifted = timestamp
            .checked_add(i64::from(tz_offset_seconds))
            .ok_or(NamingError::InvalidTimestamp)?;
        let value = OffsetDateTime::from_unix_timestamp(shifted)
            .map_err(|_| NamingError::InvalidTimestamp)?;
        return Ok(Some(value.date().to_string()));
    }
    Ok(info.upload_date.as_deref().and_then(format_upload_date))
}

fn format_upload_date(value: &str) -> Option<String> {
    if value.len() == 8 && value.bytes().all(|b| b.is_ascii_digit()) {
        Some(format!(
            "{}-{}-{}",
            &value[0..4],
            &value[4..6],
            &value[6..8]
        ))
    } else {
        None
    }
}

fn truncate_utf8(value: &str, budget: Option<usize>) -> String {
    let Some(budget) = budget else {
        return value.to_owned();
    };
    let end = value.len().min(budget);
    let mut boundary = end;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

// Built once: sanitizing runs for every field of every candidate while fitting a title.
static CLOCK_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]+(?::[0-9]+)+").expect("static regex"));
static SUBSTITUTE_EDGES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^\x00.(?:\x00.|[ _-])*)|(?:(?:\x00.|[ _-])*\x00.$)").expect("static regex")
});
static INVALID_PATH_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[/<>:\"|\\?*]|[\s.]$"#).expect("static regex"));

fn sanitize_filename(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let value = CLOCK_TIME.replace_all(value, |caps: &regex::Captures<'_>| {
        caps[0].replace(':', "_")
    });
    let mut replaced = String::new();
    for c in value.chars() {
        match c {
            '\n' => replaced.push_str("\0 "),
            '"' | '*' | ':' | '<' | '>' | '?' | '|' => {
                replaced.push(char::from_u32(u32::from(c) + 0xfee0).expect("full-width character"));
            }
            '/' => replaced.push('⧸'),
            '\\' => replaced.push('⧹'),
            _ if u32::from(c) < 32 || c == '\u{7f}' => {}
            _ => replaced.push(c),
        }
    }

    let mut collapsed = String::new();
    let chars: Vec<char> = replaced.chars().collect();
    let mut index = 0;
    let mut previous_substitute = None;
    while index < chars.len() {
        if chars[index] == '\0' && index + 1 < chars.len() {
            let pair = chars[index + 1];
            if previous_substitute != Some(pair) {
                collapsed.push('\0');
                collapsed.push(pair);
            }
            previous_substitute = Some(pair);
            index += 2;
        } else {
            collapsed.push(chars[index]);
            previous_substitute = None;
            index += 1;
        }
    }

    let result = SUBSTITUTE_EDGES
        .replace_all(&collapsed, "")
        .replace('\0', "");
    if result.is_empty() {
        "_".to_owned()
    } else {
        result
    }
}

pub(crate) fn sanitize_file_part(value: &str) -> String {
    sanitize_filename(value)
}

fn sanitize_path_parts(parts: &[String]) -> Vec<String> {
    let mut sanitized = Vec::new();
    for part in parts {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if sanitized.last().is_some_and(|last| last != "..") {
                sanitized.pop();
            } else {
                sanitized.push(part.clone());
            }
        } else {
            sanitized.push(INVALID_PATH_CHARS.replace_all(part, "#").into_owned());
        }
    }
    sanitized
}

#[cfg(test)]
fn path_fits(
    template: &NameTemplate,
    context: &NamingContext,
    info: &VideoInfo,
    dest_dir: &str,
    target: TargetOs,
) -> Result<bool, NamingError> {
    let ext = sanitize_filename(info.ext.as_deref().unwrap_or_default());
    path_fits_with_suffix(
        template,
        context,
        info,
        dest_dir,
        target,
        &format!(".{ext}"),
    )
}

fn path_fits_with_suffix(
    template: &NameTemplate,
    context: &NamingContext,
    info: &VideoInfo,
    dest_dir: &str,
    target: TargetOs,
    file_suffix: &str,
) -> Result<bool, NamingError> {
    const INTERMEDIATE_SUFFIX_RESERVE: usize = 20;
    let components = render_with_suffix(template, context, info, target, file_suffix)?;
    Ok(match target {
        TargetOs::Windows => {
            let component_units: usize = components
                .iter()
                .map(|part| part.encode_utf16().count())
                .sum();
            dest_dir.encode_utf16().count()
                + usize::from(!components.is_empty())
                + component_units
                + components.len().saturating_sub(1)
                + INTERMEDIATE_SUFFIX_RESERVE
                <= 259
        }
        TargetOs::MacOs => {
            let destination_components_fit = dest_dir.split('/').all(|part| part.len() <= 255);
            let rendered_components_fit = components.iter().enumerate().all(|(index, part)| {
                part.len()
                    + if index + 1 == components.len() {
                        INTERMEDIATE_SUFFIX_RESERVE
                    } else {
                        0
                    }
                    <= 255
            });
            let whole = dest_dir.len()
                + usize::from(!components.is_empty())
                + components.iter().map(String::len).sum::<usize>()
                + components.len().saturating_sub(1)
                + INTERMEDIATE_SUFFIX_RESERVE;
            destination_components_fit && rendered_components_fit && whole <= 1023
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn title_template() -> NameTemplate {
        NameTemplate {
            folders: vec![],
            file: vec![Part::Field {
                field: Token::Title,
            }],
        }
    }

    fn info(title: &str) -> VideoInfo {
        VideoInfo {
            title: Some(title.to_owned()),
            ext: Some("mp4".to_owned()),
            ..VideoInfo::default()
        }
    }

    fn context() -> NamingContext {
        NamingContext {
            tz_offset_seconds: 0,
            site_name: "site".to_owned(),
            title_budget_bytes: None,
        }
    }

    #[test]
    fn title_budget_returns_none_when_the_name_fits() {
        assert_eq!(
            title_budget(
                &title_template(),
                &context(),
                &info("short"),
                "C:\\out",
                TargetOs::Windows
            ),
            Ok(None)
        );
    }

    #[test]
    fn title_budget_counts_raw_bytes_before_sanitizing() {
        let title = "a/".repeat(200);
        let budget = title_budget(
            &title_template(),
            &context(),
            &info(&title),
            "C:\\out",
            TargetOs::Windows,
        )
        .expect("a budget should fit")
        .expect("the full title should not fit");
        assert!(budget < title.len());
        let mut limited = context();
        limited.title_budget_bytes = Some(budget);
        assert!(
            path_fits(
                &title_template(),
                &limited,
                &info(&title),
                "C:\\out",
                TargetOs::Windows
            )
            .unwrap()
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn returned_budget_is_maximal_and_fits(
            chars in prop::collection::vec(
                prop_oneof![
                    (0x20u8..0x7fu8).prop_map(char::from),
                    Just('日'), Just('本'), Just('語'), Just('😀'), Just('\n'),
                    Just('"'), Just('*'), Just(':'), Just('<'), Just('>'), Just('?'),
                    Just('|'), Just('/'), Just('\\'),
                ],
                0..100,
            ),
            dest_len in 3usize..201,
            windows in any::<bool>(),
        ) {
            let title: String = chars.into_iter().collect();
            let dest = "d".repeat(dest_len);
            let target = if windows { TargetOs::Windows } else { TargetOs::MacOs };
            match title_budget(&title_template(), &context(), &info(&title), &dest, target) {
                Ok(None) => prop_assert!(path_fits(&title_template(), &context(), &info(&title), &dest, target).unwrap()),
                Ok(Some(budget)) => {
                    let mut limited = context();
                    limited.title_budget_bytes = Some(budget);
                    prop_assert!(path_fits(&title_template(), &limited, &info(&title), &dest, target).unwrap());
                    if budget < title.len() {
                        limited.title_budget_bytes = Some(budget + 1);
                        prop_assert!(!path_fits(&title_template(), &limited, &info(&title), &dest, target).unwrap());
                    }
                }
                Err(NamingError::DestinationTooLong) => {
                    let mut empty = context();
                    empty.title_budget_bytes = Some(0);
                    prop_assert!(!path_fits(&title_template(), &empty, &info(&title), &dest, target).unwrap());
                }
                Err(error) => prop_assert!(false, "unexpected error: {error}"),
            }
        }
    }
}

//! What yt-dlp makes of a URL given to "Add a site": a list, a single video, a login wall,
//! nothing it knows, or a failure. The run is `executor::probe_list_json` (one flat `-J`
//! run, no download, with the site's login only when asked to); this module only reads its
//! result.

use serde::Serialize;
use serde_json::Value;

use crate::queue::JobFailure;
use crate::types::ErrorCategory;

/// Whether the entries of a list are videos yt-dlp has an extractor for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ListVideos {
    /// At least one entry names an extractor other than the generic one.
    Supported,
    /// No entry names one: whether the pages open is only known by opening one.
    Unknown,
}

/// What the app reports about a URL, as the UI receives it (`kind` is the tag).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SiteProbeReport {
    /// A list (channel, playlist, ...) with `count` entries. `logged_in`: the run used the
    /// site's login, so the list is what a logged-in person sees.
    List {
        count: usize,
        videos: ListVideos,
        #[serde(rename = "loggedIn")]
        logged_in: bool,
    },
    /// A single video yt-dlp handles; `logged_in` as for a list.
    Video {
        #[serde(rename = "loggedIn")]
        logged_in: bool,
    },
    /// The run failed because the page needs a login.
    LoginRequired,
    /// yt-dlp has no extractor for the URL, or only its generic one answered and found no
    /// video.
    Unsupported,
    /// Anything else: a network error, a broken extractor, a timeout.
    Failed {
        category: ErrorCategory,
        message: String,
    },
}

/// Turns the outcome of a list probe into the report. `logged_in`: the run was given the
/// site's login (cookies); a failure says the same either way.
pub fn classify(result: Result<Value, JobFailure>, logged_in: bool) -> SiteProbeReport {
    match result {
        Ok(json) => classify_json(&json, logged_in),
        Err(JobFailure { category, message }) => match category {
            ErrorCategory::LoginRequired => SiteProbeReport::LoginRequired,
            ErrorCategory::UnsupportedUrl | ErrorCategory::NoVideo => SiteProbeReport::Unsupported,
            category => SiteProbeReport::Failed { category, message },
        },
    }
}

fn classify_json(json: &Value, logged_in: bool) -> SiteProbeReport {
    if !json.is_object() {
        return SiteProbeReport::Failed {
            category: ErrorCategory::Unknown,
            message: "yt-dlp returned something that is not an object".to_owned(),
        };
    }
    let entries = json.get("entries").and_then(Value::as_array);
    let is_list =
        json.get("_type").and_then(Value::as_str) == Some("playlist") && entries.is_some();
    if is_list {
        let entries = entries.map_or(&[][..], Vec::as_slice);
        if entries.is_empty() && is_generic(json) {
            // The generic extractor read the page and found no video in it.
            return SiteProbeReport::Unsupported;
        }
        let supported = entries
            .iter()
            .any(|entry| extractor_key(entry).is_some_and(|key| !is_generic_key(key)));
        return SiteProbeReport::List {
            count: entries.len(),
            videos: if supported {
                ListVideos::Supported
            } else {
                ListVideos::Unknown
            },
            logged_in,
        };
    }
    if is_generic(json) && !has_media(json) {
        return SiteProbeReport::Unsupported;
    }
    SiteProbeReport::Video { logged_in }
}

/// The extractor an entry names: `ie_key`, else `extractor_key`.
fn extractor_key(entry: &Value) -> Option<&str> {
    ["ie_key", "extractor_key"]
        .into_iter()
        .filter_map(|key| entry.get(key).and_then(Value::as_str))
        .map(str::trim)
        .find(|key| !key.is_empty())
}

fn is_generic_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("generic")
}

/// Whether the generic extractor produced this result.
fn is_generic(json: &Value) -> bool {
    ["extractor_key", "extractor"]
        .into_iter()
        .filter_map(|key| json.get(key).and_then(Value::as_str))
        .any(is_generic_key)
}

/// Whether a single-video result points at something to download.
fn has_media(json: &Value) -> bool {
    let url = json
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.is_empty());
    let formats = json
        .get("formats")
        .and_then(Value::as_array)
        .is_some_and(|formats| !formats.is_empty());
    url || formats
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{ListVideos, SiteProbeReport, classify as classify_with_login};
    use crate::parse::parse_line;
    use crate::queue::JobFailure;
    use crate::types::{ErrorCategory, OutputLine, OutputStream, ParsedLine};

    /// `yt-dlp -J --flat-playlist` on a Vimeo channel: a playlist of `Vimeo` entries.
    fn vimeo_channel() -> Value {
        json!({
            "id": "staffpicks",
            "title": "Vimeo Staff Picks",
            "_type": "playlist",
            "extractor": "vimeo:channel",
            "extractor_key": "VimeoChannel",
            "webpage_url": "https://vimeo.com/channels/staffpicks",
            "entries": [
                {
                    "_type": "url",
                    "ie_key": "Vimeo",
                    "id": "1012345678",
                    "url": "https://vimeo.com/1012345678",
                    "title": "Night Swim",
                    "duration": 212
                },
                {
                    "_type": "url",
                    "ie_key": "Vimeo",
                    "id": "1012345679",
                    "url": "https://vimeo.com/1012345679",
                    "title": "Paper Boats",
                    "duration": 98
                }
            ]
        })
    }

    /// A single video as `-J` prints it: no `_type: playlist`, no `entries`.
    fn youtube_video() -> Value {
        json!({
            "id": "aqz-KE-bpKQ",
            "title": "Big Buck Bunny 60fps 4K",
            "extractor": "youtube",
            "extractor_key": "Youtube",
            "webpage_url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
            "duration": 635,
            "formats": [{ "format_id": "18", "url": "https://rr1.example/videoplayback" }]
        })
    }

    /// An article that embeds two videos the generic extractor found: a playlist of its own
    /// `Generic` entries.
    fn generic_page_with_videos() -> Value {
        json!({
            "id": "article",
            "title": "An article",
            "_type": "playlist",
            "extractor": "generic",
            "extractor_key": "Generic",
            "entries": [
                { "_type": "url", "ie_key": "Generic", "url": "https://cdn.example/a.mp4" },
                { "_type": "url", "ie_key": "Generic", "url": "https://cdn.example/b.mp4" }
            ]
        })
    }

    fn failure(category: ErrorCategory, message: &str) -> JobFailure {
        JobFailure {
            category,
            message: message.to_owned(),
        }
    }

    /// A run without the site's login, which is how the first probe of a new site goes.
    fn classify(result: Result<Value, JobFailure>) -> SiteProbeReport {
        classify_with_login(result, false)
    }

    /// The failure a probe returns for this stderr line, as `run_probe` builds it.
    fn failure_from_error_line(text: &str) -> JobFailure {
        let line = OutputLine {
            stream: OutputStream::Stderr,
            text: text.to_owned(),
        };
        match parse_line(&line) {
            ParsedLine::Error { category, message } => JobFailure { category, message },
            other => panic!("not an ERROR line: {other:?}"),
        }
    }

    #[test]
    fn a_list_of_site_videos_is_a_list_whose_videos_are_supported() {
        assert_eq!(
            classify(Ok(vimeo_channel())),
            SiteProbeReport::List {
                count: 2,
                videos: ListVideos::Supported,
                logged_in: false
            }
        );
    }

    #[test]
    fn one_non_generic_entry_is_enough_for_supported() {
        let mut list = generic_page_with_videos();
        list["entries"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "_type": "url", "ie_key": "Youtube", "id": "abcdefghijk" }));
        assert_eq!(
            classify(Ok(list)),
            SiteProbeReport::List {
                count: 3,
                videos: ListVideos::Supported,
                logged_in: false
            }
        );
    }

    #[test]
    fn a_list_of_generic_entries_is_unknown() {
        assert_eq!(
            classify(Ok(generic_page_with_videos())),
            SiteProbeReport::List {
                count: 2,
                videos: ListVideos::Unknown,
                logged_in: false
            }
        );
    }

    #[test]
    fn entries_without_an_extractor_do_not_count_as_supported() {
        let list = json!({
            "_type": "playlist",
            "extractor_key": "SomeTab",
            "entries": [{ "id": "1", "url": "https://example.com/1" }]
        });
        assert_eq!(
            classify(Ok(list)),
            SiteProbeReport::List {
                count: 1,
                videos: ListVideos::Unknown,
                logged_in: false
            }
        );
    }

    #[test]
    fn an_entry_names_its_extractor_by_extractor_key_when_it_has_no_ie_key() {
        let list = json!({
            "_type": "playlist",
            "entries": [{ "id": "1", "extractor_key": "Twitch", "title": "t" }]
        });
        assert_eq!(
            classify(Ok(list)),
            SiteProbeReport::List {
                count: 1,
                videos: ListVideos::Supported,
                logged_in: false
            }
        );
    }

    #[test]
    fn an_empty_list_of_a_known_site_is_still_a_list() {
        let list = json!({
            "_type": "playlist",
            "extractor": "vimeo:channel",
            "extractor_key": "VimeoChannel",
            "entries": []
        });
        assert_eq!(
            classify(Ok(list)),
            SiteProbeReport::List {
                count: 0,
                videos: ListVideos::Unknown,
                logged_in: false
            }
        );
    }

    #[test]
    fn a_generic_list_without_entries_is_unsupported() {
        let list = json!({
            "id": "page",
            "_type": "playlist",
            "extractor": "generic",
            "extractor_key": "Generic",
            "entries": []
        });
        assert_eq!(classify(Ok(list)), SiteProbeReport::Unsupported);
    }

    #[test]
    fn a_single_video_is_a_video() {
        assert_eq!(
            classify(Ok(youtube_video())),
            SiteProbeReport::Video { logged_in: false }
        );
    }

    #[test]
    fn a_single_generic_video_with_a_media_url_is_a_video() {
        let video = json!({
            "id": "clip",
            "title": "clip",
            "extractor": "generic",
            "extractor_key": "Generic",
            "url": "https://cdn.example/clip.mp4",
            "ext": "mp4"
        });
        assert_eq!(
            classify(Ok(video)),
            SiteProbeReport::Video { logged_in: false }
        );
    }

    #[test]
    fn a_generic_result_with_nothing_to_download_is_unsupported() {
        let page = json!({
            "id": "page",
            "title": "A page",
            "extractor": "generic",
            "extractor_key": "Generic",
            "webpage_url": "https://example.com/page"
        });
        assert_eq!(classify(Ok(page)), SiteProbeReport::Unsupported);
    }

    #[test]
    fn output_that_is_not_an_object_is_a_failure() {
        assert!(matches!(
            classify(Ok(json!([1, 2]))),
            SiteProbeReport::Failed {
                category: ErrorCategory::Unknown,
                ..
            }
        ));
    }

    #[test]
    fn login_required_errors_are_a_login_wall() {
        for line in [
            "ERROR: [youtube] aqz-KE-bpKQ: Sign in to confirm you\u{2019}re not a bot. Use --cookies-from-browser or --cookies for the authentication.",
            "ERROR: [twitter] 1234567890: NSFW tweet requires authentication. Use --cookies",
            "ERROR: [vimeo] 99: This video is members only. Log in to watch",
        ] {
            let error = failure_from_error_line(line);
            assert_eq!(error.category, ErrorCategory::LoginRequired, "{line}");
            assert_eq!(
                classify(Err(error)),
                SiteProbeReport::LoginRequired,
                "{line}"
            );
        }
    }

    #[test]
    fn unsupported_url_and_no_video_errors_are_unsupported() {
        for (line, category) in [
            (
                "ERROR: Unsupported URL: https://note.com/pichikyo/n/nfe806dde550a",
                ErrorCategory::UnsupportedUrl,
            ),
            (
                "ERROR: [generic] Unable to download webpage: No video formats found!",
                ErrorCategory::NoVideo,
            ),
        ] {
            let error = failure_from_error_line(line);
            assert_eq!(error.category, category, "{line}");
            assert_eq!(classify(Err(error)), SiteProbeReport::Unsupported, "{line}");
        }
    }

    #[test]
    fn other_errors_keep_their_category_and_message() {
        let error = failure_from_error_line(
            "ERROR: [vimeo] 99: Unable to download webpage: <urlopen error [Errno 11001] getaddrinfo failed>",
        );
        assert_eq!(error.category, ErrorCategory::Network);
        assert_eq!(
            classify(Err(error.clone())),
            SiteProbeReport::Failed {
                category: ErrorCategory::Network,
                message: error.message
            }
        );

        let timeout = failure(
            ErrorCategory::Network,
            "the list check timed out after 90 seconds",
        );
        assert!(matches!(
            classify(Err(timeout)),
            SiteProbeReport::Failed {
                category: ErrorCategory::Network,
                ..
            }
        ));
    }

    #[test]
    fn a_run_with_the_login_says_so_in_a_list_and_a_video() {
        assert_eq!(
            classify_with_login(Ok(vimeo_channel()), true),
            SiteProbeReport::List {
                count: 2,
                videos: ListVideos::Supported,
                logged_in: true
            }
        );
        assert_eq!(
            classify_with_login(Ok(youtube_video()), true),
            SiteProbeReport::Video { logged_in: true }
        );
    }

    #[test]
    fn a_run_with_the_login_does_not_change_the_other_reports() {
        let login_wall = failure_from_error_line(
            "ERROR: [vimeo] 99: This video is members only. Log in to watch",
        );
        assert_eq!(
            classify_with_login(Err(login_wall), true),
            SiteProbeReport::LoginRequired
        );
        let unsupported = failure_from_error_line("ERROR: Unsupported URL: https://example.com/x");
        assert_eq!(
            classify_with_login(Err(unsupported), true),
            SiteProbeReport::Unsupported
        );
        let network = failure(ErrorCategory::Network, "timed out");
        assert_eq!(
            classify_with_login(Err(network.clone()), true),
            classify(Err(network))
        );
    }

    #[test]
    fn the_report_serializes_as_the_ui_reads_it() {
        let value = |report| serde_json::to_value(report).unwrap();
        assert_eq!(
            value(SiteProbeReport::List {
                count: 120,
                videos: ListVideos::Supported,
                logged_in: false
            }),
            json!({ "kind": "list", "count": 120, "videos": "supported", "loggedIn": false })
        );
        assert_eq!(
            value(SiteProbeReport::List {
                count: 3,
                videos: ListVideos::Unknown,
                logged_in: true
            }),
            json!({ "kind": "list", "count": 3, "videos": "unknown", "loggedIn": true })
        );
        assert_eq!(
            value(SiteProbeReport::Video { logged_in: false }),
            json!({ "kind": "video", "loggedIn": false })
        );
        assert_eq!(
            value(SiteProbeReport::Video { logged_in: true }),
            json!({ "kind": "video", "loggedIn": true })
        );
        assert_eq!(
            value(SiteProbeReport::LoginRequired),
            json!({ "kind": "loginRequired" })
        );
        assert_eq!(
            value(SiteProbeReport::Unsupported),
            json!({ "kind": "unsupported" })
        );
        assert_eq!(
            value(SiteProbeReport::Failed {
                category: ErrorCategory::Network,
                message: "timed out".to_owned()
            }),
            json!({ "kind": "failed", "category": "Network", "message": "timed out" })
        );
    }
}

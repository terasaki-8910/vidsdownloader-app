//! What the collect tray's filter and sort need to know about one found item: who uploaded it,
//! how long it is and when it was published. Read from the JSON of one yt-dlp `-J` run on the
//! item's page (see `executor::probe_list_json`); this module only reads that result.

use serde::Serialize;
use serde_json::Value;
use time::{Date, Month};

/// How deep a playlist inside a playlist is searched for a missing field. yt-dlp nests a
/// result at most a level or two; the bound only keeps a strange result from recursing.
const MAX_PLAYLIST_DEPTH: usize = 3;

/// The facts the filter needs; a field the page did not tell is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemMeta {
    /// The first non-empty of `uploader`, `channel`, `creator`, `uploader_id`.
    pub uploader: Option<String>,
    /// Length in whole seconds (`duration`, rounded).
    pub duration_s: Option<u64>,
    /// Unix seconds: `timestamp`, else `release_timestamp`, else `upload_date` (UTC midnight).
    pub published_at: Option<i64>,
}

impl ItemMeta {
    fn complete(&self) -> bool {
        self.uploader.is_some() && self.duration_s.is_some() && self.published_at.is_some()
    }

    /// Takes each field that is still missing from `other`.
    fn fill_from(&mut self, other: ItemMeta) {
        if self.uploader.is_none() {
            self.uploader = other.uploader;
        }
        if self.duration_s.is_none() {
            self.duration_s = other.duration_s;
        }
        if self.published_at.is_none() {
            self.published_at = other.published_at;
        }
    }
}

/// Reads the facts out of yt-dlp's `-J` JSON. A playlist result (an X post with several
/// videos, for example) takes each field it lacks from the first entry that has it.
pub fn item_meta(json: &Value) -> ItemMeta {
    read(json, 0)
}

fn read(json: &Value, depth: usize) -> ItemMeta {
    let mut meta = ItemMeta {
        uploader: ["uploader", "channel", "creator", "uploader_id"]
            .iter()
            .find_map(|key| non_empty_text(json, key)),
        duration_s: duration(json),
        published_at: published_at(json),
    };
    let is_playlist = json.get("_type").and_then(Value::as_str) == Some("playlist");
    if is_playlist && depth < MAX_PLAYLIST_DEPTH {
        for entry in json
            .get("entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if meta.complete() {
                break;
            }
            meta.fill_from(read(entry, depth + 1));
        }
    }
    meta
}

fn non_empty_text(json: &Value, key: &str) -> Option<String> {
    let text = json.get(key)?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// `duration` in whole seconds: a number, rounded; a negative one is no answer.
fn duration(json: &Value) -> Option<u64> {
    let seconds = json.get("duration")?.as_f64()?;
    (seconds.is_finite() && seconds >= 0.0).then(|| seconds.round() as u64)
}

fn published_at(json: &Value) -> Option<i64> {
    unix_seconds(json, "timestamp")
        .or_else(|| unix_seconds(json, "release_timestamp"))
        .or_else(|| upload_date(json))
}

fn unix_seconds(json: &Value, key: &str) -> Option<i64> {
    let value = json.get(key)?;
    if let Some(seconds) = value.as_i64() {
        return Some(seconds);
    }
    let seconds = value.as_f64()?;
    (seconds.is_finite() && seconds.abs() < 9.0e15).then(|| seconds.round() as i64)
}

/// `upload_date` ("YYYYMMDD", a UTC date) as Unix seconds at UTC midnight.
fn upload_date(json: &Value) -> Option<i64> {
    let text = json.get("upload_date")?.as_str()?;
    if text.len() != 8 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year: i32 = text[0..4].parse().ok()?;
    let month: u8 = text[4..6].parse().ok()?;
    let day: u8 = text[6..8].parse().ok()?;
    let date = Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()?;
    Some(date.midnight().assume_utc().unix_timestamp())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_youtube_video_gives_uploader_length_and_the_upload_date_at_utc_midnight() {
        // Trimmed from a `-J` run on a single video: no `timestamp`, only `upload_date`.
        let meta = item_meta(&json!({
            "id": "eRsGyueVLvQ",
            "title": "Sintel",
            "uploader": "Blender",
            "channel": "Blender",
            "uploader_id": "@BlenderOfficial",
            "duration": 888,
            "duration_string": "14:48",
            "upload_date": "20100930",
            "timestamp": null,
            "release_timestamp": null,
            "view_count": 123,
            "live_status": "not_live"
        }));
        assert_eq!(
            meta,
            ItemMeta {
                uploader: Some("Blender".to_owned()),
                duration_s: Some(888),
                published_at: Some(1_285_804_800),
            }
        );
    }

    #[test]
    fn a_niconico_video_prefers_the_timestamp_to_the_upload_date() {
        let meta = item_meta(&json!({
            "id": "sm1715919",
            "title": "初音ミクを歌わせてみた",
            "uploader": "ika",
            "uploader_id": "1594318",
            "duration": 252.0,
            "timestamp": 1_190_270_508,
            "upload_date": "20070920",
        }));
        assert_eq!(meta.uploader.as_deref(), Some("ika"));
        assert_eq!(meta.duration_s, Some(252));
        // 2007-09-20 06:41:48 UTC, not that day's midnight.
        assert_eq!(meta.published_at, Some(1_190_270_508));
    }

    #[test]
    fn an_x_post_with_several_videos_takes_each_missing_field_from_the_first_entry_having_it() {
        let meta = item_meta(&json!({
            "_type": "playlist",
            "id": "1857000000000000000",
            "title": "NASA - Launch day",
            "uploader": "NASA",
            "uploader_id": "NASA",
            "timestamp": null,
            "entries": [
                // An animated GIF: no length, no uploader fields of its own.
                { "id": "1857000000000000001", "ext": "mp4", "uploader": "", "timestamp": 1_794_000_000 },
                { "id": "1857000000000000002", "duration": 45.4, "uploader": "Someone else", "timestamp": 1_794_000_999 },
                { "id": "1857000000000000003", "duration": 12.0 },
                null
            ]
        }));
        // The post's own uploader stays; the length comes from the second entry and the time
        // from the first.
        assert_eq!(
            meta,
            ItemMeta {
                uploader: Some("NASA".to_owned()),
                duration_s: Some(45),
                published_at: Some(1_794_000_000),
            }
        );
    }

    #[test]
    fn a_playlist_without_an_uploader_takes_it_from_its_entries_too() {
        let meta = item_meta(&json!({
            "_type": "playlist",
            "entries": [
                { "id": "a" },
                { "id": "b", "channel": "Orbit Daily", "duration": 30 }
            ]
        }));
        assert_eq!(meta.uploader.as_deref(), Some("Orbit Daily"));
        assert_eq!(meta.duration_s, Some(30));
        assert_eq!(meta.published_at, None);
    }

    #[test]
    fn a_page_with_none_of_the_fields_tells_nothing() {
        assert_eq!(item_meta(&json!({})), ItemMeta::default());
        assert_eq!(
            item_meta(&json!({
                "id": "page",
                "title": "Just a page",
                "webpage_url": "https://example.com/page",
                "extractor": "generic",
                "uploader": null,
                "channel": "",
                "creator": "   ",
                "duration": null,
                "upload_date": null
            })),
            ItemMeta::default()
        );
        // Not an object at all.
        assert_eq!(item_meta(&json!([1, 2])), ItemMeta::default());
        assert_eq!(item_meta(&Value::Null), ItemMeta::default());
    }

    #[test]
    fn the_uploader_is_the_first_non_empty_of_the_four_fields() {
        let of = |value: Value| item_meta(&value).uploader;
        assert_eq!(
            of(json!({ "uploader": "A", "channel": "B", "creator": "C", "uploader_id": "D" })),
            Some("A".to_owned())
        );
        assert_eq!(
            of(json!({ "uploader": "", "channel": "B", "creator": "C", "uploader_id": "D" })),
            Some("B".to_owned())
        );
        assert_eq!(
            of(json!({ "uploader": null, "creator": "C", "uploader_id": "D" })),
            Some("C".to_owned())
        );
        assert_eq!(of(json!({ "uploader_id": "@d" })), Some("@d".to_owned()));
        assert_eq!(
            of(json!({ "uploader": "  Spaced  " })),
            Some("Spaced".to_owned())
        );
        // A number is not a name.
        assert_eq!(of(json!({ "uploader": 5 })), None);
    }

    #[test]
    fn the_duration_is_rounded_and_a_negative_one_is_ignored() {
        let of = |value: Value| item_meta(&value).duration_s;
        assert_eq!(of(json!({ "duration": 59.4 })), Some(59));
        assert_eq!(of(json!({ "duration": 59.5 })), Some(60));
        assert_eq!(of(json!({ "duration": 0 })), Some(0));
        assert_eq!(of(json!({ "duration": -1 })), None);
        assert_eq!(of(json!({ "duration": -0.4 })), None);
        assert_eq!(of(json!({ "duration": "300" })), None);
    }

    #[test]
    fn the_publish_time_falls_back_from_timestamp_to_release_timestamp_to_the_date() {
        let of = |value: Value| item_meta(&value).published_at;
        assert_eq!(
            of(json!({ "timestamp": 100, "release_timestamp": 200, "upload_date": "20261103" })),
            Some(100)
        );
        assert_eq!(
            of(json!({ "timestamp": null, "release_timestamp": 200.0, "upload_date": "20261103" })),
            Some(200)
        );
        assert_eq!(
            of(json!({ "upload_date": "20261103" })),
            Some(1_793_664_000)
        );
        // A float timestamp is rounded to whole seconds.
        assert_eq!(
            of(json!({ "timestamp": 1_700_000_000.6 })),
            Some(1_700_000_001)
        );
        // A date that is not one is no answer.
        for bad in [
            "2026-11-03",
            "202611",
            "20261301",
            "20260230",
            "abcdefgh",
            "",
        ] {
            assert_eq!(of(json!({ "upload_date": bad })), None, "{bad}");
        }
        assert_eq!(of(json!({ "upload_date": 20261103 })), None);
    }

    #[test]
    fn a_playlist_nested_too_deeply_is_not_followed() {
        let mut deepest = json!({ "duration": 7 });
        for _ in 0..(MAX_PLAYLIST_DEPTH + 2) {
            deepest = json!({ "_type": "playlist", "entries": [deepest] });
        }
        assert_eq!(item_meta(&deepest).duration_s, None);
        let shallow = json!({ "_type": "playlist", "entries": [{ "_type": "playlist", "entries": [{ "duration": 7 }] }] });
        assert_eq!(item_meta(&shallow).duration_s, Some(7));
    }

    #[test]
    fn the_meta_serializes_the_way_the_tray_reads_it() {
        let meta = ItemMeta {
            uploader: Some("NASA".to_owned()),
            duration_s: Some(185),
            published_at: None,
        };
        assert_eq!(
            serde_json::to_value(&meta).unwrap(),
            json!({ "uploader": "NASA", "durationS": 185, "publishedAt": null })
        );
    }
}

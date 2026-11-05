//! Detect confirmation pages that stop a page-media search before video is loaded.

use url::Url;

/// Site-neutral signals collected from the visible page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateSignals {
    pub url: String,
    pub title: String,
    pub text: String,
    pub buttons: Vec<String>,
    pub has_video: bool,
    pub media_seen: bool,
}

/// Whether the signals strongly indicate an age, terms, cookie, or similar confirmation
/// page. A page that already exposed video or media is never treated as a gate.
pub fn looks_like_gate(signals: &GateSignals) -> bool {
    if signals.media_seen || signals.has_video {
        return false;
    }

    url_looks_like_gate(&signals.url)
        || signals
            .buttons
            .iter()
            .any(|button| button_looks_like_gate(button))
        || text_looks_like_gate(&signals.text)
}

fn url_looks_like_gate(value: &str) -> bool {
    const NEEDLES: [&str; 10] = [
        "/consent",
        "/age",
        "age_verify",
        "agecheck",
        "/verify",
        "/gate",
        "/challenge",
        "interstitial",
        "r18",
        "adult_check",
    ];

    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let path_and_query = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    }
    .to_ascii_lowercase();
    NEEDLES.iter().any(|needle| path_and_query.contains(needle))
}

fn button_looks_like_gate(value: &str) -> bool {
    const JAPANESE: [&str; 9] = [
        "同意する",
        "同意して進む",
        "はい",
        "17歳以下",
        "18歳以上",
        "年齢確認",
        "確認する",
        "次へ進む",
        "続ける",
    ];
    const ENGLISH: [&str; 8] = [
        "I agree",
        "Agree",
        "Accept",
        "Accept all",
        "I am over 18",
        "Enter",
        "Continue",
        "Yes",
    ];

    let value = value.trim();
    JAPANESE.contains(&value)
        || ENGLISH
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
}

fn text_looks_like_gate(value: &str) -> bool {
    const JAPANESE: [&str; 4] = [
        "年齢確認",
        "18歳以上",
        "利用規約に同意",
        "Cookieの使用に同意",
    ];
    const ENGLISH: [&str; 4] = [
        "age verification",
        "confirm your age",
        "accept cookies",
        "terms of service",
    ];

    JAPANESE.iter().any(|needle| value.contains(needle)) || {
        let value = value.to_ascii_lowercase();
        ENGLISH.iter().any(|needle| value.contains(needle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals() -> GateSignals {
        GateSignals {
            url: "https://video.example/watch/123".to_owned(),
            title: "Video".to_owned(),
            text: String::new(),
            buttons: Vec::new(),
            has_video: false,
            media_seen: false,
        }
    }

    #[test]
    fn real_looking_age_gate_matches() {
        let mut page = signals();
        page.title = "年齢確認".to_owned();
        page.text = "この先は18歳以上の方のみ閲覧できます".to_owned();
        page.buttons = vec!["18歳以上".to_owned(), "17歳以下".to_owned()];
        assert!(looks_like_gate(&page));
    }

    #[test]
    fn a_page_with_video_is_not_a_gate() {
        let mut page = signals();
        page.text = "この先は18歳以上の方のみ閲覧できます".to_owned();
        page.has_video = true;
        assert!(!looks_like_gate(&page));
    }

    #[test]
    fn an_ordinary_video_page_does_not_match() {
        let mut page = signals();
        page.text = "動画の説明と出演者情報".to_owned();
        page.buttons = vec!["続きを見る".to_owned()];
        assert!(!looks_like_gate(&page));
    }

    #[test]
    fn confirmation_alone_is_not_enough() {
        let mut page = signals();
        page.text = "内容を確認してください".to_owned();
        assert!(!looks_like_gate(&page));
    }

    #[test]
    fn the_url_rule_is_sufficient() {
        let mut page = signals();
        page.url = "https://video.example/consent?return=%2Fwatch%2F123".to_owned();
        assert!(looks_like_gate(&page));
    }

    #[test]
    fn an_english_cookie_banner_matches() {
        let mut page = signals();
        page.text = "We use cookies to personalize this site.".to_owned();
        page.buttons = vec!["  ACCEPT ALL  ".to_owned()];
        assert!(looks_like_gate(&page));
    }
}

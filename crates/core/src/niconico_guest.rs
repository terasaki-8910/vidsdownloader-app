//! Fetching niconico comments without logging in.
//!
//! yt-dlp normally does this, and uses the same route. When its extractor fails — an
//! unsupported page, a changed layout, a site error — this module asks the site directly,
//! the way niconico's own watch page does: read the JSON the page embeds, then post its
//! `threadKey` to the comment server. Only comments anyone can read are returned; nothing
//! here sends a login, and nothing here opens a paid or members-only thread.
//!
//! Written from <https://zenn.dev/negima1072/articles/howto-get-nicovideo-comments> and
//! checked against sm9 on 2026-11-06 (987 comments, the same as yt-dlp's).

use std::io::Read;
use std::time::Duration;

use serde_json::{Value, json};
use url::Url;

const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PAGE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_COMMENTS_BYTES: u64 = 32 * 1024 * 1024;
/// The watch page's own frontend id; the comment server refuses a request without it.
const FRONTEND_ID: &str = "6";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/141.0.0.0 Safari/537.36";

/// Where the comments of one video live, as the watch page describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentTarget {
    pub server: String,
    pub thread_key: String,
    pub params: Value,
}

/// The `smNNN` / `nmNNN` / `soNNN` id of a niconico watch URL, or of a bare id.
pub fn video_id(url_or_id: &str) -> Option<String> {
    let candidate = match Url::parse(url_or_id) {
        Ok(url) => {
            let host = url.host_str()?.to_ascii_lowercase();
            if !(host == "nicovideo.jp"
                || host.ends_with(".nicovideo.jp")
                || host == "nico.ms"
                || host == "www.nico.ms")
            {
                return None;
            }
            url.path_segments()?.next_back()?.to_owned()
        }
        Err(_) => url_or_id.to_owned(),
    };
    let candidate = candidate.split(['?', '#']).next().unwrap_or_default();
    let digits = candidate.get(2..)?;
    let prefix = candidate.get(..2)?.to_ascii_lowercase();
    (matches!(prefix.as_str(), "sm" | "nm" | "so")
        && !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit()))
    .then(|| format!("{prefix}{digits}"))
}

/// Read the comment target out of a watch page's embedded JSON. The 2026 page carries it
/// in a `server-response` meta tag; older pages used a `js-initial-watch-data` attribute.
pub fn parse_watch_page(html: &str) -> Result<CommentTarget, String> {
    let embedded = embedded_json(html).ok_or("the watch page has no embedded data")?;
    let document: Value =
        serde_json::from_str(&embedded).map_err(|error| format!("embedded data: {error}"))?;
    let response = document
        .pointer("/data/response")
        .or_else(|| document.pointer("/video").map(|_| &document))
        .ok_or("the embedded data has no response")?;
    let comment = response
        .pointer("/$watchV4/data/comment")
        .or_else(|| response.pointer("/comment"))
        .ok_or("the embedded data has no comment section")?;
    let nv = comment
        .get("nvComment")
        .ok_or("the comment section has no server")?;
    let server = nv
        .get("server")
        .and_then(Value::as_str)
        .ok_or("the comment section has no server address")?;
    let thread_key = nv
        .get("threadKey")
        .and_then(Value::as_str)
        .ok_or("the comment section has no key")?;
    let params = nv
        .get("params")
        .ok_or("the comment section has no thread list")?;
    Ok(CommentTarget {
        server: server.to_owned(),
        thread_key: thread_key.to_owned(),
        params: params.clone(),
    })
}

/// The comments of every thread the server returned, in one list, in the same shape
/// yt-dlp writes as `<name>.comments.json`.
pub fn comments_from_threads(body: &str) -> Result<Vec<Value>, String> {
    let document: Value =
        serde_json::from_str(body).map_err(|error| format!("comment server: {error}"))?;
    let threads = document
        .pointer("/data/threads")
        .and_then(Value::as_array)
        .ok_or("the comment server returned no threads")?;
    let mut comments = Vec::new();
    for thread in threads {
        if let Some(list) = thread.get("comments").and_then(Value::as_array) {
            comments.extend(list.iter().cloned());
        }
    }
    (!comments.is_empty())
        .then_some(comments)
        .ok_or_else(|| "the video has no comments anyone can read".to_owned())
}

/// Finds the JSON a watch page embeds, HTML-unescaped.
fn embedded_json(html: &str) -> Option<String> {
    const MARKERS: [&str; 2] = [
        "name=\"server-response\" content=\"",
        "id=\"js-initial-watch-data\" data-api-data=\"",
    ];
    for marker in MARKERS {
        if let Some(start) = html.find(marker) {
            let rest = &html[start + marker.len()..];
            let end = rest.find('"')?;
            return Some(unescape_attribute(&rest[..end]));
        }
    }
    None
}

/// The five XML entities an HTML attribute can carry. The embedded JSON uses no others.
fn unescape_attribute(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(TIMEOUT))
        .timeout_recv_response(Some(TIMEOUT))
        .timeout_per_read(Some(TIMEOUT))
        .user_agent(USER_AGENT)
        .build()
        .into()
}

fn read_capped(response: ureq::http::Response<ureq::Body>, cap: u64) -> Result<String, String> {
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > cap {
        return Err("the response was too large".to_owned());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Fetch the comments of one niconico video without logging in, as the JSON text that
/// belongs in a `.comments.json` file. Blocking: call it from a blocking task.
pub fn fetch_comments_json(url_or_id: &str) -> Result<String, String> {
    let id = video_id(url_or_id).ok_or("not a niconico video address")?;
    let agent = agent();
    let page = agent
        .get(format!("https://www.nicovideo.jp/watch/{id}"))
        .call()
        .map_err(|error| format!("could not open the watch page: {error}"))?;
    let status = page.status().as_u16();
    if status != 200 {
        return Err(format!("the watch page answered {status}"));
    }
    let target = parse_watch_page(&read_capped(page, MAX_PAGE_BYTES)?)?;
    let request = serde_json::to_string(&json!({
        "threadKey": target.thread_key,
        "params": target.params,
        "additionals": {},
    }))
    .map_err(|error| error.to_string())?;
    let response = agent
        .post(format!(
            "{}/v1/threads",
            target.server.trim_end_matches('/')
        ))
        .header("x-frontend-id", FRONTEND_ID)
        .header("x-frontend-version", "0")
        .header("Content-Type", "application/json")
        .send(request.as_str())
        .map_err(|error| format!("could not ask the comment server: {error}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("the comment server answered {status}"));
    }
    let comments = comments_from_threads(&read_capped(response, MAX_COMMENTS_BYTES)?)?;
    serde_json::to_string(&comments).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{comments_from_threads, parse_watch_page, video_id};

    #[test]
    fn video_id_reads_watch_addresses_and_bare_ids() {
        assert_eq!(
            video_id("https://www.nicovideo.jp/watch/sm9").as_deref(),
            Some("sm9")
        );
        assert_eq!(
            video_id("https://www.nicovideo.jp/watch/so12345?ref=x").as_deref(),
            Some("so12345")
        );
        assert_eq!(video_id("nm2345").as_deref(), Some("nm2345"));
        assert_eq!(video_id("https://example.com/watch/sm9"), None);
        assert_eq!(video_id("https://www.nicovideo.jp/watch/smabc"), None);
        assert_eq!(video_id("https://www.nicovideo.jp/watch/xx9"), None);
    }

    #[test]
    fn parse_watch_page_reads_the_2026_layout() {
        let html = concat!(
            "<head><meta name=\"server-response\" content=\"",
            "{&quot;data&quot;:{&quot;response&quot;:{&quot;$watchV4&quot;:{&quot;data&quot;:",
            "{&quot;comment&quot;:{&quot;nvComment&quot;:{&quot;server&quot;:",
            "&quot;https://public.nvcomment.nicovideo.jp&quot;,&quot;threadKey&quot;:&quot;KEY&quot;,",
            "&quot;params&quot;:{&quot;targets&quot;:[{&quot;id&quot;:&quot;1&quot;,&quot;fork&quot;:&quot;main&quot;}]}",
            "}}}}}}}\"></head>"
        );
        let target = parse_watch_page(html).unwrap();
        assert_eq!(target.server, "https://public.nvcomment.nicovideo.jp");
        assert_eq!(target.thread_key, "KEY");
        assert_eq!(target.params["targets"][0]["fork"], "main");
    }

    #[test]
    fn parse_watch_page_reads_the_older_layout() {
        let html = "<div id=\"js-initial-watch-data\" data-api-data=\"{&quot;video&quot;:{},\
             &quot;comment&quot;:{&quot;nvComment&quot;:{&quot;server&quot;:&quot;https://s&quot;,\
             &quot;threadKey&quot;:&quot;K&quot;,&quot;params&quot;:{}}}}\"></div>";
        let target = parse_watch_page(html).unwrap();
        assert_eq!(target.server, "https://s");
        assert_eq!(target.thread_key, "K");
    }

    #[test]
    fn parse_watch_page_reports_a_page_without_comments() {
        let error = parse_watch_page("<html><body>nothing</body></html>").unwrap_err();
        assert!(error.contains("embedded data"), "{error}");
    }

    #[test]
    fn comments_from_threads_joins_every_fork() {
        let body = r#"{"data":{"threads":[
            {"fork":"owner","comments":[]},
            {"fork":"main","comments":[{"no":1,"body":"a","vposMs":0}]},
            {"fork":"easy","comments":[{"no":2,"body":"b","vposMs":10}]}
        ]}}"#;
        let comments = comments_from_threads(body).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0]["body"], "a");
        assert_eq!(comments[1]["no"], 2);
    }

    #[test]
    fn comments_from_threads_reports_an_empty_video() {
        let error = comments_from_threads(r#"{"data":{"threads":[{"comments":[]}]}}"#).unwrap_err();
        assert!(error.contains("no comments"), "{error}");
    }
}

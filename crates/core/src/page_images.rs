//! A web page's own images, collected in the in-app browser ("このページの画像") and saved one
//! file each over HTTP, without yt-dlp or gallery-dl. The site account's cookies are sent
//! only to the site's own hosts (see [`crate::cookie_jar::SiteCookies`]).

use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::cookie_jar::{SiteCookies, unix_now};
use crate::queue::CancelToken;

/// At most this many bytes are saved for one image.
pub const MAX_IMAGE_BYTES: u64 = 100 * 1024 * 1024;

/// Connecting, waiting for the response headers and every read of the body give up after
/// this long without progress.
pub const IMAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// How many redirects an image request follows. They are followed by hand so that the
/// `Cookie` header can be left off a hop that goes to a host the cookies do not belong to.
pub const MAX_REDIRECTS: usize = 5;

/// Longest extension a saved image can get; file names are fitted with room for it.
pub const MAX_EXTENSION_CHARS: usize = 5;

/// A desktop browser's User-Agent: image hosts that refuse unknown clients serve browsers.
#[cfg(target_os = "macos")]
pub const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";
/// A desktop browser's User-Agent: image hosts that refuse unknown clients serve browsers.
#[cfg(not(target_os = "macos"))]
pub const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";

const ACCEPT: &str = "image/avif,image/webp,image/apng,image/*,*/*;q=0.8";
const READ_CHUNK: usize = 64 * 1024;

/// What a page-images job saves: the page's facts (for naming and the library) and the
/// images the user checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageImages {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub uploader: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub published_at: Option<i64>,
    /// Identifies the page within its site (library and "already saved" checks).
    #[serde(default)]
    pub id: Option<String>,
    pub images: Vec<PageImage>,
}

/// One image of a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageImage {
    pub url: String,
    /// The image's 1-based number on its page; ends the file name (`<stem> <index>.<ext>`).
    pub index: u32,
}

/// Why one image was not saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// The job was canceled.
    Canceled,
    /// The server answered with this HTTP status.
    Status(u16),
    /// The response is not an image (its content type).
    NotAnImage(String),
    /// The image is larger than the per-image limit (bytes).
    TooLarge(u64),
    /// Connecting or reading failed, or timed out.
    Network(String),
    /// Writing the file failed.
    Io(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canceled => f.write_str("canceled"),
            Self::Status(code) => write!(f, "HTTP {code}"),
            Self::NotAnImage(content_type) if content_type.is_empty() => {
                f.write_str("not an image (no content type)")
            }
            Self::NotAnImage(content_type) => write!(f, "not an image ({content_type})"),
            Self::TooLarge(limit) => write!(f, "larger than {} MB", limit / (1024 * 1024)),
            Self::Network(message) | Self::Io(message) => f.write_str(message),
        }
    }
}

/// The site name used when no registered site matches: the URL's host without `www.`.
pub fn site_from_url(url: &str) -> String {
    host(url)
        .map(|host| host.strip_prefix("www.").unwrap_or(&host).to_owned())
        .unwrap_or_else(|| url.to_owned())
}

fn host(url: &str) -> Option<String> {
    let uri = ureq::http::Uri::try_from(url).ok()?;
    Some(uri.host()?.trim_end_matches('.').to_ascii_lowercase())
}

/// The file extension for a response: from its image content type (jpeg as jpg, png, gif,
/// webp, avif), else from the URL, else from the content type's subtype; `None` when the
/// response is not an image (SVG counts as not one, like in the collector).
pub fn image_extension(content_type: &str, url: &str) -> Option<String> {
    let essence = content_type.split(';').next()?.trim().to_ascii_lowercase();
    let subtype = essence.strip_prefix("image/")?;
    if subtype.is_empty() || subtype.starts_with("svg") {
        return None;
    }
    let known = match subtype {
        "jpeg" | "jpg" | "pjpeg" => Some("jpg"),
        "png" => Some("png"),
        "gif" => Some("gif"),
        "webp" => Some("webp"),
        "avif" => Some("avif"),
        _ => None,
    };
    if let Some(known) = known {
        return Some(known.to_owned());
    }
    if let Some(extension) = url_extension(url) {
        return Some(extension);
    }
    let subtype = subtype.strip_prefix("x-").unwrap_or(subtype);
    Some(if short_extension(subtype) {
        subtype.to_owned()
    } else {
        "img".to_owned()
    })
}

fn url_extension(url: &str) -> Option<String> {
    let uri = ureq::http::Uri::try_from(url).ok()?;
    let name = uri.path().rsplit('/').next()?;
    let (stem, extension) = name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    if stem.is_empty() || !short_extension(&extension) || extension == "svg" {
        return None;
    }
    Some(if extension == "jpeg" {
        "jpg".to_owned()
    } else {
        extension
    })
}

fn short_extension(value: &str) -> bool {
    (1..=MAX_EXTENSION_CHARS).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Where an image is written while it downloads: next to its final name, so the rename
/// stays on one volume, and short enough to fit wherever the final name fits.
pub fn temp_path(folder: &Path, stem: &str, index: u32) -> PathBuf {
    folder.join(format!("{stem} {index}.part"))
}

/// Downloads images over HTTP with browser-like headers, and with the site account's
/// cookies when it was given some (see [`ImageFetcher::with_cookies`]).
pub struct ImageFetcher {
    agent: ureq::Agent,
    max_bytes: u64,
    cookies: Option<SiteCookies>,
}

impl ImageFetcher {
    /// A fetcher saving at most `max_bytes` per image, sending no cookies.
    pub fn new(max_bytes: u64) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(IMAGE_TIMEOUT))
            .timeout_recv_response(Some(IMAGE_TIMEOUT))
            .timeout_per_read(Some(IMAGE_TIMEOUT))
            .user_agent(BROWSER_USER_AGENT)
            // Redirects are followed in `get`, one hop at a time.
            .max_redirects(0)
            .build()
            .into();
        Self {
            agent,
            max_bytes,
            cookies: None,
        }
    }

    /// Sends `cookies` with every request (including each redirect hop) whose host they
    /// belong to; requests to any other host get no `Cookie` header.
    #[must_use]
    pub fn with_cookies(mut self, cookies: SiteCookies) -> Self {
        self.cookies = Some(cookies).filter(|cookies| !cookies.is_empty());
        self
    }

    /// Saves `image` as `<folder>/<stem> <index>.<ext>`, writing it to [`temp_path`] first and
    /// renaming it once complete. A file already at the final name is kept as it is and
    /// counts as saved. Blocks; checks `cancel` between reads and before the rename, and
    /// leaves no temporary file behind on any failure.
    pub fn save(
        &self,
        image: &PageImage,
        referer: &str,
        folder: &Path,
        stem: &str,
        cancel: &CancelToken,
    ) -> Result<PathBuf, ImageError> {
        if cancel.is_cancelled() {
            return Err(ImageError::Canceled);
        }
        let temp = TempFile(temp_path(folder, stem, image.index));
        let extension = self.download(&image.url, referer, &temp.0, cancel)?;
        if cancel.is_cancelled() {
            return Err(ImageError::Canceled);
        }
        let path = folder.join(format!("{stem} {}.{extension}", image.index));
        if !path.exists() {
            fs::rename(&temp.0, &path).map_err(|error| {
                ImageError::Io(format!("could not rename to {}: {error}", path.display()))
            })?;
        }
        Ok(path)
    }

    /// GETs `url`, following up to [`MAX_REDIRECTS`] redirects by hand. The `Cookie` header
    /// is built for each hop's own URL, so it is kept on a redirect within the site and
    /// dropped as soon as a hop is not on the site, also for the hops after it.
    fn get(
        &self,
        url: &str,
        referer: &str,
    ) -> Result<ureq::http::Response<ureq::Body>, ImageError> {
        let mut current = Url::parse(url)
            .map_err(|error| ImageError::Network(format!("not a valid image URL: {error}")))?;
        // Cookies stop for good once the request has been anywhere but the site: a browser
        // treats a request whose redirect chain left the site as cross-site, too.
        let mut on_site = true;
        for _ in 0..=MAX_REDIRECTS {
            let mut request = self
                .agent
                .get(current.as_str())
                .header("Referer", referer)
                .header("Accept", ACCEPT);
            if let Some(cookies) = &self.cookies {
                on_site = on_site && cookies.covers(&current);
                if on_site && let Some(header) = cookies.header_for(&current, unix_now()) {
                    request = request.header("Cookie", header);
                }
            }
            let response = request.call().map_err(|error| match error {
                ureq::Error::StatusCode(code) => ImageError::Status(code),
                other => ImageError::Network(other.to_string()),
            })?;
            let status = response.status();
            if !status.is_redirection() {
                return Ok(response);
            }
            let location = matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
                .then(|| response.headers().get("location"))
                .flatten()
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| ImageError::Status(status.as_u16()))?;
            let next = current
                .join(location)
                .map_err(|error| ImageError::Network(format!("bad redirect: {error}")))?;
            if !matches!(next.scheme(), "http" | "https") {
                return Err(ImageError::Network(format!(
                    "redirect to a {} URL",
                    next.scheme()
                )));
            }
            current = next;
        }
        Err(ImageError::Network("too many redirects".to_owned()))
    }

    fn download(
        &self,
        url: &str,
        referer: &str,
        temp: &Path,
        cancel: &CancelToken,
    ) -> Result<String, ImageError> {
        let response = self.get(url, referer)?;
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .trim()
            .to_owned();
        let extension = image_extension(&content_type, url)
            .ok_or_else(|| ImageError::NotAnImage(content_type.clone()))?;
        if response
            .body()
            .content_length()
            .is_some_and(|length| length > self.max_bytes)
        {
            return Err(ImageError::TooLarge(self.max_bytes));
        }
        let mut reader = response.into_body().into_reader();
        let mut file = File::create(temp).map_err(|error| {
            ImageError::Io(format!("could not create {}: {error}", temp.display()))
        })?;
        let mut buffer = vec![0_u8; READ_CHUNK];
        let mut total = 0_u64;
        loop {
            if cancel.is_cancelled() {
                return Err(ImageError::Canceled);
            }
            let read = match reader.read(&mut buffer) {
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(ImageError::Network(error.to_string())),
            };
            if read == 0 {
                break;
            }
            total += read as u64;
            if total > self.max_bytes {
                return Err(ImageError::TooLarge(self.max_bytes));
            }
            file.write_all(&buffer[..read]).map_err(|error| {
                ImageError::Io(format!("could not write {}: {error}", temp.display()))
            })?;
        }
        file.flush().map_err(|error| {
            ImageError::Io(format!("could not write {}: {error}", temp.display()))
        })?;
        if total == 0 {
            return Err(ImageError::NotAnImage(format!("{content_type}, empty")));
        }
        Ok(extension)
    }
}

/// Removes its file when dropped; a renamed file is gone from this path, so nothing is lost.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "could not remove partial image {}: {error}",
                self.0.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_map_to_extensions() {
        let url = "https://example.com/a/b.bin";
        for (content_type, extension) in [
            ("image/jpeg", "jpg"),
            ("image/JPEG; charset=binary", "jpg"),
            ("image/pjpeg", "jpg"),
            ("image/png", "png"),
            ("image/gif", "gif"),
            ("image/webp", "webp"),
            (" image/avif ", "avif"),
        ] {
            assert_eq!(
                image_extension(content_type, url).as_deref(),
                Some(extension),
                "{content_type}"
            );
        }
    }

    #[test]
    fn other_images_take_the_url_extension_then_the_subtype() {
        assert_eq!(
            image_extension("image/x-icon", "https://example.com/favicon.ICO?v=2").as_deref(),
            Some("ico")
        );
        assert_eq!(
            image_extension("image/tiff", "https://example.com/scan.JPEG").as_deref(),
            Some("jpg")
        );
        assert_eq!(
            image_extension("image/bmp", "https://example.com/picture").as_deref(),
            Some("bmp")
        );
        assert_eq!(
            image_extension("image/x-portable-pixmap", "https://example.com/p").as_deref(),
            Some("img")
        );
        assert_eq!(
            image_extension("image/heic", "https://example.com/p.toolongext").as_deref(),
            Some("heic")
        );
    }

    #[test]
    fn non_images_and_svg_have_no_extension() {
        for content_type in [
            "",
            "text/html; charset=utf-8",
            "application/octet-stream",
            "image/",
            "image/svg+xml",
        ] {
            assert_eq!(
                image_extension(content_type, "https://example.com/a.jpg"),
                None,
                "{content_type}"
            );
        }
    }

    #[test]
    fn the_site_is_the_host_without_www() {
        assert_eq!(site_from_url("https://www.Example.com/a/b"), "example.com");
        assert_eq!(site_from_url("https://note.com/u/n/abc"), "note.com");
        assert_eq!(site_from_url("http://127.0.0.1:8080/x"), "127.0.0.1");
    }

    #[test]
    fn page_images_round_trip_and_tolerate_missing_facts() {
        let images = PageImages {
            title: Some("Title".to_owned()),
            uploader: None,
            published_at: Some(1),
            id: Some("abc".to_owned()),
            images: vec![PageImage {
                url: "https://example.com/1.jpg".to_owned(),
                index: 3,
            }],
        };
        let value = serde_json::to_value(&images).unwrap();
        assert_eq!(serde_json::from_value::<PageImages>(value).unwrap(), images);
        let sparse: PageImages = serde_json::from_value(serde_json::json!({
            "images": [{ "url": "https://example.com/1.jpg", "index": 1 }]
        }))
        .unwrap();
        assert_eq!(sparse.title, None);
        assert_eq!(sparse.id, None);
    }
}

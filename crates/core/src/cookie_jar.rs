//! Cookies for plain HTTP requests (the images of a page-images job): a Netscape cookie
//! file or Firefox's cookies turned into the `Cookie:` header a browser would send, per
//! RFC 6265 section 5.4, and only to the site the account belongs to.
//!
//! Cookie values are secrets. Nothing here logs them, and the `Debug` output of a cookie
//! or a jar leaves them out.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use url::Url;

use crate::firefox;

/// A line of a Netscape cookie file whose domain starts with this is an HttpOnly cookie.
const HTTP_ONLY_PREFIX: &str = "#HttpOnly_";

/// One cookie, ready to be matched against request URLs.
#[derive(Clone, PartialEq, Eq)]
pub struct JarCookie {
    /// Lower case, without a leading dot.
    pub domain: String,
    /// A host-only cookie goes to `domain` itself; any other one also to its subdomains.
    pub host_only: bool,
    /// Starts with `/`.
    pub path: String,
    /// Sent over https only.
    pub secure: bool,
    /// Unix seconds; `None` for a session cookie (kept for the whole job).
    pub expires: Option<i64>,
    pub name: String,
    pub value: String,
}

impl fmt::Debug for JarCookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JarCookie")
            .field("domain", &self.domain)
            .field("host_only", &self.host_only)
            .field("path", &self.path)
            .field("secure", &self.secure)
            .field("expires", &self.expires)
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// A set of cookies. Two cookies with the same domain, path and name are one cookie; the
/// one that comes later replaces the earlier.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CookieJar {
    cookies: Vec<JarCookie>,
}

impl fmt::Debug for CookieJar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CookieJar({} cookies)", self.cookies.len())
    }
}

impl CookieJar {
    /// A jar of `cookies`; cookies that cannot be sent in a header are left out.
    pub fn new(cookies: impl IntoIterator<Item = JarCookie>) -> Self {
        let mut kept: Vec<JarCookie> = Vec::new();
        let mut positions: HashMap<(String, String, String), usize> = HashMap::new();
        for cookie in cookies {
            if !usable_name(&cookie.name) || !usable_value(&cookie.value) {
                continue;
            }
            let key = (
                cookie.domain.clone(),
                cookie.path.clone(),
                cookie.name.clone(),
            );
            match positions.get(&key) {
                Some(&position) => kept[position] = cookie,
                None => {
                    positions.insert(key, kept.len());
                    kept.push(cookie);
                }
            }
        }
        Self { cookies: kept }
    }

    /// Reads a Netscape cookie file (as yt-dlp and curl write it): seven tab-separated
    /// fields (domain, subdomain flag, path, secure flag, expiry, name, value); `#HttpOnly_`
    /// before the domain marks an HttpOnly cookie, other `#` lines and blank lines are
    /// comments, and a malformed line is skipped. The flag in the file decides whether a
    /// cookie is host-only.
    pub fn parse_netscape(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        Self::new(text.lines().filter_map(parse_netscape_line))
    }

    /// Firefox's cookies: a host with a leading dot is a domain cookie, any other one
    /// host-only.
    pub fn from_firefox(cookies: &[firefox::Cookie]) -> Self {
        Self::new(cookies.iter().map(|cookie| JarCookie {
            domain: normalize_domain(&cookie.host),
            host_only: !cookie.host.trim().starts_with('.'),
            path: normalize_path(&cookie.path),
            secure: cookie.secure,
            expires: cookie.expires,
            name: cookie.name.clone(),
            value: cookie.value.clone(),
        }))
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// The `Cookie:` header value for a request to `host` (lower case) and `path`, over
    /// https when `secure`; `None` when no cookie applies. A cookie applies when its domain
    /// matches the host, its path matches the request path, it is not `secure` on a plain
    /// http request, and it has not expired by `now` (Unix seconds). Cookies with longer
    /// paths come first, and cookies with equal paths keep their order.
    pub fn header(&self, host: &str, path: &str, secure: bool, now: i64) -> Option<String> {
        let host = host.trim_end_matches('.');
        let path = if path.is_empty() { "/" } else { path };
        let mut matching: Vec<&JarCookie> = self
            .cookies
            .iter()
            .filter(|cookie| {
                (secure || !cookie.secure)
                    && cookie.expires.is_none_or(|expires| expires > now)
                    && domain_matches(host, &cookie.domain, cookie.host_only)
                    && path_matches(path, &cookie.path)
            })
            .collect();
        if matching.is_empty() {
            return None;
        }
        matching.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        Some(
            matching
                .iter()
                .map(|cookie| format!("{}={}", cookie.name, cookie.value))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }
}

/// A site account's cookies and the domains they may be sent to: a request gets a
/// `Cookie:` header only when its host is one of `domains` or a subdomain of one, so
/// login cookies never leave the site (images on a CDN or another site get none, as for a
/// cross-site image in a browser).
#[derive(Clone, PartialEq, Eq)]
pub struct SiteCookies {
    jar: CookieJar,
    domains: Vec<String>,
}

impl fmt::Debug for SiteCookies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SiteCookies")
            .field("jar", &self.jar)
            .field("domains", &self.domains)
            .finish()
    }
}

impl SiteCookies {
    pub fn new(jar: CookieJar, domains: &[String]) -> Self {
        Self {
            jar,
            domains: domains
                .iter()
                .map(|domain| normalize_domain(domain))
                .filter(|domain| !domain.is_empty())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.jar.is_empty() || self.domains.is_empty()
    }

    /// Whether `url` is on the site: an http(s) URL whose host is one of the site's domains
    /// or a subdomain of one.
    pub fn covers(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
            && url
                .host_str()
                .is_some_and(|host| host_in_domains(host.trim_end_matches('.'), &self.domains))
    }

    /// The `Cookie:` header value for a request to `url`, or `None` when the URL is not on
    /// the site (see [`SiteCookies::covers`]) or no cookie applies. `now` is Unix seconds.
    pub fn header_for(&self, url: &Url, now: i64) -> Option<String> {
        if !self.covers(url) {
            return None;
        }
        let host = url.host_str()?.trim_end_matches('.');
        self.jar
            .header(host, url.path(), url.scheme() == "https", now)
    }
}

/// Whether `host` is one of `domains` or a subdomain of one (never for an IP address
/// other than the domain itself). Both are lower case; a domain has no leading dot.
pub fn host_in_domains(host: &str, domains: &[String]) -> bool {
    domains
        .iter()
        .any(|domain| domain_matches(host, domain, false))
}

/// RFC 6265 section 5.1.3 for lower-case names: the host is the cookie's domain, or a
/// subdomain of it unless the cookie is host-only or the host is an IP address.
pub fn domain_matches(host: &str, domain: &str, host_only: bool) -> bool {
    if domain.is_empty() {
        return false;
    }
    if host == domain {
        return true;
    }
    !host_only
        && !is_ip_address(host)
        && host.len() > domain.len()
        && host.ends_with(domain)
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
}

/// RFC 6265 section 5.1.4.
pub fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    request_path.starts_with(cookie_path)
        && (cookie_path.ends_with('/')
            || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/'))
}

fn is_ip_address(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse::<IpAddr>().is_ok()
}

fn normalize_domain(domain: &str) -> String {
    domain.trim().trim_matches('.').to_ascii_lowercase()
}

fn normalize_path(path: &str) -> String {
    let path = path.trim();
    if path.starts_with('/') {
        path.to_owned()
    } else {
        "/".to_owned()
    }
}

fn parse_netscape_line(line: &str) -> Option<JarCookie> {
    let line = line.trim_end_matches('\r');
    if line.trim().is_empty() {
        return None;
    }
    let line = line.strip_prefix(HTTP_ONLY_PREFIX).unwrap_or(line);
    if line.starts_with('#') {
        return None;
    }
    let fields: Vec<&str> = line.splitn(7, '\t').collect();
    // A cookie with an empty value may be written without the last tab.
    if fields.len() < 6 {
        return None;
    }
    let domain = normalize_domain(fields[0]);
    if domain.is_empty() || domain.contains(char::is_whitespace) {
        return None;
    }
    let flag = |field: &str| match field.trim().to_ascii_uppercase().as_str() {
        "TRUE" => Some(true),
        "FALSE" => Some(false),
        _ => None,
    };
    let include_subdomains = flag(fields[1])?;
    let secure = flag(fields[3])?;
    let expires = match fields[4].trim() {
        "" => 0,
        text => text.parse::<i64>().ok()?,
    };
    Some(JarCookie {
        domain,
        host_only: !include_subdomains,
        path: normalize_path(fields[2]),
        secure,
        expires: (expires != 0).then_some(expires),
        name: fields[5].trim().to_owned(),
        value: fields.get(6).copied().unwrap_or_default().to_owned(),
    })
}

/// A name that is a header token without the characters a cookie header uses to split.
fn usable_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'=' | b';' | b','))
}

/// A value that cannot end the header or start another cookie.
fn usable_value(value: &str) -> bool {
    !value.chars().any(|c| c.is_control() || c == ';')
}

/// Now, in Unix seconds.
pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn cookie(domain: &str, host_only: bool, path: &str, name: &str, value: &str) -> JarCookie {
        JarCookie {
            domain: domain.to_owned(),
            host_only,
            path: path.to_owned(),
            secure: false,
            expires: None,
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn the_netscape_file_the_app_writes_is_read_back() {
        let text = "# Netscape HTTP Cookie File\n\
                    #HttpOnly_.example.com\tTRUE\t/account\tTRUE\t0\tsid\tsecret\n\
                    .sub.example.com\tTRUE\t/\tFALSE\t1900000000\ttheme\tdark\n";
        let jar = CookieJar::parse_netscape(text);
        assert_eq!(jar.len(), 2);
        assert_eq!(
            jar.cookies[0],
            JarCookie {
                domain: "example.com".to_owned(),
                host_only: false,
                path: "/account".to_owned(),
                secure: true,
                expires: None,
                name: "sid".to_owned(),
                value: "secret".to_owned(),
            }
        );
        assert_eq!(jar.cookies[1].domain, "sub.example.com");
        assert_eq!(jar.cookies[1].expires, Some(1_900_000_000));
        assert!(!jar.cookies[1].secure);
    }

    #[test]
    fn the_netscape_flag_decides_host_only_and_comments_and_bad_lines_are_skipped() {
        let text = "\u{feff}# Netscape HTTP Cookie File\r\n\
                    \r\n\
                    # a comment\r\n\
                    example.com\tFALSE\t/\tFALSE\t0\thostonly\t1\r\n\
                    .example.com\tTRUE\t/\tFALSE\t0\tdomain\t2\r\n\
                    too\tfew\tfields\n\
                    example.com\tMAYBE\t/\tFALSE\t0\tbadflag\t3\n\
                    example.com\tTRUE\t/\tFALSE\tsoon\tbadexpiry\t4\n\
                    \tTRUE\t/\tFALSE\t0\tnodomain\t5\n\
                    example.com\tTRUE\t/\tFALSE\t0\t\t6\n\
                    example.com\tTRUE\t/\tFALSE\t0\tnovalue\n\
                    example.com\tTRUE\t\tFALSE\t\tdefaults\ta b\n\
                    example.com\tTRUE\t/\tFALSE\t0\ttabbed\ta\tb\n\
                    example.com\tTRUE\t/\tFALSE\t0\tinjected\tx\r\nCookie: y\n";
        let jar = CookieJar::parse_netscape(text);
        let names: Vec<&str> = jar.cookies.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["hostonly", "domain", "novalue", "defaults", "injected"]
        );
        assert!(jar.cookies[0].host_only);
        assert!(!jar.cookies[1].host_only);
        assert_eq!(jar.cookies[2].value, "");
        // An empty path becomes "/", an empty expiry is a session cookie, and a value keeps
        // its spaces (the "tabbed" one, with a tab in its value, was skipped).
        assert_eq!(jar.cookies[3].path, "/");
        assert_eq!(jar.cookies[3].expires, None);
        assert_eq!(jar.cookies[3].value, "a b");
        // A line break cannot smuggle a second header in: lines() ended the value.
        assert_eq!(jar.cookies[4].value, "x");
    }

    #[test]
    fn a_later_cookie_with_the_same_domain_path_and_name_replaces_the_earlier() {
        let jar = CookieJar::new([
            cookie("example.com", false, "/", "a", "old"),
            cookie("example.com", false, "/", "b", "1"),
            cookie("example.com", false, "/", "a", "new"),
            cookie("example.com", false, "/x", "a", "other path"),
        ]);
        assert_eq!(
            jar.header("example.com", "/x/y", true, NOW).as_deref(),
            Some("a=other path; a=new; b=1")
        );
    }

    #[test]
    fn domain_cookies_match_subdomains_and_host_only_cookies_only_their_host() {
        assert!(domain_matches("example.com", "example.com", false));
        assert!(domain_matches("img.example.com", "example.com", false));
        assert!(domain_matches("a.b.example.com", "example.com", false));
        assert!(!domain_matches("badexample.com", "example.com", false));
        assert!(!domain_matches("example.com", "img.example.com", false));
        assert!(!domain_matches(
            "example.com.evil.net",
            "example.com",
            false
        ));
        assert!(domain_matches("example.com", "example.com", true));
        assert!(!domain_matches("img.example.com", "example.com", true));
        // An IP address matches only itself.
        assert!(domain_matches("127.0.0.1", "127.0.0.1", false));
        assert!(!domain_matches("1.127.0.0.1", "127.0.0.1", true));
        assert!(!domain_matches("10.0.0.1", "0.0.1", false));
        assert!(!domain_matches("[::1]", "1]", false));
        assert!(!domain_matches("example.com", "", false));
    }

    #[test]
    fn paths_match_on_a_segment_boundary() {
        assert!(path_matches("/", "/"));
        assert!(path_matches("/a/b", "/"));
        assert!(path_matches("/a/b", "/a"));
        assert!(path_matches("/a/b", "/a/"));
        assert!(path_matches("/a/b", "/a/b"));
        assert!(path_matches("/a/b/", "/a/b"));
        assert!(!path_matches("/ab", "/a"));
        assert!(!path_matches("/a", "/a/b"));
        assert!(!path_matches("/a/b", "/c"));
    }

    #[test]
    fn the_header_has_matching_cookies_only_longest_path_first() {
        let mut secure = cookie("example.com", false, "/", "secure", "s");
        secure.secure = true;
        let mut expired = cookie("example.com", false, "/", "expired", "e");
        expired.expires = Some(NOW);
        let mut fresh = cookie("example.com", false, "/", "fresh", "f");
        fresh.expires = Some(NOW + 1);
        let jar = CookieJar::new([
            cookie("example.com", false, "/", "root", "1"),
            cookie("example.com", false, "/a/b", "deep", "2"),
            cookie("example.com", false, "/a", "mid", "3"),
            cookie("example.com", false, "/a", "mid2", "4"),
            cookie("example.com", true, "/", "hostonly", "5"),
            cookie("other.com", false, "/", "other", "6"),
            cookie("example.com", false, "/c", "elsewhere", "7"),
            secure,
            expired,
            fresh,
        ]);
        assert_eq!(
            jar.header("example.com", "/a/b/c.jpg", false, NOW)
                .as_deref(),
            Some("deep=2; mid=3; mid2=4; root=1; hostonly=5; fresh=f"),
            "http: no secure cookie, no expired one, session cookies kept"
        );
        assert_eq!(
            jar.header("example.com", "/", true, NOW).as_deref(),
            Some("root=1; hostonly=5; secure=s; fresh=f"),
            "https adds the secure cookie"
        );
        assert_eq!(
            jar.header("img.example.com", "/a", false, NOW).as_deref(),
            Some("mid=3; mid2=4; root=1; fresh=f"),
            "a subdomain gets the domain cookies only"
        );
        assert_eq!(jar.header("unrelated.net", "/", true, NOW), None);
        assert!(jar.header("example.com", "", false, NOW).is_some());
    }

    #[test]
    fn firefox_cookies_keep_their_host_only_and_secure_flags() {
        let firefox_cookie = |host: &str, secure: bool| firefox::Cookie {
            host: host.to_owned(),
            name: "k".to_owned(),
            value: "v".to_owned(),
            path: "/".to_owned(),
            expires: Some(NOW + 10),
            secure,
            http_only: true,
            same_site: firefox::SameSite::Lax,
        };
        let jar = CookieJar::from_firefox(&[
            firefox_cookie(".Example.com", false),
            firefox_cookie("www.example.com", true),
        ]);
        assert_eq!(jar.cookies[0].domain, "example.com");
        assert!(!jar.cookies[0].host_only);
        assert_eq!(jar.cookies[1].domain, "www.example.com");
        assert!(jar.cookies[1].host_only);
        assert!(jar.cookies[1].secure);
    }

    #[test]
    fn site_cookies_go_only_to_the_sites_hosts() {
        let jar = CookieJar::new([
            cookie("example.com", false, "/", "sid", "1"),
            // A cookie of another site in the same file (cookies.txt holds every site).
            cookie("cdn.net", false, "/", "cdn", "2"),
        ]);
        let site = SiteCookies::new(jar, &["Example.com".to_owned(), ".blog.test".to_owned()]);
        assert_eq!(
            site.header_for(&url("https://img.example.com/a.jpg"), NOW)
                .as_deref(),
            Some("sid=1")
        );
        assert_eq!(
            site.header_for(&url("http://example.com./a.jpg"), NOW)
                .as_deref(),
            Some("sid=1")
        );
        assert_eq!(site.header_for(&url("https://cdn.net/a.jpg"), NOW), None);
        assert_eq!(site.header_for(&url("https://notexample.com/a"), NOW), None);
        assert_eq!(site.header_for(&url("ftp://example.com/a"), NOW), None);
        assert!(site.covers(&url("https://img.example.com/")));
        assert!(site.covers(&url("http://blog.test/")));
        assert!(!site.covers(&url("https://cdn.net/")));
        assert!(!site.covers(&url("ftp://example.com/")));
        assert!(!site.is_empty());
        assert!(SiteCookies::new(CookieJar::default(), &["example.com".to_owned()]).is_empty());
        assert!(
            SiteCookies::new(CookieJar::new([cookie("a.com", false, "/", "k", "v")]), &[])
                .is_empty()
        );
    }

    #[test]
    fn debug_output_leaves_cookie_values_out() {
        let jar = CookieJar::new([cookie("example.com", false, "/", "sid", "TOP-SECRET")]);
        let site = SiteCookies::new(jar.clone(), &["example.com".to_owned()]);
        for text in [
            format!("{jar:?}"),
            format!("{site:?}"),
            format!("{:?}", jar.cookies[0]),
        ] {
            assert!(!text.contains("TOP-SECRET"), "{text}");
        }
    }
}

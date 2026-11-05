//! Firefox's profiles (`profiles.ini`) and cookies (`cookies.sqlite`), read for importing
//! logins into the in-app browser.
//!
//! Firefox's own files are only ever opened for reading: the cookie database (and its
//! write-ahead log, which holds the newest changes while Firefox runs) is copied into a
//! fresh folder that only the user can read, the copy is opened read-only, and the folder
//! is deleted again before returning, also when reading fails.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

/// Name prefix of the temporary folders holding a copy of Firefox's cookie database. A
/// folder left by a crash is removed at the next start (see [`is_temp_dir_name`]).
pub const TEMP_DIR_PREFIX: &str = "firefox-cookies-";

/// `expiry` values above this are milliseconds (newer Firefox), below it seconds: 10^11
/// seconds is the year 5138, 10^11 milliseconds is 1973.
const MILLISECONDS_THRESHOLD: i64 = 100_000_000_000;

/// How often deleting the temporary folder is tried before giving up.
const REMOVE_ATTEMPTS: u32 = 3;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, thiserror::Error)]
pub enum FirefoxError {
    #[error("Firefox profile folder {0} was not found")]
    ProfileNotFound(PathBuf),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("could not read Firefox's cookie database: {0}")]
    Database(#[from] rusqlite::Error),
}

fn io_error(context: impl Into<String>) -> impl FnOnce(io::Error) -> FirefoxError {
    move |source| FirefoxError::Io {
        context: context.into(),
        source,
    }
}

/// One `[Profile…]` section of `profiles.ini`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// `Path=` as written (forward slashes when relative).
    pub path: Option<String>,
    /// `IsRelative=1`: `path` is relative to the folder holding `profiles.ini`.
    pub is_relative: bool,
    /// `Default=1` (the default of Firefox versions before 67).
    pub default: bool,
}

/// The profiles listed in `profiles.ini`, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfilesIni {
    pub profiles: Vec<Profile>,
    /// `Default=` of each `[Install…]` section (the profile path one Firefox installation
    /// uses), in file order.
    install_defaults: Vec<String>,
}

impl ProfilesIni {
    pub fn parse(contents: &str) -> Self {
        enum Section {
            Profile(Profile),
            Install(Option<String>),
            Other,
        }

        let mut parsed = Self::default();
        let mut section = Section::Other;
        let finish = |section: Section, parsed: &mut Self| match section {
            Section::Profile(profile) if !profile.name.is_empty() => {
                parsed.profiles.push(profile);
            }
            Section::Install(Some(default)) => parsed.install_defaults.push(default),
            _ => {}
        };
        // A file saved by a text editor may start with a byte order mark.
        let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
        for raw_line in contents.lines() {
            let line = raw_line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                let name = &line[1..line.len() - 1];
                let next = if name.starts_with("Profile") {
                    Section::Profile(Profile {
                        name: String::new(),
                        path: None,
                        is_relative: false,
                        default: false,
                    })
                } else if name.starts_with("Install") {
                    Section::Install(None)
                } else {
                    Section::Other
                };
                finish(std::mem::replace(&mut section, next), &mut parsed);
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match &mut section {
                Section::Profile(profile) => {
                    if key.eq_ignore_ascii_case("Name") {
                        value.clone_into(&mut profile.name);
                    } else if key.eq_ignore_ascii_case("Path") && !value.is_empty() {
                        profile.path = Some(value.to_owned());
                    } else if key.eq_ignore_ascii_case("IsRelative") {
                        profile.is_relative = value == "1";
                    } else if key.eq_ignore_ascii_case("Default") {
                        profile.default = value == "1";
                    }
                }
                Section::Install(default) => {
                    if key.eq_ignore_ascii_case("Default") && !value.is_empty() {
                        *default = Some(value.to_owned());
                    }
                }
                Section::Other => {}
            }
        }
        finish(section, &mut parsed);
        parsed
    }

    /// Profile names in file order, as the account settings list them.
    pub fn names(&self) -> Vec<String> {
        self.profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect()
    }

    /// The profile Firefox opens by default: the one an `[Install…]` section names, else
    /// one marked `Default=1`, else the first.
    pub fn default_profile(&self) -> Option<&str> {
        self.install_defaults
            .iter()
            .find_map(|default| {
                self.profiles.iter().find(|profile| {
                    profile
                        .path
                        .as_deref()
                        .is_some_and(|path| same_ini_path(path, default))
                })
            })
            .or_else(|| self.profiles.iter().find(|profile| profile.default))
            .or_else(|| self.profiles.first())
            .map(|profile| profile.name.as_str())
    }

    /// The folder of the profile called `name`; `ini_dir` is the folder holding
    /// `profiles.ini`.
    pub fn profile_dir(&self, ini_dir: &Path, name: &str) -> Option<PathBuf> {
        let profile = self.profiles.iter().find(|profile| profile.name == name)?;
        let path = profile.path.as_deref()?;
        let relative = profile.is_relative || !Path::new(path).is_absolute();
        if relative {
            let mut dir = ini_dir.to_path_buf();
            for part in path.split(['/', '\\']).filter(|part| !part.is_empty()) {
                dir.push(part);
            }
            Some(dir)
        } else {
            Some(PathBuf::from(path))
        }
    }
}

fn same_ini_path(left: &str, right: &str) -> bool {
    let normalize = |path: &str| path.replace('\\', "/").trim_end_matches('/').to_owned();
    normalize(left).eq_ignore_ascii_case(&normalize(right))
}

/// Where this OS's Firefox keeps `profiles.ini`.
pub fn profiles_ini_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("Mozilla").join("Firefox").join("profiles.ini"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(PathBuf::from).map(|path| {
            path.join("Library")
                .join("Application Support")
                .join("Firefox")
                .join("profiles.ini")
        })
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| path.join(".mozilla").join("firefox").join("profiles.ini"))
    }
}

/// Firefox's `sameSite` column: 0 None, 1 Lax, 2 Strict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    None,
    Lax,
    Strict,
}

impl SameSite {
    fn from_firefox(value: i64) -> Self {
        match value {
            0 => Self::None,
            2 => Self::Strict,
            // 1, and any value a later Firefox adds: Lax is what browsers assume when a
            // cookie states nothing.
            _ => Self::Lax,
        }
    }
}

/// One cookie of Firefox's default container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    /// As Firefox stores it: a leading dot marks a domain cookie, no dot a host-only one.
    pub host: String,
    pub name: String,
    pub value: String,
    pub path: String,
    /// Unix seconds; `None` for a session cookie.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

/// Whether a cookie for `host` belongs to `domain`: the same host, or a host (or a
/// Firefox domain cookie, `.domain`) ending with `.` + `domain`. Case does not matter.
pub fn host_matches(host: &str, domain: &str) -> bool {
    let domain = domain.trim().trim_matches('.');
    if domain.is_empty() {
        return false;
    }
    let host = host.trim();
    host.eq_ignore_ascii_case(domain)
        || (host.len() > domain.len()
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
            && host.is_char_boundary(host.len() - domain.len())
            && host[host.len() - domain.len()..].eq_ignore_ascii_case(domain))
}

/// Whether a folder name is one of this module's temporary copies.
pub fn is_temp_dir_name(name: &str) -> bool {
    name.starts_with(TEMP_DIR_PREFIX)
}

/// Reads the unexpired cookies of Firefox's default container (no container tabs, no
/// partitioned cookies) whose host belongs to one of `domains`. `profile_dir` is the
/// Firefox profile folder; the temporary copy is made in a new folder below `temp_parent`
/// and deleted before this returns. `now` is Unix seconds. A profile without a cookie
/// database has no cookies.
pub fn read_cookies(
    profile_dir: &Path,
    temp_parent: &Path,
    domains: &[String],
    now: i64,
) -> Result<Vec<Cookie>, FirefoxError> {
    if !profile_dir.is_dir() {
        return Err(FirefoxError::ProfileNotFound(profile_dir.to_path_buf()));
    }
    let source = profile_dir.join("cookies.sqlite");
    if !source.is_file() {
        return Ok(Vec::new());
    }
    let domains: Vec<&str> = domains
        .iter()
        .map(|domain| domain.trim().trim_matches('.'))
        .filter(|domain| !domain.is_empty())
        .collect();
    if domains.is_empty() {
        return Ok(Vec::new());
    }

    let temp = TempDir::create(temp_parent)?;
    let result = copy_and_read(&source, temp.path(), &domains, now);
    let removed = temp.remove();
    let cookies = result?;
    removed?;
    Ok(cookies)
}

fn copy_and_read(
    source: &Path,
    temp: &Path,
    domains: &[&str],
    now: i64,
) -> Result<Vec<Cookie>, FirefoxError> {
    let copy = temp.join("cookies.sqlite");
    copy_private(source, &copy)?;
    let mut wal = source.as_os_str().to_owned();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    if wal.is_file() {
        copy_private(&wal, &temp.join("cookies.sqlite-wal"))?;
    }

    let connection = Connection::open_with_flags(
        &copy,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut statement = connection.prepare(
        "SELECT host, name, value, path, expiry, isSecure, isHttpOnly, sameSite \
         FROM moz_cookies WHERE originAttributes = ''",
    )?;
    let mut rows = statement.query([])?;
    let mut cookies = Vec::new();
    while let Some(row) = rows.next()? {
        let host = text(row.get_ref(0)?);
        if !domains.iter().any(|domain| host_matches(&host, domain)) {
            continue;
        }
        let expires = match row.get_ref(4)? {
            ValueRef::Null => None,
            value => {
                let expires = expiry_seconds(integer(value));
                if expires <= now {
                    continue;
                }
                Some(expires)
            }
        };
        let path = text(row.get_ref(3)?);
        cookies.push(Cookie {
            host,
            name: text(row.get_ref(1)?),
            value: text(row.get_ref(2)?),
            path: if path.is_empty() {
                "/".to_owned()
            } else {
                path
            },
            expires,
            secure: integer(row.get_ref(5)?) != 0,
            http_only: integer(row.get_ref(6)?) != 0,
            same_site: SameSite::from_firefox(integer(row.get_ref(7)?)),
        });
    }
    Ok(cookies)
}

/// Firefox's `expiry` in Unix seconds (newer versions store milliseconds).
fn expiry_seconds(expiry: i64) -> i64 {
    if expiry > MILLISECONDS_THRESHOLD {
        expiry / 1000
    } else {
        expiry
    }
}

fn text(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            String::from_utf8_lossy(bytes).into_owned()
        }
        ValueRef::Integer(number) => number.to_string(),
        ValueRef::Real(number) => number.to_string(),
        ValueRef::Null => String::new(),
    }
}

fn integer(value: ValueRef<'_>) -> i64 {
    match value {
        ValueRef::Integer(number) => number,
        // Truncation is the intent: a fractional expiry or flag is still a number of
        // seconds or a truth value.
        #[allow(clippy::cast_possible_truncation)]
        ValueRef::Real(number) => number as i64,
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0),
        ValueRef::Blob(_) | ValueRef::Null => 0,
    }
}

/// Copies `source` without changing it (opened read-only, shared so Firefox can keep
/// writing) into a new file only the user can read.
fn copy_private(source: &Path, target: &Path) -> Result<(), FirefoxError> {
    let mut input =
        File::open(source).map_err(io_error(format!("could not open {}", source.display())))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options
        .open(target)
        .map_err(io_error(format!("could not create {}", target.display())))?;
    io::copy(&mut input, &mut output)
        .map_err(io_error(format!("could not copy {}", source.display())))?;
    Ok(())
}

/// A new folder only the user can read, deleted by [`TempDir::remove`] or, failing that,
/// when dropped.
struct TempDir {
    path: Option<PathBuf>,
}

impl TempDir {
    fn create(parent: &Path) -> Result<Self, FirefoxError> {
        fs::create_dir_all(parent)
            .map_err(io_error(format!("could not create {}", parent.display())))?;
        let process = std::process::id();
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        loop {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{TEMP_DIR_PREFIX}{process}-{started}-{sequence}"));
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self { path: Some(path) }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(FirefoxError::Io {
                        context: format!("could not create {}", path.display()),
                        source: error,
                    });
                }
            }
        }
    }

    fn path(&self) -> &Path {
        self.path
            .as_deref()
            .expect("the folder is removed only once")
    }

    fn remove(mut self) -> Result<(), FirefoxError> {
        let path = self.path.take().expect("the folder is removed only once");
        remove_dir(&path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(path) = self.path.take()
            && let Err(error) = remove_dir(&path)
        {
            eprintln!("{error}");
        }
    }
}

fn remove_dir(path: &Path) -> Result<(), FirefoxError> {
    let mut attempt = 1;
    loop {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(_) if attempt < REMOVE_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => {
                return Err(FirefoxError::Io {
                    context: format!("could not delete {}", path.display()),
                    source: error,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    /// The `moz_cookies` table as Firefox 128+ creates it.
    const SCHEMA: &str = "CREATE TABLE moz_cookies (
        id INTEGER PRIMARY KEY,
        originAttributes TEXT NOT NULL DEFAULT '',
        name TEXT,
        value TEXT,
        host TEXT,
        path TEXT,
        expiry INTEGER,
        lastAccessed INTEGER,
        creationTime INTEGER,
        isSecure INTEGER,
        isHttpOnly INTEGER,
        inBrowserElement INTEGER DEFAULT 0,
        sameSite INTEGER DEFAULT 0,
        rawSameSite INTEGER DEFAULT 0,
        schemeMap INTEGER DEFAULT 0,
        isPartitionedAttributeSet INTEGER DEFAULT 0,
        CONSTRAINT moz_uniqueid UNIQUE (name, host, path, originAttributes)
    )";

    struct Row<'a> {
        origin: &'a str,
        name: &'a str,
        host: &'a str,
        expiry: Option<i64>,
        secure: i64,
        http_only: i64,
        same_site: i64,
    }

    fn row<'a>(name: &'a str, host: &'a str, expiry: Option<i64>) -> Row<'a> {
        Row {
            origin: "",
            name,
            host,
            expiry,
            secure: 0,
            http_only: 0,
            same_site: 0,
        }
    }

    fn insert(connection: &Connection, rows: &[Row<'_>]) {
        for row in rows {
            connection
                .execute(
                    "INSERT INTO moz_cookies (originAttributes, name, value, host, path, expiry, \
                     lastAccessed, creationTime, isSecure, isHttpOnly, sameSite) \
                     VALUES (?1, ?2, ?3, ?4, '/', ?5, 0, 0, ?6, ?7, ?8)",
                    rusqlite::params![
                        row.origin,
                        row.name,
                        format!("value-of-{}", row.name),
                        row.host,
                        row.expiry,
                        row.secure,
                        row.http_only,
                        row.same_site,
                    ],
                )
                .unwrap();
        }
    }

    /// A profile folder with a `cookies.sqlite` in WAL mode. The returned connection
    /// stays open (like a running Firefox) so later inserts stay in `cookies.sqlite-wal`.
    fn profile_with(rows: &[Row<'_>]) -> (tempfile::TempDir, Connection) {
        let profile = tempfile::tempdir().unwrap();
        let connection = Connection::open(profile.path().join("cookies.sqlite")).unwrap();
        connection
            .query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
            .unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        insert(&connection, rows);
        (profile, connection)
    }

    fn names(cookies: &[Cookie]) -> Vec<&str> {
        let mut names: Vec<&str> = cookies.iter().map(|cookie| cookie.name.as_str()).collect();
        names.sort_unstable();
        names
    }

    fn read(profile: &Path, temp: &Path, domains: &[&str]) -> Result<Vec<Cookie>, FirefoxError> {
        let domains: Vec<String> = domains.iter().map(|domain| (*domain).to_owned()).collect();
        read_cookies(profile, temp, &domains, NOW)
    }

    fn entries(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn keeps_only_cookies_of_the_given_domains() {
        let future = Some(NOW + 3600);
        let (profile, _firefox) = profile_with(&[
            row("domain", ".example.com", future),
            row("host_only", "example.com", future),
            row("sub", "www.example.com", future),
            row("deep", "a.b.example.com", future),
            row("upper", "WWW.EXAMPLE.COM", future),
            row("lookalike", "notexample.com", future),
            row("suffix_trap", "example.com.evil.test", future),
            row("other", ".other.test", future),
            row("second", ".second.test", future),
        ]);
        let temp = tempfile::tempdir().unwrap();

        let cookies = read(profile.path(), temp.path(), &["example.com"]).unwrap();
        assert_eq!(
            names(&cookies),
            ["deep", "domain", "host_only", "sub", "upper"]
        );
        let domain = cookies
            .iter()
            .find(|cookie| cookie.name == "domain")
            .unwrap();
        assert_eq!(domain.host, ".example.com");
        assert_eq!(domain.value, "value-of-domain");
        assert_eq!(domain.path, "/");

        let cookies = read(profile.path(), temp.path(), &[".second.test", "other.test"]).unwrap();
        assert_eq!(names(&cookies), ["other", "second"]);
        assert!(
            read(profile.path(), temp.path(), &["", "."])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn leaves_out_container_and_partitioned_cookies() {
        let future = Some(NOW + 3600);
        let (profile, _firefox) = profile_with(&[
            row("plain", ".example.com", future),
            Row {
                origin: "^userContextId=1",
                ..row("container", ".example.com", future)
            },
            Row {
                origin: "^partitionKey=%28https%2Cother.test%29",
                ..row("partitioned", ".example.com", future)
            },
            Row {
                origin: "^privateBrowsingId=1",
                ..row("private", ".example.com", future)
            },
        ]);
        let temp = tempfile::tempdir().unwrap();

        let cookies = read(profile.path(), temp.path(), &["example.com"]).unwrap();
        assert_eq!(names(&cookies), ["plain"]);
    }

    #[test]
    fn drops_expired_cookies_and_reads_seconds_and_milliseconds() {
        let (profile, _firefox) = profile_with(&[
            row("seconds_future", ".example.com", Some(NOW + 60)),
            row("seconds_past", ".example.com", Some(NOW - 60)),
            row("seconds_now", ".example.com", Some(NOW)),
            row("ms_future", ".example.com", Some((NOW + 120) * 1000)),
            row("ms_past", ".example.com", Some((NOW - 120) * 1000)),
            row("zero", ".example.com", Some(0)),
            row("session", ".example.com", None),
        ]);
        let temp = tempfile::tempdir().unwrap();

        let cookies = read(profile.path(), temp.path(), &["example.com"]).unwrap();
        assert_eq!(names(&cookies), ["ms_future", "seconds_future", "session"]);
        let expires = |name: &str| {
            cookies
                .iter()
                .find(|cookie| cookie.name == name)
                .unwrap()
                .expires
        };
        assert_eq!(expires("seconds_future"), Some(NOW + 60));
        assert_eq!(expires("ms_future"), Some(NOW + 120));
        assert_eq!(expires("session"), None);
    }

    #[test]
    fn reads_flags_and_same_site() {
        let future = Some(NOW + 3600);
        let (profile, _firefox) = profile_with(&[
            Row {
                secure: 1,
                http_only: 1,
                same_site: 0,
                ..row("none", ".example.com", future)
            },
            Row {
                same_site: 1,
                ..row("lax", ".example.com", future)
            },
            Row {
                same_site: 2,
                ..row("strict", ".example.com", future)
            },
            Row {
                same_site: 256,
                ..row("unknown", ".example.com", future)
            },
        ]);
        let temp = tempfile::tempdir().unwrap();

        let cookies = read(profile.path(), temp.path(), &["example.com"]).unwrap();
        let cookie = |name: &str| cookies.iter().find(|cookie| cookie.name == name).unwrap();
        assert!(cookie("none").secure && cookie("none").http_only);
        assert!(!cookie("lax").secure && !cookie("lax").http_only);
        assert_eq!(cookie("none").same_site, SameSite::None);
        assert_eq!(cookie("lax").same_site, SameSite::Lax);
        assert_eq!(cookie("strict").same_site, SameSite::Strict);
        assert_eq!(cookie("unknown").same_site, SameSite::Lax);
    }

    #[test]
    fn reads_the_write_ahead_log_and_leaves_firefox_files_unchanged() {
        let future = Some(NOW + 3600);
        let (profile, firefox) = profile_with(&[row("checkpointed", ".example.com", future)]);
        firefox
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        firefox
            .query_row("PRAGMA wal_autocheckpoint=0", [], |_| Ok(()))
            .unwrap();
        // Written while "Firefox" keeps the database open: only in the -wal file so far.
        insert(&firefox, &[row("in_wal", ".example.com", future)]);
        let database = profile.path().join("cookies.sqlite");
        let wal = profile.path().join("cookies.sqlite-wal");
        let before = (fs::read(&database).unwrap(), fs::read(&wal).unwrap());
        assert!(!before.1.is_empty(), "the test row must be in the WAL");
        let listing_before = entries(profile.path());
        let temp = tempfile::tempdir().unwrap();

        let cookies = read(profile.path(), temp.path(), &["example.com"]).unwrap();
        assert_eq!(names(&cookies), ["checkpointed", "in_wal"]);
        assert_eq!(fs::read(&database).unwrap(), before.0);
        assert_eq!(fs::read(&wal).unwrap(), before.1);
        let mut listing_after = entries(profile.path());
        let mut listing_before = listing_before;
        listing_after.sort();
        listing_before.sort();
        assert_eq!(
            listing_after, listing_before,
            "no file was added to the profile"
        );
        assert!(entries(temp.path()).is_empty(), "the copy was deleted");
    }

    #[test]
    fn source_is_byte_identical_and_temp_folder_is_gone_without_a_wal() {
        let (profile, firefox) = profile_with(&[row("a", ".example.com", Some(NOW + 60))]);
        drop(firefox); // Closing checkpoints and removes the -wal file.
        let database = profile.path().join("cookies.sqlite");
        assert!(!profile.path().join("cookies.sqlite-wal").exists());
        let before = fs::read(&database).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("work");

        let cookies = read(profile.path(), &parent, &["example.com"]).unwrap();
        assert_eq!(names(&cookies), ["a"]);
        assert_eq!(fs::read(&database).unwrap(), before);
        assert!(entries(&parent).is_empty());
    }

    #[test]
    fn temp_folder_is_gone_after_an_error() {
        let temp = tempfile::tempdir().unwrap();

        let garbage = tempfile::tempdir().unwrap();
        fs::write(
            garbage.path().join("cookies.sqlite"),
            b"not a database at all",
        )
        .unwrap();
        assert!(read(garbage.path(), temp.path(), &["example.com"]).is_err());
        assert!(entries(temp.path()).is_empty());
        assert_eq!(
            fs::read(garbage.path().join("cookies.sqlite")).unwrap(),
            b"not a database at all"
        );

        let no_table = tempfile::tempdir().unwrap();
        Connection::open(no_table.path().join("cookies.sqlite"))
            .unwrap()
            .execute_batch("CREATE TABLE other (x)")
            .unwrap();
        assert!(matches!(
            read(no_table.path(), temp.path(), &["example.com"]),
            Err(FirefoxError::Database(_))
        ));
        assert!(entries(temp.path()).is_empty());
    }

    #[test]
    fn missing_profile_is_an_error_and_missing_database_means_no_cookies() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing-profile");
        assert!(matches!(
            read(&missing, temp.path(), &["example.com"]),
            Err(FirefoxError::ProfileNotFound(_))
        ));
        let empty = tempfile::tempdir().unwrap();
        assert!(
            read(empty.path(), temp.path(), &["example.com"])
                .unwrap()
                .is_empty()
        );
        assert_eq!(entries(temp.path()), Vec::<PathBuf>::new());
    }

    #[test]
    fn temp_dir_names_are_recognized() {
        assert!(is_temp_dir_name("firefox-cookies-12-34-1"));
        assert!(!is_temp_dir_name("job-12-1"));
        assert!(!is_temp_dir_name("cookies-12-1.txt"));
    }

    #[test]
    fn host_matching_is_exact_or_a_dot_suffix() {
        assert!(host_matches(".youtube.com", "youtube.com"));
        assert!(host_matches("youtube.com", "youtube.com"));
        assert!(host_matches("m.YouTube.com", "youtube.com"));
        assert!(host_matches("www.youtube.com", ".youtube.com"));
        assert!(!host_matches("notyoutube.com", "youtube.com"));
        assert!(!host_matches("youtube.com.evil.test", "youtube.com"));
        assert!(!host_matches("youtube.com", "www.youtube.com"));
        assert!(!host_matches("youtube.com", ""));
    }

    const INI: &str = "[General]\nStartWithLastProfile=1\nVersion=2\n\n\
        [Profile1]\nName=default\nIsRelative=1\nPath=Profiles/old.default\nDefault=1\n\n\
        [Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/abc.default-release\n\n\
        [Profile2]\nName = Work Profile\nIsRelative=0\nPath=D:\\Firefox\\work\n\n\
        [Install308046B0AF4A39CB]\nDefault=Profiles/abc.default-release\nLocked=1\n";

    #[test]
    fn profiles_ini_lists_profile_sections_only() {
        let ini = ProfilesIni::parse(INI);
        assert_eq!(ini.names(), ["default", "default-release", "Work Profile"]);
        let old = "\n[InstallABC]\nDefault=Profiles/default\n\n[Profile0]\nName=default-release\nIsRelative=1\n\n[Profile1]\nName = Work Profile\nPath=Profiles/work\n\n[General]\nName=ignored\n";
        assert_eq!(
            ProfilesIni::parse(old).names(),
            ["default-release", "Work Profile"]
        );
        assert!(
            ProfilesIni::parse("[General]\nName=nope")
                .names()
                .is_empty()
        );
        assert!(ProfilesIni::parse("").names().is_empty());
        let bom = format!("\u{feff}{INI}");
        assert_eq!(ProfilesIni::parse(&bom), ProfilesIni::parse(INI));
    }

    #[test]
    fn default_profile_prefers_install_then_default_flag_then_first() {
        assert_eq!(
            ProfilesIni::parse(INI).default_profile(),
            Some("default-release")
        );

        let backslashes = INI.replace(
            "Default=Profiles/abc.default-release\nLocked",
            "Default=Profiles\\abc.default-release\nLocked",
        );
        assert_eq!(
            ProfilesIni::parse(&backslashes).default_profile(),
            Some("default-release")
        );

        let unknown_install = INI.replace(
            "Default=Profiles/abc.default-release\nLocked",
            "Default=Profiles/gone\nLocked",
        );
        assert_eq!(
            ProfilesIni::parse(&unknown_install).default_profile(),
            Some("default")
        );

        let no_flags = "[Profile0]\nName=first\nPath=Profiles/a\nIsRelative=1\n\
            [Profile1]\nName=second\nPath=Profiles/b\nIsRelative=1\n";
        assert_eq!(
            ProfilesIni::parse(no_flags).default_profile(),
            Some("first")
        );
        assert_eq!(ProfilesIni::parse("").default_profile(), None);
    }

    #[test]
    fn profile_dir_resolves_relative_and_absolute_paths() {
        let ini = ProfilesIni::parse(INI);
        let base = Path::new("base-dir");
        assert_eq!(
            ini.profile_dir(base, "default-release"),
            Some(base.join("Profiles").join("abc.default-release"))
        );
        #[cfg(windows)]
        assert_eq!(
            ini.profile_dir(base, "Work Profile"),
            Some(PathBuf::from("D:\\Firefox\\work"))
        );
        assert_eq!(ini.profile_dir(base, "missing"), None);

        let absolute = std::env::temp_dir().join("firefox-profile");
        let text = format!(
            "[Profile0]\nName=abs\nIsRelative=0\nPath={}\n",
            absolute.display()
        );
        assert_eq!(
            ProfilesIni::parse(&text).profile_dir(base, "abs"),
            Some(absolute)
        );
        assert_eq!(
            ProfilesIni::parse("[Profile0]\nName=nopath\n").profile_dir(base, "nopath"),
            None
        );
    }
}

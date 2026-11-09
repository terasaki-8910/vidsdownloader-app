// The contract between the webview and the Rust shell (src-tauri). The Rust side mirrors
// these shapes with serde (camelCase fields; enum values as written here). Change both
// sides together.

/** Lifecycle of a job; Done, Failed and Canceled are terminal. */
export type JobState =
  | "Queued"
  | "Probing"
  | "Ready"
  | "Downloading"
  | "PostProcessing"
  | "Done"
  | "Failed"
  | "Canceled";

/** Kind of failure; decides the next action offered on a failed row. */
export type ErrorCategory =
  | "UnsupportedUrl"
  | "PageMediaNotFound"
  | "PageGate"
  | "PageMediaChoice"
  | "LoginRequired"
  | "Unavailable"
  | "GeoBlocked"
  | "RateLimited"
  | "NoVideo"
  | "Drm"
  | "ExtractorBroken"
  | "Network"
  | "Postprocess"
  | "Unknown";

/** What the probe found about a video; absent until probing has finished. */
export interface JobDetails {
  title: string | null;
  uploader: string | null;
  /** Friendly site name, e.g. "YouTube", "ニコニコ動画". */
  site: string | null;
  /** Unix seconds. */
  publishedAt: number | null;
  durationS: number | null;
  thumbnailUrl: string | null;
}

/** One row of the download queue. */
export interface JobRow {
  id: number;
  url: string;
  state: JobState;
  attempt: number;
  /** 0..=1, or null when unknown. */
  progress: number | null;
  /** Extra work within the current lifecycle state. */
  phase: "pageSearch" | null;
  speedBytesPerS: number | null;
  etaS: number | null;
  error: { category: ErrorCategory; message: string } | null;
  details: JobDetails | null;
  /** Absolute path of the saved file once Done. */
  outputPath: string | null;
  /** Parsed raw comments saved by a comments-only job. */
  commentsCount: number | null;
  /**
   * The newest saved-library item made by this job, or null when the job has no saved item.
   * Library rename/trash/forget commands update or remove the linked queue row as documented.
   */
  libraryId: number | null;
  /** Set when a filter skipped the job (Done without a file). */
  skipped: string | null;
  /** Child jobs added when this row was a list. */
  listCount: number | null;
  /** Entries reported by the list before the 300-item cap. */
  listTotal: number | null;
  /**
   * Set when the job was skipped because the video is already saved. savedAt is Unix
   * seconds (null when unknown); fileMissing means only the library remembers it.
   */
  alreadySaved: { savedAt: number | null; fileMissing: boolean; path: string } | null;
  /** Equally plausible page videos waiting for the user to choose one. */
  pageMediaChoices: { url: string; kind: string; durationSeconds: number | null }[];
}

/** A saved video in the library. */
export interface LibraryRow {
  id: number;
  title: string | null;
  uploader: string | null;
  site: string | null;
  publishedAt: number | null;
  durationS: number | null;
  path: string;
  commentsPath: string | null;
  missing: boolean;
  addedAt: number;
}

/**
 * What erasing a saved item's records did (library_trash, library_forget), so the UI can
 * tell the user what could not be erased.
 */
export interface EraseResult {
  /** The database's write-ahead log was copied into the database and emptied. */
  checkpointed: boolean;
  /** The app UI's HTTP disk cache (thumbnails) was cleared; always false on macOS for now. */
  cacheCleared: boolean;
  /** Side files that could not be moved to the Recycle Bin / Trash and stayed where they are. */
  sideFilesFailed: string[];
}

/** Quality presets; the Rust shell maps them to yt-dlp arguments. */
export type Preset =
  | "best"
  | "upTo1080"
  | "upTo720"
  | "compatibilityH264"
  | "audioM4a"
  | "audioMp3"
  | "commentsOnly";

export type CommentsExtractionResult =
  | { id: number | null; status: "saved"; count: number }
  | { id: number | null; status: "conflict"; existingCount: number; existingModified: number | null; newCount: number; token: string; fileName: string }
  | { id: number | null; status: "failed"; message: string }
  | { id: null; status: "needsUrl" };

export type Theme = "system" | "light" | "dark";

/** How often sources are checked for new items while the app is open. */
export type SourceAutoCheck = "off" | "hourly" | "sixHours" | "daily";
export type Language = "ja" | "en";

/**
 * Which desktop notifications are shown; all on by default. `enabled: false` silences every
 * notification while the three kinds keep their own values. Events of one kind within 10 s
 * are shown as one notification, and none while the app window is in front.
 */
export interface NotifySettings {
  enabled: boolean;
  /** A download finished and saved a file. */
  done: boolean;
  /** A download failed. */
  failed: boolean;
  /** A source check queued new items. */
  found: boolean;
}

export interface Settings {
  language: Language;
  theme: Theme;
  /** Parallel downloads, 1..=5. */
  concurrency: number;
  /** Offer a copied URL in a banner when the window gains focus. */
  clipboardBanner: boolean;
  /** Include ad cards in results collected from listing pages. */
  collectIncludeAds: boolean;
  /** Keep subtitle files after they have been embedded into the media file. */
  keepSubtitleFiles: boolean;
  /** Prefer VP9, then H.264, over AV1 at the selected resolution. */
  avoidAv1: boolean;
  /** Absolute path; downloads are saved below it. */
  downloadDir: string;
  /** Folders used as the download folder, most recent first, at most 5. */
  recentDownloadDirs: string[];
  /** Preset selected when the app starts. */
  defaultPreset: Preset;
  /** The in-app browser's start page no longer offers importing logins from Firefox. */
  importOfferDismissed: boolean;
  /** Automatic source checks while the app is open (Settings > 一般 「新着を自動で確認」); off by default. */
  sourceAutoCheck: SourceAutoCheck;
  /** Desktop notifications (done / failed / new items, plus one switch for all). */
  notify: NotifySettings;
  /**
   * The version the user chose 「この版はスキップ」 for: the launch check does not offer it again
   * (a newer one is offered). null until one is skipped.
   */
  skippedUpdateVersion: string | null;
}

export type LoginMethod =
  | { kind: "inApp" }
  | { kind: "firefox"; profile: string }
  | { kind: "cookiesTxt"; path: string };

export interface Account {
  id: number;
  label: string;
  method: LoginMethod;
  cookieCount: number | null;
  /** Unix seconds. */
  updatedAt: number | null;
}

export interface FirefoxProfilesInfo {
  /** Same names, same order as firefox_profiles. */
  profiles: string[];
  /** The profile Firefox opens by default; null when there are no profiles. */
  defaultProfile: string | null;
}

export type FirefoxImportSkipReason = "loggedIn" | "notInApp" | "none";

export interface FirefoxImportedSite {
  siteId: number;
  name: string;
  /** Cookies added to the site's selected account. */
  cookies: number;
}

export interface FirefoxSkippedSite {
  siteId: number;
  name: string;
  reason: FirefoxImportSkipReason;
}

export interface FirefoxImportResult {
  imported: FirefoxImportedSite[];
  skipped: FirefoxSkippedSite[];
}

export interface Site {
  id: number;
  name: string;
  domains: string[];
  loginUrl: string;
  builtin: boolean;
  accounts: Account[];
  selectedAccountId: number | null;
  adBlock: boolean;
}

export interface BrowserState {
  url: string | null;
  loading: boolean;
  mode: "browse" | "login";
  siteId: number | null;
  accountId: number | null;
}

export interface FlowStep {
  pattern: string;
  label: string;
  url: string;
  matchCount: number;
  /** Zero-based position in the nearest row containing at least two matching elements. */
  rowIndex: number;
  rowCount: number;
  /** Zero-based position among every element matching the pattern on the page. */
  pageIndex: number;
}

export interface Flow {
  startUrl: string;
  steps: FlowStep[];
  media: string | null;
}

export type FlowScope = "this" | "restOfRow" | "allOfKind";

export interface SavedFlow {
  id: number;
  siteId: number | null;
  host: string;
  name: string;
  startUrl: string;
  steps: FlowStep[];
  scopes: FlowScope[];
  createdAt: number;
  updatedAt: number;
}

/** A page saved from the in-app browser; the URL is as it was shown (query kept, no #fragment). */
export interface SavedPage {
  id: number;
  url: string;
  name: string;
  createdAt: number;
}

export interface CollectMethod {
  kind: "rule" | "flow";
  id: number;
  name: string;
  siteId: number | null;
  /** Registered site name, or the host without "www." when it is not registered. */
  siteName: string;
  host: string;
  /** A rule's match pattern without its trailing "*", or a flow's start URL. */
  pageUrl: string;
  /** A rule's link shape; null for a recorded flow. */
  shape: string | null;
  /** Number of recorded operations; null for a collect rule. */
  stepCount: number | null;
  /** Saved replay scopes; null for a collect rule. */
  scopes: FlowScope[] | null;
  destDir: string | null;
  preset: Preset | null;
  /** Unix seconds. */
  lastUsedAt: number | null;
}

export interface CollectMethodRef {
  kind: "rule" | "flow";
  id: number;
}

export interface FlowReplayItem {
  replayLabel: string;
  media: string | null;
}

export interface FlowReplayResult {
  results: FlowReplayItem[];
  capped: boolean;
  total: number;
}

export interface CollectGroup {
  /**
   * A URL shape such as "/watch?v={id}", "{images}" for the page's own images, or
   * "__played__" for streams observed by Rust. Link and image shapes can be saved in a rule.
   */
  shape: string;
  count: number;
}

export interface CollectItem {
  url: string;
  title: string;
  thumbnail: string | null;
  /**
   * A page image's 1-based number among the page's images in page order, kept across
   * scans; null for links. The Rust shell always sends it.
   */
  index: number | null;
  /** Present only when this item is a stream observed in the visible page. */
  played?: { kind: "HLS" | "DASH" | "MP4" | "WebM" | "MOV"; durationSeconds: number | null };
}

/** Facts about the page itself, read the same way on every site; used to name page images. */
export interface CollectPage {
  /** og:title, else the document title. */
  title: string;
  /** JSON-LD author name, else meta author / article:author; null when none. */
  uploader: string | null;
  /** Unix seconds: JSON-LD datePublished, else article:published_time, else <article> <time>. */
  publishedAt: number | null;
  /** The URL's last path segment without extension, when it has at least 3 characters. */
  id: string | null;
}

/** Why a collect run is not running: the page's end was reached, it was stopped (by the user, a page change or an error), or the session holds the most items it keeps. */
export type CollectStopReason = "end" | "stopped" | "cap";

export interface CollectState {
  running: boolean;
  /** Scroll steps made so far. Informational: nothing stops on it any more. */
  scrolls: number;
  /** Kept for the saved rule's stop.maxPages; the collect loop does not stop on it any more. */
  maxScrolls: number;
  /** Why the run is over; null while it runs and before the first run. */
  stopReason: CollectStopReason | null;
  groups: CollectGroup[];
  shape: string | null;
  items: CollectItem[];
  adsExcluded: number;
  ruleId: number | null;
  error: string | null;
  /** The page facts from the last scan; null until the first scan has returned. */
  page: CollectPage | null;
  /** The best-ranked link group's URLs while the page's images are chosen; null otherwise. */
  otherLinks: CollectLinks | null;
}

/** A link group's shape and URLs, listed folded under the page's images. */
export interface CollectLinks {
  shape: string;
  urls: string[];
}

/** A badge the person clicked in the page: the found item and whether it is selected now. */
export interface CollectToggle {
  url: string;
  selected: boolean;
}

/**
 * How the element picker ("pick from the page") is going. `active`: pick mode is on in the page.
 * `ended`: it is over and nothing was picked (Esc in the page, the stop button, a page change).
 * `finding`: a video was pressed and its stream is being looked for (up to 10 s). `link`: a link
 * was pressed -- its address, the shape it belongs to and how many links have that shape.
 * `image`: a page image was pressed. `video`: a video was pressed; the played group is chosen and
 * these are its new streams. `videoNotFound`: no new stream arrived in time.
 */
export type CollectPick =
  | { kind: "active" }
  | { kind: "ended" }
  | { kind: "finding" }
  | { kind: "link"; url: string; shape: string; count: number }
  | { kind: "image"; url: string }
  | { kind: "video"; urls: string[] }
  | { kind: "videoNotFound" };

/** What a source's last check found. */
export type SourceResult =
  /** `count` new items were queued. */
  | { kind: "new"; count: number }
  | { kind: "none" }
  | { kind: "error"; category: ErrorCategory; message: string };

/** One registered source (a yt-dlp list or a page read with a saved rule). */
export interface SourceRow {
  id: number;
  name: string;
  url: string;
  kind: "list" | "rule";
  /** The registered site matching the URL, else the URL's host without "www.". */
  siteName: string;
  /** The rule's link shape; null for lists. */
  ruleShape: string | null;
  /** The site's selected account label, only when the site has two or more accounts. */
  accountLabel: string | null;
  /** Where its items are saved; null uses the download folder. */
  destDir: string | null;
  /** Its quality; null uses the default preset. */
  preset: Preset | null;
  /** Unix seconds; null before the first check. */
  lastCheckedAt: number | null;
  lastResult: SourceResult | null;
  status: "idle" | "waiting" | "checking";
}

/** What a URL or the collect session would register, with how many items it has now. */
export interface SourcePreview {
  /** For source_register; valid for 15 minutes and until the next preview. */
  token: string;
  kind: "list" | "rule";
  name: string;
  url: string;
  count: number;
  /** The source already registered for this URL, if any. */
  existingId: number | null;
}

export type SourcePreviewOutcome =
  | { kind: "ready"; preview: SourcePreview }
  | {
      kind: "failed";
      reason: "notAList" | "loginRequired" | "unsupported" | "other";
      message: string;
    };

/**
 * What yt-dlp makes of the URL of a newly added site, without downloading (site_probe).
 * `loggedIn` on a list or a video: the run was given the site's login (withLogin, and the
 * selected account had one to give), so it shows what a logged-in person sees.
 */
export type SiteProbeReport =
  /** A list of `count` entries; `videos` says whether any entry names a site extractor. */
  | { kind: "list"; count: number; videos: "supported" | "unknown"; loggedIn: boolean }
  /** A single video yt-dlp handles. */
  | { kind: "video"; loggedIn: boolean }
  /** The page needs a login. */
  | { kind: "loginRequired" }
  /** yt-dlp has no extractor for the URL (or only the generic one answered, finding no video). */
  | { kind: "unsupported" }
  /** Anything else: a network error, a broken extractor, a timeout (90 s). */
  | { kind: "failed"; category: ErrorCategory; message: string };

export type ToolId = "yt-dlp" | "ffmpeg" | "deno" | "gallery-dl" | "ublock-lite";

export interface ToolStatus {
  tool: ToolId;
  installed: boolean;
  version: string | null;
  expectedVersion: string;
}

export interface ToolProgress {
  tool: ToolId;
  phase: "downloading" | "installing" | "done" | "failed";
  downloadedBytes: number;
  totalBytes: number | null;
  message: string | null;
}

/** Commands the webview may invoke, with their argument and result types. */
export interface Commands {
  queue_snapshot: { args: Record<string, never>; result: JobRow[] };
  /** tzOffsetSeconds: -new Date().getTimezoneOffset() * 60, the local UTC offset used
   *  to date files (the webview knows it reliably on every OS). */
  enqueue_urls: {
    args: { urls: string[]; preset: Preset; tzOffsetSeconds: number; method?: CollectMethodRef | null };
    result: number[];
  };
  /**
   * Queues one job for the page (the job's URL is the page URL) that saves the chosen page
   * images directly, without yt-dlp or gallery-dl, as "<name> <index>.<ext>" from the
   * current naming settings; no cookies are sent. title / uploader / publishedAt / id are
   * CollectState.page. Returns the job id. Rejects a non-http(s) URL, an index below 1 and
   * an empty choice.
   */
  enqueue_page_images: {
    args: {
      pageUrl: string;
      title: string | null;
      uploader: string | null;
      publishedAt: number | null;
      id: string | null;
      images: { url: string; index: number }[];
      tzOffsetSeconds: number;
      method?: CollectMethodRef | null;
    };
    result: number;
  };
  /**
   * Queues one direct-stream job per item. Each job keeps pageUrl as its queue URL and stores
   * mediaUrl plus pageUrl as the page_media URL and Referer, so it never enters the
   * Unsupported-URL page search. Returns the queued job ids.
   */
  enqueue_page_media: {
    args: {
      items: { mediaUrl: string; pageUrl: string; title: string }[];
      preset: Preset;
      tzOffsetSeconds: number;
      method?: CollectMethodRef | null;
    };
    result: number[];
  };
  cancel_job: { args: { id: number }; result: null };
  retry_job: { args: { id: number }; result: null };
  /** Selects one persisted page-video choice and retries the failed job. */
  page_media_choose: { args: { jobId: number; url: string }; result: null };
  /** Downloads a job that was skipped as already saved anyway; queues it again. */
  save_again: { args: { id: number }; result: null };
  /** Reads the clipboard once; returns an http(s) URL found there, or null. */
  read_clipboard_url: { args: Record<string, never>; result: string | null };
  /** Replaces the clipboard contents with user-selected text. */
  copy_text: { args: { text: string }; result: null };
  /** Opens an http(s) URL in the operating system's default browser; any other URL is rejected. */
  open_external_url: { args: { url: string }; result: null };
  library_search: {
    args: { text: string | null; limit: number; offset: number };
    result: LibraryRow[];
  };
  /** Opens the saved item's file with the operating system's default application. */
  library_open: { args: { id: number }; result: null };
  /**
   * Lets the user pick a program to open the saved item's file with (Windows: the "Open
   * with" dialog; macOS: an application from /Applications). Cancelling is not an error;
   * a missing file or library row is.
   */
  library_open_with: { args: { id: number }; result: null };
  /**
   * Renames the saved item's file to `name` (without extension; extension and folder
   * stay) together with its side files (comments, subtitles, thumbnail). Rejects an
   * invalid, unchanged or taken name with a message, renaming nothing. Emits
   * "library-updated" and, for the linked queue row, "job-updated".
   */
  library_rename: { args: { id: number; name: string }; result: LibraryRow };
  comments_extract: { args: { libraryIds: number[] }; result: CommentsExtractionResult[] };
  comments_resolve: { args: { token: string; keep: "existing" | "new" }; result: CommentsExtractionResult };
  pick_video_file: { args: Record<string, never>; result: string | null };
  file_extract_comments: { args: { path: string; url: string | null }; result: CommentsExtractionResult };
  /**
   * Moves the saved item's file and side files to the Recycle Bin / Trash, then erases its
   * records like library_forget. Rejected, with nothing changed, while a job tied to the
   * item has not finished or when the file itself cannot be moved.
   */
  library_trash: { args: { id: number }; result: EraseResult };
  /**
   * Removes the saved item from the list and erases its records without traces (library
   * row, queue rows pointing to it, their work folders, SQLite free pages and WAL, the app
   * UI's thumbnail cache); the files stay. Rejected, with nothing changed, while a job tied
   * to the item has not finished. Emits "job-removed" per erased queue row and
   * "library-updated".
   */
  library_forget: { args: { id: number }; result: EraseResult };
  /**
   * Removes eligible rows from Downloads and forgets the saved videos (library rows, one per
   * file for a job that saved several) of the removed jobs, erasing their records without
   * traces as library_forget does; the files stay and no confirmation is asked. A saved
   * video another unfinished job is tied to stays. Waiting rows are canceled first.
   * Probing/downloading/post-processing rows are left in place (with their saved videos).
   * Emits "job-removed" once per row that is removed and "library-updated" when saved
   * videos were forgotten.
   */
  jobs_remove: { args: { ids: number[] }; result: null };
  /** Shows the file in the OS file manager. */
  reveal_path: { args: { path: string }; result: null };
  open_job_file: { args: { id: number }; result: void };
  job_file_exists: { args: { id: number }; result: boolean };
  get_settings: { args: Record<string, never>; result: Settings };
  set_settings: { args: { settings: Settings }; result: null };
  /** Opens the native folder picker without changing settings. */
  pick_download_dir: { args: Record<string, never>; result: string | null };
  tools_status: { args: Record<string, never>; result: ToolStatus[] };
  /** Installs every missing or outdated tool; progress arrives as "tools-progress" events. */
  install_tools: { args: Record<string, never>; result: null };
  sites_list: { args: Record<string, never>; result: Site[] };
  site_add: { args: { url: string }; result: Site };
  /**
   * Asks yt-dlp what it makes of the URL: one flat run without downloading, ended after
   * 90 s. The first probe of a new site goes without cookies (that is how a login wall
   * shows); with `withLogin` the run gets the selected account's cookies, as a job's does
   * (a temporary file, deleted when the run ends). Resolves with a failed report instead of
   * rejecting when the run fails; rejects only a non-http(s) URL.
   */
  site_probe: { args: { url: string; withLogin?: boolean }; result: SiteProbeReport };
  site_remove: { args: { siteId: number }; result: null };
  site_set_ad_block: { args: { siteId: number; enabled: boolean }; result: null };
  account_add: { args: { siteId: number; label: string }; result: Account };
  account_remove: { args: { accountId: number }; result: null };
  account_rename: { args: { accountId: number; label: string }; result: null };
  account_select: {
    args: { siteId: number; accountId: number };
    result: null;
  };
  account_set_method: {
    args: { accountId: number; method: LoginMethod };
    result: null;
  };
  account_logout: { args: { accountId: number }; result: null };
  firefox_profiles: { args: Record<string, never>; result: string[] };
  firefox_profiles_info: {
    args: Record<string, never>;
    result: FirefoxProfilesInfo;
  };
  /**
   * Windows only (elsewhere rejects with "not available on this system yet"). Emits
   * "sites-updated" once the accounts' cookie counts are recorded (also when a site failed).
   * Rejects with a message naming the failed sites when some site could not be imported;
   * the others are still imported.
   */
  firefox_import: { args: { profile: string }; result: FirefoxImportResult };
  pick_cookies_file: { args: Record<string, never>; result: string | null };
  site_for_url: { args: { url: string }; result: number | null };
  browser_open: {
    args: { url: string | null; accountId: number | null };
    result: null;
  };
  browser_set_bounds: {
    args: { x: number; y: number; width: number; height: number };
    result: null;
  };
  browser_set_visible: { args: { visible: boolean }; result: null };
  browser_back: { args: Record<string, never>; result: null };
  browser_forward: { args: Record<string, never>; result: null };
  browser_reload: { args: Record<string, never>; result: null };
  login_start: {
    args: { siteId: number; accountId: number };
    result: null;
  };
  login_finish: { args: Record<string, never>; result: Account };
  login_cancel: { args: Record<string, never>; result: null };
  collect_start: {
    args: { shape: string | null; maxScrolls: number };
    result: null;
  };
  collect_stop: { args: Record<string, never>; result: null };
  collect_reload_find: { args: Record<string, never>; result: { found: number } };
  collect_select: { args: { shape: string }; result: null };
  collect_close: { args: Record<string, never>; result: null };
  /** Scrolls the page to the item's card; false when the page no longer has it. */
  collect_reveal: { args: { url: string }; result: boolean };
  /**
   * The tray's selection changed: the found items it has NOT selected, so the page's badges show
   * the same checks (the page keeps clicks the app has not seen yet on top of this).
   */
  collect_set_selection: { args: { unselected: string[] }; result: null };
  /**
   * Turns on pick mode in the page ("pick from the page"); the page's labels use `language`.
   * Stops a running auto-scroll first. Results come as "collect-pick" events.
   */
  collect_pick_start: { args: { language: Language }; result: null };
  /** Turns pick mode off (and gives up a video lookup that is still waiting); emits "collect-pick" ended. */
  collect_pick_stop: { args: Record<string, never>; result: null };
  collect_save_rule: {
    args: { name: string; matchPattern: string };
    result: number;
  };
  /** Lists saved collecting rules and recorded flows with their queue options and last use. */
  methods_list: { args: Record<string, never>; result: CollectMethod[] };
  /** Sets or clears a method's absolute destination and quality override. */
  method_set_options: {
    args: { kind: "rule" | "flow"; id: number; destDir: string | null; preset: Preset | null };
    result: null;
  };
  /** Trims and replaces a method's display name; an empty name is rejected. */
  method_rename: {
    args: { kind: "rule" | "flow"; id: number; name: string };
    result: null;
  };
  /** Deletes a method, except for a rule still referenced by a registered source. */
  method_delete: {
    args: { kind: "rule" | "flow"; id: number };
    result: null;
  };
  flow_record_start: { args: Record<string, never>; result: null };
  flow_record_stop: { args: Record<string, never>; result: Flow };
  flow_save: {
    args: { name: string; flow: Flow & { scopes: FlowScope[] } };
    result: SavedFlow;
  };
  flow_list: { args: { url: string }; result: SavedFlow[] };
  flow_rename: { args: { id: number; name: string }; result: null };
  flow_delete: { args: { id: number }; result: null };
  flow_replay: {
    args: { flow: Flow; scopes: FlowScope[] };
    result: FlowReplayResult;
  };
  flow_replay_cancel: { args: Record<string, never>; result: null };
  /** Every saved page, the first saved first. */
  saved_page_list: { args: Record<string, never>; result: SavedPage[] };
  /**
   * Saves a page: http(s) only, the URL at most 2048 characters (the #fragment is dropped, the
   * rest kept as given), the name trimmed to 1..=200 characters. A URL that is saved already
   * returns its existing row.
   */
  saved_page_add: { args: { url: string; name: string }; result: SavedPage };
  /** name as for saved_page_add; returns the updated row. */
  saved_page_rename: { args: { id: number; name: string }; result: SavedPage };
  /** Removes a saved page; one that is gone already is not an error. */
  saved_page_remove: { args: { id: number }; result: null };
  /** Every source in id order. tzOffsetSeconds as in enqueue_urls (automatic checks use the newest one passed). */
  sources_list: { args: { tzOffsetSeconds: number }; result: SourceRow[] };
  /**
   * Asks yt-dlp for the URL as a list (flat, with the site's account cookies) and keeps the
   * result as the newest preview. Rejects a non-http(s) URL.
   */
  source_preview: { args: { url: string }; result: SourcePreviewOutcome };
  /**
   * A rule source's preview from the open collect session (its page, shape and items so
   * far; at most 10 scrolls). Rejects when no session is open or no link shape is chosen.
   */
  source_preview_collect: { args: Record<string, never>; result: SourcePreview };
  /**
   * Registers the preview's source and records its current items as known; "saveAll"
   * also queues them. This counts as the first check: lastCheckedAt is set and lastResult
   * is "none" ("record") or "new" with the number queued ("saveAll"). Rejects with "the
   * preview expired" when the token is gone. Emits "source-updated".
   */
  source_register: {
    args: { token: string; choice: "record" | "saveAll"; tzOffsetSeconds: number };
    result: SourceRow;
  };
  /** Queues a check (returns at once; progress arrives as "source-updated"). */
  source_check: { args: { id: number; tzOffsetSeconds: number }; result: null };
  /** Queues a check of every source, in id order. */
  source_check_all: { args: { tzOffsetSeconds: number }; result: null };
  /** name is trimmed, 1..=200 characters. Emits "source-updated". */
  source_rename: { args: { id: number; name: string }; result: SourceRow };
  /** destDir must be absolute; null clears either option. Emits "source-updated". */
  source_set_options: {
    args: { id: number; destDir: string | null; preset: Preset | null };
    result: SourceRow;
  };
  /**
   * Deletes the source and its known items; files, saved items and jobs stay. A waiting or
   * running check of it is dropped. Emits "source-removed".
   */
  source_delete: { args: { id: number }; result: null };
  /** The same folder picker as pick_download_dir (starts in the download folder); settings are not changed. */
  pick_folder: { args: Record<string, never>; result: string | null };
}

/** Events the Rust shell emits, with their payloads. */
export interface Events {
  /** The in-app browser changed page, loading state or mode. */
  "browser-state": BrowserState;
  "collect-state": CollectState;
  /** Badges clicked in the page since the last event, oldest first (the app polls the page every 500 ms). */
  "collect-toggles": { toggles: CollectToggle[] };
  "collect-pick": CollectPick;
  "flow-record-progress": { steps: number };
  "flow-progress": { done: number; total: number };
  /** The site list changed (sites, accounts or their login status). */
  "sites-updated": Site[];
  /** A job was added or changed; the payload is the whole row. */
  "job-updated": JobRow;
  /** A job left the queue (its saved item's records were erased). */
  "job-removed": { id: number };
  /** Saved items were renamed or erased; open library views reload. No payload. */
  "library-updated": null;
  "tools-progress": ToolProgress;
  /** A source was added or changed (status, result, name, options); the payload is the whole row. */
  "source-updated": SourceRow;
  /** A source was deleted. */
  "source-removed": { id: number };
}

// ---------------------------------------------------------------------------
// In-app updates. These are not app commands: the webview talks to the updater and process
// plugins directly (./client.ts), and in `pnpm dev` the mock (./mock.ts) stands in for them.
// ---------------------------------------------------------------------------

/** A version newer than the running one, as the update endpoint announced it. */
export interface UpdateInfo {
  version: string;
  currentVersion: string;
  /** The release notes as plain text ("- " lines are list items), or null when there are none. */
  notes: string | null;
}

/** How an update's download goes: its size once known (null when not sent), each chunk, the end. */
export type UpdateDownloadEvent =
  | { kind: "started"; totalBytes: number | null }
  | { kind: "progress"; chunkBytes: number }
  | { kind: "finished" };

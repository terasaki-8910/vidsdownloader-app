import { formatClock, isSameDay } from "../lib/format";
import type { Translations } from "./types";

/** Fixed month names, so the date reads the same whatever the OS locale is. */
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** "Nov 4" in local time; the year only if it is not `now`'s. */
function dayOf(unixSeconds: number, now: Date): string {
  const date = new Date(unixSeconds * 1000);
  const day = `${MONTHS[date.getMonth()]} ${date.getDate()}`;
  return date.getFullYear() === now.getFullYear() ? day : `${day}, ${date.getFullYear()}`;
}

export const en: Translations = {
  common: {
    add: "Add",
    close: "Close",
    cancel: "Remove",
    dialogCancel: "Cancel",
    retry: "Retry",
    saveAgain: "Save again",
    stop: "Stop",
    removeFromQueue: "Remove",
    revealInFolder: "Show in folder",
    settings: "Settings",
    toggleThemeToLight: "Switch to light theme",
    toggleThemeToDark: "Switch to dark theme",
    login: "Log in",
    logout: "Log out",
    delete: "Delete",
    open: "Open",
    selected: (count) => `${count} selected`,
    clearSelection: "Clear selection",
  },

  sidebar: {
    brand: "vidsdownloader",
    addUrl: "Add URL",
    downloads: "Downloads",
    library: "Saved videos",
    browser: "Browser",
    settings: "Settings",
  },

  downloads: {
    title: "Downloads",
    summary: ({ progressing, waiting, failed, done }) =>
      `Active ${progressing} · Queued ${waiting} · Failed ${failed} · Done ${done}`,
    emptyTitle: "The queue is empty",
    emptyBody: "Paste a URL above and downloads will appear here.",
    showDetails: "Show details",
    copyUrl: "Copy URL",
    removeFromList: "Remove from list",
    stopCount: (count) => `Stop (${count})`,
    retryCount: (count) => `Retry (${count})`,
    removeCount: (count) => `Remove from list (${count})`,
    removeFailed: (total, failed, message) =>
      `${failed} of ${total} could not be removed from the list: ${message}`,
    removedAlongside: (count) => ` ${count} other ${count === 1 ? "row is" : "rows are"} removed from Downloads.`,
  },

  urlbar: {
    placeholder: "Paste video or playlist URLs (one per line)",
    qualityLabel: "Quality",
    folderLabel: "Save to",
    folderMenuHeading: "Save to",
    folderMenuOther: "Choose another folder…",
    add: "Add",
  },

  presets: {
    best: "Best quality (MP4)",
    upTo1080: "Up to 1080p",
    upTo720: "Up to 720p",
    compatibilityH264: "Compatibility (H.264)",
    audioM4a: "Audio only (M4A)",
    audioMp3: "Audio only (MP3)",
    commentsOnly: "Comments only (niconico, Bilibili)",
  },

  clipboard: {
    message: "Add the copied URL?",
    add: "Add",
    dismiss: "Dismiss",
  },

  tools: {
    missingNotice: "Some of yt-dlp / ffmpeg / deno are not installed yet.",
    installAction: "Install",
    sectionTitle: "Tools",
    names: {
      "yt-dlp": "yt-dlp",
      ffmpeg: "ffmpeg",
      deno: "deno",
      "gallery-dl": "gallery-dl",
      "ublock-lite": "uBlock Origin Lite",
    },
    installed: (version) => `Version ${version}`,
    updateAvailable: (version) => `Update available (${version})`,
    notInstalled: "Not installed",
    downloading: (percent) => (percent === null ? "Downloading…" : `Downloading ${percent}%`),
    installing: "Installing…",
    installButton: "Install",
    updateButton: "Update",
    allInstalled: "Everything is available",
    installFailed: "Installation failed",
  },

  queue: {
    queued: "Queued",
    downloading: "Downloading",
    probing: "Fetching info",
    pageSearch: "Looking for the video on the page…",
    postProcessing: "Post-processing",
    canceled: "Canceled",
    done: "Done",
    commentsOnly: (count) => `Comments only · ${count.toLocaleString("en-US")}`,
    commentsOnlyUnsupported: "Only niconico and Bilibili have comments to extract",
    skipped: (reason) => `Skipped (${reason})`,
    skipReason: {
      separator: ", ",
      minutes: (count) => `${count} min`,
      seconds: (count) => `${count} s`,
      shorterThan: (duration) => `shorter than ${duration}`,
      longerThan: (duration) => `longer than ${duration}`,
      publishedBefore: (date) => `published before ${date}`,
      publishedAfter: (date) => `published after ${date}`,
      titleIncludesNone: (words) => `title does not include any of: ${words.join(", ")}`,
      titleIncludesExcluded: (text) => `title includes excluded text: ${text}`,
      notVideo: "not a video",
      liveOrUpcoming: "live or upcoming",
      alreadyDownloaded: "already downloaded",
      matchFilter: "rejected by yt-dlp's match filter",
    },
    imagesNotSaved: (failed, total) => `${failed} of ${total} images could not be saved`,
    alreadySaved: "Skipped, already saved",
    savedOn: (unixSeconds, now) => {
      const saved = new Date(unixSeconds * 1000);
      const day = `${MONTHS[saved.getMonth()]} ${saved.getDate()}`;
      return saved.getFullYear() === now.getFullYear() ? `saved ${day}` : `saved ${day}, ${saved.getFullYear()}`;
    },
    fileMissing: "File not found (it may have moved)",
    listAdded: (count, total) =>
      `Added ${count} items to the queue${total != null && total > count ? ` (the first ${count} of ${total})` : ""}`,
    remaining: (clock) => `${clock} left`,
  },

  errors: {
    UnsupportedUrl: "This URL isn't supported",
    PageMediaNotFound: "No video was found on the page",
    PageGate: "Stopped at a confirmation page",
    PageMediaChoice: "Several videos were found",
    LoginRequired: "Login required",
    Unavailable: "This video isn't available",
    GeoBlocked: "Not available in this region",
    RateLimited: "Rate limited -- wait and retry",
    NoVideo: "No video found",
    Drm: "Protected by DRM; can't be fetched",
    ExtractorBroken: "The site's layout changed; not yet supported",
    Network: "A network error occurred",
    Postprocess: "Post-processing failed",
    Unknown: "An unknown error occurred",
  },

  pageMedia: {
    foundCount: (count) => `Found ${count} videos`,
    choose: "Choose",
    title: "Choose the video to save",
    unknownLength: "Unknown length",
    save: "Save",
  },

  detail: {
    quality: "Quality",
    savedTo: "Saved to",
  },

  player: {
    play: "Play",
    pip: "Picture in picture",
    pipExit: "Close picture in picture",
    unsupported: "This app can't play this file",
    openWith: "Open in another app",
    missing: "File not found (it may have moved)",
  },

  library: {
    title: "Saved videos",
    searchPlaceholder: "Search by title or uploader",
    emptyNoneYet: "No videos saved yet.",
    emptyNoResults: "No matches found.",
    missingBadge: "Missing",
    moreActions: "More actions",
    open: "Open",
    openWith: "Open with…",
    rename: "Rename",
    extractComments: "Extract comments",
    extractCommentsCount: (count) => `Extract comments (${count})`,
    extractingComments: "Extracting comments…",
    savedComments: (count) => `Saved ${count.toLocaleString("en-US")} comments`,
    addCommentsToFile: "Add comments to a file",
    commentsConflictTitle: "A comments file with the same name exists",
    keepExistingComments: "Keep the existing file",
    useNewComments: "Use the new one",
    moreComments: "(more)",
    commentsOldMeta: (count, date) => `${count.toLocaleString("en-US")} comments · ${date}`,
    commentsNewMeta: (count) => `${count.toLocaleString("en-US")} comments · now`,
    commentsApplyRemaining: (count) => `Do the same for the remaining ${count}`,
    commentsDecide: "Decide",
    commentsUrlTitle: "Enter the video page URL",
    commentsUrlBody: "No video ID was found in the file name. Enter its niconico or Bilibili video page.",
    commentsUrlPlaceholder: "Video page URL",
    commentsUrlInvalid: "Enter a niconico or Bilibili video page URL.",
    commentsExtract: "Extract",
    newName: "New name",
    nameEmpty: "Enter a name.",
    nameBadChars: 'A name cannot contain any of: \\ / : * ? " < > |',
    forget: "Remove from list (keep the file)",
    forgetTitle: "Remove from the list?",
    forgetBody: (title) =>
      `The records of "${title}" are erased from this app. The file stays where it is. The records cannot be restored.`,
    forgetBulkBody: (first, count) =>
      `The records for "${first}" and ${count - 1} other saved videos are erased from this app. The files stay where they are. The records cannot be restored.`,
    forgetCount: (count) => `Remove from list (${count})`,
    forgetFailed: (total, failed, message) =>
      `${failed} of ${total} could not be removed from the list: ${message}`,
    forgetConfirm: "Remove",
    trash: {
      windows: {
        menu: "Move to Recycle Bin",
        title: "Move to the Recycle Bin?",
        body: (title) =>
          `The file of "${title}" goes to the Recycle Bin and its records in this app are erased. The records cannot be restored.`,
        bulkBody: (first, count) =>
          `The files for "${first}" and ${count - 1} other videos go to the Recycle Bin and their records in this app are erased. The records cannot be restored.`,
        confirm: "Move to Recycle Bin",
        count: (count) => `Move to Recycle Bin (${count})`,
        bulkFailed: (total, failed, message) =>
          `${failed} of ${total} could not be moved to the Recycle Bin: ${message}`,
        sideFilesFailed: (names) => `Some files could not be moved to the Recycle Bin: ${names.join(", ")}`,
      },
      mac: {
        menu: "Move to Trash",
        title: "Move to Trash?",
        body: (title) =>
          `The file of "${title}" goes to the Trash and its records in this app are erased. The records cannot be restored.`,
        bulkBody: (first, count) =>
          `The files for "${first}" and ${count - 1} other videos go to the Trash and their records in this app are erased. The records cannot be restored.`,
        confirm: "Move to Trash",
        count: (count) => `Move to Trash (${count})`,
        bulkFailed: (total, failed, message) =>
          `${failed} of ${total} could not be moved to the Trash: ${message}`,
        sideFilesFailed: (names) => `Some files could not be moved to the Trash: ${names.join(", ")}`,
      },
    },
    tracesLeft: "The records were erased, but some traces could not be cleared. Restart the app and try again.",
    failed: (message) => `Failed: ${message}`,
  },

  sources: {
    title: "Sources",
    summary: (count, lastCheckedAt, now) => {
      const sources = `${count} ${count === 1 ? "source" : "sources"}`;
      if (lastCheckedAt == null) return sources;
      const when = isSameDay(lastCheckedAt, now) ? formatClock(lastCheckedAt) : dayOf(lastCheckedAt, now);
      return `${sources} · last check ${when}`;
    },
    checkAll: "Check all for new items",
    urlPlaceholder: "Paste a channel or list URL to register",
    register: "Register",
    previewing: "Looking it up…",
    emptyBody: "Channels and lists you register are checked for new items here, all in one place.",
    registerFailed: {
      notAList: "This isn't a list. Add videos from the Downloads screen.",
      loginRequired: "Login required. Log in under Settings > Sites, then register it.",
      unsupported: "yt-dlp can't read this list. Open it in the browser and register it from Collect.",
      other: (message) => `Couldn't look it up (${message})`,
      alreadyRegistered: "This source is already registered",
    },
    kindList: "List",
    kindRule: "Collect rule",
    destination: (folder) => `Saved to ${folder}`,
    account: (label) => `Account ${label}`,
    waiting: "Waiting",
    checking: "Checking…",
    neverChecked: "Not checked yet",
    dateTime: (unixSeconds, now) => `${dayOf(unixSeconds, now)} ${formatClock(unixSeconds)}`,
    checkedNew: (at, count) => `Checked ${at} · ${count} new ${count === 1 ? "item" : "items"} queued`,
    checkedNone: (at) => `Checked ${at} · nothing new`,
    checkedFailed: (at, reason) => `Checked ${at} · couldn't check (${reason})`,
    checkNow: "Check for new",
    options: "Destination and quality",
    optionsFolder: "Destination",
    optionsFolderDefault: (folder) => `Default (${folder})`,
    optionsChangeFolder: "Change…",
    optionsResetFolder: "Use default",
    optionsQuality: "Quality",
    optionsQualityDefault: (label) => `Default (${label})`,
    optionsSave: "Save",
    deleteTitle: (name) => `Delete ${name}?`,
    deleteBody:
      "Saved files and the records in Saved videos stay. The record of items already seen, used to find new ones, is erased.",
    // No-break spaces keep "N items" from splitting across lines.
    firstCheckTitle: (kind, count) =>
      kind === "list"
        ? `What about the\u00a0${count}\u00a0items there now?`
        : `What about the\u00a0${count}\u00a0items visible now?`,
    firstCheckBody: (kind, name) =>
      `${name}. Checks for new items save only what appears after this.` +
      (kind === "rule" ? " A source made with a collect rule is checked by opening the page and scrolling." : ""),
    recordOnly: "Record only",
    saveAll: "Save all",
    trayRegister: "Register as a source",
    trayRegistered: "Registered as a source",
    trayNote: "Uses this page to check for new items (per collect rule)",
    rowRegister: "Register as a source",
    rowRegistered: "Already a source",
    autoCheck: "Check for new items automatically",
    autoCheckOff: "Off",
    autoCheckHourly: "Every hour",
    autoCheckSixHours: "Every 6 hours",
    autoCheckDaily: "Once a day",
    autoCheckNote: "Checks only while the app is open.",
  },

  settings: {
    title: "Settings",
    generalSection: "General",
    sitesSection: "Sites",
    adBlockSection: "Ad blocking",
    toolsSection: "Tools",
    updatesSection: "Updates",
    language: "Language",
    languageJa: "日本語",
    languageEn: "English",
    theme: "Theme",
    themeSystem: "Follow system",
    themeLight: "Light",
    themeDark: "Dark",
    concurrency: "Parallel downloads",
    clipboardBanner: "Notify about copied URLs",
    collectIncludeAds: "Include ads when collecting",
    downloadFolder: "Download folder",
    changeDownloadFolder: "Change…",
    keepSubtitleFiles: "Keep subtitle files after embedding",
    defaultPreset: "Default quality preset",
    avoidAv1: "Avoid AV1",
    avoidAv1Hint: "Files get about 20–30 % larger, but play on more players.",
  },

  notify: {
    heading: "Notifications",
    enabled: "Show notifications",
    done: "When a download finishes",
    failed: "When a download fails",
    found: "When new items are found",
    note: "Not shown while the app window is in front.",
  },

  updates: {
    title: "A new version is available",
    updatingTitle: "Updating",
    newVersion: (version) => `vidsdownloader ${version}`,
    runningVersion: (version) => ` (you have ${version})`,
    notesHeading: "What's new",
    skip: "Skip this version",
    later: "Later",
    updateNow: "Update now",
    busy: (count) =>
      `You can't update while downloading (${count}\u00a0left). You'll be asked again once they finish.`,
    progressLabel: "Download progress",
    downloading: "Downloading the new version…",
    afterDownload: "When it is done, the app closes and opens again in the new version.",
    failed: (message) => `Could not update (${message})`,
    currentVersion: "Current version",
    check: "Check for updates",
    checking: "Checking…",
    upToDate: (at) => `Up to date (checked ${at})`,
    checkedAt: (unixSeconds, now) => `${dayOf(unixSeconds, now)} ${formatClock(unixSeconds)}`,
    found: (version) => `${version} is available`,
    install: "Update",
    skipped: (version) => `You skipped ${version}`,
    unskip: "Stop skipping",
    checkFailed: (message) => `Could not check (${message})`,
  },

  browser: {
    back: "Back",
    forward: "Forward",
    reload: "Reload",
    siteList: "Back to the site list",
    address: "Address",
    addressPlaceholder: "Enter a URL, or pick a site",
    addPage: "Add this page",
    pageAdded: "Added",
    collect: "Collect",
    foundLabel: "Found",
    all: "All",
    selectedOfTotal: (selected, total) => `${selected} / ${total}`,
    scrollProgress: (scrolls, maxScrolls) => `Auto-scroll ${scrolls} / ${maxScrolls}`,
    adsExcluded: (count) => `${count === 1 ? "1 ad" : `${count} ads`} left out`,
    collectStop: "Stop",
    collectResume: "Resume",
    groupOption: (shape, count) => `${shape} · ${count}`,
    imagesGroup: (count) => `Images on this page (${count})`,
    playedGroup: (count) => `Videos played on this page (${count})`,
    reloadFind: "Reload and look for videos",
    reloadFinding: "Looking for videos…",
    reloadFound: (count) => `Found ${count} ${count === 1 ? "video" : "videos"}`,
    reloadFoundNone: "No videos found",
    imageLabel: (index) => `Image ${index}`,
    otherLinks: (count) => `Other links (${count})`,
    collectEmpty: "Nothing found on this page",
    playedEmpty: "No video has played on this page yet",
    addCollected: (count) => `Add to queue (${count})`,
    collectedAdded: "Added",
    saveRule: "Save as rule",
    ruleName: "Rule name",
    matchPattern: "Matching URL",
    save: "Save",
    ruleSaved: "Rule saved",
    closeCollect: "Close collected results",
    xCollectWarning:
      "X prohibits collection in its terms. A logged-in account may be restricted. We recommend using a secondary account.",
    collectFailed: (message) => `Could not collect: ${message}`,
    revealItem: "Show on page",
    revealGone: "No longer on the page",
    loginPresent: "Login saved",
    loginAbsent: "No login",
    chipWithAccount: (state, account) => `${state} · ${account}`,
    loginBanner: (site) =>
      `Log in to ${site}. When you are done, press Done to save the login and close the page.`,
    loginCancel: "Cancel",
    loginDone: "Done",
    startTitle: "Sites",
    siteLoginPresent: (date) => (date ? `Login saved · updated ${date}` : "Login saved"),
    addSite: "Add a site",
    pagePlaceholder: "The page appears only inside the app",
    invalidUrl: "Enter an http or https URL",
    failed: (message) => `Could not open: ${message}`,
  },

  sites: {
    lead: "Sites you have registered. Login info stays on this PC and is used only for downloads from that site.",
    methodLabel: "Login method",
    inApp: "In-app browser",
    firefox: (profile) => `Firefox (${profile})`,
    cookiesTxt: "cookies.txt",
    cookiesTxtPick: "cookies.txt…",
    changeFile: "Change file…",
    statusCookies: (count, date) =>
      date ? `${count} cookies · updated ${date}` : `${count} cookies`,
    statusNone: "No login",
    statusFirefox: "Uses the Firefox login",
    statusFile: (name, date) => (date ? `${name} · ${date}` : name),
    addAccount: "Add account",
    accountName: (n) => `Account ${n}`,
    selectAccount: (label) => `Use ${label}`,
    addSiteTitle: "Add a site",
    addSitePlaceholder: "Site URL (e.g. https://www.example.com)",
    addSiteButton: "Add",
    addSiteNote: "Sites that need no login can be added too. You can log in later.",
    failed: (message) => `Failed: ${message}`,
    deleteSite: (name) => `Delete ${name}`,
    deleteSiteMenu: "Delete this site",
    deleteConfirmTitle: (name) => `Delete ${name}?`,
    deleteConfirmBody: (accounts) =>
      `Its login info (${accounts} ${accounts === 1 ? "account" : "accounts"}) is removed from this PC too.`,
  },

  siteReport: {
    heading: "What we found for this URL",
    probing: "Checking…",
    list: (count) => `Works as a list (yt-dlp) · ${count} ${count === 1 ? "video" : "videos"}`,
    noLoginNeeded: "Login: not needed",
    loginUsed: "Login: required (checked while logged in)",
    videosSupported: "Video pages: supported",
    videosUnknown: "Video pages: unknown until one is opened",
    registerSource: "Register as a source",
    registeredNow: "Registered as a source",
    registeredAlready: "Already a source",
    openInBrowser: "Open in browser",
    video: "Works as a video page (yt-dlp)",
    addToDownloads: "Add to downloads",
    addedToDownloads: "Added to downloads",
    loginRequired: "Login required",
    loginThenRetry: "You can log in, then check again",
    recheck: "Check again",
    unsupported: "yt-dlp can't handle this",
    unsupportedHint: "Open it in the browser and use Collect to make a rule",
    openAndCollect: "Open in browser and collect",
    failed: (reason) => `Could not check (${reason})`,
  },

  importLogins: {
    offerTitle: "Use logins from another browser",
    offerBody:
      "You can bring the sites you are signed in to in Firefox into this app's browser. Chrome and Edge logins cannot be read by other apps because of Windows' protection.",
    profileLabel: "Profile to import",
    profileOption: (name) => `Firefox: ${name}`,
    importButton: "Import",
    importing: "Importing…",
    notNow: "Not now",
    close: "Close",
    result: (imported, alreadySignedIn) =>
      (imported > 0
        ? `Imported logins for ${imported} ${imported === 1 ? "site" : "sites"}.`
        : "There were no logins to import.") +
      (alreadySignedIn > 0
        ? ` ${alreadySignedIn} already signed in ${alreadySignedIn === 1 ? "was left as it was" : "were left as they were"}.`
        : ""),
    blockTitle: "Import from Firefox",
    blockLead: "Brings Firefox's logins into sites you are not signed in to here.",
  },

  adBlock: {
    lead: "Blocks ads in the in-app browser (uBlock Origin Lite). You can turn it off per site.",
    siteSwitch: (name) => `Block ads on ${name}`,
    note: "Pages of sites that are not in the list always have ads blocked.",
  },
};

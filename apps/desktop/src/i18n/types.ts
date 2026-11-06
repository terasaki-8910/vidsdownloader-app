import type { ErrorCategory, Preset, ToolId } from "../ipc/contract";

/** What the "move the file to the Recycle Bin / Trash" action says. */
export interface TrashWords {
  menu: string;
  title: string;
  body: (title: string) => string;
  bulkBody: (first: string, count: number) => string;
  confirm: string;
  count: (count: number) => string;
  bulkFailed: (total: number, failed: number, message: string) => string;
  /** Notice: these side files could not be moved (file names only). */
  sideFilesFailed: (names: string[]) => string;
}

/**
 * Every visible string in the app goes through one of these. ja.ts and en.ts
 * both implement this shape so a missing translation is a compile error.
 */
export interface Translations {
  common: {
    add: string;
    close: string;
    cancel: string;
    /** The cancel button of a confirmation dialog (the other side of a destructive button). */
    dialogCancel: string;
    retry: string;
    saveAgain: string;
    stop: string;
    removeFromQueue: string;
    revealInFolder: string;
    settings: string;
    toggleThemeToLight: string;
    toggleThemeToDark: string;
    login: string;
    logout: string;
    delete: string;
    open: string;
    selected: (count: number) => string;
    clearSelection: string;
  };

  sidebar: {
    brand: string;
    addUrl: string;
    downloads: string;
    library: string;
    browser: string;
    settings: string;
  };

  downloads: {
    title: string;
    summary: (counts: { progressing: number; waiting: number; failed: number; done: number }) => string;
    emptyTitle: string;
    emptyBody: string;
    showDetails: string;
    copyUrl: string;
    removeFromList: string;
    stopCount: (count: number) => string;
    retryCount: (count: number) => string;
    removeCount: (count: number) => string;
    removeFailed: (total: number, failed: number, message: string) => string;
    removedAlongside: (count: number) => string;
  };

  urlbar: {
    placeholder: string;
    qualityLabel: string;
    folderLabel: string;
    folderMenuHeading: string;
    folderMenuOther: string;
    add: string;
  };

  presets: Record<Preset, string>;

  clipboard: {
    message: string;
    add: string;
    dismiss: string;
  };

  tools: {
    missingNotice: string;
    installAction: string;
    sectionTitle: string;
    names: Record<ToolId, string>;
    installed: (version: string) => string;
    updateAvailable: (version: string) => string;
    notInstalled: string;
    downloading: (percent: number | null) => string;
    installing: string;
    installButton: string;
    updateButton: string;
    allInstalled: string;
    installFailed: string;
  };

  queue: {
    queued: string;
    downloading: string;
    probing: string;
    pageSearch: string;
    postProcessing: string;
    canceled: string;
    done: string;
    commentsOnly: (count: number) => string;
    commentsOnlyUnsupported: string;
    skipped: (reason: string) => string;
    /**
     * The parts of a filter skip's reason (see skipReasonText in lib/jobNote.ts, which matches
     * the English texts of crates/core/src/executor.rs and fills these in).
     */
    skipReason: {
      /** Joins several reasons. */
      separator: string;
      /** "10 min": a duration that is a whole number of minutes. */
      minutes: (count: number) => string;
      /** "45 s": any other duration. */
      seconds: (count: number) => string;
      /** `duration` is the result of `minutes` or `seconds`. */
      shorterThan: (duration: string) => string;
      longerThan: (duration: string) => string;
      /** `date` is shown as the core wrote it. */
      publishedBefore: (date: string) => string;
      publishedAfter: (date: string) => string;
      /** The title has none of these words. */
      titleIncludesNone: (words: string[]) => string;
      titleIncludesExcluded: (text: string) => string;
      notVideo: string;
      liveOrUpcoming: string;
      alreadyDownloaded: string;
      matchFilter: string;
    };
    /** A page-image job saved some of its images but not all: the note after its done line. */
    imagesNotSaved: (failed: number, total: number) => string;
    /** The job was skipped because the video is already saved. */
    alreadySaved: string;
    /** "saved Nov 2": when the saved copy was made, in local time; the year only if it is not `now`'s. */
    savedOn: (unixSeconds: number, now: Date) => string;
    /** Only the library remembers the video: its file is gone from where it was saved. */
    fileMissing: string;
    listAdded: (count: number, total: number | null) => string;
    remaining: (clock: string) => string;
  };

  errors: Record<ErrorCategory, string>;

  pageMedia: {
    foundCount: (count: number) => string;
    choose: string;
    title: string;
    unknownLength: string;
    save: string;
  };

  detail: {
    quality: string;
    savedTo: string;
    errorText: string;
    copyText: string;
    copied: string;
  };

  player: {
    play: string;
    pip: string;
    pipExit: string;
    unsupported: string;
    openWith: string;
    missing: string;
  };

  library: {
    title: string;
    searchPlaceholder: string;
    emptyNoneYet: string;
    emptyNoResults: string;
    missingBadge: string;
    /** Label of a saved item's "..." button. */
    moreActions: string;
    open: string;
    openWith: string;
    rename: string;
    extractComments: string;
    extractCommentsCount: (count: number) => string;
    extractingComments: string;
    savedComments: (count: number) => string;
    addCommentsToFile: string;
    commentsConflictTitle: string;
    keepExistingComments: string;
    useNewComments: string;
    moreComments: string;
    commentsOldMeta: (count: number, date: string) => string;
    commentsNewMeta: (count: number) => string;
    commentsApplyRemaining: (count: number) => string;
    commentsDecide: string;
    commentsUrlTitle: string;
    commentsUrlBody: string;
    commentsUrlPlaceholder: string;
    commentsUrlInvalid: string;
    commentsExtract: string;
    /** Label of the rename field. */
    newName: string;
    nameEmpty: string;
    nameBadChars: string;
    /** Menu entry: erase the records, keep the file. */
    forget: string;
    forgetTitle: string;
    forgetBody: (title: string) => string;
    forgetBulkBody: (first: string, count: number) => string;
    forgetCount: (count: number) => string;
    forgetFailed: (total: number, failed: number, message: string) => string;
    forgetConfirm: string;
    /** Wording for moving the file to the Recycle Bin (Windows) or the Trash (macOS). */
    trash: Record<"windows" | "mac", TrashWords>;
    /** Notice: the records are gone but part of the traces could not be cleared. */
    tracesLeft: string;
    /** The row's error line when an action failed. */
    failed: (message: string) => string;
  };

  /**
   * Sources: lists and pages checked for new items, and the ways to register one. The rename
   * field, its short messages and the "..." label reuse the library's (`library.newName`,
   * `library.nameEmpty`, `library.rename`, `library.moreActions`, `library.failed`).
   */
  sources: {
    /** The destination's title and its sidebar label. */
    title: string;
    /** Under the title: the count and the newest last check (a time if today, else a date). */
    summary: (count: number, lastCheckedAt: number | null, now: Date) => string;
    checkAll: string;
    /** The URL field's placeholder and accessible name. */
    urlPlaceholder: string;
    register: string;
    /** The register button (and the other entry points') while the preview runs. */
    previewing: string;
    emptyBody: string;
    /** The line under the field when registering did not work. */
    registerFailed: {
      notAList: string;
      loginRequired: string;
      unsupported: string;
      other: (message: string) => string;
      alreadyRegistered: string;
    };
    /** The row's meta parts. */
    kindList: string;
    kindRule: string;
    destination: (folder: string) => string;
    account: (label: string) => string;
    /** The row's status line. */
    waiting: string;
    checking: string;
    neverChecked: string;
    /** "Nov 4 15:20" in local time; the year only if it is not `now`'s. */
    dateTime: (unixSeconds: number, now: Date) => string;
    checkedNew: (at: string, count: number) => string;
    checkedNone: (at: string) => string;
    checkedFailed: (at: string, reason: string) => string;
    checkNow: string;
    /** Menu entry and title of the destination and quality dialog. */
    options: string;
    optionsFolder: string;
    optionsFolderDefault: (folder: string) => string;
    optionsChangeFolder: string;
    optionsResetFolder: string;
    optionsQuality: string;
    optionsQualityDefault: (label: string) => string;
    optionsSave: string;
    deleteTitle: (name: string) => string;
    deleteBody: string;
    /** The question after a registration: what to do with the items there now. */
    firstCheckTitle: (kind: "list" | "rule", count: number) => string;
    firstCheckBody: (kind: "list" | "rule", name: string) => string;
    recordOnly: string;
    saveAll: string;
    /** The 集める tray's footer. */
    trayRegister: string;
    trayRegistered: string;
    trayNote: string;
    /** A list row in Downloads. */
    rowRegister: string;
    rowRegistered: string;
    /** Settings > General: automatic checks. */
    autoCheck: string;
    autoCheckOff: string;
    autoCheckHourly: string;
    autoCheckSixHours: string;
    autoCheckDaily: string;
    autoCheckNote: string;
  };

  settings: {
    title: string;
    generalSection: string;
    sitesSection: string;
    adBlockSection: string;
    toolsSection: string;
    updatesSection: string;
    language: string;
    languageJa: string;
    languageEn: string;
    theme: string;
    themeSystem: string;
    themeLight: string;
    themeDark: string;
    concurrency: string;
    clipboardBanner: string;
    collectIncludeAds: string;
    downloadFolder: string;
    changeDownloadFolder: string;
    keepSubtitleFiles: string;
    defaultPreset: string;
    avoidAv1: string;
    avoidAv1Hint: string;
  };

  notify: {
    /** The group heading in Settings > General. */
    heading: string;
    /** The master switch: off silences every notification below while keeping their values. */
    enabled: string;
    done: string;
    failed: string;
    found: string;
    note: string;
  };

  /** In-app updates: the dialog (launch check, 更新する) and Settings > アップデート. */
  updates: {
    /** The dialog's title while it offers the new version (also after a failed update). */
    title: string;
    /** The title once the download has started. */
    updatingTitle: string;
    /** "vidsdownloader 0.2.0". */
    newVersion: (version: string) => string;
    /** Right after newVersion, as context: 「（いまの版は 0.1.0）」. */
    runningVersion: (version: string) => string;
    notesHeading: string;
    skip: string;
    later: string;
    updateNow: string;
    /** Under the buttons while jobs download or post-process; `count` of them. */
    busy: (count: number) => string;
    progressLabel: string;
    downloading: string;
    /** Under the bar: what happens once the download is done. */
    afterDownload: string;
    /** The one line in the dialog when downloading or installing failed. */
    failed: (message: string) => string;
    /** Settings > アップデート. */
    currentVersion: string;
    check: string;
    checking: string;
    /** `at` is the time of the check ("11月4日 18:40"). */
    upToDate: (at: string) => string;
    checkedAt: (unixSeconds: number, now: Date) => string;
    found: (version: string) => string;
    install: string;
    skipped: (version: string) => string;
    unskip: string;
    checkFailed: (message: string) => string;
  };

  browser: {
    back: string;
    forward: string;
    reload: string;
    siteList: string;
    address: string;
    addressPlaceholder: string;
    addPage: string;
    pageAdded: string;
    collect: string;
    flowRecord: string;
    flowRecordStop: string;
    flowRecording: (steps: number) => string;
    savedFlows: string;
    savedFlowSteps: (steps: number) => string;
    useSavedFlow: string;
    learnedFlow: string;
    flowStep: (label: string) => string;
    flowScopeThis: string;
    flowScopeRow: (count: number) => string;
    flowScopeAll: (count: number) => string;
    flowReplay: string;
    flowReplaying: string;
    flowProgress: (done: number, total: number) => string;
    flowCapped: string;
    flowResults: (count: number) => string;
    flowNoMedia: string;
    flowName: string;
    flowSave: string;
    flowSaved: string;
    flowStored: string;
    flowNameEmpty: string;
    flowMismatch: (label: string) => string;
    flowRecordAgain: string;
    foundLabel: string;
    all: string;
    selectedOfTotal: (selected: number, total: number) => string;
    scrollProgress: (scrolls: number, maxScrolls: number) => string;
    adsExcluded: (count: number) => string;
    collectStop: string;
    collectResume: string;
    groupOption: (shape: string, count: number) => string;
    /** The group of the page's own images: the group select's option and the grid's heading. */
    imagesGroup: (count: number) => string;
    playedGroup: (count: number) => string;
    reloadFind: string;
    reloadFinding: string;
    reloadFound: (count: number) => string;
    reloadFoundNone: string;
    /** The checkbox label of one page image, by its number on the page. */
    imageLabel: (index: number) => string;
    /** The collapsed toggle of the best-ranked link group under the images. */
    otherLinks: (count: number) => string;
    collectEmpty: string;
    playedEmpty: string;
    addCollected: (count: number) => string;
    collectedAdded: string;
    saveRule: string;
    ruleName: string;
    matchPattern: string;
    save: string;
    ruleSaved: string;
    closeCollect: string;
    xCollectWarning: string;
    collectFailed: (message: string) => string;
    revealItem: string;
    revealGone: string;
    loginPresent: string;
    loginAbsent: string;
    /** Chip text with the account label, shown when a site has several accounts. */
    chipWithAccount: (state: string, account: string) => string;
    loginBanner: (site: string) => string;
    loginCancel: string;
    loginDone: string;
    startTitle: string;
    /** Start page login info for a site; the date is MM/DD or null. */
    siteLoginPresent: (date: string | null) => string;
    addSite: string;
    pagePlaceholder: string;
    invalidUrl: string;
    failed: (message: string) => string;
  };

  sites: {
    lead: string;
    methodLabel: string;
    inApp: string;
    firefox: (profile: string) => string;
    cookiesTxt: string;
    cookiesTxtPick: string;
    changeFile: string;
    statusCookies: (count: number, date: string | null) => string;
    statusNone: string;
    statusFirefox: string;
    statusFile: (name: string, date: string | null) => string;
    addAccount: string;
    accountName: (n: number) => string;
    selectAccount: (label: string) => string;
    addSiteTitle: string;
    addSitePlaceholder: string;
    addSiteButton: string;
    addSiteNote: string;
    failed: (message: string) => string;
    /** Label of a site row's trash button. */
    deleteSite: (name: string) => string;
    deleteSiteMenu: string;
    deleteConfirmTitle: (name: string) => string;
    deleteConfirmBody: (accounts: number) => string;
    flowCount: (count: number) => string;
    viewFlows: string;
    flowsTitle: (site: string) => string;
    flowSteps: (count: number) => string;
    renameFlow: string;
    flowNameLabel: string;
    flowNameEmpty: string;
    deleteFlowTitle: (name: string) => string;
    deleteFlowBody: string;
  };

  /** The box in a newly added site's row: what yt-dlp makes of the URL that was added. */
  siteReport: {
    heading: string;
    probing: string;
    list: (count: number) => string;
    noLoginNeeded: string;
    /** The login line of a list or video that was read while logged in. */
    loginUsed: string;
    videosSupported: string;
    videosUnknown: string;
    registerSource: string;
    registeredNow: string;
    registeredAlready: string;
    openInBrowser: string;
    video: string;
    addToDownloads: string;
    addedToDownloads: string;
    loginRequired: string;
    loginThenRetry: string;
    recheck: string;
    unsupported: string;
    unsupportedHint: string;
    openAndCollect: string;
    /** `reason` is the text of the failure's category. */
    failed: (reason: string) => string;
  };

  /** Importing logins from Firefox: the browser start page's one-time offer and Settings > Sites. */
  importLogins: {
    offerTitle: string;
    offerBody: string;
    /** Accessible name of the profile select. */
    profileLabel: string;
    /** An option of the profile select; also the text shown when Firefox has one profile only. */
    profileOption: (name: string) => string;
    importButton: string;
    importing: string;
    notNow: string;
    close: string;
    /** The result line: sites filled, and sites left alone because they were already signed in. */
    result: (imported: number, alreadySignedIn: number) => string;
    blockTitle: string;
    blockLead: string;
  };

  /** Settings > Ad blocking. */
  adBlock: {
    lead: string;
    /** Label of a site's switch. */
    siteSwitch: (name: string) => string;
    note: string;
  };
}

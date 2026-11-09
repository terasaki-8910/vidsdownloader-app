// In-memory mock backend, used only when the page is not running inside Tauri
// (no window.__TAURI_INTERNALS__ -- see ./index.ts). This lets `pnpm dev` run
// in a plain browser for UI work and screenshots without the Rust shell.
//
// ALL data below (titles, uploaders, dates, paths, tool versions, the clipboard
// URL) is SAMPLE DATA for UI development only, not a real catalogue or a real
// clipboard/filesystem read.
//
// The five queue rows mirror apps/desktop/design/round3-mock.html's sample set
// (real yt-dlp extractor-test videos, real titles/uploaders/dates): sm9
// (downloading), Big Buck Bunny / YE7VzlLtp-4 (post-processing -- the SPEC D1
// fixture video), so44060088 (failed, login required), Bad Apple!! / sm8628149
// (queued), nl1872567 (done, long title).
import type {
  Account,
  BrowserState,
  CollectGroup,
  CollectItem,
  CollectLinks,
  CollectMethod,
  CollectPage,
  CollectState,
  Commands,
  EraseResult,
  Events,
  FirefoxImportResult,
  FirefoxProfilesInfo,
  Flow,
  FlowReplayResult,
  JobDetails,
  JobRow,
  JobState,
  LibraryRow,
  LoginMethod,
  Preset,
  SavedFlow,
  SavedPage,
  Settings,
  Site,
  SiteProbeReport,
  SourcePreview,
  SourcePreviewOutcome,
  SourceRow,
  ToolId,
  ToolStatus,
  UpdateDownloadEvent,
  UpdateInfo,
} from "./contract";
import type { IpcClient, UpdaterClient } from "./types";
import { readDebugHash } from "../lib/debugHash";
import { IMAGES_SHAPE, PLAYED_SHAPE } from "../lib/collect";

/** What the mock's update check finds (#mockUpdate=); also how the seeded queue starts. */
const MOCK_UPDATE = readDebugHash().mockUpdate;

const SAMPLE_DOWNLOAD_DIR = "C:\\Users\\user\\Videos\\vidsdownloader";

function utc(y: number, m: number, d: number, hh = 0, mm = 0): number {
  return Math.floor(Date.UTC(y, m - 1, d, hh, mm) / 1000);
}

/** Unix seconds for a local wall-clock time, like the dates the real Rust side shows. */
function local(y: number, m: number, d: number, hh = 0, mm = 0): number {
  return Math.floor(new Date(y, m - 1, d, hh, mm).getTime() / 1000);
}

function details(d: Partial<JobDetails>): JobDetails {
  return {
    title: null,
    uploader: null,
    site: null,
    publishedAt: null,
    durationS: null,
    thumbnailUrl: null,
    ...d,
  };
}

// ---------------------------------------------------------------------------
// Tiny typed pub-sub, standing in for Tauri's event system.
// ---------------------------------------------------------------------------

const listeners: { [K in keyof Events]: Set<(payload: Events[K]) => void> } = {
  "job-updated": new Set(),
  "tools-progress": new Set(),
  "browser-state": new Set(),
  "collect-state": new Set(),
  "collect-toggles": new Set(),
  "collect-pick": new Set(),
  "flow-record-progress": new Set(),
  "flow-progress": new Set(),
  "sites-updated": new Set(),
  "job-removed": new Set(),
  "library-updated": new Set(),
  "source-updated": new Set(),
  "source-removed": new Set(),
};

function emit<K extends keyof Events>(event: K, payload: Events[K]): void {
  for (const fn of listeners[event]) fn(payload);
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

let settings: Settings = {
  language: "ja",
  theme: "system",
  concurrency: 2,
  clipboardBanner: true,
  collectIncludeAds: false,
  keepSubtitleFiles: false,
  avoidAv1: false,
  downloadDir: SAMPLE_DOWNLOAD_DIR,
  recentDownloadDirs: [
    SAMPLE_DOWNLOAD_DIR,
    "D:\\Videos\\anime",
    "D:\\Videos\\music",
    "C:\\Users\\user\\OneDrive\\動画",
    "D:\\Videos\\talks",
  ],
  defaultPreset: "best",
  importOfferDismissed: false,
  sourceAutoCheck: "off",
  notify: { enabled: true, done: true, failed: true, found: true },
  skippedUpdateVersion: MOCK_UPDATE === "skipped" ? "0.2.0" : null,
};

// ---------------------------------------------------------------------------
// Jobs / queue
// ---------------------------------------------------------------------------

const jobs = new Map<number, JobRow>();
let nextJobId = 17;
export let lastCopiedText: string | null = null;
/** The last URL "open in the default browser" was asked for (nothing is opened in the mock). */
export let lastExternalUrl: string | null = null;
/** tzOffsetSeconds passed with enqueue_urls, by job id; used to date the output
 *  path in local time like the real Rust side does. Seed rows do not go through
 *  enqueue_urls, so they fall back to UTC. */
const tzOffsetByJobId = new Map<number, number>();
const presetByJobId = new Map<number, Preset>();
const destDirByJobId = new Map<number, string>();

function seedJob(
  row: Omit<JobRow, "listCount" | "listTotal" | "alreadySaved" | "libraryId" | "phase" | "commentsCount" | "pageMediaChoices"> &
    Partial<Pick<JobRow, "listCount" | "listTotal" | "alreadySaved" | "libraryId" | "pageMediaChoices">>,
): void {
  jobs.set(row.id, { listCount: null, listTotal: null, alreadySaved: null, libraryId: null, phase: null, commentsCount: null, pageMediaChoices: [], ...row });
}

seedJob({
  id: 1,
  url: "https://www.nicovideo.jp/watch/sm9",
  state: "Downloading",
  attempt: 1,
  progress: 0.64,
  speedBytesPerS: 4.8 * 1024 * 1024,
  etaS: 21,
  error: null,
  details: details({
    title: "新・豪血寺一族 -煩悩解放 - レッツゴー！陰陽師",
    uploader: "中の",
    site: "ニコニコ動画",
    publishedAt: utc(2007, 3, 6, 0, 33),
    durationS: 320,
  }),
  outputPath: null,
  skipped: null,
});

seedJob({
  id: 7,
  url: "https://www.nicovideo.jp/watch/sm1715919",
  state: "Downloading",
  attempt: 1,
  progress: null,
  speedBytesPerS: 1.6 * 1024 * 1024,
  etaS: null,
  error: null,
  details: details({
    title: "【初音ミク】みくみくにしてあげる\u266a【してやんよ】",
    uploader: "ika",
    site: "ニコニコ動画",
    publishedAt: utc(2007, 9, 20, 0, 0),
    durationS: 239,
  }),
  outputPath: null,
  skipped: null,
});

seedJob({
  id: 6,
  url: "https://www.youtube.com/playlist?list=PL-example",
  state: "Done",
  attempt: 1,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: null,
  outputPath: null,
  skipped: null,
  listCount: 300,
  listTotal: 347,
});

seedJob({
  id: 2,
  url: "https://www.youtube.com/watch?v=YE7VzlLtp-4",
  state: "PostProcessing",
  attempt: 1,
  progress: 1,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: details({
    title: "Big Buck Bunny",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2008, 5, 29, 12, 0),
    durationS: 597,
  }),
  outputPath: null,
  skipped: null,
});

seedJob({
  id: 3,
  url: "https://www.nicovideo.jp/watch/so44060088",
  state: "Failed",
  attempt: 1,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: {
    category: "LoginRequired",
    message:
      "ERROR: [niconico] so44060088: This video is only available to channel members. " +
      "Join the channel or sign in with an account that has already joined.",
  },
  details: details({
    title: "松田的超英雄電波。《仮面ライダーガッチャード 放送終了記念特別番組》",
    uploader: "あみあみチャンネル",
    site: "ニコニコ動画",
    publishedAt: utc(2024, 9, 3, 19, 0),
    durationS: 1881,
  }),
  outputPath: null,
  skipped: null,
});

seedJob({
  id: 4,
  url: "https://www.nicovideo.jp/watch/sm8628149",
  state: "Queued",
  attempt: 0,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: details({
    title: "【東方】Bad Apple!!　ＰＶ【影絵】",
    uploader: "あにら",
    site: "ニコニコ動画",
    publishedAt: utc(2009, 10, 27, 21, 0),
    durationS: 219,
  }),
  outputPath: null,
  skipped: null,
});

seedJob({
  id: 5,
  url: "https://www.nicovideo.jp/watch/nl1872567",
  state: "Done",
  attempt: 1,
  progress: 1,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: details({
    title: "【12/25放送分】『生対談!!ひろゆきと戀塚のニコニコを作った人 』前半",
    uploader: "nicolive",
    site: "ニコニコ動画",
    publishedAt: utc(2007, 12, 26, 0, 5),
    durationS: 586,
  }),
  outputPath:
    SAMPLE_DOWNLOAD_DIR +
    "\\nicolive\\2007-12-26 【12⧸25放送分】『生対談!!ひろゆきと戀塚のニコニコを作った人 』前半 [nl1872567].mp4",
  skipped: null,
  libraryId: 1,
});

// Skipped because the video is already saved: one found in the library, one whose file
// has gone from where the library recorded it.
seedJob({
  id: 8,
  url: "https://www.youtube.com/watch?v=eRsGyueVLvQ",
  state: "Done",
  attempt: 1,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: details({
    title: "Sintel",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2010, 9, 30, 10, 0),
    durationS: 888,
  }),
  outputPath: null,
  skipped: "already saved",
  alreadySaved: {
    savedAt: local(2026, 11, 2, 21, 14),
    fileMissing: false,
    path: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2010-09-30 Sintel [eRsGyueVLvQ].mp4",
  },
  libraryId: null,
});

seedJob({
  id: 9,
  url: "https://www.youtube.com/watch?v=R6MlUcmOul8",
  state: "Done",
  attempt: 1,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: details({
    title: "Tears of Steel",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2013, 5, 15, 10, 0),
    durationS: 734,
  }),
  outputPath: null,
  skipped: "already saved",
  alreadySaved: {
    savedAt: local(2026, 10, 20, 18, 2),
    fileMissing: true,
    path: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2013-05-15 Tears of Steel [R6MlUcmOul8].mp4",
  },
  libraryId: null,
});

// A page-image job that saved four of its six images: Done with a file and a note, as the
// Rust executor reports it (crates/core/src/executor.rs).
seedJob({
  id: 10,
  url: "https://blog.example.com/articles/rainy-day-cafe",
  state: "Done",
  attempt: 1,
  progress: 1,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: details({
    title: "雨の日の喫茶店と文庫本",
    uploader: "ひなた",
    site: "blog.example.com",
    publishedAt: utc(2026, 10, 20, 1, 15),
    thumbnailUrl: "/mock-assets/page-3.jpg",
  }),
  outputPath: SAMPLE_DOWNLOAD_DIR + "\\ひなた\\2026-10-20 雨の日の喫茶店と文庫本 [rainy-day-cafe] 1.jpg",
  skipped: "2 of 6 images could not be saved",
  libraryId: 7,
});

// Skipped by a filter: Done without a file, with the core's English reason text (the UI shows
// it in the UI language, lib/jobNote.ts). The second one has two reasons joined with "; ".
seedJob({
  id: 11,
  url: "https://www.nicovideo.jp/user/1234567/video",
  state: "Done",
  attempt: 1,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: details({
    title: "【お知らせ】チャンネル更新のご案内",
    uploader: "中の",
    site: "ニコニコ動画",
    publishedAt: utc(2026, 10, 28, 9, 0),
  }),
  outputPath: null,
  skipped: "not a video",
});

seedJob({
  id: 12,
  url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
  state: "Done",
  attempt: 1,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: details({
    title: "Big Buck Bunny (extended cut) [ad]",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2008, 5, 30, 12, 0),
    durationS: 1380,
  }),
  outputPath: null,
  skipped: "longer than 600 s; title includes excluded text: ad",
});

seedJob({
  id: 13,
  url: "https://www.youtube.com/watch?v=Man_CHWykwc",
  state: "Done",
  attempt: 1,
  progress: 1,
  speedBytesPerS: null,
  etaS: null,
  error: null,
  details: details({
    title: "Cosmos Laundromat",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2015, 10, 9, 10, 0),
    durationS: 723,
  }),
  outputPath: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2015-10-09 Cosmos Laundromat [Man_CHWykwc].mp4",
  skipped: null,
  libraryId: 4,
});

seedJob({
  id: 14,
  url: "https://example.com/player-page/two-episodes",
  state: "Failed",
  attempt: 1,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: { category: "PageMediaChoice", message: "several videos were found" },
  details: details({ title: "Two episodes", site: "example.com" }),
  outputPath: null,
  skipped: null,
  pageMediaChoices: [
    { url: "https://cdn.example.com/ep01_1080.mp4", kind: "MP4", durationSeconds: 23 * 60 + 40 },
    { url: "https://cdn.example.com/ep02_1080.mp4", kind: "MP4", durationSeconds: 23 * 60 + 40 },
  ],
});

seedJob({
  id: 15,
  url: "https://video.example.com/age-check?return=%2Fwatch%2Ffeatured",
  state: "Failed",
  attempt: 1,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: { category: "PageGate", message: "stopped at a confirmation page" },
  details: details({ title: "Featured video", site: "video.example.com" }),
  outputPath: null,
  skipped: null,
});

seedJob({
  id: 16,
  url: "https://video.example.com/watch/live-archive-0412",
  state: "Failed",
  attempt: 1,
  progress: null,
  speedBytesPerS: null,
  etaS: null,
  error: {
    category: "Unknown",
    message:
      "fragment 3 not found, unable to continue\n" +
      "WARNING: [download] Got error: HTTP Error 403: Forbidden. Retrying fragment 3 (1/10)...\n" +
      "WARNING: [download] Got error: HTTP Error 403: Forbidden. Retrying fragment 3 (10/10)...",
  },
  details: details({ title: "配信アーカイブ 4月12日", site: "video.example.com" }),
  outputPath: null,
  skipped: null,
});

function update(id: number, patch: Partial<JobRow>): void {
  const current = jobs.get(id);
  if (!current) return;
  const next = { ...current, ...patch };
  jobs.set(id, next);
  emit("job-updated", next);
}

const ACTIVE_STATES: JobState[] = ["Queued", "Probing", "Ready", "Downloading", "PostProcessing"];

function activeCount(): number {
  let n = 0;
  for (const job of jobs.values()) if (ACTIVE_STATES.includes(job.state)) n++;
  return n;
}

/** Starts (or continues) a job's simulated lifecycle, advancing over time. */
async function runLifecycle(id: number, opts: { fromProgress?: number; skipProbe?: boolean } = {}): Promise<void> {
  const job = jobs.get(id);
  if (!job) return;

  if (!opts.skipProbe && job.state === "Queued") {
    const pageSearch = job.url.includes("player-page") || job.url.includes("no-video-page");
    update(id, { state: "Probing", phase: pageSearch ? "pageSearch" : null });
    await delay(pageSearch ? 3000 : 700);
    if (!jobs.has(id)) return;
    if (job.url.includes("no-video-page")) {
      update(id, {
        state: "Failed",
        phase: null,
        error: { category: "PageMediaNotFound", message: "no video was found on the page" },
      });
      maybeStartNext();
      return;
    }
  }

  let progress = opts.fromProgress ?? 0;
  if (opts.fromProgress == null) {
    // Starting fresh: no speed/eta sample yet. Resuming a seeded job keeps
    // its existing (already plausible) speed/eta until the first real tick.
    update(id, { state: "Downloading", phase: null, progress, speedBytesPerS: 3.2 * 1024 * 1024, etaS: null });
  }

  while (progress < 1) {
    await delay(500);
    const current = jobs.get(id);
    if (!current || current.state !== "Downloading") return; // canceled meanwhile
    progress = Math.min(1, progress + 0.06 + Math.random() * 0.05);
    const speed = (3 + Math.random() * 2.5) * 1024 * 1024;
    const remainingFraction = 1 - progress;
    const etaS = progress >= 1 ? 0 : Math.max(1, Math.round((remainingFraction / 0.08) * 0.5));
    update(id, { progress, speedBytesPerS: speed, etaS });
  }

  const current = jobs.get(id);
  if (!current || current.state !== "Downloading") return;
  update(id, { state: "PostProcessing", speedBytesPerS: null, etaS: null });
  await delay(1100);
  if (!jobs.get(id) || jobs.get(id)?.state !== "PostProcessing") return;
  finishDownload(id);
}

function finishDownload(id: number): void {
  const job = jobs.get(id);
  if (!job) return;
  const d = job.details;
  const title = (d?.title ?? "video").replace(/\//g, "⧸");
  const tzOffsetSeconds = tzOffsetByJobId.get(id) ?? 0;
  const dateStr = d?.publishedAt
    ? new Date((d.publishedAt + tzOffsetSeconds) * 1000).toISOString().slice(0, 10)
    : "0000-00-00";
  const folder = d?.uploader ?? "misc";
  const idMatch = /\/watch\/([a-z0-9]+)/i.exec(job.url) ?? /v=([\w-]+)/.exec(job.url);
  const videoId = idMatch?.[1] ?? String(id);
  const commentsOnly = presetByJobId.get(id) === "commentsOnly";
  const outputPath = `${destDirByJobId.get(id) ?? settings.downloadDir}\\${folder}\\${dateStr} ${title} [${videoId}]${commentsOnly ? ".comments.json" : ".mp4"}`;
  update(id, { state: "Done", progress: 1, outputPath, commentsCount: commentsOnly ? 1250 : null, libraryId: commentsOnly ? null : id === 2 ? 9 : job.libraryId });
  maybeStartNext();
}

function maybeStartNext(): void {
  if (activeCount() >= settings.concurrency) return;
  for (const job of jobs.values()) {
    if (job.state === "Queued") {
      void runLifecycle(job.id);
      return;
    }
  }
}

/**
 * A page-image job: the page's chosen images are saved directly (no yt-dlp), one library row
 * per file; the job's path is the first file. Always saves every image in the mock.
 */
async function runPageImagesJob(id: number, args: Commands["enqueue_page_images"]["args"]): Promise<void> {
  const { pageUrl, title, uploader, publishedAt, id: pageId, images, tzOffsetSeconds } = args;
  const host = new URL(pageUrl).hostname.replace(/^www\./, "");
  const siteId = siteIdForUrl(pageUrl);
  const siteName = sites.find((s) => s.id === siteId)?.name ?? host;
  update(id, {
    state: "Probing",
    details: details({
      title,
      uploader,
      site: siteName,
      publishedAt,
      thumbnailUrl: images[0]?.url ?? null,
    }),
  });
  await delay(600);
  if (jobs.get(id)?.state !== "Probing") return;
  update(id, { state: "Downloading", progress: 0 });
  for (let done = 1; done <= images.length; done++) {
    await delay(450);
    if (jobs.get(id)?.state !== "Downloading") return;
    update(id, { progress: done / images.length });
  }

  const dateStr = publishedAt
    ? new Date((publishedAt + tzOffsetSeconds) * 1000).toISOString().slice(0, 10)
    : "0000-00-00";
  const stem =
    `${dateStr} ${(title ?? host).replace(/\//g, "⧸")}` + (pageId ? ` [${pageId}]` : "");
  const folder = uploader ?? host;
  const baseDir = destDirByJobId.get(id) ?? settings.downloadDir;
  const paths = images.map((image) => `${baseDir}\\${folder}\\${stem} ${image.index}.jpg`);
  let nextLibraryId = library.reduce((max, row) => Math.max(max, row.id), 0) + 1;
  const rows: LibraryRow[] = paths.map((path) => ({
    id: nextLibraryId++,
    title,
    uploader,
    site: siteName,
    publishedAt,
    durationS: null,
    path,
    commentsPath: null,
    missing: false,
    addedAt: nowSeconds(),
  }));
  library.unshift(...rows);
  libraryIdsByJob.set(id, rows.map((row) => row.id));
  emit("library-updated", null);
  update(id, { state: "Done", progress: 1, outputPath: paths[0] ?? null, skipped: null, libraryId: rows[0]?.id ?? null });
}

if (MOCK_UPDATE === "busy" || MOCK_UPDATE === "draining") {
  // Exactly two downloads run (jobs 1 and 7) and the queued job 4 waits; none of them moves on.
  // "draining" finishes them after about 6 s, so the queue goes idle.
  finishDownload(2);
  if (MOCK_UPDATE === "draining") {
    void (async () => {
      await delay(6000);
      for (const id of [4, 1, 7]) finishDownload(id);
    })();
  }
} else if (MOCK_UPDATE) {
  // An idle queue, so 今すぐ更新 is allowed: every seeded job is finished (the order matters:
  // finishing a job may start the queued one).
  for (const id of [4, 2, 1, 7]) finishDownload(id);
} else {
  // Continue the two seed jobs that start mid-flight.
  void runLifecycle(1, { fromProgress: 0.64, skipProbe: true });
  void (async () => {
    await delay(1400);
    if (jobs.get(2)?.state === "PostProcessing") finishDownload(2);
  })();
}

// ---------------------------------------------------------------------------
// Library (previously saved videos)
// ---------------------------------------------------------------------------

const daysAgo = (n: number) => Math.floor(Date.now() / 1000) - n * 86400;

const library: LibraryRow[] = [
  {
    id: 1,
    title: "【12/25放送分】『生対談!!ひろゆきと戀塚のニコニコを作った人 』前半",
    uploader: "nicolive",
    site: "ニコニコ動画",
    publishedAt: utc(2007, 12, 26, 0, 5),
    durationS: 586,
    path:
      SAMPLE_DOWNLOAD_DIR +
      "\\nicolive\\2007-12-26 【12⧸25放送分】『生対談!!ひろゆきと戀塚のニコニコを作った人 』前半 [nl1872567].mp4",
    commentsPath: null,
    missing: false,
    addedAt: daysAgo(1),
  },
  {
    id: 10,
    title: "【初音ミク】みくみくにしてあげる\u266a【してやんよ】",
    uploader: "ika",
    site: "ニコニコ動画",
    publishedAt: utc(2007, 9, 20, 0, 0),
    durationS: 239,
    path: SAMPLE_DOWNLOAD_DIR + "\\ika\\2007-09-20 みくみくにしてあげる [sm1715919].mp4",
    commentsPath: null,
    missing: false,
    addedAt: daysAgo(4),
  },
  {
    id: 2,
    title: "Sintel",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2010, 9, 30, 10, 0),
    durationS: 888,
    path: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2010-09-30 Sintel [eRsGyueVLvQ].mp4",
    commentsPath: null,
    missing: false,
    addedAt: daysAgo(5),
  },
  {
    id: 3,
    title: "Tears of Steel",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2012, 9, 26, 10, 0),
    durationS: 734,
    path: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2012-09-26 Tears of Steel [R6MlUcmOul8].mp4",
    commentsPath: null,
    missing: true,
    addedAt: daysAgo(30),
  },
  {
    id: 4,
    title: "Cosmos Laundromat",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2015, 10, 9, 10, 0),
    durationS: 723,
    path: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2015-10-09 Cosmos Laundromat [Man_CHWykwc].mp4",
    commentsPath: null,
    missing: false,
    addedAt: daysAgo(12),
  },
  {
    id: 5,
    title: "Agent 327: Operation Barbershop",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2017, 5, 17, 10, 0),
    durationS: 162,
    path: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2017-05-17 Agent 327 Operation Barbershop [5PWf4PHxTfI].mp4",
    commentsPath: null,
    missing: false,
    addedAt: daysAgo(40),
  },
  {
    // A title long enough to wrap to two lines in the library at 1280px: the row's buttons
    // stay at its top-right edge.
    id: 6,
    title:
      "【公式】Spring - Blender Open Movie: 羊飼いの少女と愛犬が、季節の巡りを守るために古い精霊たちと向き合う短編アニメーション（監督コメンタリー・メイキング付き完全版）",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2019, 4, 4, 10, 0),
    durationS: 464,
    path: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2019-04-04 Spring Blender Open Movie [WhWc3b3KhnY].mp4",
    commentsPath: null,
    missing: false,
    addedAt: daysAgo(60),
  },
  {
    id: 7,
    title: "雨の日の喫茶店と文庫本",
    uploader: "ひなた",
    site: "blog.example.com",
    publishedAt: utc(2026, 10, 20, 1, 15),
    durationS: null,
    path: SAMPLE_DOWNLOAD_DIR + "\\ひなた\\2026-10-20 雨の日の喫茶店と文庫本 [rainy-day-cafe] 1.jpg",
    commentsPath: null,
    missing: false,
    addedAt: daysAgo(2),
  },
  {
    id: 8,
    title: "Elephants Dream",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2006, 5, 18, 10, 0),
    durationS: 653,
    path: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2006-05-18 Elephants Dream [TLkA0RELQ1g].mp4",
    commentsPath: null,
    missing: false,
    addedAt: daysAgo(90),
  },
  {
    id: 9,
    title: "Big Buck Bunny",
    uploader: "Blender",
    site: "YouTube",
    publishedAt: utc(2008, 5, 29, 12, 0),
    durationS: 597,
    path: SAMPLE_DOWNLOAD_DIR + "\\Blender\\2008-05-29 Big Buck Bunny [YE7VzlLtp-4].mp4",
    commentsPath: null,
    missing: false,
    addedAt: daysAgo(3),
  },
];

// ---------------------------------------------------------------------------
// Tools (yt-dlp / ffmpeg / deno)
// ---------------------------------------------------------------------------

const TOOL_SIZES: Record<ToolId, number> = {
  "yt-dlp": 18_500_000,
  ffmpeg: 92_000_000,
  deno: 46_000_000,
  "gallery-dl": 12_000_000,
  "ublock-lite": 8_962_846,
};
const TOOL_EXPECTED_VERSIONS: Record<ToolId, string> = {
  "yt-dlp": "2026.10.26",
  ffmpeg: "2026-10-31",
  deno: "2.9.8",
  "gallery-dl": "2026.11.03",
  "ublock-lite": "2026.1026.1812",
};

const toolVersions: Record<ToolId, string | null> = {
  "yt-dlp": null,
  ffmpeg: null,
  deno: null,
  "gallery-dl": null,
  "ublock-lite": null,
};

const MOCK_TOOL_IDS = (Object.keys(toolVersions) as ToolId[]).filter(
  (tool) => tool !== "ublock-lite" || navigator.userAgent.includes("Windows"),
);

// The update samples: an app that is up to date has its tools, so no "tools missing" notice
// sits behind the update dialog.
if (MOCK_UPDATE) for (const tool of MOCK_TOOL_IDS) toolVersions[tool] = TOOL_EXPECTED_VERSIONS[tool];

function toolsStatus(): ToolStatus[] {
  return MOCK_TOOL_IDS.map((tool) => ({
    tool,
    installed: toolVersions[tool] !== null,
    version: toolVersions[tool],
    expectedVersion: TOOL_EXPECTED_VERSIONS[tool],
  }));
}

async function installOneTool(tool: ToolId): Promise<void> {
  const total = TOOL_SIZES[tool];
  const steps = 6;
  for (let i = 1; i <= steps; i++) {
    await delay(220);
    emit("tools-progress", {
      tool,
      phase: "downloading",
      downloadedBytes: Math.round((total * i) / steps),
      totalBytes: total,
      message: null,
    });
  }
  emit("tools-progress", {
    tool,
    phase: "installing",
    downloadedBytes: total,
    totalBytes: total,
    message: null,
  });
  await delay(300);
  toolVersions[tool] = TOOL_EXPECTED_VERSIONS[tool];
  emit("tools-progress", {
    tool,
    phase: "done",
    downloadedBytes: total,
    totalBytes: total,
    message: null,
  });
}

// ---------------------------------------------------------------------------
// Sites, accounts and the in-app browser
//
// There is no native webview in a plain browser: browser_open only changes the
// state below (and emits it), and the page area shows a placeholder. The five
// built-in sites mirror the Rust seed (YouTube, ニコニコ動画, X, Twitch,
// Bilibili) and are followed by two the user added; YouTube starts logged in so
// both states are visible.
// ---------------------------------------------------------------------------

const nowSeconds = () => Math.floor(Date.now() / 1000);

let nextSiteId = 1;
let nextAccountId = 1;

function newAccount(label: string, patch: Partial<Account> = {}): Account {
  return { id: nextAccountId++, label, method: { kind: "inApp" }, cookieCount: null, updatedAt: null, ...patch };
}

function newSite(
  name: string,
  domains: string[],
  loginUrl: string,
  builtin: boolean,
  patch: Partial<Account> = {},
): Site {
  const account = newAccount("メイン", patch);
  return {
    id: nextSiteId++,
    name,
    domains,
    loginUrl,
    builtin,
    adBlock: true,
    accounts: [account],
    selectedAccountId: account.id,
  };
}

const sites: Site[] = [
  newSite(
    "YouTube",
    ["youtube.com"],
    "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2F",
    true,
    { cookieCount: 22, updatedAt: nowSeconds() },
  ),
  newSite("ニコニコ動画", ["nicovideo.jp"], "https://account.nicovideo.jp/login", true),
  newSite("X", ["x.com", "twitter.com"], "https://x.com/i/flow/login", true),
  newSite("Twitch", ["twitch.tv"], "https://www.twitch.tv/login", true),
  newSite("Bilibili", ["bilibili.com"], "https://passport.bilibili.com/login", true),
  // Two sites the user added themselves (not built in, so their rows show the trash button):
  // note has no login yet, Vimeo is logged in.
  newSite("note", ["note.com"], "https://note.com/login", false),
  newSite("Vimeo", ["vimeo.com"], "https://vimeo.com/log_in", false, {
    cookieCount: 12,
    updatedAt: nowSeconds() - 86400,
  }),
];

const IDLE_BROWSER: BrowserState = { url: null, loading: false, mode: "browse", siteId: null, accountId: null };
let browserState: BrowserState = { ...IDLE_BROWSER };

const MOCK_FLOW_URL = "http://127.0.0.1:18766/";
const MOCK_FLOW: Flow = {
  startUrl: MOCK_FLOW_URL,
  steps: [
    {
      pattern: "section div.row div.card",
      label: "動画 2",
      url: MOCK_FLOW_URL,
      matchCount: 10,
      rowIndex: 1,
      rowCount: 5,
      pageIndex: 1,
    },
    {
      pattern: "dialog div.modal button.play",
      label: "再生",
      url: MOCK_FLOW_URL,
      matchCount: 1,
      rowIndex: 0,
      rowCount: 1,
      pageIndex: 0,
    },
  ],
  media: "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8?card=2",
};

const NICONICO_RANKING_URL = "https://www.nicovideo.jp/ranking";
const NICONICO_MYLIST_URL = "https://www.nicovideo.jp/user/1234567/mylist/7654321";

function mockSteps(url: string, count: number): Flow["steps"] {
  const steps = structuredClone(MOCK_FLOW.steps);
  while (steps.length < count) {
    steps.push({ ...steps[steps.length - 1], pattern: `main [data-mock-step='${steps.length + 1}']` });
  }
  return steps.slice(0, count).map((step) => ({ ...step, url }));
}

// Saved pages (the toolbar's star, the start page's list): empty, or the three sample pages
// with #mockSaved=1 (see lib/debugHash.ts).
const SAVED_PAGE_URL_MAX = 2048;
const SAVED_PAGE_NAME_MAX = 200;
let savedPages: SavedPage[] = readDebugHash().mockSaved
  ? [
      { name: "猫 · メディア", url: "https://x.com/search?q=%E7%8C%AB&src=typed_query&f=media" },
      { name: "Blender 新しい順", url: "https://www.youtube.com/@BlenderFoundation/videos?view=0&sort=dd" },
      { name: "ランキング 毎時", url: "https://www.nicovideo.jp/ranking/genre/all?term=hour" },
    ].map((page, index) => ({ id: index + 1, ...page, createdAt: nowSeconds() - (3 - index) * 3600 }))
  : [];
let nextSavedPageId = savedPages.length + 1;

function savedPageName(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) throw new Error("the name is empty");
  if ([...trimmed].length > SAVED_PAGE_NAME_MAX) {
    throw new Error(`the name is longer than ${SAVED_PAGE_NAME_MAX} characters`);
  }
  return trimmed;
}

const collectMethodsEmpty = readDebugHash().mockCollectMethods === "empty";
let nextFlowId = 4;
let savedFlows: SavedFlow[] = collectMethodsEmpty
  ? []
  : [
      {
        id: 1,
        siteId: null,
        host: "127.0.0.1",
        name: "再生（127.0.0.1）",
        startUrl: MOCK_FLOW.startUrl,
        steps: structuredClone(MOCK_FLOW.steps),
        scopes: ["restOfRow", "this"],
        createdAt: nowSeconds() - 6 * 86400,
        updatedAt: nowSeconds() - 6 * 86400,
      },
      {
        id: 2,
        siteId: 2,
        host: "www.nicovideo.jp",
        name: "ランキングの動画を再生",
        startUrl: NICONICO_RANKING_URL,
        steps: mockSteps(NICONICO_RANKING_URL, 2),
        scopes: ["restOfRow"],
        createdAt: nowSeconds() - 86400,
        updatedAt: nowSeconds() - 86400,
      },
      {
        id: 3,
        siteId: 2,
        host: "www.nicovideo.jp",
        name: "週刊ゲーム実況まとめ — 再生リストのページからいちばん新しい回だけを開いて再生する",
        startUrl: NICONICO_MYLIST_URL,
        steps: mockSteps(NICONICO_MYLIST_URL, 3),
        scopes: ["this"],
        createdAt: nowSeconds() - 5 * 86400,
        updatedAt: nowSeconds() - 5 * 86400,
      },
    ];

let collectMethods: CollectMethod[] = collectMethodsEmpty
  ? []
  : [
      {
        kind: "rule",
        id: 1,
        name: "YouTube ホーム",
        siteId: 1,
        siteName: "YouTube",
        host: "www.youtube.com",
        pageUrl: "https://www.youtube.com/",
        shape: "/watch?v={id}",
        stepCount: null,
        scopes: null,
        destDir: null,
        preset: null,
        lastUsedAt: nowSeconds() - 2 * 86400,
      },
      {
        kind: "rule",
        id: 2,
        name: "NASA のメディア",
        siteId: 3,
        siteName: "X",
        host: "x.com",
        pageUrl: "https://x.com/NASA/media",
        shape: "/NASA/status/{num}",
        stepCount: null,
        scopes: null,
        destDir: "D:\\Videos\\X\\NASA",
        preset: "upTo1080",
        lastUsedAt: nowSeconds() - 3 * 3600,
      },
      {
        kind: "rule",
        id: 3,
        name: "猫 の検索（メディア）",
        siteId: 3,
        siteName: "X",
        host: "x.com",
        pageUrl: "https://x.com/search",
        shape: "/{any}/status/{num}",
        stepCount: null,
        scopes: null,
        destDir: null,
        preset: null,
        lastUsedAt: null,
      },
      {
        kind: "flow",
        id: 1,
        name: "再生（127.0.0.1）",
        siteId: null,
        siteName: "127.0.0.1",
        host: "127.0.0.1",
        pageUrl: MOCK_FLOW_URL,
        shape: null,
        stepCount: 2,
        scopes: ["restOfRow", "this"],
        destDir: null,
        preset: null,
        lastUsedAt: null,
      },
      {
        kind: "flow",
        id: 2,
        name: "ランキングの動画を再生",
        siteId: 2,
        siteName: "ニコニコ動画",
        host: "www.nicovideo.jp",
        pageUrl: NICONICO_RANKING_URL,
        shape: null,
        stepCount: 2,
        scopes: ["restOfRow"],
        destDir: "D:\\Videos\\niconico\\ランキング",
        preset: null,
        lastUsedAt: nowSeconds() - 86400,
      },
      {
        kind: "flow",
        id: 3,
        name: "週刊ゲーム実況まとめ — 再生リストのページからいちばん新しい回だけを開いて再生する",
        siteId: 2,
        siteName: "ニコニコ動画",
        host: "www.nicovideo.jp",
        pageUrl: NICONICO_MYLIST_URL,
        shape: null,
        stepCount: 3,
        scopes: ["this"],
        destDir: null,
        preset: null,
        lastUsedAt: nowSeconds() - 5 * 86400,
      },
    ];


const SAMPLE_COLLECT_ITEMS: CollectItem[] = [
  {
    url: "https://www.youtube.com/watch?v=YE7VzlLtp-4",
    title: "Big Buck Bunny",
    thumbnail: "https://i.ytimg.com/vi/YE7VzlLtp-4/hqdefault.jpg",
    index: null,
  },
  {
    url: "https://www.youtube.com/watch?v=eRsGyueVLvQ",
    title: "Sintel — a deliberately long title that wraps to verify the tray row layout",
    thumbnail: "https://i.ytimg.com/vi/eRsGyueVLvQ/hqdefault.jpg",
    index: null,
  },
  {
    url: "https://www.youtube.com/watch?v=R6MlUcmOul8",
    title: "Tears of Steel",
    thumbnail: null,
    index: null,
  },
  {
    url: "https://www.youtube.com/watch?v=5PWf4PHxTfI",
    title: "Agent 327: Operation Barbershop",
    thumbnail: null,
    index: null,
  },
  {
    url: "https://www.youtube.com/watch?v=WhWc3b3KhnY",
    title: "Spring",
    thumbnail: null,
    index: null,
  },
  {
    url: "https://www.youtube.com/watch?v=TLkA0RELQ1g",
    title: "Elephants Dream",
    thumbnail: null,
    index: null,
  },
  {
    url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
    title: "Big Buck Bunny 60fps 4K",
    thumbnail: null,
    index: null,
  },
  {
    url: "https://www.youtube.com/watch?v=Man_CHWykwc",
    title: "Cosmos Laundromat",
    thumbnail: null,
    index: null,
  },
];

/** What a mock page's 集める finds; each page URL has its own (see mockCollectFor). */
interface MockCollectPage {
  groups: CollectGroup[];
  /** The group chosen when none is asked for. */
  defaultShape: string;
  itemsFor: (shape: string) => CollectItem[];
  adsExcluded: number;
  page: CollectPage;
}

const VIDEOS_COLLECT: MockCollectPage = {
  groups: [
    { shape: "/watch?v={id}", count: SAMPLE_COLLECT_ITEMS.length },
    { shape: "/shorts/{id}", count: 3 },
  ],
  defaultShape: "/watch?v={id}",
  itemsFor: (shape) => (shape === "/shorts/{id}" ? SAMPLE_COLLECT_ITEMS.slice(0, 3) : SAMPLE_COLLECT_ITEMS),
  adsExcluded: 2,
  page: { title: "Blender Foundation - YouTube", uploader: "Blender Foundation", publishedAt: null, id: null },
};

// An article page whose images are not links: 集める finds its six images first and, ranked
// below them, eleven hashtag links. The six frames are files under mock-assets/ (served by the
// dev server from the project root, kept out of public/ so they never ship in the app).
const IMAGES_PAGE_URL = "https://blog.example.com/articles/bbb-making-of";
const IMAGE_ITEMS: CollectItem[] = [1, 2, 3, 4, 5, 6].map((index) => ({
  url: `/mock-assets/page-${index}.jpg`,
  title: "",
  thumbnail: `/mock-assets/page-${index}.jpg`,
  index,
}));
const HASHTAG_WORDS = ["猫", "写真", "日常", "旅行", "カメラ", "京都", "散歩", "風景", "秋", "夕焼け", "note"];
const HASHTAG_ITEMS: CollectItem[] = HASHTAG_WORDS.map((word) => ({
  url: `https://blog.example.com/hashtag/${word}`,
  title: `#${word}`,
  thumbnail: null,
  index: null,
}));

const IMAGES_COLLECT: MockCollectPage = {
  groups: [
    { shape: "{images}", count: IMAGE_ITEMS.length },
    { shape: "/hashtag/{any}", count: HASHTAG_ITEMS.length },
  ],
  defaultShape: "{images}",
  itemsFor: (shape) => (shape === "{images}" ? IMAGE_ITEMS : shape === "/hashtag/{any}" ? HASHTAG_ITEMS : []),
  adsExcluded: 0,
  page: {
    title: "『Big Buck Bunny』の森を歩く：制作の裏側",
    uploader: "Blender Foundation",
    publishedAt: local(2026, 10, 28, 7, 30),
    id: "bbb-making-of",
  },
};

const PLAYED_PAGE_URL = "https://example.com/watch/spring";
const PLAYED_ITEMS: CollectItem[] = [
  {
    url: "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8",
    title: "Spring · Open Movie · Blender Studio",
    thumbnail: null,
    index: null,
    played: { kind: "HLS", durationSeconds: 634 },
  },
  {
    url: "https://media.example.com/spring.mp4",
    title: "Spring · Open Movie · Blender Studio · 2",
    thumbnail: null,
    index: null,
    played: { kind: "MP4", durationSeconds: null },
  },
];
const RELOAD_FIND_PLAYED_ITEM: CollectItem = {
  url: "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8",
  title: "Sample player page",
  thumbnail: null,
  index: null,
  played: { kind: "HLS", durationSeconds: 635 },
};
const PLAYED_COLLECT: MockCollectPage = {
  groups: [
    { shape: "/related/{id}", count: 1 },
    { shape: PLAYED_SHAPE, count: PLAYED_ITEMS.length },
  ],
  defaultShape: PLAYED_SHAPE,
  itemsFor: (shape) => (shape === PLAYED_SHAPE ? PLAYED_ITEMS : SAMPLE_COLLECT_ITEMS.slice(0, 1)),
  adsExcluded: 0,
  page: {
    title: "Spring · Open Movie · Blender Studio",
    uploader: "Blender Studio",
    publishedAt: null,
    id: "spring",
  },
};

// #mockPick=empty: the same page, with no video played yet (the played-videos group is empty).
const PLAYED_EMPTY_COLLECT: MockCollectPage = {
  groups: [
    { shape: "/related/{id}", count: 1 },
    { shape: PLAYED_SHAPE, count: 0 },
  ],
  defaultShape: PLAYED_SHAPE,
  itemsFor: (shape) => (shape === PLAYED_SHAPE ? [] : SAMPLE_COLLECT_ITEMS.slice(0, 1)),
  adsExcluded: 0,
  page: PLAYED_COLLECT.page,
};

function mockCollectFor(url: string | null): MockCollectPage {
  if (url === IMAGES_PAGE_URL) return IMAGES_COLLECT;
  if (url === PLAYED_PAGE_URL) return readDebugHash().mockPick === "empty" ? PLAYED_EMPTY_COLLECT : PLAYED_COLLECT;
  return VIDEOS_COLLECT;
}

/** Like the collector: with the images chosen, the best link group's URLs come along. */
function mockOtherLinks(found: MockCollectPage, shape: string): CollectLinks | null {
  if (shape !== IMAGES_SHAPE) return null;
  const best = found.groups.find((group) => group.shape !== IMAGES_SHAPE && group.shape !== PLAYED_SHAPE);
  return best ? { shape: best.shape, urls: found.itemsFor(best.shape).map((item) => item.url) } : null;
}

let collectState: CollectState | null = null;
let collectTimer: ReturnType<typeof setTimeout> | null = null;
let nextRuleId = 4;
/** The mock scrolls for about 2 s (this many ticks) and then reports the end of the page. */
const MOCK_SCROLL_TICKS = 3;
const MOCK_SCROLL_TICK_MS = 700;
/** The most items a collect session keeps (the Rust side's MAX_SCAN_ITEMS). */
const MOCK_COLLECT_CAP = 2000;
let collectTicks = 0;
/** #mockScroll puts only the first collect_start into its state; buttons act normally after that. */
let mockScrollApplied = false;
/** The same for #mockPick: only the first collect_start is put into the picker's state. */
let mockPickApplied = false;
/** How long the mock waits, once picking starts without #mockPick, before it "presses" a link. */
const MOCK_PICK_DELAY_MS = 2500;
let mockPickTimer: ReturnType<typeof setTimeout> | null = null;

function emitPick(event: Events["collect-pick"]): void {
  emit("collect-pick", event);
}

function stopMockPickTimer(): void {
  if (mockPickTimer) clearTimeout(mockPickTimer);
  mockPickTimer = null;
}

/** What pressing the third card of the mock page does: a link pick, as the page would report it. */
function mockLinkPick(): Events["collect-pick"] | null {
  const item = collectState?.items[2];
  if (!collectState?.shape || !item) return null;
  const count = collectState.groups.find((group) => group.shape === collectState?.shape)?.count ?? collectState.items.length;
  return { kind: "link", url: item.url, shape: collectState.shape, count };
}

/** `items` followed by made-up videos up to `total`, so a capped session holds what it says. */
function padCollectItems(items: CollectItem[], total: number): CollectItem[] {
  const padded = items.slice();
  for (let index = padded.length; index < total; index += 1) {
    padded.push({
      url: `https://www.youtube.com/watch?v=mock${String(index).padStart(7, "0")}`,
      title: `Sample video ${index + 1}`,
      thumbnail: null,
      index: null,
    });
  }
  return padded;
}

function emitCollect(): void {
  if (collectState) emit("collect-state", structuredClone(collectState));
}

function stopCollectTimer(): void {
  if (collectTimer) clearTimeout(collectTimer);
  collectTimer = null;
}

function scheduleCollectTick(): void {
  stopCollectTimer();
  if (!collectState?.running) return;
  collectTimer = setTimeout(() => {
    if (!collectState?.running) return;
    collectState.scrolls += 1;
    collectTicks += 1;
    if (collectTicks >= MOCK_SCROLL_TICKS) {
      collectState.running = false;
      collectState.stopReason = "end";
    }
    emitCollect();
    scheduleCollectTick();
  }, MOCK_SCROLL_TICK_MS);
}

function siteById(siteId: number): Site {
  const site = sites.find((s) => s.id === siteId);
  if (!site) throw new Error(`site ${siteId} not found`);
  return site;
}

function accountById(accountId: number): { site: Site; account: Account } {
  for (const site of sites) {
    const account = site.accounts.find((a) => a.id === accountId);
    if (account) return { site, account };
  }
  throw new Error(`account ${accountId} not found`);
}

function siteIdForUrl(url: string): number | null {
  let host: string;
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return null;
  }
  let best: { id: number; length: number } | null = null;
  for (const site of sites) {
    for (const domain of site.domains) {
      if ((host === domain || host.endsWith(`.${domain}`)) && (!best || domain.length > best.length)) {
        best = { id: site.id, length: domain.length };
      }
    }
  }
  return best?.id ?? null;
}

function emitSites(): void {
  emit("sites-updated", structuredClone(sites));
}

function setBrowser(next: BrowserState): void {
  browserState = next;
  emit("browser-state", { ...next });
}

function startPage(): void {
  setBrowser({ ...IDLE_BROWSER });
}

function parseHttpUrl(url: string): URL {
  let parsed: URL;
  try {
    parsed = new URL(url.trim());
  } catch {
    throw new Error("invalid URL");
  }
  if (!/^https?:$/.test(parsed.protocol) || !parsed.hostname) {
    throw new Error("url must be an http(s) URL with a host");
  }
  return parsed;
}

// Debug seeds (see lib/debugHash.ts), so screenshots reach any state.
{
  const hash = readDebugHash();
  if (hash.mockMulti) {
    sites[0].accounts.push(newAccount("サブ", { cookieCount: 19, updatedAt: nowSeconds() - 5 * 86400 }));
  }
  if (hash.mockMethods) {
    sites[2].accounts[0].method = { kind: "firefox", profile: "default-release" };
    const bilibili = sites[4].accounts[0];
    bilibili.method = { kind: "cookiesTxt", path: "C:\\Users\\user\\Downloads\\bilibili-cookies.txt" };
    bilibili.updatedAt = nowSeconds() - 6 * 86400;
  }
  if (hash.mockLogin) {
    const site = sites.find((s) => s.domains.includes(hash.mockLogin ?? ""));
    if (site) {
      browserState = {
        url: site.loginUrl,
        loading: false,
        mode: "login",
        siteId: site.id,
        accountId: site.selectedAccountId,
      };
    }
  } else if (hash.mockUrl) {
    const siteId = siteIdForUrl(hash.mockUrl);
    const site = sites.find((s) => s.id === siteId);
    browserState = {
      url: hash.mockUrl,
      loading: false,
      mode: "browse",
      siteId,
      accountId: site?.selectedAccountId ?? null,
    };
  } else if (hash.mockFlow) {
    browserState = {
      url: MOCK_FLOW_URL,
      loading: false,
      mode: "browse",
      siteId: null,
      accountId: null,
    };
  } else if (hash.mockCollect || hash.mockScroll || hash.mockPick) {
    const url = hash.mockCollect === "images"
      ? IMAGES_PAGE_URL
      : hash.mockCollect === "played" || hash.mockPick === "empty"
        ? PLAYED_PAGE_URL
        : "https://www.youtube.com/@BlenderFoundation/videos";
    const siteId = siteIdForUrl(url);
    const site = sites.find((s) => s.id === siteId);
    browserState = {
      url,
      loading: false,
      mode: "browse",
      siteId,
      accountId: site?.selectedAccountId ?? null,
    };
  }
}

/** The Firefox profiles the mock "finds": one by default (see #mockFirefox in lib/debugHash.ts). */
function mockFirefoxProfiles(): FirefoxProfilesInfo {
  const mode = readDebugHash().mockFirefox;
  if (mode === "none") return { profiles: [], defaultProfile: null };
  if (mode === "many") {
    return { profiles: ["default-release", "default", "work"], defaultProfile: "default-release" };
  }
  return { profiles: ["default-release"], defaultProfile: "default-release" };
}

/**
 * A fixed result, whatever the sites hold: ニコニコ動画 and Twitch are imported and YouTube,
 * which has a login of its own, is left as it was. The two imported sites get cookies
 * unless they already have some, so the site list shows the effect. #mockFirefox=failing
 * rejects the way the Rust command does when a site could not be imported.
 */
async function mockFirefoxImport(): Promise<FirefoxImportResult> {
  await delay(900);
  const byDomain = (domain: string) => sites.find((s) => s.domains.includes(domain));
  const imported: FirefoxImportResult["imported"] = [];
  for (const [domain, cookies] of [
    ["nicovideo.jp", 14],
    ["twitch.tv", 9],
  ] as const) {
    const site = byDomain(domain);
    if (!site) continue;
    imported.push({ siteId: site.id, name: site.name, cookies });
    const account = site.accounts.find((a) => a.id === site.selectedAccountId);
    if (account && account.cookieCount == null) {
      account.cookieCount = cookies;
      account.updatedAt = nowSeconds();
    }
  }
  emitSites();
  if (readDebugHash().mockFirefox === "failing") {
    throw new Error("Bilibili: none of 6 cookies could be added");
  }
  const youtube = byDomain("youtube.com");
  return {
    imported,
    skipped: youtube ? [{ siteId: youtube.id, name: youtube.name, reason: "loggedIn" }] : [],
  };
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/** The sources, in id order: two lists and a collect-rule page (see #mockSources in lib/debugHash.ts). */
const sources: SourceRow[] = [
  {
    id: 1,
    name: "Blender Foundation の動画",
    url: "https://www.youtube.com/@BlenderOfficial/videos",
    kind: "list",
    siteName: "YouTube",
    ruleShape: null,
    accountLabel: null,
    destDir: "D:\\Videos\\Blender",
    preset: null,
    lastCheckedAt: nowSeconds() - 45 * 60,
    lastResult: { kind: "new", count: 3 },
    status: "idle",
  },
  {
    id: 2,
    name: "ひなたの投稿動画",
    url: "https://www.nicovideo.jp/user/12345678/video",
    kind: "list",
    siteName: "ニコニコ動画",
    ruleShape: null,
    accountLabel: null,
    destDir: null,
    preset: null,
    lastCheckedAt: null,
    lastResult: null,
    status: "checking",
  },
  {
    id: 3,
    name: "NASA のメディア",
    url: "https://x.com/NASA/media",
    kind: "rule",
    siteName: "X",
    ruleShape: "/NASA/status/{num}",
    accountLabel: "メイン",
    destDir: null,
    preset: null,
    lastCheckedAt: local(2026, 11, 3, 22, 10),
    lastResult: { kind: "none" },
    status: "idle",
  },
];
let nextSourceId = 4;

/** The seeded check of the second source ends after this long; a check started here takes CHECK_MS. */
const SEEDED_CHECK_MS = 25_000;
const CHECK_MS = 2_000;
/** What successive checks find: 2, 0, 1 and 3 new items. */
const CHECK_FINDS = [2, 0, 1, 3];
let checkCount = 0;
/** One check runs at a time; the others wait in this order. */
let checkRunning: number | null = null;
const checkQueue: number[] = [];

function patchSource(id: number, patch: Partial<SourceRow>): SourceRow | null {
  const index = sources.findIndex((s) => s.id === id);
  if (index === -1) return null;
  sources[index] = { ...sources[index], ...patch };
  emit("source-updated", { ...sources[index] });
  return sources[index];
}

function sourceById(id: number): SourceRow {
  const source = sources.find((s) => s.id === id);
  if (!source) throw new Error(`source ${id} not found`);
  return source;
}

function finishCheck(id: number): void {
  checkRunning = null;
  const found = CHECK_FINDS[checkCount++ % CHECK_FINDS.length];
  patchSource(id, {
    status: "idle",
    lastCheckedAt: nowSeconds(),
    lastResult: found > 0 ? { kind: "new", count: found } : { kind: "none" },
  });
  pumpChecks();
}

function pumpChecks(): void {
  if (checkRunning !== null) return;
  const id = checkQueue.shift();
  if (id === undefined) return;
  if (!sources.some((s) => s.id === id)) {
    pumpChecks();
    return;
  }
  checkRunning = id;
  patchSource(id, { status: "checking" });
  setTimeout(() => finishCheck(id), CHECK_MS);
}

function queueCheck(id: number): void {
  const source = sourceById(id);
  if (source.status !== "idle") return;
  patchSource(id, { status: "waiting" });
  checkQueue.push(id);
  pumpChecks();
}

{
  const mode = readDebugHash().mockSources;
  if (mode === "empty") {
    sources.length = 0;
  } else {
    if (mode === "long") {
      sources.push({
        id: nextSourceId++,
        name: "【公式】とても長いチャンネル名の例 ― 週末のライブ配信アーカイブと切り抜きをまとめた再生リスト（2026年版）",
        url: "https://example.com/very/long/path/with/many/segments/to/force/the/row/to/wrap/media",
        kind: "rule",
        siteName: "Example Media",
        ruleShape: "/very/long/path/with/many/segments/{id}/and/more/segments/to/force/wrapping/{num}/status/{any}/photo",
        accountLabel: "サブアカウント（長いラベルの例）",
        destDir: "D:\\Videos\\とても長いフォルダ名のサンプル\\さらに深い階層\\最後のフォルダ",
        preset: "upTo720",
        lastCheckedAt: nowSeconds() - 3 * 3600,
        lastResult: { kind: "error", category: "Network", message: "ERROR: unable to download webpage: timed out" },
        status: "idle",
      });
    }
    // The second source is mid-check when the page opens; it ends on its own a while later.
    checkRunning = 2;
    setTimeout(() => finishCheck(2), SEEDED_CHECK_MS);
  }
}

/** What a preview would register; the newest one is the only one that can be registered. */
interface MockPreview {
  preview: SourcePreview;
  ruleShape: string | null;
}
let newestPreview: MockPreview | null = null;
let nextPreviewToken = 1;

/** A readable name for a list URL: its first path segment, else the host. */
function nameFromUrl(parsed: URL): string {
  const first = parsed.pathname.split("/").find((part) => part.length > 0);
  const host = parsed.hostname.replace(/^www\./, "");
  if (!first) return host;
  try {
    return `${decodeURIComponent(first).replace(/^@/, "")} (${host})`;
  } catch {
    return `${first} (${host})`;
  }
}

function siteNameForUrl(url: string): string {
  const site = sites.find((s) => s.id === siteIdForUrl(url));
  return site?.name ?? new URL(url).hostname.replace(/^www\./, "");
}

/** The selected account's label, only for a site with two or more accounts. */
function accountLabelForUrl(url: string): string | null {
  const site = sites.find((s) => s.id === siteIdForUrl(url));
  if (!site || site.accounts.length < 2) return null;
  return site.accounts.find((a) => a.id === site.selectedAccountId)?.label ?? null;
}

function rememberPreview(entry: Omit<MockPreview, "preview"> & { preview: Omit<SourcePreview, "token"> }): SourcePreview {
  const preview: SourcePreview = { ...entry.preview, token: `mock-preview-${nextPreviewToken++}` };
  newestPreview = { preview, ruleShape: entry.ruleShape };
  return preview;
}

async function mockSourcePreview(rawUrl: string): Promise<SourcePreviewOutcome> {
  const parsed = parseHttpUrl(rawUrl);
  // Looking a URL up takes a moment.
  await delay(900);
  const url = rawUrl.trim();
  if (url.includes("watch")) {
    return { kind: "failed", reason: "notAList", message: "the URL is a single video" };
  }
  if (url.includes("login")) {
    return { kind: "failed", reason: "loginRequired", message: "Sign in to confirm your age" };
  }
  if (url.includes("unsupported")) {
    return { kind: "failed", reason: "unsupported", message: "Unsupported URL" };
  }
  if (url.includes("broken")) {
    return { kind: "failed", reason: "other", message: "yt-dlp exited with code 1" };
  }
  const existing = sources.find((s) => s.kind === "list" && s.url === url);
  return {
    kind: "ready",
    preview: rememberPreview({
      preview: { kind: "list", name: nameFromUrl(parsed), url, count: 248, existingId: existing?.id ?? null },
      ruleShape: null,
    }),
  };
}

/**
 * What yt-dlp would say about a new site's URL, told by the URL's words; takes a moment. A URL
 * with "login" is behind a login wall: it fails that way until the run is given the login (one
 * that also says "locked" stays behind it).
 */
async function mockSiteProbe(rawUrl: string, withLogin: boolean): Promise<SiteProbeReport> {
  parseHttpUrl(rawUrl);
  await delay(1500);
  const url = rawUrl.trim();
  if (url.includes("broken")) return { kind: "failed", category: "Network", message: "yt-dlp exited with code 1" };
  if (url.includes("login")) {
    return withLogin && !url.includes("locked")
      ? { kind: "list", count: 120, videos: "supported", loggedIn: true }
      : { kind: "loginRequired" };
  }
  if (url.includes("note.com")) return { kind: "unsupported" };
  if (url.includes("watch")) return { kind: "video", loggedIn: false };
  if (url.includes("generic")) return { kind: "list", count: 14, videos: "unknown", loggedIn: false };
  if (url.includes("channels") || url.includes("playlist") || url.includes("@")) {
    return { kind: "list", count: 120, videos: "supported", loggedIn: false };
  }
  return { kind: "unsupported" };
}

function mockSourcePreviewCollect(): SourcePreview {
  if (!collectState) throw new Error("no collect session is open");
  if (!collectState.shape || !browserState.url) throw new Error("no link shape is chosen");
  const url = browserState.url;
  const shape = collectState.shape;
  const existing = sources.find((s) => s.kind === "rule" && s.url === url && s.ruleShape === shape);
  return rememberPreview({
    preview: {
      kind: "rule",
      name: collectState.page?.title || new URL(url).hostname.replace(/^www\./, ""),
      url,
      count: collectState.items.length,
      existingId: existing?.id ?? null,
    },
    ruleShape: shape,
  });
}

function mockSourceRegister(args: Commands["source_register"]["args"]): SourceRow {
  if (!newestPreview || newestPreview.preview.token !== args.token) throw new Error("the preview expired");
  const { preview, ruleShape } = newestPreview;
  if (sources.some((s) => s.kind === preview.kind && s.url === preview.url && s.ruleShape === ruleShape)) {
    throw new Error("this URL is already a source");
  }
  newestPreview = null;
  // Registering is the first check; "saveAll" would queue the items (the mock queues nothing).
  const row: SourceRow = {
    id: nextSourceId++,
    name: preview.name,
    url: preview.url,
    kind: preview.kind,
    siteName: siteNameForUrl(preview.url),
    ruleShape,
    accountLabel: accountLabelForUrl(preview.url),
    destDir: null,
    preset: null,
    lastCheckedAt: nowSeconds(),
    lastResult: args.choice === "saveAll" ? { kind: "new", count: preview.count } : { kind: "none" },
    status: "idle",
  };
  sources.push(row);
  emit("source-updated", { ...row });
  return { ...row };
}

/** The library row with this id, or the error the real commands give for a gone one. */
function libraryRow(id: number): LibraryRow {
  const row = library.find((r) => r.id === id);
  if (!row) throw new Error(`library item ${id} not found`);
  return row;
}

/**
 * Every library row a job saved. A job saving several files (page images) has one row per
 * file, as in the real library; JobRow.libraryId names only the first.
 */
const libraryIdsByJob = new Map<number, number[]>();

/** The jobs that point at a library row (the real store also matches by URL and skipped paths). */
function jobsTiedTo(row: LibraryRow): JobRow[] {
  return [...jobs.values()].filter((job) => job.libraryId === row.id || job.outputPath === row.path);
}

/** Drops a library row and the jobs tied to it, as the real erase does; the file is not touched. */
function eraseLibraryRow(row: LibraryRow, tied: JobRow[]): void {
  library.splice(library.indexOf(row), 1);
  for (const job of tied) {
    jobs.delete(job.id);
    emit("job-removed", { id: job.id });
  }
}

// ---------------------------------------------------------------------------
// IpcClient implementation
// ---------------------------------------------------------------------------

// Untyped dispatcher: `cmd` is a plain union here (not a function-level generic),
// so the switch narrows normally and `args`/the return value can stay `unknown`
// without fighting TS over a generic indexed-access type per branch. The thin
// `call` wrapper below is the only place that asserts the real contract types,
// once, at the boundary.
async function dispatch(cmd: keyof Commands, args: unknown): Promise<unknown> {
  switch (cmd) {
    case "queue_snapshot":
      return [...jobs.values()];

    case "enqueue_urls": {
      const { urls, preset, tzOffsetSeconds, method: methodRef } = args as Commands["enqueue_urls"]["args"];
      const method = methodRef && collectMethods.find((item) => item.kind === methodRef.kind && item.id === methodRef.id);
      const ids: number[] = [];
      for (const url of urls) {
        const id = nextJobId++;
        tzOffsetByJobId.set(id, tzOffsetSeconds);
        presetByJobId.set(id, method?.preset ?? preset);
        if (method?.destDir) destDirByJobId.set(id, method.destDir);
        const row: JobRow = {
          id,
          url,
          state: "Queued",
          attempt: 0,
          progress: null,
          phase: null,
          speedBytesPerS: null,
          etaS: null,
          error: null,
          details: null,
          outputPath: null,
          commentsCount: null,
          libraryId: null,
          skipped: null,
          listCount: null,
          listTotal: null,
          alreadySaved: null,
          pageMediaChoices: [],
        };
        jobs.set(id, row);
        emit("job-updated", row);
        ids.push(id);
      }
      maybeStartNext();
      if (method && ids.length > 0) method.lastUsedAt = nowSeconds();
      return ids;
    }

    case "enqueue_page_images": {
      const page = args as Commands["enqueue_page_images"]["args"];
      parseHttpUrl(page.pageUrl);
      if (page.images.length === 0) throw new Error("no images were chosen");
      if (page.images.some((image) => image.index < 1)) throw new Error("an image number is below 1");
      const id = nextJobId++;
      tzOffsetByJobId.set(id, page.tzOffsetSeconds);
      const method = page.method && collectMethods.find((item) => item.kind === page.method?.kind && item.id === page.method?.id);
      if (method?.destDir) destDirByJobId.set(id, method.destDir);
      const row: JobRow = {
        id,
        url: page.pageUrl,
        state: "Queued",
        attempt: 0,
        progress: null,
        phase: null,
        speedBytesPerS: null,
        etaS: null,
        error: null,
        details: null,
        outputPath: null,
        commentsCount: null,
        libraryId: null,
        skipped: null,
        listCount: null,
        listTotal: null,
        alreadySaved: null,
        pageMediaChoices: [],
      };
      jobs.set(id, row);
      emit("job-updated", row);
      void runPageImagesJob(id, page);
      if (method) method.lastUsedAt = nowSeconds();
      return id;
    }

    case "enqueue_page_media": {
      const request = args as Commands["enqueue_page_media"]["args"];
      const method = request.method && collectMethods.find((item) => item.kind === request.method?.kind && item.id === request.method?.id);
      if (request.items.length === 0) throw new Error("no page media were chosen");
      const ids: number[] = [];
      for (const item of request.items) {
        parseHttpUrl(item.pageUrl);
        parseHttpUrl(item.mediaUrl);
        const id = nextJobId++;
        tzOffsetByJobId.set(id, request.tzOffsetSeconds);
        presetByJobId.set(id, method?.preset ?? request.preset);
        if (method?.destDir) destDirByJobId.set(id, method.destDir);
        const row: JobRow = {
          id,
          url: item.pageUrl,
          state: "Queued",
          attempt: 0,
          progress: null,
          phase: null,
          speedBytesPerS: null,
          etaS: null,
          error: null,
          details: null,
          outputPath: null,
          commentsCount: null,
          libraryId: null,
          skipped: null,
          listCount: null,
          listTotal: null,
          alreadySaved: null,
          pageMediaChoices: [],
        };
        jobs.set(id, row);
        emit("job-updated", row);
        ids.push(id);
      }
      maybeStartNext();
      if (method && ids.length > 0) method.lastUsedAt = nowSeconds();
      return ids;
    }

    case "cancel_job": {
      const { id } = args as Commands["cancel_job"]["args"];
      const job = jobs.get(id);
      if (job && ACTIVE_STATES.includes(job.state)) {
        update(id, {
          state: "Canceled",
          phase: null,
          progress: null,
          speedBytesPerS: null,
          etaS: null,
        });
        maybeStartNext();
      }
      return null;
    }

    case "retry_job": {
      const { id } = args as Commands["retry_job"]["args"];
      const job = jobs.get(id);
      if (job && (job.state === "Failed" || job.state === "Canceled")) {
        update(id, { state: "Queued", phase: null, attempt: job.attempt + 1, error: null, progress: null });
        maybeStartNext();
      }
      return null;
    }

    case "page_media_choose": {
      const { jobId, url } = args as Commands["page_media_choose"]["args"];
      const job = jobs.get(jobId);
      if (!job || !job.pageMediaChoices.some((choice) => choice.url === url)) {
        throw new Error("url is not one of this job's page media choices");
      }
      update(jobId, {
        state: "Queued",
        phase: null,
        attempt: job.attempt + 1,
        error: null,
        progress: null,
        pageMediaChoices: [],
      });
      maybeStartNext();
      return null;
    }

    case "save_again": {
      const { id } = args as Commands["save_again"]["args"];
      const job = jobs.get(id);
      if (job && job.state === "Done" && job.alreadySaved) {
        update(id, {
          state: "Queued",
          attempt: job.attempt + 1,
          skipped: null,
          alreadySaved: null,
          progress: null,
        });
        maybeStartNext();
      }
      return null;
    }

    case "read_clipboard_url":
      // Sample only: a fixed URL so the clipboard banner is reachable in dev/screenshots.
      return "https://www.youtube.com/watch?v=BaW_jenozKc";

    case "copy_text":
      lastCopiedText = (args as Commands["copy_text"]["args"]).text;
      return null;

    case "open_external_url":
      lastExternalUrl = parseHttpUrl((args as Commands["open_external_url"]["args"]).url).toString();
      return null;

    case "library_search": {
      const { text, limit, offset } = args as Commands["library_search"]["args"];
      const needle = text?.trim().toLowerCase();
      const filtered = needle
        ? library.filter(
            (row) =>
              row.title?.toLowerCase().includes(needle) ||
              row.uploader?.toLowerCase().includes(needle),
          )
        : library;
      return filtered.slice(offset, offset + limit);
    }

    case "library_open": {
      const { id } = args as Commands["library_open"]["args"];
      const row = libraryRow(id);
      if (row.missing) throw new Error(`library item ${id} is missing`);
      return null;
    }

    case "library_open_with": {
      const { id } = args as Commands["library_open_with"]["args"];
      libraryRow(id); // no real program picker in the mock
      return null;
    }

    case "library_rename": {
      const { id, name } = args as Commands["library_rename"]["args"];
      const row = libraryRow(id);
      const slash = Math.max(row.path.lastIndexOf("\\"), row.path.lastIndexOf("/"));
      const dot = row.path.lastIndexOf(".");
      const extension = dot > slash + 1 ? row.path.slice(dot) : "";
      const stem = row.path.slice(slash + 1, extension ? dot : undefined);
      if (name.trim() === "") throw new Error("the name is empty");
      if (/[\\/:*?"<>|]/.test(name)) {
        throw new Error('the name contains a character that is not allowed: \\ / : * ? " < > |');
      }
      if (name === stem) throw new Error("the name is unchanged");
      const path = row.path.slice(0, slash + 1) + name + extension;
      if (library.some((other) => other.id !== id && other.path.toLowerCase() === path.toLowerCase())) {
        throw new Error(`a file named "${name}${extension}" already exists`);
      }
      const updated: LibraryRow = { ...row, path };
      library[library.indexOf(row)] = updated;
      for (const job of [...jobs.values()]) {
        if (job.libraryId === row.id || job.outputPath === row.path) update(job.id, { outputPath: path });
      }
      emit("library-updated", null);
      return updated;
    }

    case "comments_extract": {
      const { libraryIds } = args as Commands["comments_extract"]["args"];
      await delay(1500);
      return libraryIds.map((id) => {
        const row = libraryRow(id);
        if (row.missing || !/ニコニコ|niconico|bilibili/i.test(row.site ?? "")) return { id, status: "failed", message: "the saved item has no comments extractor" };
        if (id === 10) return { id, status: "conflict", existingCount: 1072, existingModified: Math.floor(new Date(2026, 10, 2).getTime() / 1000), newCount: 1250, token: `mock-${id}`, fileName: row.path.replace(/^.*[\\/]/, "").replace(/\.[^.]+$/, ".comments.json") };
        row.commentsPath = row.path.replace(/\.[^.]+$/, ".comments.ass");
        return { id, status: "saved", count: 1250 };
      });
    }

    case "comments_resolve": {
      const { token, keep } = args as Commands["comments_resolve"]["args"];
      const match = /(?:mock-)?(\d+)$/.exec(token);
      const id = match ? Number(match[1]) : null;
      if (id != null) {
        const row = libraryRow(id);
        row.commentsPath = row.path.replace(/\.[^.]+$/, ".comments.ass");
      }
      return { id, status: "saved", count: keep === "existing" ? 1072 : 1250 };
    }

    case "pick_video_file":
      return `${settings.downloadDir}\\other\\movie without id.mp4`;

    case "file_extract_comments": {
      const { url } = args as Commands["file_extract_comments"]["args"];
      if (!url) return { id: null, status: "needsUrl" };
      await delay(1500);
      return { id: null, status: "saved", count: 1250 };
    }

    case "library_trash":
    case "library_forget": {
      const { id } = args as Commands["library_trash"]["args"];
      const row = libraryRow(id);
      const tied = jobsTiedTo(row);
      const running = tied.find((job) => ACTIVE_STATES.includes(job.state));
      if (running) {
        throw new Error(`job ${running.id} has not finished (${running.state}); cancel it or wait until it ends`);
      }
      eraseLibraryRow(row, tied);
      emit("library-updated", null);
      // #mockErase=sideFiles / traces: results that are not clean, for the notice line.
      const unclean = readDebugHash().mockErase;
      const result: EraseResult = {
        checkpointed: unclean !== "traces",
        cacheCleared: true,
        sideFilesFailed:
          cmd === "library_trash" && unclean === "sideFiles"
            ? [row.path.replace(/\.[^.\\/]+$/, ".ass"), row.path.replace(/\.[^.\\/]+$/, ".en.srt")]
            : [],
      };
      return result;
    }

    case "jobs_remove": {
      const { ids } = args as Commands["jobs_remove"]["args"];
      // The saved videos of the removed jobs go too (files stay), like the real command.
      const savedIds = new Set<number>();
      for (const id of ids) {
        const job = jobs.get(id);
        if (!job || ["Downloading", "Probing", "PostProcessing"].includes(job.state)) continue;
        if (job.state === "Queued" || job.state === "Ready") {
          update(id, { state: "Canceled", phase: null, progress: null, speedBytesPerS: null, etaS: null });
        }
        for (const libraryId of libraryIdsByJob.get(id) ?? (job.libraryId != null ? [job.libraryId] : [])) {
          savedIds.add(libraryId);
        }
        jobs.delete(id);
        emit("job-removed", { id });
      }
      let forgotten = false;
      for (const libraryId of savedIds) {
        const row = library.find((r) => r.id === libraryId);
        if (!row) continue;
        const tied = jobsTiedTo(row);
        // A saved video another running job is tied to stays, without an error.
        if (tied.some((job) => ACTIVE_STATES.includes(job.state))) continue;
        eraseLibraryRow(row, tied);
        forgotten = true;
      }
      if (forgotten) emit("library-updated", null);
      maybeStartNext();
      return null;
    }

    case "reveal_path":
      // No real filesystem in the mock; nothing to do.
      return null;

    case "open_job_file":
      return undefined;

    case "job_file_exists":
      return true;

    case "get_settings":
      return { ...settings };

    case "set_settings": {
      const { settings: next } = args as Commands["set_settings"]["args"];
      settings = { ...next };
      return null;
    }

    case "pick_download_dir":
      // A deterministic stand-in for the native picker in browser-only UI development.
      return "C:\\Users\\user\\Videos\\chosen-folder";

    case "pick_folder":
      return "D:\\Videos\\picked-folder";

    case "tools_status":
      return toolsStatus();

    case "install_tools": {
      const needed = MOCK_TOOL_IDS.filter(
        (tool) => toolVersions[tool] !== TOOL_EXPECTED_VERSIONS[tool],
      );
      await Promise.all(needed.map(installOneTool));
      return null;
    }

    case "sites_list":
      return structuredClone(sites);

    case "site_add": {
      const { url } = args as Commands["site_add"]["args"];
      const parsed = parseHttpUrl(url);
      const domain = parsed.hostname.replace(/^www\./, "").toLowerCase();
      if (sites.some((s) => s.domains.includes(domain))) {
        throw new Error(`a site already has domain ${domain}`);
      }
      const site = newSite(domain, [domain], url.trim(), false);
      sites.push(site);
      emitSites();
      return structuredClone(site);
    }

    case "site_probe": {
      const { url, withLogin } = args as Commands["site_probe"]["args"];
      return mockSiteProbe(url, withLogin ?? false);
    }

    case "site_remove": {
      const { siteId } = args as Commands["site_remove"]["args"];
      const site = siteById(siteId);
      if (site.builtin) throw new Error("built-in sites cannot be removed");
      sites.splice(sites.indexOf(site), 1);
      emitSites();
      return null;
    }

    case "site_set_ad_block": {
      const { siteId, enabled } = args as Commands["site_set_ad_block"]["args"];
      siteById(siteId).adBlock = enabled;
      emitSites();
      return null;
    }

    case "account_add": {
      const { siteId, label } = args as Commands["account_add"]["args"];
      if (!label.trim()) throw new Error("account label cannot be empty");
      const account = newAccount(label.trim());
      siteById(siteId).accounts.push(account);
      emitSites();
      return { ...account };
    }

    case "account_remove": {
      const { accountId } = args as Commands["account_remove"]["args"];
      const { site, account } = accountById(accountId);
      if (site.accounts.length <= 1) throw new Error("a site needs at least one account");
      site.accounts.splice(site.accounts.indexOf(account), 1);
      if (site.selectedAccountId === accountId) site.selectedAccountId = site.accounts[0].id;
      emitSites();
      return null;
    }

    case "account_rename": {
      const { accountId, label } = args as Commands["account_rename"]["args"];
      if (!label.trim()) throw new Error("account label cannot be empty");
      accountById(accountId).account.label = label.trim();
      emitSites();
      return null;
    }

    case "account_select": {
      const { siteId, accountId } = args as Commands["account_select"]["args"];
      const site = siteById(siteId);
      if (!site.accounts.some((a) => a.id === accountId)) {
        throw new Error(`account ${accountId} does not belong to site ${siteId}`);
      }
      site.selectedAccountId = accountId;
      emitSites();
      return null;
    }

    case "account_set_method": {
      const { accountId, method } = args as Commands["account_set_method"]["args"];
      const { account } = accountById(accountId);
      const previous: LoginMethod = account.method;
      account.method = method;
      if (method.kind !== "inApp" || previous.kind !== "inApp") {
        // The in-app cookie count belongs to the in-app profile only.
        account.cookieCount = null;
        account.updatedAt = method.kind === "cookiesTxt" ? nowSeconds() : null;
      }
      emitSites();
      return null;
    }

    case "account_logout": {
      const { accountId } = args as Commands["account_logout"]["args"];
      const { account } = accountById(accountId);
      account.cookieCount = null;
      account.updatedAt = null;
      emitSites();
      return null;
    }

    case "firefox_profiles":
      return [...mockFirefoxProfiles().profiles];

    case "firefox_profiles_info":
      return mockFirefoxProfiles();

    case "firefox_import": {
      const { profile } = args as Commands["firefox_import"]["args"];
      if (!mockFirefoxProfiles().profiles.includes(profile)) {
        throw new Error(`Firefox profile ${profile} was not found`);
      }
      return mockFirefoxImport();
    }

    case "pick_cookies_file":
      // The native file dialog does not exist here; behave as if it was cancelled.
      return null;

    case "site_for_url": {
      const { url } = args as Commands["site_for_url"]["args"];
      return siteIdForUrl(url);
    }

    case "browser_open": {
      const { url, accountId } = args as Commands["browser_open"]["args"];
      if (url === null) {
        startPage();
        return null;
      }
      const parsed = parseHttpUrl(url);
      const siteId = siteIdForUrl(parsed.toString());
      const site = sites.find((s) => s.id === siteId);
      setBrowser({
        url: parsed.toString(),
        loading: false,
        mode: "browse",
        siteId,
        accountId: accountId ?? site?.selectedAccountId ?? null,
      });
      if (collectState?.running) {
        collectState.running = false;
        collectState.stopReason = "stopped";
        emitCollect();
        stopCollectTimer();
      }
      return null;
    }

    case "browser_set_bounds":
    case "browser_set_visible":
    case "browser_back":
    case "browser_forward":
    case "browser_reload":
      return null;

    case "login_start": {
      const { siteId, accountId } = args as Commands["login_start"]["args"];
      const site = siteById(siteId);
      const account = site.accounts.find((a) => a.id === accountId);
      if (!account) throw new Error(`account ${accountId} does not belong to site ${siteId}`);
      if (account.method.kind !== "inApp") throw new Error("only in-app accounts can use the login browser");
      setBrowser({ url: site.loginUrl, loading: false, mode: "login", siteId, accountId });
      return null;
    }

    case "login_finish": {
      if (browserState.mode !== "login" || browserState.accountId == null) {
        throw new Error("no login is in progress");
      }
      const { account } = accountById(browserState.accountId);
      account.cookieCount = 14 + Math.floor(Math.random() * 12);
      account.updatedAt = nowSeconds();
      startPage();
      emitSites();
      return { ...account };
    }

    case "login_cancel": {
      if (browserState.mode !== "login") throw new Error("no login is in progress");
      startPage();
      return null;
    }

    case "collect_start": {
      if (!browserState.url || browserState.mode === "login") throw new Error("browser page is not open");
      const { shape, maxScrolls } = args as Commands["collect_start"]["args"];
      const found = mockCollectFor(browserState.url);
      const selectedShape = shape ?? found.defaultShape;
      const savedRule = collectMethods.find(
        (item) =>
          item.kind === "rule" &&
          item.shape === selectedShape &&
          browserState.url?.startsWith(item.pageUrl),
      );
      collectTicks = 0;
      collectState = {
        running: true,
        scrolls: collectState?.scrolls ?? 0,
        maxScrolls,
        stopReason: null,
        groups: found.groups.map((group) => ({ ...group })),
        shape: selectedShape,
        items: found.itemsFor(selectedShape),
        adsExcluded: settings.collectIncludeAds ? 0 : found.adsExcluded,
        ruleId: savedRule?.id ?? null,
        error: null,
        page: { ...found.page },
        otherLinks: mockOtherLinks(found, selectedShape),
      };
      const scrollState = readDebugHash().mockScroll;
      if (scrollState && !mockScrollApplied) {
        mockScrollApplied = true;
        collectState.running = scrollState === "running";
        collectState.stopReason = scrollState === "running" ? null : scrollState;
        if (scrollState === "cap") collectState.items = padCollectItems(collectState.items, MOCK_COLLECT_CAP);
        emitCollect();
        // "running" is left running so a screenshot of it stays put.
        return null;
      }
      const pickState = readDebugHash().mockPick;
      if (pickState && !mockPickApplied) {
        mockPickApplied = true;
        // Starting to pick stops a running scroll, as it does in the app.
        collectState.running = false;
        collectState.stopReason = pickState === "empty" ? "end" : "stopped";
        emitCollect();
        if (pickState === "active") emitPick({ kind: "active" });
        if (pickState === "picked") {
          const pick = mockLinkPick();
          if (pick) emitPick(pick);
        }
        return null;
      }
      emitCollect();
      scheduleCollectTick();
      return null;
    }

    case "collect_stop":
      if (!collectState) throw new Error("no collect session is open");
      if (collectState.running) {
        collectState.running = false;
        collectState.stopReason = "stopped";
      }
      stopCollectTimer();
      emitCollect();
      return null;

    case "collect_reload_find": {
      if (!collectState || !browserState.url) throw new Error("no collect session is open on the current page");
      const pageUrl = browserState.url;
      if (collectState.running) {
        collectState.running = false;
        collectState.stopReason = "stopped";
      }
      stopCollectTimer();
      emitCollect();
      await delay(1500);
      if (!collectState) throw new Error("collect session closed while looking for videos");
      if (browserState.url !== pageUrl) throw new Error("browser page changed while looking for videos");
      collectState.groups = [
        ...collectState.groups.filter((group) => group.shape !== PLAYED_SHAPE),
        { shape: PLAYED_SHAPE, count: 1 },
      ];
      collectState.shape = PLAYED_SHAPE;
      collectState.items = [{ ...RELOAD_FIND_PLAYED_ITEM }];
      collectState.otherLinks = null;
      emitCollect();
      return { found: 1 };
    }

    case "collect_select": {
      if (!collectState) throw new Error("no collect session is open");
      const { shape } = args as Commands["collect_select"]["args"];
      const found = mockCollectFor(browserState.url);
      collectState.shape = shape;
      collectState.items = found.itemsFor(shape);
      // Like the Rust side: the items start again, so "collected the most" no longer holds.
      if (collectState.stopReason === "cap") collectState.stopReason = "stopped";
      collectState.otherLinks = mockOtherLinks(found, shape);
      collectState.adsExcluded = settings.collectIncludeAds ? 0 : found.adsExcluded;
      emitCollect();
      return null;
    }

    case "collect_close":
      stopCollectTimer();
      stopMockPickTimer();
      collectState = null;
      return null;

    case "collect_reveal":
      return collectState?.items.some((item) => item.url === (args as Commands["collect_reveal"]["args"]).url) ?? false;

    // The mock page has no badges to keep in step; the selection is only accepted.
    case "collect_set_selection":
      return null;

    case "collect_pick_start": {
      if (!collectState) throw new Error("no collect session is open on the current page");
      if (collectState.running) {
        collectState.running = false;
        collectState.stopReason = "stopped";
        stopCollectTimer();
        emitCollect();
      }
      stopMockPickTimer();
      emitPick({ kind: "active" });
      // There is no page to press in the mock: after a moment it "presses" the third card,
      // unless #mockPick holds the screenshot state still.
      if (!readDebugHash().mockPick) {
        mockPickTimer = setTimeout(() => {
          mockPickTimer = null;
          const pick = mockLinkPick();
          if (pick) emitPick(pick);
        }, MOCK_PICK_DELAY_MS);
      }
      return null;
    }

    case "collect_pick_stop":
      stopMockPickTimer();
      emitPick({ kind: "ended" });
      return null;

    case "collect_save_rule":
      if (!collectState?.shape) throw new Error("no link shape is selected");
      {
        const { name, matchPattern } = args as Commands["collect_save_rule"]["args"];
        const pageUrl = matchPattern.replace(/\*$/, "");
        const parsed = parseHttpUrl(pageUrl);
        const siteId = siteIdForUrl(pageUrl);
        const site = sites.find((item) => item.id === siteId);
        collectState.ruleId = nextRuleId++;
        collectMethods.push({
          kind: "rule",
          id: collectState.ruleId,
          name: name.trim(),
          siteId,
          siteName: site?.name ?? parsed.hostname.toLowerCase().replace(/^www\./, ""),
          host: parsed.hostname.toLowerCase(),
          pageUrl,
          shape: collectState.shape,
          stepCount: null,
          scopes: null,
          destDir: null,
          preset: null,
          lastUsedAt: null,
        });
      }
      emitCollect();
      return collectState.ruleId;

    case "methods_list":
      return structuredClone(collectMethods);

    case "method_set_options": {
      const { kind, id, destDir, preset } = args as Commands["method_set_options"]["args"];
      const method = collectMethods.find((item) => item.kind === kind && item.id === id);
      if (!method) throw new Error(`${kind} ${id} was not found`);
      if (destDir !== null && !/^([A-Za-z]:[\\/]|\/)/.test(destDir)) {
        throw new Error("destDir must be an absolute path");
      }
      method.destDir = destDir;
      method.preset = preset;
      return null;
    }

    case "method_rename": {
      const { kind, id, name } = args as Commands["method_rename"]["args"];
      const method = collectMethods.find((item) => item.kind === kind && item.id === id);
      if (!method) throw new Error(`${kind} ${id} was not found`);
      const trimmed = name.trim();
      if (!trimmed) throw new Error("name must not be empty");
      method.name = trimmed;
      if (kind === "flow") {
        const flow = savedFlows.find((item) => item.id === id);
        if (flow) {
          flow.name = method.name;
          flow.updatedAt = nowSeconds();
        }
      }
      return null;
    }

    case "method_delete": {
      const { kind, id } = args as Commands["method_delete"]["args"];
      const index = collectMethods.findIndex((item) => item.kind === kind && item.id === id);
      if (index < 0) throw new Error(`${kind} ${id} was not found`);
      if (kind === "rule" && id === 2) throw new Error("rule-in-use:NASA のメディア");
      collectMethods.splice(index, 1);
      if (kind === "flow") {
        const flowIndex = savedFlows.findIndex((item) => item.id === id);
        if (flowIndex >= 0) {
          savedFlows.splice(flowIndex, 1);
          emitSites();
        }
      }
      return null;
    }

    case "flow_record_start":
      emit("flow-record-progress", { steps: 0 });
      return null;

    case "flow_record_stop":
      return structuredClone(MOCK_FLOW);

    case "flow_save": {
      const { name, flow } = args as Commands["flow_save"]["args"];
      const parsed = parseHttpUrl(flow.startUrl);
      const siteId = siteIdForUrl(flow.startUrl);
      const now = nowSeconds();
      const saved: SavedFlow = {
        id: nextFlowId++,
        siteId,
        host: parsed.hostname.toLowerCase(),
        name: name.trim(),
        startUrl: flow.startUrl,
        steps: structuredClone(flow.steps),
        scopes: [...flow.scopes],
        createdAt: now,
        updatedAt: now,
      };
      savedFlows.unshift(saved);
      const site = sites.find((item) => item.id === siteId);
      collectMethods.push({
        kind: "flow",
        id: saved.id,
        name: saved.name,
        siteId,
        siteName: site?.name ?? parsed.hostname.toLowerCase().replace(/^www\./, ""),
        host: saved.host,
        pageUrl: saved.startUrl,
        shape: null,
        stepCount: saved.steps.length,
        scopes: [...saved.scopes],
        destDir: null,
        preset: null,
        lastUsedAt: null,
      });
      emitSites();
      return structuredClone(saved);
    }

    case "flow_list": {
      const host = parseHttpUrl((args as Commands["flow_list"]["args"]).url).hostname.toLowerCase();
      return structuredClone(savedFlows.filter((flow) => flow.host === host));
    }

    case "flow_rename": {
      const { id, name } = args as Commands["flow_rename"]["args"];
      const flow = savedFlows.find((item) => item.id === id);
      if (!flow) throw new Error(`flow ${id} was not found`);
      flow.name = name.trim();
      flow.updatedAt = nowSeconds();
      const method = collectMethods.find((item) => item.kind === "flow" && item.id === id);
      if (method) method.name = flow.name;
      return null;
    }

    case "flow_delete": {
      const { id } = args as Commands["flow_delete"]["args"];
      const index = savedFlows.findIndex((flow) => flow.id === id);
      if (index >= 0) {
        savedFlows.splice(index, 1);
        collectMethods = collectMethods.filter((item) => item.kind !== "flow" || item.id !== id);
        emitSites();
      }
      return null;
    }

    case "flow_replay": {
      const { scopes } = args as Commands["flow_replay"]["args"];
      const first = scopes[0] ?? "this";
      const indexes = first === "restOfRow" ? [1, 2, 3, 4] : first === "allOfKind" ? [...Array(10).keys()] : [1];
      const total = indexes.length;
      emit("flow-progress", { done: 0, total });
      const results: FlowReplayResult["results"] = [];
      for (const [done, index] of indexes.entries()) {
        await delay(250);
        results.push({
          replayLabel: `動画 ${index + 1}`,
          media: `https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8?card=${index + 1}`,
        });
        emit("flow-progress", { done: done + 1, total });
      }
      return { results, capped: false, total };
    }

    case "flow_replay_cancel":
      return null;

    case "saved_page_list":
      return structuredClone(savedPages);

    case "saved_page_add": {
      const { url, name } = args as Commands["saved_page_add"]["args"];
      const kept = url.trim().split("#")[0];
      parseHttpUrl(kept);
      if (/[\s\u0000-\u001f]/.test(kept)) throw new Error("the URL contains a space or a control character");
      if ([...kept].length > SAVED_PAGE_URL_MAX) {
        throw new Error(`the URL is longer than ${SAVED_PAGE_URL_MAX} characters`);
      }
      const cleanName = savedPageName(name);
      const existing = savedPages.find((page) => page.url === kept);
      if (existing) return structuredClone(existing);
      const page: SavedPage = { id: nextSavedPageId++, url: kept, name: cleanName, createdAt: nowSeconds() };
      savedPages = [...savedPages, page];
      return structuredClone(page);
    }

    case "saved_page_rename": {
      const { id, name } = args as Commands["saved_page_rename"]["args"];
      const cleanName = savedPageName(name);
      const page = savedPages.find((item) => item.id === id);
      if (!page) throw new Error("saved page not found");
      page.name = cleanName;
      return structuredClone(page);
    }

    case "saved_page_remove":
      savedPages = savedPages.filter((page) => page.id !== (args as Commands["saved_page_remove"]["args"]).id);
      return null;

    case "sources_list":
      return structuredClone(sources);

    case "source_preview":
      return mockSourcePreview((args as Commands["source_preview"]["args"]).url);

    case "source_preview_collect":
      return mockSourcePreviewCollect();

    case "source_register":
      return mockSourceRegister(args as Commands["source_register"]["args"]);

    case "source_check":
      queueCheck((args as Commands["source_check"]["args"]).id);
      return null;

    case "source_check_all":
      for (const source of [...sources]) queueCheck(source.id);
      return null;

    case "source_rename": {
      const { id, name } = args as Commands["source_rename"]["args"];
      sourceById(id);
      const trimmed = name.trim();
      if (trimmed.length < 1 || [...trimmed].length > 200) throw new Error("the name must be 1 to 200 characters");
      return { ...patchSource(id, { name: trimmed }) } as SourceRow;
    }

    case "source_set_options": {
      const { id, destDir, preset } = args as Commands["source_set_options"]["args"];
      sourceById(id);
      if (destDir !== null && !/^([A-Za-z]:[\\/]|\/)/.test(destDir)) {
        throw new Error("destDir must be an absolute path");
      }
      return { ...patchSource(id, { destDir, preset }) } as SourceRow;
    }

    case "source_delete": {
      const { id } = args as Commands["source_delete"]["args"];
      sources.splice(sources.indexOf(sourceById(id)), 1);
      const waiting = checkQueue.indexOf(id);
      if (waiting !== -1) checkQueue.splice(waiting, 1);
      emit("source-removed", { id });
      return null;
    }

    default:
      throw new Error(`mock ipc: unhandled command ${String(cmd)}`);
  }
}

async function call<K extends keyof Commands>(
  cmd: K,
  args: Commands[K]["args"],
): Promise<Commands[K]["result"]> {
  // Opt-in call log for scripted checks: a page that defines window.__ipcLog = []
  // before the app loads gets every command appended to it.
  const log = (globalThis as { __ipcLog?: unknown }).__ipcLog;
  if (Array.isArray(log)) log.push({ cmd, args });
  // A touch of latency so loading states are visible/testable, same as a real IPC round trip.
  await delay(40);
  const result = await dispatch(cmd, args);
  return result as Commands[K]["result"];
}

function on<K extends keyof Events>(
  event: K,
  handler: (payload: Events[K]) => void,
): () => void {
  listeners[event].add(handler as (payload: Events[typeof event]) => void);
  // Rust would already have told a page that loaded after the state changed;
  // the mock replays a non-idle state (seeded by a debug hash) to new listeners.
  if (event === "browser-state" && browserState.url !== null) {
    const replay = { ...browserState };
    setTimeout(() => (handler as (payload: BrowserState) => void)(replay), 0);
  }
  return () => listeners[event].delete(handler as (payload: Events[typeof event]) => void);
}

export const mockIpcClient: IpcClient = { call, on };

// ---------------------------------------------------------------------------
// In-app updates (#mockUpdate=, see lib/debugHash.ts)
// ---------------------------------------------------------------------------

const MOCK_APP_VERSION = "0.1.0";
const MOCK_UPDATE_SIZE = 18 * 1024 * 1024;
const MOCK_UPDATE_CHUNK = 0.9 * 1024 * 1024;

const MOCK_UPDATE_NOTES = [
  "ソースの新着を自動で確認できるようになりました",
  "通知を出せるようになりました",
  "サイトを追加したときに、扱えるかどうかを調べて出すようにしました",
  "ページにある画像を選んで、まとめて保存できるようになりました",
];

const MOCK_UPDATE_LONG_NOTES = [
  ...MOCK_UPDATE_NOTES,
  "保存先のフォルダ名とファイル名を、自分の形式に変えられるようになりました",
  "ログイン済みの Firefox から、サイトのログインを取り込めるようになりました",
  "保存した動画を、アプリの中で再生できるようになりました",
  "ライブラリから、保存したファイルの場所を開けるようになりました",
  "yt-dlp の更新を確かめるときの待ち時間を短くしました",
  "ダークテーマで見づらかった表示を直しました",
];

function mockUpdateInfo(): UpdateInfo {
  const notes = MOCK_UPDATE === "availableLong" ? MOCK_UPDATE_LONG_NOTES : MOCK_UPDATE_NOTES;
  return {
    version: "0.2.0",
    currentVersion: MOCK_APP_VERSION,
    notes: notes.map((line) => `- ${line}`).join("\n"),
  };
}

export const mockUpdaterClient: UpdaterClient = {
  async appVersion() {
    await delay(40);
    return MOCK_APP_VERSION;
  },

  async check() {
    await delay(900);
    if (MOCK_UPDATE === "offline") {
      throw new Error(
        "error sending request for url (https://github.com/terasaki-8910/vidsdownloader-releases/releases/latest/download/latest.json)",
      );
    }
    if (!MOCK_UPDATE || MOCK_UPDATE === "latest") return null;
    return mockUpdateInfo();
  },

  // 0.9 MB every 450 ms up to 18.0 MB (about 9 s), then a short install. "failing" stops at 7.2 MB.
  async downloadAndInstall(onEvent: (event: UpdateDownloadEvent) => void) {
    await delay(300);
    onEvent({ kind: "started", totalBytes: MOCK_UPDATE_SIZE });
    for (let done = 0; done < MOCK_UPDATE_SIZE; done += MOCK_UPDATE_CHUNK) {
      await delay(450);
      if (MOCK_UPDATE === "failing" && done >= 7 * 1024 * 1024) {
        throw new Error("error decoding response body: connection closed before message completed");
      }
      onEvent({ kind: "progress", chunkBytes: Math.min(MOCK_UPDATE_CHUNK, MOCK_UPDATE_SIZE - done) });
    }
    onEvent({ kind: "finished" });
    await delay(600);
  },

  // The app would start again in the new version; the page starts again instead.
  async relaunch() {
    location.reload();
  },
};

// Formatting helpers. Unit abbreviations (KB/MB, B/s) are kept as plain Latin
// technical notation in both languages, matching common practice in Japanese
// download-manager UIs -- they are not run through i18n.

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/** seconds -> "M:SS" or "H:MM:SS". Dates are formatted with formatDate(Time) below. */
export function formatClockDuration(totalSeconds: number): string {
  const s = Math.max(0, Math.round(totalSeconds));
  const hours = Math.floor(s / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  const seconds = s % 60;
  if (hours > 0) {
    return `${hours}:${pad2(minutes)}:${pad2(seconds)}`;
  }
  return `${minutes}:${pad2(seconds)}`;
}

export function formatByteSize(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${Math.round(bytes)} B`;
}

export function formatSpeed(bytesPerSecond: number): string {
  return `${formatByteSize(bytesPerSecond)}/s`;
}

/** Unix seconds -> "YYYY-MM-DD" in local time, the same date the saved file name uses. */
export function formatDate(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

/** Unix seconds -> "YYYY/MM/DD" in local time (the tray's per-item line). */
export function formatSlashDate(unixSeconds: number): string {
  return formatDate(unixSeconds).replace(/-/g, "/");
}

/** Unix seconds -> "MM/DD" in local time (login status lines). */
export function formatMonthDay(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  return `${pad2(d.getMonth() + 1)}/${pad2(d.getDate())}`;
}

/** Unix seconds -> locale month/day for short conflict metadata ("11月2日" / "Nov 2"). */
export function formatCommentDate(unixSeconds: number): string {
  const locale = typeof document === "undefined" ? undefined : document.documentElement.lang || undefined;
  return new Intl.DateTimeFormat(locale, { month: "short", day: "numeric" }).format(new Date(unixSeconds * 1000));
}

/** Unix seconds -> "YYYY-MM-DD HH:MM" in local time. */
export function formatDateTime(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  return `${formatDate(unixSeconds)} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

/** Unix seconds -> "HH:MM" in local time. */
export function formatClock(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

/** Whether the Unix time falls on the same local calendar day as `now`. */
export function isSameDay(unixSeconds: number, now: Date): boolean {
  const d = new Date(unixSeconds * 1000);
  return d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth() && d.getDate() === now.getDate();
}

/**
 * Shortens a long folder path in the middle, keeping the drive (or root) and the last two
 * folders, which are what tell recent folders apart: `C:\…\scratchpad\dl-test`.
 */
export function shortenPath(path: string, max = 44): string {
  if (path.length <= max) return path;
  const sep = path.includes("\\") ? "\\" : "/";
  const parts = path.split(sep);
  if (parts.length <= 4) return path;
  return [parts[0], "…", ...parts.slice(-2)].join(sep);
}

/** Joins the non-null parts of a meta line with " · ", e.g. "uploader · site · date". */
export function joinMeta(parts: Array<string | null | undefined>): string {
  return parts.filter((p): p is string => Boolean(p && p.length > 0)).join(" · ");
}

/** The last segment of a Windows or POSIX path. */
export function fileName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/")) + 1);
}

/** A path's file name split into the part a person renames and the extension (with its
 *  dot, "" when there is none); a leading dot is part of the name (".hidden"). */
export function splitFileName(path: string): { stem: string; extension: string } {
  const name = fileName(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? { stem: name.slice(0, dot), extension: name.slice(dot) } : { stem: name, extension: "" };
}

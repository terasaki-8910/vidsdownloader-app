// The tray's filter and sort over the found items, using what yt-dlp said about each (uploader,
// length, upload date; see the tray's look-up in BrowserView). An item whose answer is not in yet,
// or failed, or lacks the field a filter needs is left out while that filter is set; the number
// left out is reported so the tray can say so. Pure functions: BrowserView owns the state.
import type { CollectItem, CollectProbeMeta } from "../ipc/contract";
import type { Translations } from "../i18n/types";

export type SortKey = "page" | "newest" | "oldest" | "longest" | "shortest";

export interface TrayFilter {
  sort: SortKey;
  /** Whole minutes as typed; "" = no bound. */
  minMinutes: string;
  maxMinutes: string;
  /** Uploader names; none = everyone. */
  uploaders: string[];
  /** "YYYY-MM-DD" as a date input gives it; "" = no bound. */
  from: string;
  to: string;
}

export const NO_FILTER: TrayFilter = { sort: "page", minMinutes: "", maxMinutes: "", uploaders: [], from: "", to: "" };

/** The filter the mock states #mockFilter=open and =active show. */
export const SAMPLE_FILTER: TrayFilter = {
  sort: "newest",
  minMinutes: "1",
  maxMinutes: "10",
  uploaders: ["NASA", "JAXA"],
  from: "2026-11-01",
  to: "",
};

export const SORT_KEYS: SortKey[] = ["page", "newest", "oldest", "longest", "shortest"];

/**
 * What is known about an item: its details, null when the look-up failed, undefined when it has not
 * finished (or was not asked for). A null field inside the details is a field yt-dlp did not give.
 */
export type MetaOf = (item: CollectItem) => CollectProbeMeta | null | undefined;

function bound(text: string): number | null {
  const value = Number(text);
  return text.trim() !== "" && Number.isFinite(value) ? value : null;
}

/** Local midnight of a "YYYY-MM-DD" date, in Unix seconds. */
function dayStart(date: string): number | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!match) return null;
  return new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3])).getTime() / 1000;
}

export function lengthBounds(filter: TrayFilter): { min: number | null; max: number | null } {
  return { min: bound(filter.minMinutes), max: bound(filter.maxMinutes) };
}

export function filterConditions(filter: TrayFilter): { length: boolean; uploaders: boolean; from: boolean; to: boolean } {
  const { min, max } = lengthBounds(filter);
  return {
    length: min !== null || max !== null,
    uploaders: filter.uploaders.length > 0,
    from: dayStart(filter.from) !== null,
    to: dayStart(filter.to) !== null,
  };
}

/** True when any condition is set or the order is not the page's. */
export function filterActive(filter: TrayFilter): boolean {
  const c = filterConditions(filter);
  return filter.sort !== "page" || c.length || c.uploaders || c.from || c.to;
}

export interface FilterResult {
  shown: CollectItem[];
  /** Items left out only because nothing is known about a field a filter needs. */
  noInfo: number;
}

/** The items a filter keeps, in its order: the page's order among equals, items lacking the sorted field last. */
export function applyFilter(items: CollectItem[], filter: TrayFilter, metaOf: MetaOf): FilterResult {
  const { min, max } = lengthBounds(filter);
  const c = filterConditions(filter);
  const from = dayStart(filter.from);
  const toEnd = dayStart(filter.to);
  const kept: CollectItem[] = [];
  let noInfo = 0;
  for (const item of items) {
    const meta = metaOf(item);
    let missing = false;
    let pass = true;
    if (c.length) {
      const length = meta?.durationS;
      if (length == null) missing = true;
      else if ((min !== null && length < min * 60) || (max !== null && length > max * 60)) pass = false;
    }
    if (c.uploaders) {
      const uploader = meta?.uploader;
      if (uploader == null) missing = true;
      else if (!filter.uploaders.includes(uploader)) pass = false;
    }
    if (c.from || c.to) {
      const published = meta?.publishedAt;
      if (published == null) missing = true;
      else if ((from !== null && published < from) || (toEnd !== null && published >= toEnd + 86400)) pass = false;
    }
    if (missing) noInfo += 1;
    else if (pass) kept.push(item);
  }
  const byNumber = (pick: (meta: CollectProbeMeta | null | undefined) => number | null | undefined, direction: 1 | -1) =>
    [...kept]
      .map((item, index) => ({ item, index, value: pick(metaOf(item)) }))
      .sort((a, b) => {
        if (a.value == null && b.value == null) return a.index - b.index;
        if (a.value == null) return 1;
        if (b.value == null) return -1;
        return a.value === b.value ? a.index - b.index : direction * (a.value - b.value);
      })
      .map((entry) => entry.item);
  let shown = kept;
  if (filter.sort === "newest") shown = byNumber((meta) => meta?.publishedAt, -1);
  else if (filter.sort === "oldest") shown = byNumber((meta) => meta?.publishedAt, 1);
  else if (filter.sort === "longest") shown = byNumber((meta) => meta?.durationS, -1);
  else if (filter.sort === "shortest") shown = byNumber((meta) => meta?.durationS, 1);
  return { shown, noInfo };
}

export interface UploaderCount {
  name: string;
  count: number;
}

/** Uploaders of the found items that are known, most items first (ties by name). */
export function uploaderCounts(items: CollectItem[], metaOf: MetaOf): UploaderCount[] {
  const counts = new Map<string, number>();
  for (const item of items) {
    const name = metaOf(item)?.uploader;
    if (name) counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
}

/** "2026-11-01" -> "11/01". */
export function monthDay(date: string): string {
  return date.slice(5).replace("-", "/");
}

/** The label of a sort order in the UI language. */
export function sortLabel(t: Translations, key: SortKey): string {
  return {
    page: t.browser.sortPage,
    newest: t.browser.sortNewest,
    oldest: t.browser.sortOldest,
    longest: t.browser.sortLongest,
    shortest: t.browser.sortShortest,
  }[key];
}

import type { SourceRow } from "../ipc/contract";
import type { Translations } from "../i18n/types";
import type { RegisterOutcome } from "../state/SourcesContext";
import { fileName } from "./format";

/** A folder's own name: the last path segment, ignoring a trailing separator. */
export function folderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  return fileName(trimmed) || trimmed || path;
}

/** The line that says why a registration did not end in a new source; null when none is due. */
export function registerFailureText(t: Translations, outcome: RegisterOutcome): string | null {
  if (outcome.kind === "exists") return t.sources.registerFailed.alreadyRegistered;
  if (outcome.kind !== "failed") return null;
  switch (outcome.reason) {
    case "notAList":
      return t.sources.registerFailed.notAList;
    case "loginRequired":
      return t.sources.registerFailed.loginRequired;
    case "unsupported":
      return t.sources.registerFailed.unsupported;
    default:
      return t.sources.registerFailed.other(outcome.message);
  }
}

/** What a source row's status line shows. */
export interface SourceStatusLine {
  kind: "waiting" | "checking" | "idle";
  text: string;
  /** The last check could not be made; the full message, for the line's tooltip. */
  error?: string;
}

export function sourceStatusLine(t: Translations, source: SourceRow, now: Date): SourceStatusLine {
  if (source.status === "waiting") return { kind: "waiting", text: t.sources.waiting };
  if (source.status === "checking") return { kind: "checking", text: t.sources.checking };
  if (source.lastCheckedAt == null) return { kind: "idle", text: t.sources.neverChecked };
  const at = t.sources.dateTime(source.lastCheckedAt, now);
  const result = source.lastResult;
  if (result?.kind === "new") return { kind: "idle", text: t.sources.checkedNew(at, result.count) };
  if (result?.kind === "error") {
    return {
      kind: "idle",
      text: t.sources.checkedFailed(at, t.errors[result.category]),
      error: result.message,
    };
  }
  return { kind: "idle", text: t.sources.checkedNone(at) };
}

/** The newest last check among the sources, or null when none was checked yet. */
export function newestCheck(sources: SourceRow[]): number | null {
  let newest: number | null = null;
  for (const source of sources) {
    if (source.lastCheckedAt != null && (newest == null || source.lastCheckedAt > newest)) {
      newest = source.lastCheckedAt;
    }
  }
  return newest;
}

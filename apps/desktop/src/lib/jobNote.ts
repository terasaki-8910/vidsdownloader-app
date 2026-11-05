import type { JobRow } from "../ipc/contract";
import type { Translations } from "../i18n/types";

/** A Done job with a file on disk: shown as saved, whatever `skipped` says. */
export function hasSavedFile(job: JobRow): boolean {
  return job.state === "Done" && Boolean(job.outputPath);
}

/**
 * The text crates/core/src/executor.rs puts in `skipped` when a page-image job saved some of
 * its images but not all ("2 of 6 images could not be saved"). Keep it in sync with that file.
 */
const IMAGES_NOT_SAVED = /^(\d+) of (\d+) images could not be saved$/;

/**
 * The note shown after a saved row's done line: `skipped` text on a Done job that has a file.
 * The page-image text is translated; any other text is shown as is.
 */
export function savedNote(skipped: string, t: Translations): string {
  const match = IMAGES_NOT_SAVED.exec(skipped);
  return match ? t.queue.imagesNotSaved(Number(match[1]), Number(match[2])) : skipped;
}

type SkipReasonText = Translations["queue"]["skipReason"];

/** "10 min" when the seconds are a whole number of minutes, else "45 s". */
function skipDuration(seconds: number, text: SkipReasonText): string {
  return seconds % 60 === 0 ? text.minutes(seconds / 60) : text.seconds(seconds);
}

/**
 * One reason each, matched against the whole part. The English texts are written by
 * `skip_reason` in crates/core/src/executor.rs (the last one is set in the same
 * file when yt-dlp's own match filter rejects the video). Rows already stored in users'
 * databases carry these texts, so the UI translates them instead of the core changing them.
 * Keep the patterns in sync with that file.
 */
const SKIP_REASONS: ReadonlyArray<readonly [RegExp, (m: RegExpExecArray, text: SkipReasonText) => string]> = [
  [/^shorter than (\d+) s$/, (m, text) => text.shorterThan(skipDuration(Number(m[1]), text))],
  [/^longer than (\d+) s$/, (m, text) => text.longerThan(skipDuration(Number(m[1]), text))],
  [/^published before (.+)$/, (m, text) => text.publishedBefore(m[1])],
  [/^published after (.+)$/, (m, text) => text.publishedAfter(m[1])],
  [/^title does not include any of: (.+)$/, (m, text) => text.titleIncludesNone(m[1].split(", "))],
  [/^title includes excluded text: (.+)$/, (m, text) => text.titleIncludesExcluded(m[1])],
  [/^not a video$/, (_m, text) => text.notVideo],
  [/^live or upcoming$/, (_m, text) => text.liveOrUpcoming],
  [/^already downloaded$/, (_m, text) => text.alreadyDownloaded],
  [/^rejected by yt-dlp's match filter$/, (_m, text) => text.matchFilter],
];

/**
 * The reason shown in 「スキップ（…）」 / "Skipped (…)": `skipped` text of a job a filter
 * skipped, in the UI language. The texts come from `skip_reason` in
 * crates/core/src/executor.rs (see SKIP_REASONS); the Rust side is not changed. The core joins
 * several reasons with "; ", so each part is translated on its own and the parts are rejoined;
 * a part that is not a known text stays as it is, and a text with no known part comes back
 * unchanged.
 */
export function skipReasonText(skipped: string, t: Translations): string {
  const text = t.queue.skipReason;
  let known = false;
  const parts = skipped.split("; ").map((part) => {
    for (const [pattern, translate] of SKIP_REASONS) {
      const match = pattern.exec(part);
      if (match) {
        known = true;
        return translate(match, text);
      }
    }
    return part;
  });
  return known ? parts.join(text.separator) : skipped;
}

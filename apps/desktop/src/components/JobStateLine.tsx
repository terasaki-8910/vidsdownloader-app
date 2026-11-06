import { AlertTriangle, Check, ListPlus } from "lucide-react";
import type { JobRow } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { formatClockDuration, formatSpeed } from "../lib/format";
import { hasSavedFile, savedNote, skipReasonText } from "../lib/jobNote";
import styles from "./JobStateLine.module.css";

export const COMMENTS_ONLY_UNSUPPORTED = "Only niconico and Bilibili have comments to extract";

/**
 * The row's (and the detail panel's) state line: progress while active, a
 * moving segment while probing/post-processing, plain status text while
 * queued/canceled, the short per-category error while failed, or the saved
 * path while done. Shared so the queue row and the detail panel never
 * disagree about what a given job's state looks like.
 */
export function JobStateLine({ job, showPath = true }: { job: JobRow; showPath?: boolean }) {
  const t = useT();

  if (job.alreadySaved) {
    const { savedAt, fileMissing } = job.alreadySaved;
    const detail = fileMissing
      ? t.queue.fileMissing
      : savedAt != null
        ? t.queue.savedOn(savedAt, new Date())
        : null;
    return (
      <div className={styles.state}>
        <span>{detail ? `${t.queue.alreadySaved} · ${detail}` : t.queue.alreadySaved}</span>
      </div>
    );
  }

  // A skipped job with no file keeps this line; one that saved a file shows as saved below.
  if (job.skipped && !hasSavedFile(job)) {
    return (
      <div className={styles.state}>
        <span>{t.queue.skipped(skipReasonText(job.skipped, t))}</span>
      </div>
    );
  }

  switch (job.state) {
    case "Downloading": {
      const percent = job.progress == null ? null : Math.round(job.progress * 100);
      return (
        <div className={styles.prog}>
          <div
            className={`${styles.bar} ${percent == null ? styles.indeterminate : ""}`}
            role="progressbar"
            aria-label={t.queue.downloading}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent ?? undefined}
          >
            <span style={percent == null ? undefined : { width: `${percent}%` }} />
          </div>
          <div className={styles.pstat}>
            <span>{percent == null ? t.queue.downloading : `${percent}%`}</span>
            {job.speedBytesPerS != null && <span>{formatSpeed(job.speedBytesPerS)}</span>}
            {job.etaS != null && <span>{t.queue.remaining(formatClockDuration(job.etaS))}</span>}
          </div>
        </div>
      );
    }

    case "Probing":
    case "PostProcessing":
      return (
        <div className={styles.prog}>
          <div className={`${styles.bar} ${styles.busy}`}>
            <span />
          </div>
          <div className={styles.pstat}>
            <span>{job.phase === "pageSearch" ? t.queue.pageSearch : job.state === "Probing" ? t.queue.probing : t.queue.postProcessing}</span>
          </div>
        </div>
      );

    case "Queued":
    case "Ready":
      return (
        <div className={styles.state}>
          <span>{t.queue.queued}</span>
        </div>
      );

    case "Failed":
      // Waiting for the user's choice, not a failure: the plain line, like a skipped job,
      // with the row's 「選ぶ」 button as the action.
      if (job.error?.category === "PageMediaChoice" && job.pageMediaChoices.length > 0) {
        return (
          <div className={styles.state}>
            <span>{t.pageMedia.foundCount(job.pageMediaChoices.length)}</span>
          </div>
        );
      }
      return (
        <div className={`${styles.state} ${styles.err}`}>
          <AlertTriangle size={14} aria-hidden />
          <span>{job.error?.message === COMMENTS_ONLY_UNSUPPORTED ? t.queue.commentsOnlyUnsupported : job.error ? t.errors[job.error.category] : t.errors.Unknown}</span>
        </div>
      );

    case "Canceled":
      return (
        <div className={styles.state}>
          <span>{t.queue.canceled}</span>
        </div>
      );

    case "Done":
      if (job.commentsCount != null) {
        return <div className={styles.state}><Check size={14} aria-hidden /><span>{t.queue.commentsOnly(job.commentsCount)}</span>{showPath && job.outputPath && <span className={styles.path}>{job.outputPath}</span>}</div>;
      }
      if (job.listCount != null) {
        return (
          <div className={styles.state}>
            <ListPlus size={14} aria-hidden />
            <span>{t.queue.listAdded(job.listCount, job.listTotal)}</span>
          </div>
        );
      }
      return (
        <div className={styles.state}>
          <Check size={14} aria-hidden />
          <span>{t.queue.done}</span>
          {job.skipped && <span>{`· ${savedNote(job.skipped, t)}`}</span>}
          {showPath && job.outputPath && <span className={styles.path}>{job.outputPath}</span>}
        </div>
      );

    default:
      return null;
  }
}

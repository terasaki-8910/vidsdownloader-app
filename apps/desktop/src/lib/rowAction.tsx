import { FolderOpen, RotateCcw, Square, X } from "lucide-react";
import type { ReactNode } from "react";
import type { JobRow, LibraryRow } from "../ipc/contract";
import type { Translations } from "../i18n/types";
import { hasSavedFile } from "./jobNote";

export interface RowAction {
  /**
   * "icon": an icon button. "login": a tinted text button (the row stopped for a login).
   * "text": a quiet text button in the same fixed spot, for a low-frequency choice.
   */
  kind: "icon" | "login" | "text";
  icon: ReactNode;
  label: string;
  onClick: () => void;
}

export interface RowActionHandlers {
  onCancel: () => void;
  onRetry: () => void;
  onReveal: (path: string) => void;
  /** Starts a login for the site of a row that stopped with LoginRequired. */
  onLogin: () => void;
  /** Downloads a video that was skipped because it is already saved. */
  onSaveAgain: () => void;
  /** Opens the persisted page-media choice dialog. */
  onChooseMedia: () => void;
  /** Opens this job's original page in the in-app browser. */
  onOpenInBrowser?: () => void;
}

const WAITING_STATES: JobRow["state"][] = ["Queued", "Ready"];
const BUSY_STATES: JobRow["state"][] = ["Downloading", "Probing", "PostProcessing"];

export function isCommentsSite(site: string | null | undefined): boolean {
  return /ニコニコ|niconico|bilibili/i.test(site ?? "");
}

export function libraryCanExtractComments(row: LibraryRow): boolean {
  return !row.missing && isCommentsSite(row.site);
}

export function jobCanExtractComments(job: JobRow): boolean {
  return job.state === "Done" && job.libraryId != null && Boolean(job.outputPath) && isCommentsSite(job.details?.site);
}

export type JobSelectionAction = "stop" | "retry" | "trash" | "remove";

export interface JobSelectionActionGroup {
  action: JobSelectionAction;
  jobs: JobRow[];
  danger?: boolean;
}

/**
 * The multi-row actions and their exact targets. DownloadsView uses this one result for
 * both the selection bar and the multi-row menu, so their counts cannot drift apart.
 */
export function getJobSelectionActions(jobs: readonly JobRow[]): JobSelectionActionGroup[] {
  const groups: JobSelectionActionGroup[] = [
    { action: "stop", jobs: jobs.filter((job) => BUSY_STATES.includes(job.state)) },
    { action: "retry", jobs: jobs.filter((job) => job.state === "Failed" || job.state === "Canceled") },
    { action: "trash", jobs: jobs.filter((job) => job.state === "Done" && job.libraryId != null), danger: true },
    { action: "remove", jobs: jobs.filter((job) => !BUSY_STATES.includes(job.state)) },
  ];
  return groups.filter((group) => group.jobs.length > 0);
}

export function jobCanBeRemoved(job: JobRow): boolean {
  return !BUSY_STATES.includes(job.state);
}

export function jobIsBusy(job: JobRow): boolean {
  return BUSY_STATES.includes(job.state);
}

/**
 * The one contextual action offered for a job, shared by the queue row and
 * the detail panel so they never disagree: stop while busy, remove while
 * waiting, log in when it stopped for a login (retried automatically once the
 * login is done), retry once stopped, reveal once saved, save again when it was
 * skipped because the video is already saved -- nothing once skipped any other way. A Done job
 * that has a file counts as saved even with a `skipped` note (a page-image job that saved only
 * some of its images), so it keeps the reveal action.
 */
export function getRowAction(job: JobRow, t: Translations, handlers: RowActionHandlers): RowAction | null {
  if (job.alreadySaved && job.state === "Done") {
    return { kind: "text", icon: null, label: t.common.saveAgain, onClick: handlers.onSaveAgain };
  }
  if ((job.skipped && !hasSavedFile(job)) || job.listCount != null) return null;

  if (BUSY_STATES.includes(job.state)) {
    return { kind: "icon", icon: <Square size={16} aria-hidden />, label: t.common.stop, onClick: handlers.onCancel };
  }
  if (WAITING_STATES.includes(job.state)) {
    return {
      kind: "icon",
      icon: <X size={16} aria-hidden />,
      label: t.common.removeFromQueue,
      onClick: handlers.onCancel,
    };
  }
  if (job.state === "Failed" && job.error?.category === "LoginRequired") {
    return { kind: "login", icon: null, label: t.common.login, onClick: handlers.onLogin };
  }
  if (
    job.state === "Failed" &&
    (job.error?.category === "PageMediaNotFound" || job.error?.category === "PageGate") &&
    handlers.onOpenInBrowser
  ) {
    return { kind: "text", icon: null, label: t.siteReport.openInBrowser, onClick: handlers.onOpenInBrowser };
  }
  if (job.state === "Failed" && job.error?.category === "PageMediaChoice" && job.pageMediaChoices.length > 0) {
    return { kind: "text", icon: null, label: t.pageMedia.choose, onClick: handlers.onChooseMedia };
  }
  if (job.state === "Failed" || job.state === "Canceled") {
    return { kind: "icon", icon: <RotateCcw size={16} aria-hidden />, label: t.common.retry, onClick: handlers.onRetry };
  }
  if (job.state === "Done" && job.outputPath) {
    const path = job.outputPath;
    return {
      kind: "icon",
      icon: <FolderOpen size={16} aria-hidden />,
      label: t.common.revealInFolder,
      onClick: () => handlers.onReveal(path),
    };
  }
  return null;
}

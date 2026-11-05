// The in-app update dialog (round 11, as sampled on spike/round11-samples), in ConfirmDialog's
// look (the same <dialog> and CSS module): a modal <dialog> with role "dialog"; Escape and a
// backdrop click are あとで. Phases (UpdatesContext):
//   offer        この版はスキップ / あとで / 今すぐ更新; 今すぐ更新 is off, with the reason under
//                the buttons, while jobs download or post-process
//   downloading  after 今すぐ更新: a bar with the megabytes; the dialog can no longer be closed
//   failed       one line saying why, and 閉じる
// Opening focuses あとで: the dialog appears by itself at launch, so a stray Enter must not start
// an update that restarts the app. Mounted by App, so every entry point shares it.
import { useEffect, useId, useMemo, useRef, useState, type MouseEvent, type RefObject } from "react";
import { useT } from "../i18n/I18nContext";
import { parseReleaseNotes } from "../lib/releaseNotes";
import { useUpdates, type UpdateDialogState } from "../state/UpdatesContext";
import dialogStyles from "./ConfirmDialog.module.css";
import jobStyles from "./JobStateLine.module.css";
import styles from "./UpdateDialog.module.css";

export function UpdateDialogHost() {
  const { dialog } = useUpdates();
  return dialog ? <UpdateDialog state={dialog} /> : null;
}

const MB = 1024 * 1024;

function megabytes(bytes: number): string {
  return (bytes / MB).toFixed(1);
}

function UpdateDialog({ state }: { state: UpdateDialogState }) {
  const t = useT();
  const { runningJobs, skip, later, updateNow, closeFailed } = useUpdates();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const laterRef = useRef<HTMLButtonElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const progressRef = useRef<HTMLDivElement>(null);
  const notesRef = useRef<HTMLDivElement>(null);
  /** Which way out the next "close" event means; Escape and the backdrop leave it at "later". */
  const dismissal = useRef<"later" | "skip">("later");
  const titleId = useId();
  const versionId = useId();
  const notesId = useId();
  const busyId = useId();
  const statusId = useId();
  const errorId = useId();

  const { phase, update } = state;
  const locked = phase === "downloading";
  const busy = phase === "offer" && runningJobs > 0;
  const blocks = useMemo(() => parseReleaseNotes(update.notes), [update.notes]);
  const [scrolls, setScrolls] = useState(false);

  // Open as a modal and move focus; again whenever the phase changes (the buttons go away in
  // the downloading phase, so focus moves to the progress block).
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (!dialog.open) dialog.showModal();
    if (phase === "downloading") progressRef.current?.focus();
    else if (phase === "failed") closeRef.current?.focus();
    else laterRef.current?.focus();
  }, [phase]);

  // A long list scrolls inside the dialog; only then does it get rules and a tab stop.
  useEffect(() => {
    const el = notesRef.current;
    if (!el) return undefined;
    const measure = () => setScrolls(el.scrollHeight > el.clientHeight + 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [phase, blocks]);

  function close(how: "later" | "skip") {
    dismissal.current = how;
    dialogRef.current?.close();
  }

  return (
    <dialog
      ref={dialogRef}
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
      aria-describedby={locked ? statusId : phase === "failed" ? errorId : versionId}
      className={dialogStyles.dialog}
      // Escape. While downloading the dialog cannot be dismissed.
      onCancel={(e) => {
        if (locked) e.preventDefault();
      }}
      // Chromium (so WebView2 too) closes a modal dialog on a second Escape even if the first
      // cancel event was prevented: put it back.
      onClose={() => {
        if (locked) {
          dialogRef.current?.showModal();
          progressRef.current?.focus();
          return;
        }
        const how = dismissal.current;
        dismissal.current = "later";
        if (phase === "failed") closeFailed();
        else if (how === "skip") skip();
        else later();
      }}
      onClick={(e: MouseEvent<HTMLDialogElement>) => {
        if (!locked && e.target === dialogRef.current) close("later");
      }}
    >
      <div className={dialogStyles.inner}>
        <h2 id={titleId} className={dialogStyles.title}>
          {locked ? t.updates.updatingTitle : t.updates.title}
        </h2>
        <p id={versionId} className={`${dialogStyles.body} ${styles.version}`}>
          {t.updates.newVersion(update.version)}
          <span className={styles.current}>{t.updates.runningVersion(update.currentVersion)}</span>
        </p>

        {state.phase === "downloading" ? (
          <DownloadProgress
            progressRef={progressRef}
            statusId={statusId}
            downloadedBytes={state.downloadedBytes}
            totalBytes={state.totalBytes}
          />
        ) : state.phase === "failed" ? (
          <>
            <p id={errorId} className={styles.error}>
              {t.updates.failed(state.message)}
            </p>
            <div className={styles.footer}>
              <button type="button" ref={closeRef} className={dialogStyles.cancel} onClick={() => close("later")}>
                {t.common.close}
              </button>
            </div>
          </>
        ) : (
          <>
            {blocks.length > 0 && (
              <>
                <h3 id={notesId} className={styles.notesHeading}>
                  {t.updates.notesHeading}
                </h3>
                <div
                  ref={notesRef}
                  role="region"
                  aria-labelledby={notesId}
                  tabIndex={scrolls ? 0 : undefined}
                  className={`${styles.notes} ${scrolls ? styles.scrolls : ""}`}
                >
                  {blocks.map((block, index) =>
                    block.kind === "list" ? (
                      <ul key={index} className={styles.list}>
                        {block.items.map((item, itemIndex) => (
                          <li key={itemIndex}>{item}</li>
                        ))}
                      </ul>
                    ) : (
                      <p key={index} className={styles.paragraph}>
                        {block.text}
                      </p>
                    ),
                  )}
                </div>
              </>
            )}

            <div className={styles.footer}>
              <button type="button" className={styles.skip} onClick={() => close("skip")}>
                {t.updates.skip}
              </button>
              <button type="button" ref={laterRef} className={dialogStyles.cancel} onClick={() => close("later")}>
                {t.updates.later}
              </button>
              <button
                type="button"
                className={styles.primary}
                // Kept focusable so that its reason is read out; the click does nothing.
                aria-disabled={busy ? "true" : undefined}
                aria-describedby={busy ? busyId : undefined}
                onClick={() => {
                  if (!busy) updateNow();
                }}
              >
                {t.updates.updateNow}
              </button>
            </div>
            {busy && (
              <p id={busyId} className={styles.busy}>
                {t.updates.busy(runningJobs)}
              </p>
            )}
          </>
        )}
      </div>
    </dialog>
  );
}

function DownloadProgress({
  progressRef,
  statusId,
  downloadedBytes,
  totalBytes,
}: {
  progressRef: RefObject<HTMLDivElement | null>;
  statusId: string;
  downloadedBytes: number;
  totalBytes: number | null;
}) {
  const t = useT();
  const known = totalBytes != null && totalBytes > 0;
  const done = known ? Math.min(downloadedBytes, totalBytes) : downloadedBytes;
  const figures = known ? `${megabytes(done)} / ${megabytes(totalBytes)} MB` : `${megabytes(done)} MB`;

  return (
    <div ref={progressRef} tabIndex={-1} className={styles.progress}>
      <div
        className={`${jobStyles.bar} ${known ? "" : jobStyles.indeterminate}`}
        role="progressbar"
        aria-label={t.updates.progressLabel}
        aria-valuemin={known ? 0 : undefined}
        aria-valuemax={known ? Number(megabytes(totalBytes)) : undefined}
        aria-valuenow={known ? Number(megabytes(done)) : undefined}
        aria-valuetext={figures}
      >
        <span style={known ? { width: `${(done / totalBytes) * 100}%` } : undefined} />
      </div>
      <p id={statusId} className={styles.status}>
        {t.updates.downloading} <span className={styles.figures}>{figures}</span>
      </p>
      <p className={styles.after}>{t.updates.afterDownload}</p>
    </div>
  );
}

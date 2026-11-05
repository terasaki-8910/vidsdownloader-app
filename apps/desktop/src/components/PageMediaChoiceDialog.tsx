import { useEffect, useId, useRef, useState, type MouseEvent } from "react";
import type { JobRow } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { formatClockDuration } from "../lib/format";
import { errorMessage } from "../lib/url";
import dialogStyles from "./ConfirmDialog.module.css";
import styles from "./PageMediaChoiceDialog.module.css";

interface Props {
  job: JobRow;
  onCancel: () => void;
  onSave: (url: string) => Promise<void>;
}

function lastPathSegment(url: string): string {
  try {
    const segments = new URL(url).pathname.split("/").filter(Boolean);
    const segment = segments[segments.length - 1] ?? url;
    return decodeURIComponent(segment);
  } catch {
    return url;
  }
}

export function PageMediaChoiceDialog({ job, onCancel, onSave }: Props) {
  const t = useT();
  const [choice, setChoice] = useState(job.pageMediaChoices[0]?.url ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const firstRef = useRef<HTMLInputElement>(null);
  const savedRef = useRef(false);
  const titleId = useId();

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog || dialog.open) return;
    dialog.showModal();
    firstRef.current?.focus();
  }, []);

  function cancel() {
    if (!busy) dialogRef.current?.close();
  }

  async function save() {
    if (!choice || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onSave(choice);
      savedRef.current = true;
      dialogRef.current?.close();
    } catch (cause) {
      setError(t.library.failed(errorMessage(cause)));
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={dialogRef}
      aria-labelledby={titleId}
      className={dialogStyles.dialog}
      onCancel={(event) => { if (busy) event.preventDefault(); }}
      onClose={() => { if (!savedRef.current) onCancel(); }}
      onClick={(event: MouseEvent<HTMLDialogElement>) => { if (event.target === dialogRef.current) cancel(); }}
    >
      <div className={dialogStyles.inner}>
        <h2 id={titleId} className={dialogStyles.title}>{t.pageMedia.title}</h2>
        <fieldset className={styles.choices} aria-labelledby={titleId}>
          {job.pageMediaChoices.map((media, index) => (
            <label key={media.url} className={styles.choice}>
              <input
                ref={index === 0 ? firstRef : undefined}
                type="radio"
                name="page-media-choice"
                value={media.url}
                checked={choice === media.url}
                disabled={busy}
                onChange={() => setChoice(media.url)}
              />
              <span className={styles.details}>
                <span className={styles.meta}>{media.kind} · {media.durationSeconds == null ? t.pageMedia.unknownLength : formatClockDuration(media.durationSeconds)}</span>
                <span className={styles.url} title={media.url}>{lastPathSegment(media.url)}</span>
              </span>
            </label>
          ))}
        </fieldset>
        {error && <p role="alert" className={styles.error}>{error}</p>}
        <div className={dialogStyles.buttons}>
          <button type="button" className={styles.cancel} disabled={busy} onClick={cancel}>{t.common.dialogCancel}</button>
          <button type="button" className={dialogStyles.cancel} disabled={busy || !choice} onClick={() => void save()}>{t.pageMedia.save}</button>
        </div>
      </div>
    </dialog>
  );
}

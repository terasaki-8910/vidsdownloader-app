import { X } from "lucide-react";
import { useEffect, useRef, useState, type MouseEvent } from "react";
import type { JobRow, Preset } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import type { RowActionHandlers } from "../lib/rowAction";
import { DetailBody } from "./DetailBody";
import { IconButton } from "./IconButton";
import styles from "./DetailSheet.module.css";

interface DetailSheetProps extends RowActionHandlers {
  job: JobRow | null;
  preset: Preset | undefined;
  open: boolean;
  onClose: () => void;
}

/**
 * The narrow-width (<1320px) equivalent of the detail panel: an overlay sheet
 * sliding in from the right, with light dismiss (Escape + backdrop click) and
 * an explicit close button. Built on <dialog> so Escape, the backdrop and
 * focus containment come for free; a single native "close" event is the only
 * path that calls onClose, so every dismissal route (button, backdrop,
 * Escape) stays in sync.
 */
export function DetailSheet({
  job,
  preset,
  open,
  onClose,
  onCancel,
  onRetry,
  onReveal,
  onLogin,
  onSaveAgain,
  onChooseMedia,
}: DetailSheetProps) {
  const t = useT();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [entered, setEntered] = useState(false);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    if (open && job) {
      if (!dialog.open) dialog.showModal();
      const raf = requestAnimationFrame(() => setEntered(true));
      return () => cancelAnimationFrame(raf);
    }

    setEntered(false);
    if (dialog.open) dialog.close();
    return undefined;
  }, [open, job]);

  if (!job) return null;

  const title = job.details?.title ?? job.url;

  return (
    <dialog
      ref={dialogRef}
      className={`${styles.sheet} ${entered ? styles.entered : ""}`}
      aria-label={title}
      onClose={onClose}
      onClick={(e: MouseEvent<HTMLDialogElement>) => {
        if (e.target === dialogRef.current) dialogRef.current?.close();
      }}
    >
      <div className={styles.inner}>
        <div className={styles.header}>
          <IconButton label={t.common.close} onClick={() => dialogRef.current?.close()}>
            <X size={16} aria-hidden />
          </IconButton>
        </div>
        <div className={styles.content}>
          <DetailBody
            job={job}
            preset={preset}
            onCancel={onCancel}
            onRetry={onRetry}
            onReveal={onReveal}
            onLogin={onLogin}
            onSaveAgain={onSaveAgain}
            onChooseMedia={onChooseMedia}
          />
        </div>
      </div>
    </dialog>
  );
}

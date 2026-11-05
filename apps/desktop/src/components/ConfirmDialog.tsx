// A confirmation for a destructive action. A <dialog> opened with showModal(), so Escape,
// focus containment and the backdrop come for free; role "alertdialog" because the person
// has to answer. Focus starts on the safe button. The backdrop click and Escape both
// cancel (the native "close" event is the only path that calls onCancel, the same pattern
// as DetailSheet); closing returns focus to the control that opened it.
import { useEffect, useId, useRef, type MouseEvent } from "react";
import styles from "./ConfirmDialog.module.css";

interface ConfirmDialogProps {
  title: string;
  body: string;
  cancelLabel: string;
  confirmLabel: string;
  onCancel: () => void;
  onConfirm: () => void;
}

export function ConfirmDialog({ title, body, cancelLabel, confirmLabel, onCancel, onConfirm }: ConfirmDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const bodyId = useId();

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog || dialog.open) return;
    dialog.showModal();
    cancelRef.current?.focus();
  }, []);

  return (
    <dialog
      ref={dialogRef}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby={titleId}
      aria-describedby={bodyId}
      className={styles.dialog}
      onClose={onCancel}
      onClick={(e: MouseEvent<HTMLDialogElement>) => {
        if (e.target === dialogRef.current) dialogRef.current?.close();
      }}
    >
      <div className={styles.inner}>
        <h2 id={titleId} className={styles.title}>
          {title}
        </h2>
        <p id={bodyId} className={styles.body}>
          {body}
        </p>
        <div className={styles.buttons}>
          <button ref={cancelRef} type="button" className={styles.cancel} onClick={() => dialogRef.current?.close()}>
            {cancelLabel}
          </button>
          <button type="button" className={styles.destructive} onClick={onConfirm}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </dialog>
  );
}

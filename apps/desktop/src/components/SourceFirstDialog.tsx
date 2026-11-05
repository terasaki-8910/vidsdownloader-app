// The question right after a registration is looked up: what to do with the items there now.
// In ConfirmDialog's look (a modal <dialog> with role alertdialog, focus on the first, safe
// button, Escape and a backdrop click cancel) but with three buttons, so it reuses
// ConfirmDialog's CSS module rather than the component. It reads the question from
// SourcesContext and is mounted by App, so every entry point shares it.
import { useEffect, useId, useRef, type MouseEvent } from "react";
import { useT } from "../i18n/I18nContext";
import { useSources, type PendingFirstCheck } from "../state/SourcesContext";
import dialogStyles from "./ConfirmDialog.module.css";
import styles from "./SourceFirstDialog.module.css";

export function SourceFirstDialog() {
  const { firstCheck } = useSources();
  return firstCheck ? <FirstCheckDialog check={firstCheck} /> : null;
}

function FirstCheckDialog({ check }: { check: PendingFirstCheck }) {
  const t = useT();
  const { answerFirstCheck, cancelFirstCheck } = useSources();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const firstRef = useRef<HTMLButtonElement>(null);
  const answer = useRef<"record" | "saveAll" | null>(null);
  const titleId = useId();
  const bodyId = useId();

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog || dialog.open) return;
    dialog.showModal();
    firstRef.current?.focus();
  }, []);

  /** Every way out ends in the dialog's "close" event, which is where the answer is passed on. */
  function close(choice: "record" | "saveAll" | null) {
    if (answer.current === null) answer.current = choice;
    dialogRef.current?.close();
  }

  return (
    <dialog
      ref={dialogRef}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby={titleId}
      aria-describedby={bodyId}
      className={dialogStyles.dialog}
      onClose={() => (answer.current ? answerFirstCheck(answer.current) : cancelFirstCheck())}
      onClick={(e: MouseEvent<HTMLDialogElement>) => {
        if (e.target === dialogRef.current) close(null);
      }}
    >
      <div className={dialogStyles.inner}>
        <h2 id={titleId} className={dialogStyles.title}>
          {t.sources.firstCheckTitle(check.kind, check.count)}
        </h2>
        <p id={bodyId} className={dialogStyles.body}>
          {t.sources.firstCheckBody(check.kind, check.name)}
        </p>
        <div className={dialogStyles.buttons}>
          {/* The safe choice first and tinted, like ConfirmDialog's Cancel. */}
          <button ref={firstRef} type="button" className={dialogStyles.cancel} onClick={() => close("record")}>
            {t.sources.recordOnly}
          </button>
          <button type="button" className={styles.saveAll} onClick={() => close("saveAll")}>
            {t.sources.saveAll}
          </button>
          <button type="button" className={styles.cancel} onClick={() => close(null)}>
            {t.common.dialogCancel}
          </button>
        </div>
      </div>
    </dialog>
  );
}

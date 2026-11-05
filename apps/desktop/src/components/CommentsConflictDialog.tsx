import { useEffect, useId, useRef, useState, type MouseEvent } from "react";
import { useT } from "../i18n/I18nContext";
import dialogStyles from "./ConfirmDialog.module.css";
import styles from "./CommentsConflictDialog.module.css";

export interface CommentsConflictChoice {
  choice: "existing" | "new";
  applyRemaining: boolean;
}

interface Props {
  fileName: string;
  existingCount: number;
  existingDate: string;
  newCount: number;
  remaining: number;
  onCancel: () => void;
  onDecide: (answer: CommentsConflictChoice) => void;
}

export function CommentsConflictDialog({ fileName, existingCount, existingDate, newCount, remaining, onCancel, onDecide }: Props) {
  const t = useT();
  const initialChoice = newCount > existingCount ? "new" : "existing";
  const [choice, setChoice] = useState<"existing" | "new">(initialChoice);
  const [applyRemaining, setApplyRemaining] = useState(false);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const existingRef = useRef<HTMLInputElement>(null);
  const newRef = useRef<HTMLInputElement>(null);
  const answerRef = useRef<CommentsConflictChoice | null>(null);
  const titleId = useId();
  const fileId = useId();

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog || dialog.open) return;
    dialog.showModal();
    (initialChoice === "new" ? newRef : existingRef).current?.focus();
  }, [initialChoice]);

  function close(answer: CommentsConflictChoice | null) {
    answerRef.current = answer;
    dialogRef.current?.close();
  }

  return (
    <dialog ref={dialogRef} role="alertdialog" aria-modal="true" aria-labelledby={titleId} aria-describedby={fileId} className={dialogStyles.dialog} onClose={() => (answerRef.current ? onDecide(answerRef.current) : onCancel())} onClick={(event: MouseEvent<HTMLDialogElement>) => { if (event.target === dialogRef.current) close(null); }}>
      <div className={dialogStyles.inner}>
        <h2 id={titleId} className={dialogStyles.title}>{t.library.commentsConflictTitle}</h2>
        <p id={fileId} className={styles.fileName}>{fileName}</p>
        <fieldset className={styles.choices} aria-labelledby={titleId}>
          <label className={styles.choice}>
            <input ref={existingRef} type="radio" name="comments-conflict" value="existing" checked={choice === "existing"} onChange={() => setChoice("existing")} />
            <span><span className={styles.choiceTitle}>{t.library.keepExistingComments}{existingCount > newCount && <span className={styles.more}>{t.library.moreComments}</span>}</span><span className={styles.choiceMeta}>{t.library.commentsOldMeta(existingCount, existingDate)}</span></span>
          </label>
          <label className={styles.choice}>
            <input ref={newRef} type="radio" name="comments-conflict" value="new" checked={choice === "new"} onChange={() => setChoice("new")} />
            <span><span className={styles.choiceTitle}>{t.library.useNewComments}{newCount > existingCount && <span className={styles.more}>{t.library.moreComments}</span>}</span><span className={styles.choiceMeta}>{t.library.commentsNewMeta(newCount)}</span></span>
          </label>
        </fieldset>
        {remaining > 0 && <label className={styles.applyRemaining}><input type="checkbox" checked={applyRemaining} onChange={(event) => setApplyRemaining(event.target.checked)} /><span>{t.library.commentsApplyRemaining(remaining)}</span></label>}
        <div className={dialogStyles.buttons}>
          <button type="button" className={styles.cancel} onClick={() => close(null)}>{t.common.dialogCancel}</button>
          <button type="button" className={dialogStyles.cancel} onClick={() => close({ choice, applyRemaining })}>{t.library.commentsDecide}</button>
        </div>
      </div>
    </dialog>
  );
}

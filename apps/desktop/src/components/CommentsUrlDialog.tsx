import { useEffect, useId, useRef, useState, type FormEvent, type MouseEvent } from "react";
import { useT } from "../i18n/I18nContext";
import dialogStyles from "./ConfirmDialog.module.css";
import styles from "./CommentsUrlDialog.module.css";

export function CommentsUrlDialog({ onCancel, onSubmit }: { onCancel: () => void; onSubmit: (url: string) => void }) {
  const t = useT();
  const [url, setUrl] = useState("");
  const [error, setError] = useState(false);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const submitted = useRef(false);
  const titleId = useId();
  const bodyId = useId();

  useEffect(() => {
    dialogRef.current?.showModal();
    inputRef.current?.focus();
  }, []);

  function submit(event: FormEvent) {
    event.preventDefault();
    let parsed: URL;
    try { parsed = new URL(url.trim()); } catch { setError(true); return; }
    if (!/(^|\.)(nicovideo\.jp|bilibili\.com)$/i.test(parsed.hostname)) { setError(true); return; }
    submitted.current = true;
    onSubmit(parsed.href);
  }

  return (
    <dialog ref={dialogRef} aria-labelledby={titleId} aria-describedby={bodyId} className={dialogStyles.dialog} onClose={() => { if (!submitted.current) onCancel(); }} onClick={(event: MouseEvent<HTMLDialogElement>) => { if (event.target === dialogRef.current) dialogRef.current?.close(); }}>
      <form className={dialogStyles.inner} onSubmit={submit}>
        <h2 id={titleId} className={dialogStyles.title}>{t.library.commentsUrlTitle}</h2>
        <p id={bodyId} className={dialogStyles.body}>{t.library.commentsUrlBody}</p>
        <input ref={inputRef} type="url" className={styles.input} value={url} placeholder={t.library.commentsUrlPlaceholder} aria-label={t.library.commentsUrlPlaceholder} aria-invalid={error || undefined} onChange={(event) => { setUrl(event.target.value); setError(false); }} />
        {error && <p role="alert" className={styles.error}>{t.library.commentsUrlInvalid}</p>}
        <div className={dialogStyles.buttons}>
          <button type="button" className={styles.plain} onClick={() => dialogRef.current?.close()}>{t.common.dialogCancel}</button>
          <button type="submit" className={dialogStyles.cancel}>{t.library.commentsExtract}</button>
        </div>
      </form>
    </dialog>
  );
}

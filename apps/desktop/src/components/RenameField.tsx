import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { splitFileName } from "../lib/format";
import styles from "./RenameField.module.css";

interface RenameFieldProps {
  path: string;
  label: string;
  emptyError: string;
  badCharsError: string;
  busy: boolean;
  error: string | null;
  onError: (message: string | null) => void;
  onSubmit: (name: string) => void | Promise<void>;
  onCancel: (returnFocus: boolean) => void;
}

/** Inline filename editor shared by saved items and linked queue rows. */
export function RenameField({
  path,
  label,
  emptyError,
  badCharsError,
  busy,
  error,
  onError,
  onSubmit,
  onCancel,
}: RenameFieldProps) {
  const { stem, extension } = useMemo(() => splitFileName(path), [path]);
  const [draft, setDraft] = useState(stem);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  function submit() {
    if (busy) return;
    if (draft === stem) {
      onCancel(true);
      return;
    }
    if (draft.trim() === "") {
      onError(emptyError);
      return;
    }
    if (/[\\/:*?"<>|\u0000-\u001f]/.test(draft)) {
      onError(badCharsError);
      return;
    }
    void onSubmit(draft);
  }

  function handleKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (!busy) onCancel(true);
    }
  }

  return (
    <div className={styles.rename}>
      <input
        ref={inputRef}
        type="text"
        className={styles.input}
        value={draft}
        readOnly={busy}
        spellCheck={false}
        autoComplete="off"
        aria-label={label}
        aria-invalid={error !== null}
        onChange={(e) => {
          setDraft(e.target.value);
          onError(null);
        }}
        onKeyDown={handleKeyDown}
        onBlur={() => {
          if (!busy) onCancel(false);
        }}
      />
      {extension && <span className={styles.extension}>{extension}</span>}
    </div>
  );
}

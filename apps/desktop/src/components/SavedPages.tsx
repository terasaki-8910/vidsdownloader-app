// "Saved pages" on the browser's start page, above the sites: a row per page (site letter, name,
// the full URL cut in the middle) that opens it in the in-app browser, and a "..." menu with
// rename (inline, as the other lists do) and delete.
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { SavedPage } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { nextFrame } from "../lib/frame";
import { SAVED_PAGE_NAME_MAX } from "../lib/savedPages";
import { errorMessage } from "../lib/url";
import { RowMenu, type MenuEntry, type RowMenuHandle } from "./RowMenu";
import renameStyles from "./RenameField.module.css";
import styles from "./BrowserView.module.css";

interface SavedPagesProps {
  pages: SavedPage[];
  /** The letter on a page's icon: its site's first letter. */
  letterFor: (page: SavedPage) => string;
  onOpen: (url: string) => void;
  onRename: (id: number, name: string) => Promise<void>;
  onRemove: (id: number) => Promise<void>;
  /** The last page was removed: the section is gone, so focus goes on elsewhere. */
  onEmptied: () => void;
}

export function SavedPages({ pages, letterFor, onOpen, onRename, onRemove, onEmptied }: SavedPagesProps) {
  const t = useT();
  const listRef = useRef<HTMLUListElement>(null);

  /** The row that takes the place of a removed one gets the focus. */
  async function remove(id: number) {
    const index = pages.findIndex((page) => page.id === id);
    await onRemove(id);
    if (pages.length === 1) {
      onEmptied();
      return;
    }
    await nextFrame();
    const rows = listRef.current?.querySelectorAll<HTMLButtonElement>("li > button");
    rows?.[Math.min(index, rows.length - 1)]?.focus();
  }

  return (
    <section className={styles.savedSection} aria-labelledby="saved-pages-title">
      <h2 id="saved-pages-title" className={styles.startTitle}>
        {t.browser.savedPages}
      </h2>
      <ul className={styles.siteList} ref={listRef}>
        {pages.map((page) => (
          <SavedPageRow
            key={page.id}
            page={page}
            letter={letterFor(page)}
            onOpen={onOpen}
            onRename={onRename}
            onRemove={remove}
          />
        ))}
      </ul>
    </section>
  );
}

interface SavedPageRowProps {
  page: SavedPage;
  letter: string;
  onOpen: (url: string) => void;
  onRename: (id: number, name: string) => Promise<void>;
  onRemove: (id: number) => Promise<void>;
}

function SavedPageRow({ page, letter, onOpen, onRename, onRemove }: SavedPageRowProps) {
  const t = useT();
  const menuRef = useRef<RowMenuHandle>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!renaming) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [renaming]);

  function startRename() {
    setError(null);
    setDraft(page.name);
    setRenaming(true);
  }

  function stopRename(returnFocus: boolean) {
    setRenaming(false);
    setError(null);
    if (returnFocus) void nextFrame().then(() => menuRef.current?.focus());
  }

  async function submitRename() {
    if (busy) return;
    const name = draft.trim();
    if (!name) {
      setError(t.library.nameEmpty);
      return;
    }
    if (name === page.name) {
      stopRename(true);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onRename(page.id, name);
      stopRename(true);
    } catch (failure) {
      setError(t.library.failed(errorMessage(failure)));
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    setError(null);
    try {
      await onRemove(page.id);
    } catch (failure) {
      setError(t.library.failed(errorMessage(failure)));
    }
  }

  function onRenameKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key === "Enter") {
      event.preventDefault();
      void submitRename();
    } else if (event.key === "Escape") {
      event.preventDefault();
      if (!busy) stopRename(true);
    }
  }

  const entries: MenuEntry[] = [
    { kind: "item", label: t.library.rename, onSelect: startRename },
    { kind: "item", label: t.common.delete, danger: true, onSelect: () => void remove() },
  ];

  const icon = (
    <span className={styles.siteIcon} aria-hidden>
      {letter}
    </span>
  );

  return (
    <li className={styles.savedItem} aria-busy={busy}>
      {renaming ? (
        <div className={styles.savedRow}>
          {icon}
          <input
            ref={inputRef}
            type="text"
            className={`${renameStyles.input} ${styles.savedRename}`}
            value={draft}
            readOnly={busy}
            maxLength={SAVED_PAGE_NAME_MAX}
            spellCheck={false}
            autoComplete="off"
            aria-label={t.library.newName}
            aria-invalid={error !== null}
            onChange={(event) => {
              setDraft(event.target.value);
              setError(null);
            }}
            onKeyDown={onRenameKeyDown}
            onBlur={() => {
              if (!busy) stopRename(false);
            }}
          />
        </div>
      ) : (
        <button type="button" className={styles.savedRow} onClick={() => onOpen(page.url)}>
          {icon}
          <span className={styles.siteName}>{page.name}</span>
          <MiddleEllipsis text={page.url} className={styles.savedUrl} />
        </button>
      )}
      <RowMenu
        ref={menuRef}
        label={t.browser.savedMenu}
        entries={entries}
        disabled={busy}
        buttonTabIndex={0}
        onReturnFocus={() => menuRef.current?.focus()}
      />
      {error && (
        <p role="alert" className={styles.savedError}>
          {error}
        </p>
      )}
    </li>
  );
}

/** A URL cut in the middle when it does not fit: the start stays, and so does the tail (the part that tells pages apart). */
function MiddleEllipsis({ text, className }: { text: string; className?: string }) {
  const tail = 14;
  const split = text.length > tail * 2 ? text.length - tail : text.length;
  return (
    <span className={`${styles.middle} ${className ?? ""}`} title={text}>
      <span className={styles.middleHead}>{text.slice(0, split)}</span>
      <span className={styles.middleTail}>{text.slice(split)}</span>
    </span>
  );
}

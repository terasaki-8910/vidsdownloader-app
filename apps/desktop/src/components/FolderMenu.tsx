// The 保存先 control in the URL bar: a menu button listing the recently used download
// folders plus "choose another folder". The chosen folder stays the download folder until
// it is changed again (R7-4 B).
// WAI-ARIA menu button: Enter / Space / ArrowDown open it on the checked item, ArrowUp
// opens it on the last item, ArrowUp / ArrowDown / Home / End move, Escape closes and
// returns to the button, Tab and an outside click close.
import { Check, ChevronDown } from "lucide-react";
import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { useT } from "../i18n/I18nContext";
import { shortenPath } from "../lib/format";
import urlStyles from "./UrlBar.module.css";
import styles from "./FolderMenu.module.css";

interface FolderMenuProps {
  /** The current download folder; the checked item. */
  downloadDir: string;
  /** Recently used folders, most recent first. */
  recentDownloadDirs: string[];
  onChoose: (path: string) => void;
  /** Opens the OS folder picker. */
  onPickOther: () => void;
}

/** The recent list, or just the current folder when it is empty; the current folder is
 *  always present (first) so it can be shown as checked. */
function menuFolders(downloadDir: string, recent: string[]): string[] {
  const list = recent.length > 0 ? recent : [downloadDir];
  const withCurrent = list.includes(downloadDir) ? list : [downloadDir, ...list];
  return withCurrent.filter((path) => path.length > 0);
}

export function FolderMenu({ downloadDir, recentDownloadDirs, onChoose, onPickOther }: FolderMenuProps) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const headingId = useId();
  const menuId = useId();
  const focusTarget = useRef<"checked" | "last">("checked");

  const folders = menuFolders(downloadDir, recentDownloadDirs);

  function items(): HTMLButtonElement[] {
    return Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("[role^='menuitem']") ?? []);
  }

  function close(returnFocus: boolean) {
    setOpen(false);
    if (returnFocus) buttonRef.current?.focus();
  }

  useEffect(() => {
    if (!open) return undefined;
    const list = items();
    const target =
      focusTarget.current === "last"
        ? list[list.length - 1]
        : (list.find((el) => el.getAttribute("aria-checked") === "true") ?? list[0]);
    target?.focus();
    focusTarget.current = "checked";

    function onPointerDown(e: MouseEvent) {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [open]);

  function onButtonKeyDown(e: KeyboardEvent<HTMLButtonElement>) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      focusTarget.current = e.key === "ArrowUp" ? "last" : "checked";
      setOpen(true);
    }
  }

  function onMenuKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const list = items();
    const index = list.indexOf(document.activeElement as HTMLButtonElement);
    let next: number | null = null;
    if (e.key === "ArrowDown") next = (index + 1) % list.length;
    else if (e.key === "ArrowUp") next = (index - 1 + list.length) % list.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = list.length - 1;
    else if (e.key === "Escape") {
      e.preventDefault();
      close(true);
      return;
    } else if (e.key === "Tab") {
      // Leave from the button, so Tab / Shift+Tab continue from where the control sits
      // in the page rather than from a menu item that is about to disappear.
      close(true);
      return;
    }
    if (next !== null) {
      e.preventDefault();
      list[next]?.focus();
    }
  }

  return (
    <div className={styles.wrap} ref={wrapRef}>
      <button
        ref={buttonRef}
        type="button"
        className={`${urlStyles.folderButton} ${styles.button}`}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        title={downloadDir}
        aria-label={`${t.urlbar.folderLabel} ${downloadDir}`}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={onButtonKeyDown}
      >
        <span className={urlStyles.selectLabel}>{t.urlbar.folderLabel}</span>
        <span className={urlStyles.folderPath}>{shortenPath(downloadDir)}</span>
        <ChevronDown size={16} aria-hidden className={urlStyles.selectChevron} />
      </button>

      {open && (
        <div className={styles.popover}>
          <div id={headingId} className={styles.heading}>
            {t.urlbar.folderMenuHeading}
          </div>
          <div id={menuId} role="menu" aria-labelledby={headingId} ref={menuRef} onKeyDown={onMenuKeyDown}>
            {folders.map((path) => (
              <button
                key={path}
                type="button"
                role="menuitemradio"
                aria-checked={path === downloadDir}
                tabIndex={-1}
                className={styles.item}
                title={path}
                aria-label={path}
                onClick={() => {
                  close(true);
                  onChoose(path);
                }}
              >
                <span className={styles.mark}>{path === downloadDir && <Check size={16} aria-hidden />}</span>
                <span className={styles.path}>{shortenPath(path)}</span>
              </button>
            ))}
            <div role="separator" className={styles.separator} />
            <button
              type="button"
              role="menuitem"
              tabIndex={-1}
              className={styles.item}
              onClick={() => {
                close(true);
                onPickOther();
              }}
            >
              <span className={styles.mark} />
              <span className={styles.path}>{t.urlbar.folderMenuOther}</span>
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

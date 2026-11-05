// One registered source in the Sources list: the site's badge, the name (renamed in place),
// what it is, when it was last checked, and at the row's top right "check for new" and the
// "..." menu (rename, destination and quality, delete).
import { AlertTriangle } from "lucide-react";
import { Fragment, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useT } from "../i18n/I18nContext";
import type { SourceRow as SourceRowData } from "../ipc/contract";
import { folderName, sourceStatusLine } from "../lib/sources";
import { errorMessage } from "../lib/url";
import { useSettings } from "../state/SettingsContext";
import { useSources } from "../state/SourcesContext";
import { ConfirmDialog } from "./ConfirmDialog";
import stateStyles from "./JobStateLine.module.css";
import queueStyles from "./QueueRow.module.css";
import { RowMenu, type MenuEntry, type RowMenuHandle } from "./RowMenu";
import { SourceOptionsDialog } from "./SourceOptionsDialog";
import sitesStyles from "./SitesSection.module.css";
import styles from "./SourceRow.module.css";

interface SourceRowProps {
  source: SourceRowData;
  /** Deletes the source; the list moves the keyboard focus on when the row is gone. */
  onRemove: (id: number) => Promise<void>;
}

/** The longest name the backend accepts. */
const MAX_NAME = 200;

export function SourceRow({ source, onRemove }: SourceRowProps) {
  const t = useT();
  const { settings } = useSettings();
  const { check, rename, setOptions } = useSources();
  const menuRef = useRef<RowMenuHandle>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState("");
  const [dialog, setDialog] = useState<"options" | "delete" | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Bumped to move the focus back to the "..." button once it is enabled again. */
  const [focusTick, setFocusTick] = useState(0);

  const status = sourceStatusLine(t, source, new Date());
  const working = source.status !== "idle";
  const badge = Array.from(source.siteName)[0]?.toUpperCase() ?? "";

  useEffect(() => {
    if (focusTick > 0) menuRef.current?.focus();
  }, [focusTick]);

  useEffect(() => {
    if (!renaming) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [renaming]);

  function fail(e: unknown) {
    setError(t.library.failed(errorMessage(e)));
  }

  function startRename() {
    setError(null);
    setDraft(source.name);
    setRenaming(true);
  }

  function stopRename(returnFocus: boolean) {
    setRenaming(false);
    setError(null);
    if (returnFocus) setFocusTick((n) => n + 1);
  }

  async function submitRename() {
    if (busy) return;
    const name = draft.trim();
    if (name === "") {
      setError(t.library.nameEmpty);
      return;
    }
    if (name === source.name) {
      stopRename(true);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await rename(source.id, name);
      setRenaming(false);
      setFocusTick((n) => n + 1);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  }

  function onRenameKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    // Enter / Escape belong to the IME while it converts (keyCode 229: WebKit reports the
    // key that ends a composition after isComposing has already cleared).
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter") {
      e.preventDefault();
      void submitRename();
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (!busy) stopRename(true);
    }
  }

  async function checkNow() {
    setError(null);
    try {
      await check(source.id);
    } catch (e) {
      fail(e);
    }
  }

  async function remove() {
    setDialog(null);
    setBusy(true);
    setError(null);
    try {
      await onRemove(source.id);
    } catch (e) {
      fail(e);
      setFocusTick((n) => n + 1);
    } finally {
      setBusy(false);
    }
  }

  const entries: MenuEntry[] = [
    { kind: "item", label: t.library.rename, onSelect: startRename },
    { kind: "item", label: t.sources.options, onSelect: () => setDialog("options") },
    { kind: "separator" },
    { kind: "item", label: t.common.delete, danger: true, onSelect: () => setDialog("delete") },
  ];

  const parts: ReactNode[] = [source.siteName];
  if (source.kind === "list") {
    parts.push(t.sources.kindList);
  } else {
    parts.push(
      <>
        {t.sources.kindRule}
        {source.ruleShape && (
          <>
            {" "}
            <span className={styles.shape}>{source.ruleShape}</span>
          </>
        )}
      </>,
    );
  }
  if (source.destDir) parts.push(<span title={source.destDir}>{t.sources.destination(folderName(source.destDir))}</span>);
  if (source.accountLabel) parts.push(t.sources.account(source.accountLabel));

  return (
    <li
      className={styles.row}
      aria-busy={busy}
      onContextMenu={(e) => {
        e.preventDefault();
        if (!busy) menuRef.current?.openAt(e.clientX, e.clientY);
      }}
      onKeyDown={(e) => {
        if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
          e.preventDefault();
          if (!busy) menuRef.current?.open();
        }
      }}
    >
      <span className={sitesStyles.icon} aria-hidden>
        {badge}
      </span>

      <div className={styles.body}>
        {renaming ? (
          <input
            ref={inputRef}
            type="text"
            className={styles.renameInput}
            value={draft}
            readOnly={busy}
            maxLength={MAX_NAME}
            spellCheck={false}
            autoComplete="off"
            aria-label={t.library.newName}
            aria-invalid={error !== null}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onRenameKeyDown}
            onBlur={() => {
              if (!busy) stopRename(false);
            }}
          />
        ) : (
          <div className={styles.title}>{source.name}</div>
        )}

        <div className={styles.meta}>
          {parts.map((part, index) => (
            <Fragment key={index}>
              {index > 0 && " · "}
              {part}
            </Fragment>
          ))}
        </div>

        {status.kind === "checking" ? (
          <div className={stateStyles.prog}>
            <div
              className={`${stateStyles.bar} ${stateStyles.indeterminate}`}
              role="progressbar"
              aria-label={status.text}
            >
              <span />
            </div>
            <div className={stateStyles.pstat}>
              <span>{status.text}</span>
            </div>
          </div>
        ) : (
          <div
            className={`${stateStyles.state} ${status.error ? `${stateStyles.err} ${styles.statusError}` : ""}`}
            title={status.error}
          >
            {status.error && <AlertTriangle size={14} aria-hidden />}
            <span>{status.text}</span>
          </div>
        )}

        {error && (
          <p role="alert" className={styles.error}>
            {error}
          </p>
        )}
      </div>

      {/* Fixed at the top right edge, whatever the row's height. */}
      <div className={styles.actions}>
        <button
          type="button"
          className={`${queueStyles.textAction} ${styles.check}`}
          disabled={working || busy}
          onClick={() => void checkNow()}
        >
          {t.sources.checkNow}
        </button>
        <RowMenu
          ref={menuRef}
          label={t.library.moreActions}
          entries={entries}
          disabled={busy}
          onReturnFocus={() => menuRef.current?.focus()}
        />
      </div>

      {dialog === "options" && settings && (
        <SourceOptionsDialog
          source={source}
          defaultDir={settings.downloadDir}
          defaultPreset={settings.defaultPreset}
          onSave={(destDir, preset) => setOptions(source.id, destDir, preset)}
          onClose={() => {
            setDialog(null);
            setFocusTick((n) => n + 1);
          }}
        />
      )}
      {dialog === "delete" && (
        <ConfirmDialog
          title={t.sources.deleteTitle(source.name)}
          body={t.sources.deleteBody}
          cancelLabel={t.common.dialogCancel}
          confirmLabel={t.common.delete}
          onCancel={() => {
            setDialog(null);
            setFocusTick((n) => n + 1);
          }}
          onConfirm={() => void remove()}
        />
      )}
    </li>
  );
}

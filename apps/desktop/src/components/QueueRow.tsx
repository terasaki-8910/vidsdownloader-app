import { Play } from "lucide-react";
import {
  useImperativeHandle,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type Ref,
} from "react";
import { ipc } from "../ipc";
import type { JobRow } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { errorMessage } from "../lib/url";
import { formatClockDuration, formatDate, joinMeta } from "../lib/format";
import { getRowAction, type RowActionHandlers } from "../lib/rowAction";
import { registerFailureText } from "../lib/sources";
import { useSources } from "../state/SourcesContext";
import { useNavigation } from "../state/NavigationContext";
import { IconButton } from "./IconButton";
import { JobStateLine } from "./JobStateLine";
import { RenameField } from "./RenameField";
import { RowMenu, type MenuEntry, type RowMenuHandle } from "./RowMenu";
import styles from "./QueueRow.module.css";

export interface QueueRowHandle {
  focus: () => void;
  openMenu: () => void;
  openMenuAt: (x: number, y: number) => void;
  startRename: () => void;
  register: () => void;
  showError: (message: string) => void;
}

interface QueueRowProps extends RowActionHandlers {
  rowRef?: Ref<QueueRowHandle>;
  job: JobRow;
  selected: boolean;
  tabIndex: number;
  menuEntries: MenuEntry[];
  commentStatus?: string;
  commentNotice?: string;
  commentError?: string;
  onSelect: (event: MouseEvent<HTMLDivElement>) => void;
  onSelectForMenu: () => void;
  onRowKeyDown: (event: KeyboardEvent<HTMLDivElement>) => void;
  onOpenDetail: () => void;
}

export function QueueRow({
  rowRef: handleRef,
  job,
  selected,
  tabIndex,
  menuEntries,
  commentStatus,
  commentNotice,
  commentError,
  onSelect,
  onSelectForMenu,
  onRowKeyDown,
  onOpenDetail,
  onCancel,
  onRetry,
  onReveal,
  onLogin,
  onSaveAgain,
  onChooseMedia,
}: QueueRowProps) {
  const t = useT();
  const { openInBrowser } = useNavigation();
  const { sources, phase, register } = useSources();
  const elementRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<RowMenuHandle>(null);
  const [registering, setRegistering] = useState(false);
  const [registered, setRegistered] = useState(false);
  const [registerNotice, setRegisterNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [renameBusy, setRenameBusy] = useState(false);
  const title = job.details?.title ?? job.url;
  const meta = joinMeta([
    job.details?.uploader,
    job.details?.site,
    job.details?.publishedAt != null ? formatDate(job.details.publishedAt) : null,
  ]);
  const action = getRowAction(job, t, {
    onCancel,
    onRetry,
    onReveal,
    onLogin,
    onSaveAgain,
    onChooseMedia,
    onOpenInBrowser: () => void openInBrowser(job.url),
  });
  const isList = job.listCount != null;
  const alreadySource = registered || sources.some((source) => source.url === job.url);

  async function registerAsSource() {
    if (!isList || alreadySource || phase !== "idle") return;
    setRegisterNotice(null);
    setRegistering(true);
    const outcome = await register(job.url);
    setRegistering(false);
    if (outcome.kind === "registered" || outcome.kind === "exists") setRegistered(true);
    if (outcome.kind === "failed") setRegisterNotice(registerFailureText(t, outcome));
  }

  function startRename() {
    if (job.libraryId != null && job.outputPath) {
      setError(null);
      setRenaming(true);
    }
  }

  useImperativeHandle(handleRef, () => ({
    focus: () => elementRef.current?.focus(),
    openMenu: () => menuRef.current?.open(),
    openMenuAt: (x, y) => menuRef.current?.openAt(x, y),
    startRename,
    register: () => void registerAsSource(),
    showError: (message) => setError(message),
  }));

  async function rename(name: string) {
    if (job.libraryId == null) return;
    setRenameBusy(true);
    setError(null);
    try {
      await ipc.call("library_rename", { id: job.libraryId, name });
      setRenaming(false);
      requestAnimationFrame(() => elementRef.current?.focus());
    } catch (e) {
      setError(t.library.failed(errorMessage(e)));
    } finally {
      setRenameBusy(false);
    }
  }

  function actionButtonsKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if ((e.target as Element).closest("[role='menu']")) return;
    if (!e.key.startsWith("Arrow") && e.key !== "Escape") return;
    const buttons = Array.from(e.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape" || (e.key === "ArrowLeft" && index <= 0) || (e.key === "ArrowRight" && index === buttons.length - 1)) {
      e.preventDefault();
      elementRef.current?.focus();
    } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      buttons[index + (e.key === "ArrowRight" ? 1 : -1)]?.focus();
    }
  }

  return (
    <div
      ref={elementRef}
      className={`${styles.row} ${selected ? styles.selected : ""}`}
      role="row"
      aria-selected={selected}
      tabIndex={tabIndex}
      aria-label={title}
      onClick={(e) => {
        if (!(e.target as Element).closest(`.${styles.actions}`)) {
          onSelect(e);
          elementRef.current?.focus();
        }
      }}
      onDoubleClick={(e) => {
        if (!(e.target as Element).closest("button, input")) onOpenDetail();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onSelectForMenu();
        elementRef.current?.focus();
        menuRef.current?.openAt(e.clientX, e.clientY);
      }}
      onKeyDown={onRowKeyDown}
    >
      <div className={styles.content} role="gridcell">
        <div className={styles.thumb}>
          {job.details?.thumbnailUrl ? <img src={job.details.thumbnailUrl} alt="" /> : <Play size={20} aria-hidden />}
          {job.details?.durationS != null && <span className={styles.dur}>{formatClockDuration(job.details.durationS)}</span>}
        </div>
        <div className={styles.body}>
          <div className={styles.title}>{title}</div>
          {meta && <div className={styles.meta}>{meta}</div>}
          {renaming && job.outputPath ? (
            <RenameField path={job.outputPath} label={t.library.newName} emptyError={t.library.nameEmpty} badCharsError={t.library.nameBadChars} busy={renameBusy} error={error} onError={setError} onSubmit={rename} onCancel={(returnFocus) => { setRenaming(false); setError(null); if (returnFocus) requestAnimationFrame(() => elementRef.current?.focus()); }} />
          ) : commentStatus ? <div className={styles.commentNotice}>{commentStatus}</div> : <JobStateLine job={job} />}
          {commentNotice && !commentStatus && <div className={styles.commentNotice}>{commentNotice}</div>}
          {commentError && <p role="alert" className={styles.notice}>{commentError}</p>}
          {registerNotice && <p role="alert" className={styles.notice}>{registerNotice}</p>}
          {error && <p role="alert" className={styles.notice}>{error}</p>}
        </div>
      </div>

      <div className={styles.actions} role="gridcell" onKeyDown={actionButtonsKeyDown}>
        {isList && (
          <button type="button" tabIndex={-1} className={styles.textAction} disabled={alreadySource || phase !== "idle"} onClick={(e) => { e.stopPropagation(); void registerAsSource(); }}>
            {alreadySource ? t.sources.rowRegistered : registering && phase === "previewing" ? t.sources.previewing : t.sources.rowRegister}
          </button>
        )}
        {action?.kind === "login" && <button type="button" tabIndex={-1} className={styles.loginAction} onClick={(e) => { e.stopPropagation(); action.onClick(); }}>{action.label}</button>}
        {action?.kind === "text" && <button type="button" tabIndex={-1} className={styles.textAction} onClick={(e) => { e.stopPropagation(); action.onClick(); }}>{action.label}</button>}
        {action?.kind === "icon" && <IconButton tabIndex={-1} label={action.label} onClick={(e) => { e.stopPropagation(); action.onClick(); }}>{action.icon}</IconButton>}
        <RowMenu ref={menuRef} label={t.library.moreActions} entries={menuEntries} disabled={renameBusy} onBeforeOpen={onSelectForMenu} onReturnFocus={() => elementRef.current?.focus({ preventScroll: true })} />
      </div>
    </div>
  );
}

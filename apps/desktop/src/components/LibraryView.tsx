import { Folder, FolderOpen, Search, TriangleAlert, X } from "lucide-react";
import {
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type Ref,
} from "react";
import { useListSelection } from "../hooks/useListSelection";
import type { CommentsExtractionResult, EraseResult, LibraryRow } from "../ipc/contract";
import { ipc } from "../ipc";
import { useT } from "../i18n/I18nContext";
import { useDebouncedValue } from "../hooks/useDebouncedValue";
import { fileName, formatCommentDate, formatDate, joinMeta } from "../lib/format";
import { readDebugHash } from "../lib/debugHash";
import { IS_MAC, IS_WINDOWS } from "../lib/platform";
import { libraryCanExtractComments } from "../lib/rowAction";
import { revealPath } from "../lib/reveal";
import { errorMessage } from "../lib/url";
import { ConfirmDialog } from "./ConfirmDialog";
import { CommentsConflictDialog, type CommentsConflictChoice } from "./CommentsConflictDialog";
import { CommentsUrlDialog } from "./CommentsUrlDialog";
import { EmptyState } from "./EmptyState";
import { IconButton } from "./IconButton";
import { RenameField } from "./RenameField";
import { RowMenu, type MenuEntry, type RowMenuHandle } from "./RowMenu";
import { SelectionBar } from "./SelectionBar";
import styles from "./LibraryView.module.css";

const PAGE_SIZE = 200;

interface LibraryItemHandle {
  focus: () => void;
  openMenu: () => void;
  openMenuAt: (x: number, y: number) => void;
  startRename: () => void;
}

function uncleanNotice(result: EraseResult, sideFilesFailed: (names: string[]) => string, tracesLeft: string) {
  const parts: string[] = [];
  if (result.sideFilesFailed.length > 0) parts.push(sideFilesFailed(result.sideFilesFailed.map(fileName)));
  if (!result.checkpointed || (IS_WINDOWS && !result.cacheCleared)) parts.push(tracesLeft);
  return parts.join(" ");
}

export function LibraryView() {
  const t = useT();
  const [query, setQuery] = useState("");
  const debouncedQuery = useDebouncedValue(query, 300);
  const [rows, setRows] = useState<LibraryRow[] | null>(null);
  const [reloadTick, setReloadTick] = useState(0);
  const [notice, setNotice] = useState<string | null>(null);
  const [busyIds, setBusyIds] = useState<Set<number>>(new Set());
  const [savedComments, setSavedComments] = useState<Record<number, number>>({});
  const [commentErrors, setCommentErrors] = useState<Record<number, string>>({});
  const [commentConflicts, setCommentConflicts] = useState<Array<Extract<CommentsExtractionResult, { status: "conflict" }>>>([]);
  const [fileNeedingUrl, setFileNeedingUrl] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<{ kind: "trash" | "forget"; rows: LibraryRow[] } | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const itemRefs = useRef(new Map<number, LibraryItemHandle>());
  const focusIndex = useRef<number | null>(null);
  const debugApplied = useRef(false);
  const ids = useMemo(() => rows?.map((row) => row.id) ?? [], [rows]);
  const selection = useListSelection(ids);
  const selectedRows = useMemo(
    () => rows?.filter((row) => selection.selected.has(row.id)) ?? [],
    [rows, selection.selected],
  );
  const words = t.library.trash[IS_MAC ? "mac" : "windows"];

  useEffect(() => {
    let cancelled = false;
    const text = debouncedQuery.trim() || null;
    void ipc.call("library_search", { text, limit: PAGE_SIZE, offset: 0 }).then((result) => {
      if (!cancelled) setRows(result);
    });
    return () => {
      cancelled = true;
    };
  }, [debouncedQuery, reloadTick]);

  useEffect(() => ipc.on("library-updated", () => setReloadTick((n) => n + 1)), []);

  useLayoutEffect(() => {
    const index = focusIndex.current;
    if (index === null || rows === null) return;
    focusIndex.current = null;
    const row = rows[Math.min(index, rows.length - 1)];
    if (row) itemRefs.current.get(row.id)?.focus();
    else searchRef.current?.focus();
  }, [rows]);

  useEffect(() => {
    if (debugApplied.current || !rows?.length) return;
    debugApplied.current = true;
    const hash = readDebugHash();
    const chosen = hash.select?.filter((id) => rows.some((row) => row.id === id)) ?? [];
    if (chosen.length > 0) selection.act({ type: "set", ids: chosen, focus: chosen[chosen.length - 1] });
    if (hash.commentsConflict) {
      const row = rows.find((item) => item.id === 10) ?? rows.find(libraryCanExtractComments);
      if (row) {
        setBusyIds(new Set([row.id]));
        setCommentConflicts([{ id: row.id, status: "conflict", existingCount: 1072, existingModified: Math.floor(new Date(2026, 10, 2).getTime() / 1000), newCount: 1250, token: `mock-${row.id}`, fileName: fileName(row.path).replace(/\.[^.]+$/, ".comments.json") }]);
      }
    }
    requestAnimationFrame(() => {
      const menuId = hash.menu;
      if (menuId != null && rows.some((row) => row.id === menuId)) itemRefs.current.get(menuId)?.openMenu();
      if (hash.confirmDelete) runDelete(chosen.length > 0 ? rows.filter((row) => chosen.includes(row.id)) : []);
    });
  }, [rows, selection]);

  function selectForMenu(id: number) {
    selection.act({ type: "context", id });
  }

  function rowsForMenu(row: LibraryRow): LibraryRow[] {
    return selection.selected.has(row.id) ? selectedRows : [row];
  }

  function openRow(row: LibraryRow) {
    if (!row.missing) void ipc.call("library_open", { id: row.id });
  }

  function requestTrash(targets: LibraryRow[]) {
    if (targets.length > 0) setConfirming({ kind: "trash", rows: targets });
  }

  function requestForget(targets: LibraryRow[]) {
    if (targets.length > 0) setConfirming({ kind: "forget", rows: targets });
  }

  function runDelete(targets = selectedRows) {
    if (targets.length === 0) return;
    const present = targets.filter((row) => !row.missing);
    if (present.length > 0) requestTrash(present);
    else requestForget(targets);
  }

  async function erase(kind: "trash" | "forget", targets: LibraryRow[]) {
    setConfirming(null);
    if (!rows || targets.length === 0) return;
    focusIndex.current = Math.min(...targets.map((target) => rows.findIndex((row) => row.id === target.id)).filter((i) => i >= 0));
    setBusyIds(new Set(targets.map((row) => row.id)));
    const removed = new Set<number>();
    const failures: string[] = [];
    const leftover: string[] = [];
    for (const row of targets) {
      try {
        const result = await ipc.call(kind === "trash" ? "library_trash" : "library_forget", { id: row.id });
        removed.add(row.id);
        const message = uncleanNotice(result, words.sideFilesFailed, t.library.tracesLeft);
        if (message) leftover.push(message);
      } catch (e) {
        failures.push(errorMessage(e));
      }
    }
    setRows((current) => current?.filter((row) => !removed.has(row.id)) ?? current);
    selection.act({ type: "clear" });
    setBusyIds(new Set());
    if (failures.length > 0) {
      setNotice(
        kind === "trash"
          ? words.bulkFailed(targets.length, failures.length, failures[0])
          : t.library.forgetFailed(targets.length, failures.length, failures[0]),
      );
    } else if (leftover.length > 0) setNotice(leftover[0]);
  }

  function markCommentTargets(targets: LibraryRow[], busy: boolean) {
    setBusyIds((current) => {
      const next = new Set(current);
      for (const row of targets) busy ? next.add(row.id) : next.delete(row.id);
      return next;
    });
  }

  function acceptCommentResults(results: CommentsExtractionResult[]) {
    const conflicts: Array<Extract<CommentsExtractionResult, { status: "conflict" }>> = [];
    for (const result of results) {
      if (result.status === "conflict") conflicts.push(result);
      else if (result.status === "saved") {
        if (result.id != null) {
          setSavedComments((current) => ({ ...current, [result.id!]: result.count }));
          setBusyIds((current) => { const next = new Set(current); next.delete(result.id!); return next; });
        } else setNotice(t.library.savedComments(result.count));
      } else if (result.status === "failed" && result.id != null) {
        setCommentErrors((current) => ({ ...current, [result.id!]: t.library.failed(result.message) }));
        setBusyIds((current) => { const next = new Set(current); next.delete(result.id!); return next; });
      } else if (result.status === "failed") setNotice(t.library.failed(result.message));
    }
    if (conflicts.length > 0) setCommentConflicts((current) => [...current, ...conflicts]);
  }

  async function extractComments(targets: LibraryRow[]) {
    const eligible = targets.filter(libraryCanExtractComments);
    if (eligible.length === 0) return;
    markCommentTargets(eligible, true);
    setCommentErrors((current) => {
      const next = { ...current };
      for (const row of eligible) delete next[row.id];
      return next;
    });
    try {
      acceptCommentResults(await ipc.call("comments_extract", { libraryIds: eligible.map((row) => row.id) }));
    } catch (error) {
      for (const row of eligible) {
        setCommentErrors((current) => ({ ...current, [row.id]: t.library.failed(errorMessage(error)) }));
      }
      markCommentTargets(eligible, false);
    }
  }

  async function resolveComments(answer: CommentsConflictChoice) {
    const [current, ...rest] = commentConflicts;
    if (!current) return;
    setCommentConflicts([]);
    const resolve = async (conflict: typeof current, choice: "existing" | "new") => {
      try {
        const result = await ipc.call("comments_resolve", { token: conflict.token, keep: choice });
        acceptCommentResults([result]);
      } catch (error) {
        if (conflict.id != null) setCommentErrors((errors) => ({ ...errors, [conflict.id!]: t.library.failed(errorMessage(error)) }));
      } finally {
        if (conflict.id != null) setBusyIds((busy) => { const next = new Set(busy); next.delete(conflict.id!); return next; });
      }
    };
    await resolve(current, answer.choice);
    if (answer.applyRemaining) {
      for (const conflict of rest) await resolve(conflict, answer.choice);
    } else setCommentConflicts(rest);
  }

  async function addCommentsToFile() {
    try {
      const path = await ipc.call("pick_video_file", {});
      if (!path) return;
      const result = await ipc.call("file_extract_comments", { path, url: null });
      if (result.status === "needsUrl") setFileNeedingUrl(path);
      else acceptCommentResults([result]);
    } catch (error) {
      setNotice(t.library.failed(errorMessage(error)));
    }
  }

  async function submitFileUrl(url: string) {
    const path = fileNeedingUrl;
    setFileNeedingUrl(null);
    if (!path) return;
    try { acceptCommentResults([await ipc.call("file_extract_comments", { path, url })]); }
    catch (error) { setNotice(t.library.failed(errorMessage(error))); }
  }

  function handleRowKeyDown(e: KeyboardEvent<HTMLLIElement>, row: LibraryRow) {
    const mod = IS_MAC ? e.metaKey : e.ctrlKey;
    const targetElement = e.target as Element;
    if (targetElement.closest("[role='menu'], input")) return;
    const fromButton = targetElement.closest("button") !== null;
    const buttonShortcut =
      e.key === "F2" ||
      mod && e.key.toLowerCase() === "a" ||
      e.key === "ContextMenu" ||
      e.shiftKey && e.key === "F10" ||
      !IS_MAC && e.key === "Delete" ||
      IS_MAC && e.metaKey && e.key === "Backspace";
    if (fromButton && !buttonShortcut) return;
    if (!fromButton && e.target !== e.currentTarget) return;
    const index = ids.indexOf(row.id);
    let target: number | undefined;
    if (e.key === "ArrowUp") target = ids[Math.max(0, index - 1)];
    else if (e.key === "ArrowDown") target = ids[Math.min(ids.length - 1, index + 1)];
    else if (e.key === "Home") target = ids[0];
    else if (e.key === "End") target = ids[ids.length - 1];
    if (target != null) {
      e.preventDefault();
      selection.act({ type: "move", id: target, mod, shift: e.shiftKey });
      requestAnimationFrame(() => itemRefs.current.get(target!)?.focus());
      return;
    }
    if (e.key === "ArrowRight") {
      e.preventDefault();
      (e.currentTarget.querySelector("[role='gridcell']:last-child button:not(:disabled)") as HTMLButtonElement | null)?.focus();
    } else if (e.key === " " && !e.shiftKey) {
      e.preventDefault();
      selection.act({ type: "space", id: row.id, mod });
    } else if (mod && e.key.toLowerCase() === "a") {
      e.preventDefault();
      selection.act({ type: "selectAll" });
    } else if (e.key === "Escape" && selection.selected.size > 0) {
      e.preventDefault();
      selection.act({ type: "clear" });
    } else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      e.preventDefault();
      itemRefs.current.get(row.id)?.openMenu();
    } else if (e.key === "F2" || (IS_MAC && !mod && e.key === "Enter")) {
      if (selectedRows.length === 1 && !selectedRows[0].missing) {
        e.preventDefault();
        itemRefs.current.get(selectedRows[0].id)?.startRename();
      }
    } else if ((!IS_MAC && e.key === "Enter") || (IS_MAC && mod && (e.key.toLowerCase() === "o" || e.key === "ArrowDown"))) {
      e.preventDefault();
      openRow(row);
    } else if ((!IS_MAC && e.key === "Delete") || (IS_MAC && e.metaKey && e.key === "Backspace")) {
      e.preventDefault();
      runDelete();
    }
  }

  const barActions = selectedRows.length >= 2
    ? [
        ...(selectedRows.some(libraryCanExtractComments)
          ? [{ label: t.library.extractCommentsCount(selectedRows.filter(libraryCanExtractComments).length), onClick: () => void extractComments(selectedRows) }]
          : []),
        ...(selectedRows.some((row) => !row.missing)
          ? [{ label: words.count(selectedRows.filter((row) => !row.missing).length), danger: true, onClick: () => requestTrash(selectedRows.filter((row) => !row.missing)) }]
          : []),
        { label: t.library.forgetCount(selectedRows.length), onClick: () => requestForget(selectedRows) },
      ]
    : [];

  return (
    <section className={styles.library}>
      <div className={styles.head}>
        {selectedRows.length >= 2
          ? <SelectionBar summary={t.common.selected(selectedRows.length)} clearLabel={t.common.clearSelection} actions={barActions} onClear={() => selection.act({ type: "clear" })} />
          : <><h1>{t.library.title}</h1><button type="button" className={styles.headAction} onClick={() => void addCommentsToFile()}>{t.library.addCommentsToFile}</button></>}
      </div>
      <div className={styles.searchWrap}>
        <Search size={16} aria-hidden className={styles.searchIcon} />
        <input ref={searchRef} type="search" className={styles.search} value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t.library.searchPlaceholder} aria-label={t.library.searchPlaceholder} />
      </div>
      {notice && <NoticeLine message={notice} onDismiss={() => setNotice(null)} />}
      {rows === null ? null : rows.length === 0 ? (
        <EmptyState icon={query.trim() ? <Search size={32} /> : <Folder size={32} />} body={query.trim() ? t.library.emptyNoResults : t.library.emptyNoneYet} />
      ) : (
        <div className={styles.listArea}>
          <ul className={styles.list} role="grid" aria-multiselectable="true" aria-label={t.library.title}>
            {rows.map((row, index) => (
              <LibraryRowItem
                key={row.id}
                itemRef={(handle) => {
                  if (handle) itemRefs.current.set(row.id, handle);
                  else itemRefs.current.delete(row.id);
                }}
                row={row}
                selected={selection.selected.has(row.id)}
                tabIndex={(selection.focus ?? ids[0]) === row.id ? 0 : -1}
                busy={busyIds.has(row.id)}
                menuRows={rowsForMenu(row)}
                onSelect={(event) => selection.act({ type: "click", id: row.id, mod: IS_MAC ? event.metaKey : event.ctrlKey, shift: event.shiftKey })}
                onSelectForMenu={() => selectForMenu(row.id)}
                onKeyDown={(event) => handleRowKeyDown(event, row)}
                onOpen={() => openRow(row)}
                onTrash={requestTrash}
                onForget={requestForget}
                onExtractComments={(targets) => void extractComments(targets)}
                savedComments={savedComments[row.id]}
                commentError={commentErrors[row.id]}
                onChanged={(updated) => setRows((current) => current?.map((item) => item.id === updated.id ? updated : item) ?? current)}
                position={index}
              />
            ))}
          </ul>
        </div>
      )}
      {confirming && (
        <ConfirmDialog
          title={confirming.kind === "trash" ? words.title : t.library.forgetTitle}
          body={confirming.kind === "trash"
            ? confirming.rows.length === 1 ? words.body(confirming.rows[0].title ?? fileName(confirming.rows[0].path)) : words.bulkBody(confirming.rows[0].title ?? fileName(confirming.rows[0].path), confirming.rows.length)
            : confirming.rows.length === 1 ? t.library.forgetBody(confirming.rows[0].title ?? fileName(confirming.rows[0].path)) : t.library.forgetBulkBody(confirming.rows[0].title ?? fileName(confirming.rows[0].path), confirming.rows.length)}
          cancelLabel={t.common.dialogCancel}
          confirmLabel={confirming.kind === "trash" ? words.confirm : t.library.forgetConfirm}
          onCancel={() => setConfirming(null)}
          onConfirm={() => void erase(confirming.kind, confirming.rows)}
        />
      )}
      {commentConflicts[0] && (
        <CommentsConflictDialog
          fileName={commentConflicts[0].fileName}
          existingCount={commentConflicts[0].existingCount}
          existingDate={commentConflicts[0].existingModified == null ? "—" : formatCommentDate(commentConflicts[0].existingModified)}
          newCount={commentConflicts[0].newCount}
          remaining={commentConflicts.length - 1}
          onCancel={() => void resolveComments({ choice: "existing", applyRemaining: false })}
          onDecide={(answer) => void resolveComments(answer)}
        />
      )}
      {fileNeedingUrl && <CommentsUrlDialog onCancel={() => setFileNeedingUrl(null)} onSubmit={(url) => void submitFileUrl(url)} />}
    </section>
  );
}

function NoticeLine({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  const t = useT();
  return <div className={styles.notice} role="status"><TriangleAlert size={16} aria-hidden className={styles.noticeIcon} /><div className={styles.noticeText}>{message}</div><IconButton label={t.common.close} size="sm" onClick={onDismiss}><X size={16} aria-hidden /></IconButton></div>;
}

interface LibraryRowItemProps {
  itemRef?: Ref<LibraryItemHandle>;
  row: LibraryRow;
  selected: boolean;
  tabIndex: number;
  busy: boolean;
  menuRows: LibraryRow[];
  position: number;
  onSelect: (event: MouseEvent<HTMLLIElement>) => void;
  onSelectForMenu: () => void;
  onKeyDown: (event: KeyboardEvent<HTMLLIElement>) => void;
  onOpen: () => void;
  onTrash: (rows: LibraryRow[]) => void;
  onForget: (rows: LibraryRow[]) => void;
  onExtractComments: (rows: LibraryRow[]) => void;
  savedComments?: number;
  commentError?: string;
  onChanged: (row: LibraryRow) => void;
}

function LibraryRowItem({ itemRef, row, selected, tabIndex, busy, menuRows, onSelect, onSelectForMenu, onKeyDown, onOpen, onTrash, onForget, onExtractComments, savedComments, commentError, onChanged }: LibraryRowItemProps) {
  const t = useT();
  const words = t.library.trash[IS_MAC ? "mac" : "windows"];
  const menuRef = useRef<RowMenuHandle>(null);
  const rowRef = useRef<HTMLLIElement>(null);
  const [renaming, setRenaming] = useState(false);
  const [renameBusy, setRenameBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const meta = joinMeta([row.uploader, row.site, row.publishedAt != null ? formatDate(row.publishedAt) : null]);
  const multi = menuRows.length >= 2;

  function startRename() {
    if (!row.missing && menuRows.length === 1) {
      setError(null);
      setRenaming(true);
    }
  }

  useImperativeHandle(itemRef, () => ({
    focus: () => rowRef.current?.focus(),
    openMenu: () => menuRef.current?.open(),
    openMenuAt: (x, y) => menuRef.current?.openAt(x, y),
    startRename,
  }));

  async function rename(name: string) {
    setRenameBusy(true);
    setError(null);
    try {
      const updated = await ipc.call("library_rename", { id: row.id, name });
      setRenaming(false);
      onChanged(updated);
      requestAnimationFrame(() => rowRef.current?.focus());
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
      e.preventDefault(); rowRef.current?.focus();
    } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault(); buttons[index + (e.key === "ArrowRight" ? 1 : -1)]?.focus();
    }
  }

  const present = menuRows.filter((item) => !item.missing);
  const commentRows = menuRows.filter(libraryCanExtractComments);
  let entries: MenuEntry[];
  if (multi) {
    entries = [
      ...(commentRows.length > 0 ? [{ kind: "item" as const, label: t.library.extractCommentsCount(commentRows.length), onSelect: () => onExtractComments(commentRows) }, { kind: "separator" as const }] : []),
      ...(present.length > 0 ? [{ kind: "item" as const, label: words.count(present.length), hint: IS_MAC ? "⌘⌫" : "Delete", danger: true, onSelect: () => onTrash(present) }] : []),
      { kind: "item", label: t.library.forgetCount(menuRows.length), onSelect: () => onForget(menuRows) },
    ];
  } else if (row.missing) {
    entries = [{ kind: "item", label: t.library.forget, onSelect: () => onForget([row]) }];
  } else {
    entries = [
      { kind: "item", label: t.library.open, hint: IS_MAC ? "⌘O" : "Enter", onSelect: onOpen },
      { kind: "item", label: t.library.openWith, onSelect: () => ipc.call("library_open_with", { id: row.id }).catch((e) => setError(t.library.failed(errorMessage(e)))) },
      { kind: "item", label: t.library.rename, hint: IS_MAC ? "\u21a9" : "F2", onSelect: startRename },
      ...(libraryCanExtractComments(row) ? [{ kind: "item" as const, label: t.library.extractComments, onSelect: () => onExtractComments([row]) }] : []),
      { kind: "separator" },
      { kind: "item", label: words.menu, hint: IS_MAC ? "⌘⌫" : "Delete", danger: true, onSelect: () => onTrash([row]) },
      { kind: "item", label: t.library.forget, onSelect: () => onForget([row]) },
    ];
  }

  return (
    <li
      ref={rowRef}
      className={`${styles.row} ${selected ? styles.selected : ""}`}
      role="row"
      aria-selected={selected}
      aria-busy={busy || renameBusy}
      tabIndex={tabIndex}
      onClick={(e) => { if (e.target === e.currentTarget || (e.target as Element).closest("[role='gridcell']")?.classList.contains(styles.body)) { onSelect(e); rowRef.current?.focus(); } }}
      onDoubleClick={(e) => { if (!(e.target as Element).closest("button, input")) onOpen(); }}
      onContextMenu={(e) => { e.preventDefault(); onSelectForMenu(); rowRef.current?.focus(); menuRef.current?.openAt(e.clientX, e.clientY); }}
      onKeyDown={onKeyDown}
    >
      <div className={styles.body} role="gridcell">
        <div className={styles.title}>{row.title ?? row.path}{row.missing && <span className={styles.badge}>{t.library.missingBadge}</span>}</div>
        {meta && <div className={styles.meta}>{meta}</div>}
        {renaming ? <RenameField path={row.path} label={t.library.newName} emptyError={t.library.nameEmpty} badCharsError={t.library.nameBadChars} busy={renameBusy} error={error} onError={setError} onSubmit={rename} onCancel={(returnFocus) => { setRenaming(false); setError(null); if (returnFocus) requestAnimationFrame(() => rowRef.current?.focus()); }} /> : <div className={styles.path}>{row.path}</div>}
        {busy && !renameBusy && <div className={styles.commentsSaved}>{t.library.extractingComments}</div>}
        {!busy && savedComments != null && <div className={styles.commentsSaved}>{t.library.savedComments(savedComments)}</div>}
        {commentError && <p role="alert" className={styles.error}>{commentError}</p>}
        {error && <p role="alert" className={styles.error}>{error}</p>}
      </div>
      <div className={styles.actions} role="gridcell" onKeyDown={actionButtonsKeyDown}>
        {!row.missing && <IconButton tabIndex={-1} label={t.common.revealInFolder} onClick={() => revealPath(row.path)}><FolderOpen size={16} aria-hidden /></IconButton>}
        <RowMenu ref={menuRef} label={t.library.moreActions} entries={entries} disabled={busy || renameBusy} onBeforeOpen={onSelectForMenu} onReturnFocus={() => rowRef.current?.focus({ preventScroll: true })} />
      </div>
    </li>
  );
}

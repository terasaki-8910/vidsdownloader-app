import { Inbox, TriangleAlert, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { useListSelection } from "../hooks/useListSelection";
import { ipc } from "../ipc";
import type { CommentsExtractionResult, JobRow } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { useJobs } from "../state/JobsContext";
import { useSettings } from "../state/SettingsContext";
import { useSources } from "../state/SourcesContext";
import { useTools } from "../state/ToolsContext";
import { errorMessage } from "../lib/url";
import { formatCommentDate } from "../lib/format";
import { readDebugHash } from "../lib/debugHash";
import { hasSavedFile } from "../lib/jobNote";
import { summarizeJobs } from "../lib/jobStats";
import { IS_MAC } from "../lib/platform";
import { getJobSelectionActions, jobCanBeRemoved, jobCanExtractComments, jobIsBusy, type JobSelectionAction } from "../lib/rowAction";
import { revealPath } from "../lib/reveal";
import { ClipboardBanner } from "./ClipboardBanner";
import { ConfirmDialog } from "./ConfirmDialog";
import { CommentsConflictDialog, type CommentsConflictChoice } from "./CommentsConflictDialog";
import { EmptyState } from "./EmptyState";
import { IconButton } from "./IconButton";
import { QueueRow, type QueueRowHandle } from "./QueueRow";
import type { MenuEntry } from "./RowMenu";
import { SelectionBar } from "./SelectionBar";
import { ToolsNotice } from "./ToolsNotice";
import { UrlBar } from "./UrlBar";
import styles from "./DownloadsView.module.css";

interface DownloadsViewProps {
  urlFieldRef: RefObject<HTMLTextAreaElement | null>;
  isWide: boolean;
  onFocusJob: (jobId: number) => void;
  onOpenDetail: (jobId: number) => void;
  onOpenToolsSettings: () => void;
  onLogin: (job: JobRow) => void;
  onChooseMedia: (job: JobRow) => void;
}

export function DownloadsView({ urlFieldRef, isWide, onFocusJob, onOpenDetail, onOpenToolsSettings, onLogin, onChooseMedia }: DownloadsViewProps) {
  const t = useT();
  const { jobs, enqueue, cancel, retry, saveAgain } = useJobs();
  const { settings, chooseDownloadDir, pickDownloadDir } = useSettings();
  const { sources } = useSources();
  const { missing: missingTools, install: installTools } = useTools();
  const [clipboardUrl, setClipboardUrl] = useState<string | null>(null);
  const [dismissedUrls, setDismissedUrls] = useState<Set<string>>(new Set());
  const [notice, setNotice] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<{ trash: JobRow[]; remove: JobRow[] } | null>(null);
  const [commentBusyIds, setCommentBusyIds] = useState<Set<number>>(new Set());
  const [savedComments, setSavedComments] = useState<Record<number, number>>({});
  const [commentErrors, setCommentErrors] = useState<Record<number, string>>({});
  const [commentConflicts, setCommentConflicts] = useState<Array<Extract<CommentsExtractionResult, { status: "conflict" }>>>([]);
  const rowRefs = useRef(new Map<number, QueueRowHandle>());
  /**
   * Rows being removed and where the first of them sat, so keyboard focus can stay near it. The
   * queue changes for other reasons meanwhile (progress), so focus moves only once they are gone.
   */
  const pendingFocus = useRef<{ index: number; ids: Set<number> } | null>(null);
  const debugApplied = useRef(false);
  const ids = useMemo(() => jobs.map((job) => job.id), [jobs]);
  const selection = useListSelection(ids);
  const selectedJobs = useMemo(() => jobs.filter((job) => selection.selected.has(job.id)), [jobs, selection.selected]);
  const words = t.library.trash[IS_MAC ? "mac" : "windows"];

  useEffect(() => {
    function handleFocus() {
      if (!settings?.clipboardBanner) return;
      void ipc.call("read_clipboard_url", {}).then((url) => {
        if (!url || dismissedUrls.has(url) || jobs.some((j) => j.url === url)) setClipboardUrl(null);
        else setClipboardUrl(url);
      });
    }
    window.addEventListener("focus", handleFocus);
    return () => window.removeEventListener("focus", handleFocus);
  }, [settings?.clipboardBanner, dismissedUrls, jobs]);

  useEffect(() => {
    if (readDebugHash().forceClipboardBanner) setClipboardUrl("https://www.youtube.com/watch?v=BaW_jenozKc");
  }, []);

  useEffect(() => {
    const pending = pendingFocus.current;
    if (pending === null || jobs.some((job) => pending.ids.has(job.id))) return;
    pendingFocus.current = null;
    const job = jobs[Math.min(pending.index, jobs.length - 1)];
    if (job) {
      selection.act({ type: "set", ids: [job.id], focus: job.id });
      rowRefs.current.get(job.id)?.focus();
      onFocusJob(job.id);
    } else urlFieldRef.current?.focus();
  }, [jobs, onFocusJob, selection, urlFieldRef]);

  useEffect(() => {
    if (debugApplied.current || jobs.length === 0) return;
    debugApplied.current = true;
    const hash = readDebugHash();
    const chosen = hash.select?.filter((id) => jobs.some((job) => job.id === id)) ?? [];
    if (chosen.length > 0) {
      selection.act({ type: "set", ids: chosen, focus: chosen[chosen.length - 1] });
      onFocusJob(chosen[chosen.length - 1]);
    }
    requestAnimationFrame(() => {
      if (hash.menu != null && jobs.some((job) => job.id === hash.menu)) rowRefs.current.get(hash.menu)?.openMenu();
      if (hash.confirmDelete) runDelete(chosen.length > 0 ? jobs.filter((job) => chosen.includes(job.id)) : []);
    });
  }, [jobs, onFocusJob, selection]);

  const counts = summarizeJobs(jobs);

  function menuJobs(job: JobRow): JobRow[] {
    return selection.selected.has(job.id) ? selectedJobs : [job];
  }

  function actionLabel(action: JobSelectionAction, count: number) {
    if (action === "stop") return t.downloads.stopCount(count);
    if (action === "retry") return t.downloads.retryCount(count);
    if (action === "trash") return words.count(count);
    return t.downloads.removeCount(count);
  }

  async function removeRows(targets: JobRow[]) {
    if (targets.length === 0) return;
    expectRemoved(targets);
    try {
      await ipc.call("jobs_remove", { ids: targets.map((job) => job.id) });
    } catch (e) {
      setNotice(t.downloads.removeFailed(targets.length, targets.length, errorMessage(e)));
    }
    settleRemoved();
  }

  async function runAction(action: JobSelectionAction, targets: JobRow[]) {
    if (action === "stop") {
      for (const job of targets) await cancel(job.id);
    } else if (action === "retry") {
      for (const job of targets) await retry(job.id);
    } else if (action === "trash") {
      setConfirming({ trash: targets, remove: [] });
    } else {
      await removeRows(targets);
    }
  }

  function runDelete(targets = selectedJobs) {
    if (targets.length === 0) return;
    const trash = targets.filter((job) => job.state === "Done" && job.libraryId != null);
    const trashIds = new Set(trash.map((job) => job.id));
    const remove = targets.filter((job) => !trashIds.has(job.id) && jobCanBeRemoved(job));
    if (trash.length > 0) setConfirming({ trash, remove });
    else void removeRows(remove);
  }

  async function confirmTrash() {
    if (!confirming) return;
    const { trash, remove } = confirming;
    setConfirming(null);
    expectRemoved([...trash, ...remove]);
    const failures: string[] = [];
    for (const job of trash) {
      if (job.libraryId == null) continue;
      try {
        await ipc.call("library_trash", { id: job.libraryId });
      } catch (e) {
        failures.push(errorMessage(e));
      }
    }
    if (remove.length > 0) {
      try {
        await ipc.call("jobs_remove", { ids: remove.map((job) => job.id) });
      } catch (e) {
        setNotice(t.downloads.removeFailed(remove.length, remove.length, errorMessage(e)));
      }
    }
    if (failures.length > 0) setNotice(words.bulkFailed(trash.length, failures.length, failures[0]));
    settleRemoved();
  }

  function expectRemoved(targets: JobRow[]) {
    const positions = targets.map((target) => jobs.findIndex((job) => job.id === target.id)).filter((index) => index >= 0);
    pendingFocus.current = positions.length > 0 ? { index: Math.min(...positions), ids: new Set(targets.map((job) => job.id)) } : null;
  }

  /** Rows that were refused stay in the list; stop waiting for them once the commands are done. */
  function settleRemoved() {
    const pending = pendingFocus.current;
    if (pending) pendingFocus.current = { index: pending.index, ids: new Set() };
  }

  function copyUrl(job: JobRow) {
    navigator.clipboard.writeText(job.url).catch((e) => rowRefs.current.get(job.id)?.showError(t.library.failed(errorMessage(e))));
  }

  function commentJob(libraryId: number | null): JobRow | undefined {
    return libraryId == null ? undefined : jobs.find((job) => job.libraryId === libraryId);
  }

  function acceptCommentResults(results: CommentsExtractionResult[]) {
    const conflicts: Array<Extract<CommentsExtractionResult, { status: "conflict" }>> = [];
    for (const result of results) {
      if (result.status === "conflict") conflicts.push(result);
      else if (result.status === "saved") {
        const job = commentJob(result.id);
        if (job) {
          setSavedComments((current) => ({ ...current, [job.id]: result.count }));
          setCommentBusyIds((current) => { const next = new Set(current); next.delete(job.id); return next; });
        }
      } else if (result.status === "failed") {
        const job = commentJob(result.id);
        if (job) {
          setCommentErrors((current) => ({ ...current, [job.id]: t.library.failed(result.message) }));
          setCommentBusyIds((current) => { const next = new Set(current); next.delete(job.id); return next; });
        }
      }
    }
    if (conflicts.length > 0) setCommentConflicts((current) => [...current, ...conflicts]);
  }

  async function extractComments(targets: JobRow[]) {
    const eligible = targets.filter(jobCanExtractComments);
    if (eligible.length === 0) return;
    setCommentBusyIds((current) => new Set([...current, ...eligible.map((job) => job.id)]));
    setCommentErrors((current) => { const next = { ...current }; for (const job of eligible) delete next[job.id]; return next; });
    try {
      acceptCommentResults(await ipc.call("comments_extract", { libraryIds: eligible.map((job) => job.libraryId!) }));
    } catch (error) {
      for (const job of eligible) setCommentErrors((current) => ({ ...current, [job.id]: t.library.failed(errorMessage(error)) }));
      setCommentBusyIds((current) => { const next = new Set(current); for (const job of eligible) next.delete(job.id); return next; });
    }
  }

  async function resolveComments(answer: CommentsConflictChoice) {
    const [current, ...rest] = commentConflicts;
    if (!current) return;
    setCommentConflicts([]);
    const resolve = async (conflict: typeof current, choice: "existing" | "new") => {
      const job = commentJob(conflict.id);
      try { acceptCommentResults([await ipc.call("comments_resolve", { token: conflict.token, keep: choice })]); }
      catch (error) { if (job) setCommentErrors((errors) => ({ ...errors, [job.id]: t.library.failed(errorMessage(error)) })); }
      finally { if (job) setCommentBusyIds((busy) => { const next = new Set(busy); next.delete(job.id); return next; }); }
    };
    await resolve(current, answer.choice);
    if (answer.applyRemaining) for (const conflict of rest) await resolve(conflict, answer.choice);
    else setCommentConflicts(rest);
  }

  function singleMenuEntries(job: JobRow): MenuEntry[] {
    const entries: MenuEntry[] = [];
    if (!isWide) entries.push({ kind: "item", label: t.downloads.showDetails, hint: "Enter", onSelect: () => onOpenDetail(job.id) });
    const separator = () => { if (entries[entries.length - 1]?.kind !== "separator") entries.push({ kind: "separator" }); };
    if (job.listCount != null) {
      const registered = sources.some((source) => source.url === job.url);
      entries.push({ kind: "item", label: t.sources.rowRegister, disabled: registered, onSelect: () => rowRefs.current.get(job.id)?.register() });
      entries.push({ kind: "item", label: t.downloads.copyUrl, onSelect: () => copyUrl(job) });
      separator();
      entries.push({ kind: "item", label: t.downloads.removeFromList, onSelect: () => void removeRows([job]) });
      return entries;
    }
    if (hasSavedFile(job)) {
      entries.push({ kind: "item", label: t.common.open, onSelect: () => void ipc.call("open_job_file", { id: job.id }) });
      if (job.libraryId != null) {
        entries.push({ kind: "item", label: t.library.openWith, onSelect: () => void ipc.call("library_open_with", { id: job.libraryId! }) });
        entries.push({ kind: "item", label: t.library.rename, hint: IS_MAC ? "\u21a9" : "F2", onSelect: () => rowRefs.current.get(job.id)?.startRename() });
        if (jobCanExtractComments(job)) entries.push({ kind: "item", label: t.library.extractComments, disabled: commentBusyIds.has(job.id), onSelect: () => void extractComments([job]) });
      }
      entries.push({ kind: "item", label: t.common.revealInFolder, onSelect: () => void revealPath(job.outputPath!) });
      entries.push({ kind: "item", label: t.downloads.copyUrl, onSelect: () => copyUrl(job) });
      separator();
      if (job.libraryId != null) entries.push({ kind: "item", label: words.menu, hint: IS_MAC ? "⌘⌫" : "Delete", danger: true, onSelect: () => setConfirming({ trash: [job], remove: [] }) });
      entries.push({ kind: "item", label: t.downloads.removeFromList, onSelect: () => void removeRows([job]) });
      return entries;
    }
    if (job.alreadySaved && job.state === "Done") entries.push({ kind: "item", label: t.common.saveAgain, onSelect: () => void saveAgain(job.id) });
    else if (job.state === "Queued" || job.state === "Ready") entries.push({ kind: "item", label: t.common.removeFromQueue, onSelect: () => void cancel(job.id) });
    else if (jobIsBusy(job)) entries.push({ kind: "item", label: t.common.stop, onSelect: () => void cancel(job.id) });
    else if (job.state === "Failed" || job.state === "Canceled") {
      entries.push(job.error?.category === "LoginRequired"
        ? { kind: "item", label: t.common.login, onSelect: () => onLogin(job) }
        : { kind: "item", label: t.common.retry, onSelect: () => void retry(job.id) });
    }
    entries.push({ kind: "item", label: t.downloads.copyUrl, onSelect: () => copyUrl(job) });
    if (jobCanBeRemoved(job)) {
      separator();
      entries.push({ kind: "item", label: t.downloads.removeFromList, onSelect: () => void removeRows([job]) });
    }
    return entries;
  }

  function menuEntries(job: JobRow): MenuEntry[] {
    const targets = menuJobs(job);
    if (targets.length < 2) return singleMenuEntries(job);
    const actions = getJobSelectionActions(targets);
    const entries: MenuEntry[] = [];
    const commentJobs = targets.filter(jobCanExtractComments);
    if (commentJobs.length > 0) {
      entries.push({ kind: "item", label: t.library.extractCommentsCount(commentJobs.length), onSelect: () => void extractComments(commentJobs) });
      entries.push({ kind: "separator" });
    }
    const hasUpperActions = actions.some((group) => group.action === "stop" || group.action === "retry");
    let lowerActionsStarted = false;
    for (const group of actions) {
      const lower = group.action === "trash" || group.action === "remove";
      if (lower && hasUpperActions && !lowerActionsStarted) entries.push({ kind: "separator" });
      if (lower) lowerActionsStarted = true;
      entries.push({ kind: "item", label: actionLabel(group.action, group.jobs.length), danger: group.danger, onSelect: () => void runAction(group.action, group.jobs) });
    }
    return entries;
  }

  function handleRowKeyDown(e: KeyboardEvent<HTMLDivElement>, job: JobRow) {
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
    const index = ids.indexOf(job.id);
    let target: number | undefined;
    if (e.key === "ArrowUp") target = ids[Math.max(0, index - 1)];
    else if (e.key === "ArrowDown") target = ids[Math.min(ids.length - 1, index + 1)];
    else if (e.key === "Home") target = ids[0];
    else if (e.key === "End") target = ids[ids.length - 1];
    if (target != null) {
      e.preventDefault();
      selection.act({ type: "move", id: target, mod, shift: e.shiftKey });
      onFocusJob(target);
      requestAnimationFrame(() => rowRefs.current.get(target!)?.focus());
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      (e.currentTarget.querySelector("[role='gridcell']:last-child button:not(:disabled)") as HTMLButtonElement | null)?.focus();
    } else if (e.key === " " && !e.shiftKey) {
      e.preventDefault(); selection.act({ type: "space", id: job.id, mod });
    } else if (mod && e.key.toLowerCase() === "a") {
      e.preventDefault(); selection.act({ type: "selectAll" });
    } else if (e.key === "Escape" && selection.selected.size > 0) {
      e.preventDefault(); selection.act({ type: "clear" });
    } else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      e.preventDefault(); rowRefs.current.get(job.id)?.openMenu();
    } else if (e.key === "F2" || (IS_MAC && !mod && e.key === "Enter")) {
      if (selectedJobs.length === 1 && selectedJobs[0].libraryId != null && selectedJobs[0].outputPath) {
        e.preventDefault(); rowRefs.current.get(selectedJobs[0].id)?.startRename();
      }
    } else if (e.key === "Enter") {
      e.preventDefault(); onOpenDetail(job.id);
    } else if ((!IS_MAC && e.key === "Delete") || (IS_MAC && e.metaKey && e.key === "Backspace")) {
      e.preventDefault(); runDelete();
    }
  }

  const headerActions = selectedJobs.length >= 2
    ? getJobSelectionActions(selectedJobs).filter((group) => group.action === "trash" || group.action === "remove")
    : [];
  const selectedCommentJobs = selectedJobs.filter(jobCanExtractComments);

  return (
    <section className={styles.downloads}>
      <div className={styles.head}>
        {selectedJobs.length >= 2
          ? <SelectionBar summary={t.common.selected(selectedJobs.length)} clearLabel={t.common.clearSelection} actions={[...(selectedCommentJobs.length > 0 ? [{ label: t.library.extractCommentsCount(selectedCommentJobs.length), onClick: () => void extractComments(selectedCommentJobs) }] : []), ...headerActions.map((group) => ({ label: actionLabel(group.action, group.jobs.length), danger: group.danger, onClick: () => void runAction(group.action, group.jobs) }))]} onClear={() => selection.act({ type: "clear" })} />
          : <><h1>{t.downloads.title}</h1><div className={styles.summary}>{t.downloads.summary(counts)}</div></>}
      </div>
      <UrlBar textareaRef={urlFieldRef} defaultPreset={settings?.defaultPreset ?? "best"} downloadDir={settings?.downloadDir ?? ""} recentDownloadDirs={settings?.recentDownloadDirs ?? []} onAdd={(urls, preset) => void enqueue(urls, preset)} onChooseDownloadDir={chooseDownloadDir} onPickDownloadDir={() => void pickDownloadDir()} />
      {clipboardUrl && <ClipboardBanner url={clipboardUrl} onAdd={() => { void enqueue([clipboardUrl], settings?.defaultPreset ?? "best"); setDismissedUrls((prev) => new Set(prev).add(clipboardUrl)); setClipboardUrl(null); }} onDismiss={() => { setDismissedUrls((prev) => new Set(prev).add(clipboardUrl)); setClipboardUrl(null); }} />}
      {missingTools.length > 0 && <ToolsNotice onInstall={() => { onOpenToolsSettings(); void installTools(); }} />}
      {notice && <div className={styles.notice} role="status"><TriangleAlert size={16} aria-hidden /><span>{notice}</span><IconButton label={t.common.close} size="sm" onClick={() => setNotice(null)}><X size={16} aria-hidden /></IconButton></div>}
      {jobs.length === 0 ? <EmptyState icon={<Inbox size={32} />} title={t.downloads.emptyTitle} body={t.downloads.emptyBody} /> : (
        <div className={styles.listArea}>
          <ul className={styles.list} role="grid" aria-multiselectable="true" aria-label={t.downloads.title}>
            {jobs.map((job) => (
              <li key={job.id} role="presentation">
                <QueueRow
                  rowRef={(handle) => {
                    if (handle) rowRefs.current.set(job.id, handle);
                    else rowRefs.current.delete(job.id);
                  }}
                  job={job}
                  selected={selection.selected.has(job.id)}
                  tabIndex={(selection.focus ?? ids[0]) === job.id ? 0 : -1}
                  menuEntries={menuEntries(job)}
                  commentStatus={commentBusyIds.has(job.id) ? t.library.extractingComments : undefined}
                  commentNotice={savedComments[job.id] != null ? t.library.savedComments(savedComments[job.id]) : undefined}
                  commentError={commentErrors[job.id]}
                  onSelect={(event) => {
                    selection.act({ type: "click", id: job.id, mod: IS_MAC ? event.metaKey : event.ctrlKey, shift: event.shiftKey });
                    onFocusJob(job.id);
                  }}
                  onSelectForMenu={() => {
                    selection.act({ type: "context", id: job.id });
                    onFocusJob(job.id);
                  }}
                  onRowKeyDown={(event) => handleRowKeyDown(event, job)}
                  onOpenDetail={() => onOpenDetail(job.id)}
                  onCancel={() => void cancel(job.id)}
                  onRetry={() => void retry(job.id)}
                  onReveal={(path) => void revealPath(path)}
                  onLogin={() => onLogin(job)}
                  onSaveAgain={() => void saveAgain(job.id)}
                  onChooseMedia={() => onChooseMedia(job)}
                />
              </li>
            ))}
          </ul>
        </div>
      )}
      {confirming && <ConfirmDialog title={words.title} body={`${confirming.trash.length === 1 ? words.body(confirming.trash[0].details?.title ?? confirming.trash[0].url) : words.bulkBody(confirming.trash[0].details?.title ?? confirming.trash[0].url, confirming.trash.length)}${confirming.remove.length > 0 ? t.downloads.removedAlongside(confirming.remove.length) : ""}`} cancelLabel={t.common.dialogCancel} confirmLabel={words.confirm} onCancel={() => setConfirming(null)} onConfirm={() => void confirmTrash()} />}
      {commentConflicts[0] && <CommentsConflictDialog fileName={commentConflicts[0].fileName} existingCount={commentConflicts[0].existingCount} existingDate={commentConflicts[0].existingModified == null ? "—" : formatCommentDate(commentConflicts[0].existingModified)} newCount={commentConflicts[0].newCount} remaining={commentConflicts.length - 1} onCancel={() => void resolveComments({ choice: "existing", applyRemaining: false })} onDecide={(answer) => void resolveComments(answer)} />}
    </section>
  );
}

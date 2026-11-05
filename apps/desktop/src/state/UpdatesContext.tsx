// In-app updates (SPEC "In-app updates"): the check at launch, Settings > アップデート and the
// update dialog share this state. At launch the app checks once settings have loaded; a newer
// version that is not the skipped one opens the dialog, anything else (latest, offline, an
// error) says nothing. 今すぐ更新 waits while a job downloads or post-processes; あとで taken
// then asks again once the queue is idle, once per launch. Once the download starts, the dialog
// stays until the app restarts or the update fails.
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { updater } from "../ipc";
import type { UpdateInfo } from "../ipc/contract";
import { activeJobCount } from "../lib/jobStats";
import { useJobs } from "./JobsContext";
import { useSettings } from "./SettingsContext";

/**
 * What the last check said (checkedAt in Unix seconds). A launch check that failed says
 * nothing, so it leaves "none".
 */
export type UpdateCheck =
  | { status: "none" }
  | { status: "checking" }
  | { status: "latest"; checkedAt: number }
  | { status: "found"; update: UpdateInfo; checkedAt: number }
  | { status: "failed"; message: string };

export type UpdateDialogState =
  | { phase: "offer"; update: UpdateInfo }
  | { phase: "downloading"; update: UpdateInfo; downloadedBytes: number; totalBytes: number | null }
  | { phase: "failed"; update: UpdateInfo; message: string };

interface UpdatesContextValue {
  /** The running version; null until known. */
  appVersion: string | null;
  check: UpdateCheck;
  /** The found version is the one the user skipped. */
  skipped: boolean;
  /** Jobs downloading or post-processing; 今すぐ更新 is off while there are any. */
  runningJobs: number;
  /** The update dialog, or null while it is closed. */
  dialog: UpdateDialogState | null;
  /** Settings' アップデートを確認: checks and always reports in Settings, never opens the dialog. */
  checkNow: () => void;
  /** Settings' 更新する: downloads at once, or opens the dialog to say why it has to wait. */
  install: () => void;
  /** Settings' スキップをやめる. */
  unskip: () => void;
  /** The dialog's この版はスキップ. */
  skip: () => void;
  /** The dialog's あとで (and Escape, a backdrop click). */
  later: () => void;
  /** The dialog's 今すぐ更新. */
  updateNow: () => void;
  /** The dialog's 閉じる after a failed update. */
  closeFailed: () => void;
}

/** Progress is drawn at most this often; the updater reports every chunk. */
const PROGRESS_INTERVAL_MS = 100;

const UpdatesContext = createContext<UpdatesContextValue | null>(null);

/** One line: the plugins reject with a string, the mock with an Error. */
function messageOf(error: unknown): string {
  const text = error instanceof Error ? error.message : typeof error === "string" ? error : String(error);
  return text.replace(/\s+/g, " ").trim() || "unknown error";
}

export function UpdatesProvider({ children }: { children: ReactNode }) {
  const { settings, updateSettings } = useSettings();
  const { jobs } = useJobs();

  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [check, setCheck] = useState<UpdateCheck>({ status: "none" });
  const [dialog, setDialog] = useState<UpdateDialogState | null>(null);
  /** あとで was taken while jobs ran: the update to offer again once the queue is idle. */
  const [askAgain, setAskAgain] = useState<UpdateInfo | null>(null);

  const checking = useRef(false);
  const launchChecked = useRef(false);
  const askedAgain = useRef(false);
  const downloading = useRef(false);

  const loaded = settings != null;
  const skippedVersion = settings?.skippedUpdateVersion ?? null;
  const skippedRef = useRef(skippedVersion);
  useEffect(() => {
    skippedRef.current = skippedVersion;
  }, [skippedVersion]);

  const runningJobs = useMemo(
    () => jobs.filter((job) => job.state === "Downloading" || job.state === "PostProcessing").length,
    [jobs],
  );
  const queueIdle = activeJobCount(jobs) === 0;

  useEffect(() => {
    updater.appVersion().then(setAppVersion, () => {});
  }, []);

  /** Resolves with the update found, or null (latest, or the check failed). */
  const runCheck = useCallback(async (fromLaunch: boolean): Promise<UpdateInfo | null> => {
    if (checking.current) return null;
    checking.current = true;
    setCheck({ status: "checking" });
    try {
      const update = await updater.check();
      const checkedAt = Math.floor(Date.now() / 1000);
      setCheck(update ? { status: "found", update, checkedAt } : { status: "latest", checkedAt });
      if (!update) setAskAgain(null);
      return update;
    } catch (error) {
      setCheck(fromLaunch ? { status: "none" } : { status: "failed", message: messageOf(error) });
      return null;
    } finally {
      checking.current = false;
    }
  }, []);

  // The launch check, once settings (and with them the skipped version) have loaded.
  useEffect(() => {
    if (!loaded || launchChecked.current) return;
    launchChecked.current = true;
    void runCheck(true).then((update) => {
      if (update && update.version !== skippedRef.current) {
        setDialog((current) => current ?? { phase: "offer", update });
      }
    });
  }, [loaded, runCheck]);

  // あとで while jobs ran: 「終わってからもう一度お知らせします」.
  useEffect(() => {
    if (!askAgain || !queueIdle || dialog) return;
    setAskAgain(null);
    if (askAgain.version !== skippedVersion) setDialog({ phase: "offer", update: askAgain });
  }, [askAgain, queueIdle, dialog, skippedVersion]);

  const startDownload = useCallback(async (update: UpdateInfo) => {
    if (downloading.current) return;
    downloading.current = true;
    setAskAgain(null);
    setDialog({ phase: "downloading", update, downloadedBytes: 0, totalBytes: null });
    let downloadedBytes = 0;
    let totalBytes: number | null = null;
    let timer: number | undefined;
    const flush = () => {
      timer = undefined;
      setDialog((current) =>
        current?.phase === "downloading" ? { ...current, downloadedBytes, totalBytes } : current,
      );
    };
    try {
      await updater.downloadAndInstall((event) => {
        if (event.kind === "started") totalBytes = event.totalBytes;
        else if (event.kind === "progress") downloadedBytes += event.chunkBytes;
        // Done without a size: the bar ends full.
        else if (totalBytes == null) totalBytes = downloadedBytes;
        if (event.kind === "finished") flush();
        else if (timer === undefined) timer = window.setTimeout(flush, PROGRESS_INTERVAL_MS);
      });
      // Windows does not get here: the installer has taken over and the app has exited.
      await updater.relaunch();
    } catch (error) {
      window.clearTimeout(timer);
      setDialog({ phase: "failed", update, message: messageOf(error) });
    } finally {
      downloading.current = false;
    }
  }, []);

  const checkNow = useCallback(() => {
    void runCheck(false);
  }, [runCheck]);

  const install = useCallback(() => {
    if (check.status !== "found" || dialog) return;
    if (runningJobs > 0) setDialog({ phase: "offer", update: check.update });
    else void startDownload(check.update);
  }, [check, dialog, runningJobs, startDownload]);

  const unskip = useCallback(() => {
    updateSettings({ skippedUpdateVersion: null });
  }, [updateSettings]);

  const skip = useCallback(() => {
    if (dialog?.phase !== "offer") return;
    updateSettings({ skippedUpdateVersion: dialog.update.version });
    setAskAgain(null);
    setDialog(null);
  }, [dialog, updateSettings]);

  const later = useCallback(() => {
    if (dialog?.phase !== "offer") return;
    if (runningJobs > 0 && !askedAgain.current) {
      askedAgain.current = true;
      setAskAgain(dialog.update);
    }
    setDialog(null);
  }, [dialog, runningJobs]);

  const updateNow = useCallback(() => {
    if (dialog?.phase !== "offer" || runningJobs > 0) return;
    void startDownload(dialog.update);
  }, [dialog, runningJobs, startDownload]);

  const closeFailed = useCallback(() => {
    if (dialog?.phase === "failed") setDialog(null);
  }, [dialog]);

  const skipped = check.status === "found" && check.update.version === skippedVersion;

  return (
    <UpdatesContext.Provider
      value={{
        appVersion,
        check,
        skipped,
        runningJobs,
        dialog,
        checkNow,
        install,
        unskip,
        skip,
        later,
        updateNow,
        closeFailed,
      }}
    >
      {children}
    </UpdatesContext.Provider>
  );
}

export function useUpdates(): UpdatesContextValue {
  const ctx = useContext(UpdatesContext);
  if (!ctx) throw new Error("useUpdates() used outside <UpdatesProvider>");
  return ctx;
}

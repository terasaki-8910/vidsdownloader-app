import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import type { Commands, JobRow, Preset } from "../ipc/contract";
import { ipc } from "../ipc";

/** enqueue_page_images' arguments without the UTC offset, which this context adds. */
export type PageImagesRequest = Omit<Commands["enqueue_page_images"]["args"], "tzOffsetSeconds">;
/** enqueue_page_media's arguments without the UTC offset, which this context adds. */
export type PageMediaRequest = Omit<Commands["enqueue_page_media"]["args"], "tzOffsetSeconds">;

/**
 * The local UTC offset in seconds. The Rust side uses it to date output file names in the
 * user's local time; the webview knows it reliably on every OS.
 */
function localTzOffsetSeconds(): number {
  return -new Date().getTimezoneOffset() * 60;
}

interface JobsContextValue {
  jobs: JobRow[];
  /**
   * The preset a job was enqueued with, known only for jobs added during this
   * session (see the report: JobRow carries no preset field, so the detail
   * panel's "quality" row has nothing to show for jobs that already existed
   * before the webview started, e.g. restored from a previous run).
   */
  presetFor: (id: number) => Preset | undefined;
  enqueue: (urls: string[], preset: Preset) => Promise<number[]>;
  /** Queues one job that saves the chosen images of a page directly; resolves to its id. */
  enqueuePageImages: (page: PageImagesRequest) => Promise<number>;
  /** Queues one direct-stream job per item and resolves to their ids. */
  enqueuePageMedia: (request: PageMediaRequest) => Promise<number[]>;
  cancel: (id: number) => Promise<void>;
  retry: (id: number) => Promise<void>;
  choosePageMedia: (id: number, url: string) => Promise<void>;
  /** Downloads a video that was skipped because it is already saved. */
  saveAgain: (id: number) => Promise<void>;
}

const JobsContext = createContext<JobsContextValue | null>(null);

export function JobsProvider({ children }: { children: ReactNode }) {
  const [jobs, setJobs] = useState<JobRow[]>([]);
  const [presetByJobId, setPresetByJobId] = useState<Record<number, Preset>>({});

  useEffect(() => {
    // Rows that arrive as events before the snapshot are newer than it, so the
    // snapshot must not overwrite them.
    let snapshotLoaded = false;
    const earlyRows = new Map<number, JobRow>();
    // Likewise a removal that arrives before the snapshot: the snapshot may still list it.
    const removedEarly = new Set<number>();

    const unsubscribe = ipc.on("job-updated", (row) => {
      if (!snapshotLoaded) earlyRows.set(row.id, row);
      setJobs((prev) => {
        const idx = prev.findIndex((j) => j.id === row.id);
        if (idx === -1) return [...prev, row];
        const next = prev.slice();
        next[idx] = row;
        return next;
      });
    });

    // A saved item's records were erased: its queue row is gone for good.
    const unsubscribeRemoved = ipc.on("job-removed", ({ id }) => {
      if (!snapshotLoaded) {
        earlyRows.delete(id);
        removedEarly.add(id);
      }
      setJobs((prev) => prev.filter((j) => j.id !== id));
      setPresetByJobId((prev) => {
        if (!(id in prev)) return prev;
        const next = { ...prev };
        delete next[id];
        return next;
      });
    });

    void ipc.call("queue_snapshot", {}).then((snapshot) => {
      snapshotLoaded = true;
      const kept = snapshot.filter((row) => !removedEarly.has(row.id));
      const merged = kept.map((row) => earlyRows.get(row.id) ?? row);
      const known = new Set(kept.map((row) => row.id));
      for (const row of earlyRows.values()) {
        if (!known.has(row.id)) merged.push(row);
      }
      setJobs(merged);
    });

    return () => {
      unsubscribe();
      unsubscribeRemoved();
    };
  }, []);

  const presetFor = useCallback(
    (id: number) => presetByJobId[id],
    [presetByJobId],
  );

  const enqueue = useCallback(async (urls: string[], preset: Preset) => {
    // Computed here, once, for every caller (the URL bar and the clipboard
    // banner both go through this method).
    const tzOffsetSeconds = localTzOffsetSeconds();
    const ids = await ipc.call("enqueue_urls", { urls, preset, tzOffsetSeconds });
    setPresetByJobId((prev) => {
      const next = { ...prev };
      for (const id of ids) next[id] = preset;
      return next;
    });
    return ids;
  }, []);

  const enqueuePageImages = useCallback(
    (page: PageImagesRequest) => ipc.call("enqueue_page_images", { ...page, tzOffsetSeconds: localTzOffsetSeconds() }),
    [],
  );

  const enqueuePageMedia = useCallback(async (request: PageMediaRequest) => {
    const ids = await ipc.call("enqueue_page_media", { ...request, tzOffsetSeconds: localTzOffsetSeconds() });
    setPresetByJobId((prev) => {
      const next = { ...prev };
      for (const id of ids) next[id] = request.preset;
      return next;
    });
    return ids;
  }, []);

  const cancel = useCallback(async (id: number) => {
    await ipc.call("cancel_job", { id });
  }, []);

  const retry = useCallback(async (id: number) => {
    await ipc.call("retry_job", { id });
  }, []);

  const choosePageMedia = useCallback(async (id: number, url: string) => {
    await ipc.call("page_media_choose", { jobId: id, url });
  }, []);

  const saveAgain = useCallback(async (id: number) => {
    await ipc.call("save_again", { id });
  }, []);

  return (
    <JobsContext.Provider value={{ jobs, presetFor, enqueue, enqueuePageImages, enqueuePageMedia, cancel, retry, choosePageMedia, saveAgain }}>
      {children}
    </JobsContext.Provider>
  );
}

export function useJobs(): JobsContextValue {
  const ctx = useContext(JobsContext);
  if (!ctx) throw new Error("useJobs() used outside <JobsProvider>");
  return ctx;
}

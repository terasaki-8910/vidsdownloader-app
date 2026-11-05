import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ipc } from "../ipc";
import type { Preset, SourcePreview, SourcePreviewOutcome, SourceRow } from "../ipc/contract";
import { errorMessage } from "../lib/url";

/** The local UTC offset in seconds, which the Rust side uses to date files and checks. */
function localTzOffsetSeconds(): number {
  return -new Date().getTimezoneOffset() * 60;
}

/** How a registration ended. The caller shows a failure or an existing source; the rest is silent. */
export type RegisterOutcome =
  | { kind: "registered"; source: SourceRow }
  /** The first-check question was answered with キャンセル (or the dialog was dismissed). */
  | { kind: "cancelled" }
  /** A source with this URL exists already; id is null when only the register command knew. */
  | { kind: "exists"; id: number | null }
  | { kind: "failed"; reason: "notAList" | "loginRequired" | "unsupported" | "other"; message: string };

/** What the first-check question needs: the preview of the source being registered. */
export type PendingFirstCheck = Pick<SourcePreview, "kind" | "name" | "count">;

/**
 * Where a registration is: previewing (yt-dlp or the collect session is being read), asking
 * (the first-check question is open), registering (the answer is being stored). Only one
 * registration runs at a time, because the backend keeps the newest preview only.
 */
export type RegisterPhase = "idle" | "previewing" | "asking" | "registering";

interface SourcesContextValue {
  /** Every source, in id order. */
  sources: SourceRow[];
  /** False until the first list has arrived. */
  loaded: boolean;
  phase: RegisterPhase;
  /** The question being asked, else null. */
  firstCheck: PendingFirstCheck | null;
  /**
   * Looks the URL up and, for a new list, asks what to do with the items there now; resolves
   * when the registration is over (registered, cancelled or failed).
   */
  register: (url: string) => Promise<RegisterOutcome>;
  /** The same for the page of the open collect session (a rule source). */
  registerFromCollect: () => Promise<RegisterOutcome>;
  /** The answer to the first-check question. */
  answerFirstCheck: (choice: "record" | "saveAll") => void;
  cancelFirstCheck: () => void;
  check: (id: number) => Promise<void>;
  checkAll: () => Promise<void>;
  rename: (id: number, name: string) => Promise<void>;
  setOptions: (id: number, destDir: string | null, preset: Preset | null) => Promise<void>;
  remove: (id: number) => Promise<void>;
}

const SourcesContext = createContext<SourcesContextValue | null>(null);

function inIdOrder(list: SourceRow[]): SourceRow[] {
  return [...list].sort((a, b) => a.id - b.id);
}

function upsert(list: SourceRow[], row: SourceRow): SourceRow[] {
  const index = list.findIndex((s) => s.id === row.id);
  if (index === -1) return inIdOrder([...list, row]);
  const next = list.slice();
  next[index] = row;
  return next;
}

/** A waiting registration: its preview and how to finish the caller's promise. */
interface Asking {
  preview: SourcePreview;
  resolve: (outcome: RegisterOutcome) => void;
}

export function SourcesProvider({ children }: { children: ReactNode }) {
  const [sources, setSources] = useState<SourceRow[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [phase, setPhase] = useState<RegisterPhase>("idle");
  const [firstCheck, setFirstCheck] = useState<PendingFirstCheck | null>(null);

  const sourcesRef = useRef(sources);
  const busy = useRef(false);
  const asking = useRef<Asking | null>(null);
  useEffect(() => {
    sourcesRef.current = sources;
  });

  useEffect(() => {
    // Rows that arrive as events before the list are newer than it, so the list must not
    // overwrite them; likewise a removal that arrives first.
    let listLoaded = false;
    const earlyRows = new Map<number, SourceRow>();
    const removedEarly = new Set<number>();

    const offUpdated = ipc.on("source-updated", (row) => {
      if (!listLoaded) {
        earlyRows.set(row.id, row);
        removedEarly.delete(row.id);
      }
      setSources((prev) => upsert(prev, row));
    });
    const offRemoved = ipc.on("source-removed", ({ id }) => {
      if (!listLoaded) {
        earlyRows.delete(id);
        removedEarly.add(id);
      }
      setSources((prev) => prev.filter((s) => s.id !== id));
    });

    void ipc.call("sources_list", { tzOffsetSeconds: localTzOffsetSeconds() }).then(
      (list) => {
        listLoaded = true;
        const kept = list.filter((row) => !removedEarly.has(row.id)).map((row) => earlyRows.get(row.id) ?? row);
        const known = new Set(kept.map((row) => row.id));
        for (const row of earlyRows.values()) {
          if (!known.has(row.id)) kept.push(row);
        }
        setSources(inIdOrder(kept));
        setLoaded(true);
      },
      () => setLoaded(true),
    );

    return () => {
      offUpdated();
      offRemoved();
    };
  }, []);

  const finish = useCallback(() => {
    busy.current = false;
    asking.current = null;
    setFirstCheck(null);
    setPhase("idle");
  }, []);

  const startRegistration = useCallback(
    async (lookUp: () => Promise<SourcePreviewOutcome>): Promise<RegisterOutcome> => {
      if (busy.current) return { kind: "failed", reason: "other", message: "another registration is running" };
      busy.current = true;
      setPhase("previewing");

      let outcome: SourcePreviewOutcome;
      try {
        outcome = await lookUp();
      } catch (error) {
        finish();
        return { kind: "failed", reason: "other", message: errorMessage(error) };
      }
      if (outcome.kind === "failed") {
        finish();
        return { kind: "failed", reason: outcome.reason, message: outcome.message };
      }
      const { preview } = outcome;
      if (preview.existingId != null) {
        finish();
        return { kind: "exists", id: preview.existingId };
      }

      return new Promise<RegisterOutcome>((resolve) => {
        asking.current = { preview, resolve };
        setFirstCheck({ kind: preview.kind, name: preview.name, count: preview.count });
        setPhase("asking");
      });
    },
    [finish],
  );

  const register = useCallback(
    (url: string) => startRegistration(() => ipc.call("source_preview", { url })),
    [startRegistration],
  );

  const registerFromCollect = useCallback(
    () =>
      startRegistration(async () => ({
        kind: "ready" as const,
        preview: await ipc.call("source_preview_collect", {}),
      })),
    [startRegistration],
  );

  const answerFirstCheck = useCallback(
    (choice: "record" | "saveAll") => {
      const current = asking.current;
      if (!current) return;
      asking.current = null;
      setFirstCheck(null);
      setPhase("registering");
      ipc
        .call("source_register", { token: current.preview.token, choice, tzOffsetSeconds: localTzOffsetSeconds() })
        .then(
          (source) => {
            // Its "source-updated" event usually arrived first and may be newer.
            setSources((prev) => (prev.some((s) => s.id === source.id) ? prev : upsert(prev, source)));
            finish();
            current.resolve({ kind: "registered", source });
          },
          (error) => {
            const message = errorMessage(error);
            // The same URL was registered between the preview and the answer.
            const existing = /already a source/i.test(message)
              ? (sourcesRef.current.find((s) => s.url === current.preview.url)?.id ?? null)
              : undefined;
            finish();
            current.resolve(
              existing !== undefined ? { kind: "exists", id: existing } : { kind: "failed", reason: "other", message },
            );
          },
        );
    },
    [finish],
  );

  const cancelFirstCheck = useCallback(() => {
    const current = asking.current;
    if (!current) return;
    finish();
    current.resolve({ kind: "cancelled" });
  }, [finish]);

  const check = useCallback(async (id: number) => {
    await ipc.call("source_check", { id, tzOffsetSeconds: localTzOffsetSeconds() });
  }, []);

  const checkAll = useCallback(async () => {
    await ipc.call("source_check_all", { tzOffsetSeconds: localTzOffsetSeconds() });
  }, []);

  // The result is applied field by field: a "source-updated" event about a check may have
  // arrived first, and its status is newer than the row this command returns.
  const rename = useCallback(async (id: number, name: string) => {
    const row = await ipc.call("source_rename", { id, name });
    setSources((prev) => prev.map((s) => (s.id === id ? { ...s, name: row.name } : s)));
  }, []);

  const setOptions = useCallback(async (id: number, destDir: string | null, preset: Preset | null) => {
    const row = await ipc.call("source_set_options", { id, destDir, preset });
    setSources((prev) => prev.map((s) => (s.id === id ? { ...s, destDir: row.destDir, preset: row.preset } : s)));
  }, []);

  const remove = useCallback(async (id: number) => {
    await ipc.call("source_delete", { id });
    setSources((prev) => prev.filter((s) => s.id !== id));
  }, []);

  const value = useMemo<SourcesContextValue>(
    () => ({
      sources,
      loaded,
      phase,
      firstCheck,
      register,
      registerFromCollect,
      answerFirstCheck,
      cancelFirstCheck,
      check,
      checkAll,
      rename,
      setOptions,
      remove,
    }),
    [
      sources,
      loaded,
      phase,
      firstCheck,
      register,
      registerFromCollect,
      answerFirstCheck,
      cancelFirstCheck,
      check,
      checkAll,
      rename,
      setOptions,
      remove,
    ],
  );

  return <SourcesContext.Provider value={value}>{children}</SourcesContext.Provider>;
}

export function useSources(): SourcesContextValue {
  const ctx = useContext(SourcesContext);
  if (!ctx) throw new Error("useSources() used outside <SourcesProvider>");
  return ctx;
}

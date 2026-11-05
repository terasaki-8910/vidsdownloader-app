import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { ToolId, ToolProgress, ToolStatus } from "../ipc/contract";
import { ipc } from "../ipc";

interface ToolsContextValue {
  status: ToolStatus[];
  progress: Partial<Record<ToolId, ToolProgress>>;
  missing: ToolId[];
  installing: boolean;
  install: () => Promise<void>;
  /** Reads the installed versions again (tools can change outside the app). */
  refresh: () => void;
}

const ToolsContext = createContext<ToolsContextValue | null>(null);

export function ToolsProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<ToolStatus[]>([]);
  const [progress, setProgress] = useState<Partial<Record<ToolId, ToolProgress>>>({});
  const [installing, setInstalling] = useState(false);
  // A second call while one runs would be refused by the backend; ignore it here.
  const installingRef = useRef(false);

  const refresh = useCallback(() => {
    void ipc.call("tools_status", {}).then(setStatus);
  }, []);

  useEffect(() => {
    refresh();
    const unsubscribe = ipc.on("tools-progress", (p) => {
      setProgress((prev) => ({ ...prev, [p.tool]: p }));
    });
    return unsubscribe;
  }, [refresh]);

  const install = useCallback(async () => {
    if (installingRef.current) return;
    installingRef.current = true;
    setInstalling(true);
    try {
      await ipc.call("install_tools", {});
    } finally {
      installingRef.current = false;
      refresh();
      setInstalling(false);
    }
  }, [refresh]);

  const missing = useMemo(
    () =>
      status
        .filter((s) => s.tool !== "ublock-lite" && s.tool !== "gallery-dl" && !s.installed)
        .map((s) => s.tool),
    [status],
  );

  return (
    <ToolsContext.Provider value={{ status, progress, missing, installing, install, refresh }}>
      {children}
    </ToolsContext.Provider>
  );
}

export function useTools(): ToolsContextValue {
  const ctx = useContext(ToolsContext);
  if (!ctx) throw new Error("useTools() used outside <ToolsProvider>");
  return ctx;
}

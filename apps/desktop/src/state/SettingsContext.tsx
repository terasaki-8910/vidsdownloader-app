import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import type { Settings } from "../ipc/contract";
import { ipc } from "../ipc";

interface SettingsContextValue {
  /** null until the first get_settings resolves. */
  settings: Settings | null;
  updateSettings: (patch: Partial<Settings>) => void;
  /** Makes `path` the download folder and moves it to the front of the recent list. */
  chooseDownloadDir: (path: string) => void;
  /** Opens the OS folder picker; a picked folder goes through chooseDownloadDir. */
  pickDownloadDir: () => Promise<void>;
}

const MAX_RECENT_DOWNLOAD_DIRS = 5;

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  // Holds the latest settings synchronously so rapid updateSettings calls (e.g.
  // two toggles in a row) always merge onto the most recent value, not a stale
  // render's closure.
  const latest = useRef<Settings | null>(null);

  useEffect(() => {
    void ipc.call("get_settings", {}).then((loaded) => {
      latest.current = loaded;
      setSettings(loaded);
    });
  }, []);

  const updateSettings = useCallback((patch: Partial<Settings>) => {
    const base = latest.current;
    if (!base) return; // ignore changes attempted before settings have loaded
    const next = { ...base, ...patch };
    latest.current = next;
    setSettings(next);
    void ipc.call("set_settings", { settings: next });
  }, []);

  const chooseDownloadDir = useCallback(
    (path: string) => {
      // The folder in use counts as recent too: before this list existed, it was the
      // only folder, and it must not drop out of the menu on the first change.
      const current = latest.current;
      const previous = current ? [current.downloadDir, ...current.recentDownloadDirs].filter(Boolean) : [];
      updateSettings({
        downloadDir: path,
        recentDownloadDirs: [...new Set([path, ...previous])].slice(0, MAX_RECENT_DOWNLOAD_DIRS),
      });
    },
    [updateSettings],
  );

  const pickDownloadDir = useCallback(async () => {
    const selected = await ipc.call("pick_download_dir", {});
    if (selected !== null) chooseDownloadDir(selected);
  }, [chooseDownloadDir]);

  return (
    <SettingsContext.Provider value={{ settings, updateSettings, chooseDownloadDir, pickDownloadDir }}>
      {children}
    </SettingsContext.Provider>
  );
}

export function useSettings(): SettingsContextValue {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error("useSettings() used outside <SettingsProvider>");
  return ctx;
}

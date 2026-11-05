// Picks the real Tauri IPC client or the in-memory mock, once, at module load.
// window.__TAURI_INTERNALS__ is the marker @tauri-apps/api itself relies on to
// detect that it's running inside a Tauri webview.
import { tauriIpcClient, tauriUpdaterClient } from "./client";
import type { Commands, Events } from "./contract";
import type { IpcClient, UpdaterClient } from "./types";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

function isTauri(): boolean {
  return typeof window !== "undefined" && window.__TAURI_INTERNALS__ !== undefined;
}

// Loads the mock on first use, so production builds (where this is never
// called) do not bundle it or its sample data.
function lazyMockClient(): IpcClient {
  const loaded = import("./mock").then((m) => m.mockIpcClient);
  return {
    call<K extends keyof Commands>(cmd: K, args: Commands[K]["args"]) {
      return loaded.then((client) => client.call(cmd, args));
    },
    on<K extends keyof Events>(event: K, handler: (payload: Events[K]) => void) {
      let unsubscribe: (() => void) | null = null;
      let cancelled = false;
      void loaded.then((client) => {
        if (!cancelled) unsubscribe = client.on(event, handler);
      });
      return () => {
        cancelled = true;
        unsubscribe?.();
      };
    },
  };
}

// The same for the updater: the mock's update is chosen with #mockUpdate= (lib/debugHash.ts).
function lazyMockUpdater(): UpdaterClient {
  const loaded = import("./mock").then((m) => m.mockUpdaterClient);
  return {
    appVersion: () => loaded.then((client) => client.appVersion()),
    check: () => loaded.then((client) => client.check()),
    downloadAndInstall: (onEvent) => loaded.then((client) => client.downloadAndInstall(onEvent)),
    relaunch: () => loaded.then((client) => client.relaunch()),
  };
}

/** True in `pnpm dev` in a plain browser, where there is no native browser view. */
export const usesMockIpc: boolean = import.meta.env.DEV && !isTauri();

// The mock exists for `pnpm dev` in a plain browser only.
export const ipc: IpcClient = usesMockIpc ? lazyMockClient() : tauriIpcClient;
export const updater: UpdaterClient = usesMockIpc ? lazyMockUpdater() : tauriUpdaterClient;

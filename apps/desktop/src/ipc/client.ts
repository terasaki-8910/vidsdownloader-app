// Real IPC: wraps @tauri-apps/api so the rest of the app only sees IpcClient, and the updater
// and process plugins so it only sees UpdaterClient.
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import type { Commands, Events, UpdateDownloadEvent, UpdateInfo } from "./contract";
import type { IpcClient, UpdaterClient } from "./types";

function call<K extends keyof Commands>(
  cmd: K,
  args: Commands[K]["args"],
): Promise<Commands[K]["result"]> {
  return invoke<Commands[K]["result"]>(cmd, args as Record<string, unknown>);
}

function on<K extends keyof Events>(
  event: K,
  handler: (payload: Events[K]) => void,
): () => void {
  let unlisten: UnlistenFn | null = null;
  let cancelled = false;

  listen<Events[K]>(event, (e) => handler(e.payload)).then((fn) => {
    if (cancelled) {
      fn();
    } else {
      unlisten = fn;
    }
  });

  return () => {
    cancelled = true;
    unlisten?.();
  };
}

export const tauriIpcClient: IpcClient = { call, on };

/** A check that gets no answer in this time fails like an offline one. */
const CHECK_TIMEOUT_MS = 30_000;

/** The update the last check found; it holds a resource on the Rust side until closed. */
let pendingUpdate: Update | null = null;

async function checkForUpdate(): Promise<UpdateInfo | null> {
  const found = await check({ timeout: CHECK_TIMEOUT_MS });
  const previous = pendingUpdate;
  pendingUpdate = found;
  if (previous && previous !== found) void previous.close().catch(() => {});
  if (!found) return null;
  return {
    version: found.version,
    currentVersion: found.currentVersion,
    notes: found.body?.trim() ? found.body : null,
  };
}

async function downloadAndInstall(onEvent: (event: UpdateDownloadEvent) => void): Promise<void> {
  const update = pendingUpdate;
  if (!update) throw new Error("no update was found; check again");
  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case "Started":
        onEvent({ kind: "started", totalBytes: event.data.contentLength ?? null });
        break;
      case "Progress":
        onEvent({ kind: "progress", chunkBytes: event.data.chunkLength });
        break;
      case "Finished":
        onEvent({ kind: "finished" });
        break;
    }
  });
}

export const tauriUpdaterClient: UpdaterClient = {
  appVersion: getVersion,
  check: checkForUpdate,
  downloadAndInstall,
  relaunch,
};

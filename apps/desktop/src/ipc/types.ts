import type { Commands, Events, UpdateDownloadEvent, UpdateInfo } from "./contract";

/**
 * The shape both the real Tauri client (./client.ts) and the in-memory
 * mock (./mock.ts) implement. index.ts picks one at module load time.
 */
export interface IpcClient {
  call<K extends keyof Commands>(
    cmd: K,
    args: Commands[K]["args"],
  ): Promise<Commands[K]["result"]>;
  /** Subscribes to an event; returns an unsubscribe function. */
  on<K extends keyof Events>(
    event: K,
    handler: (payload: Events[K]) => void,
  ): () => void;
}

/**
 * The updater and process plugins behind one interface: the real ones (./client.ts) or the
 * mock (./mock.ts). index.ts picks one at module load time, together with the IpcClient.
 */
export interface UpdaterClient {
  /** The running app's version (tauri.conf.json). */
  appVersion(): Promise<string>;
  /**
   * Asks the update endpoint. Resolves with the newer version, or null when the running one is
   * the newest; rejects with a message when offline or the answer is unusable.
   */
  check(): Promise<UpdateInfo | null>;
  /**
   * Downloads and installs the update the last check found. Windows: the installer takes over
   * and the app exits, so the promise does not settle; macOS: resolves once it is installed.
   */
  downloadAndInstall(onEvent: (event: UpdateDownloadEvent) => void): Promise<void>;
  /** Restarts the app (after an install on macOS). */
  relaunch(): Promise<void>;
}

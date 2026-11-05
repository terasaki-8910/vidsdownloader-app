import { ipc } from "../ipc";

/** Shows a saved file in the OS file manager. Used by both the queue (Done
 *  rows) and the library -- neither is really a "jobs" concern. */
export async function revealPath(path: string): Promise<void> {
  await ipc.call("reveal_path", { path });
}

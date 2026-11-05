import { convertFileSrc } from "@tauri-apps/api/core";

const PLAYABLE_EXTENSIONS = new Set([
  "mp4",
  "m4v",
  "webm",
  "mkv",
  "mov",
  "m4a",
  "mp3",
  "opus",
  "ogg",
  "wav",
  "flac",
  "aac",
]);

export function mediaUrl(jobId: number): string {
  if (typeof window === "undefined" || window.__TAURI_INTERNALS__ === undefined) return "";
  return convertFileSrc(`job/${jobId}`, "vdlmedia");
}

export function isPlayablePath(path: string): boolean {
  const fileName = path.split(/[\\/]/).pop() ?? "";
  const extension = fileName.includes(".") ? fileName.slice(fileName.lastIndexOf(".") + 1) : "";
  return PLAYABLE_EXTENSIONS.has(extension.toLowerCase());
}

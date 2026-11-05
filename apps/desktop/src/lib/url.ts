const HAS_SCHEME = /^[a-z][a-z0-9+.-]*:\/\//i;

/**
 * Turns what a person typed into an http(s) URL: adds "https://" when there is
 * no scheme. Returns null for empty input or anything that is not a plain
 * http(s) URL with a host.
 */
export function normalizeUrl(input: string): string | null {
  const text = input.trim();
  if (!text || /\s/.test(text)) return null;
  const candidate = HAS_SCHEME.test(text) ? text : `https://${text}`;
  try {
    const url = new URL(candidate);
    if ((url.protocol !== "https:" && url.protocol !== "http:") || !url.hostname) return null;
    return candidate;
  } catch {
    return null;
  }
}

/** The message of a rejected IPC call (Tauri rejects with a plain string). */
export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

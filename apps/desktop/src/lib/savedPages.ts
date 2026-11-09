// Saved pages (the browser's "bookmarks"): what is stored for a page, and what it is called until
// the person renames it. The list itself lives in SQLite behind the saved_page_* commands.

/** The URL a page is saved under: exactly as shown (query kept), without the #fragment. */
export function savedPageUrl(url: string): string {
  const fragment = url.indexOf("#");
  return fragment < 0 ? url : url.slice(0, fragment);
}

/** The most characters a saved page's name may have (the same limit as the Rust side). */
export const SAVED_PAGE_NAME_MAX = 200;

/**
 * The name a page is saved under: host and path ("www.youtube.com/@Blender/videos"), since the
 * browser reports no page title. The query is left out; the full URL is shown beside the name.
 */
export function defaultSavedPageName(url: string): string {
  let name: string;
  try {
    const parsed = new URL(url);
    const path = parsed.pathname === "/" ? "" : decodeBestEffort(parsed.pathname.replace(/\/$/, ""));
    name = `${parsed.hostname}${path}`;
  } catch {
    name = url;
  }
  return [...name].slice(0, SAVED_PAGE_NAME_MAX).join("");
}

function decodeBestEffort(path: string): string {
  try {
    return decodeURIComponent(path);
  } catch {
    return path;
  }
}

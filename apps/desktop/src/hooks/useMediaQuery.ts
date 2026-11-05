import { useEffect, useState } from "react";

/**
 * Tracks a media query's match state via MediaQueryList's own "change" event,
 * which by construction only fires when the boolean actually flips -- exactly
 * the "fires only when a breakpoint is crossed, not on every resize" behaviour
 * the sidebar rail and detail panel need (SPEC "GUI" / UI2).
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => window.matchMedia(query).matches);

  useEffect(() => {
    const mql = window.matchMedia(query);
    setMatches(mql.matches);
    const handler = (e: MediaQueryListEvent) => setMatches(e.matches);
    mql.addEventListener("change", handler);
    return () => mql.removeEventListener("change", handler);
  }, [query]);

  return matches;
}

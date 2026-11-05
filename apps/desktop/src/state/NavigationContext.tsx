import { createContext, useContext } from "react";

/** Moves between destinations on behalf of a component that does not own the view state. */
export interface NavigationValue {
  /**
   * Shows the in-app browser at `url`. With `collect`, 集める starts by itself once the page
   * has loaded, as if the person had pressed it.
   */
  openInBrowser: (url: string, options?: { collect?: boolean }) => Promise<void>;
}

export const NavigationContext = createContext<NavigationValue | null>(null);

export function useNavigation(): NavigationValue {
  const ctx = useContext(NavigationContext);
  if (!ctx) throw new Error("useNavigation() used outside <NavigationContext.Provider>");
  return ctx;
}

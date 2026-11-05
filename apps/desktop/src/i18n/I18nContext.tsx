import { createContext, useContext, useMemo, type ReactNode } from "react";
import type { Language } from "../ipc/contract";
import { en } from "./en";
import { ja } from "./ja";
import type { Translations } from "./types";

const dictionaries: Record<Language, Translations> = { ja, en };

const I18nContext = createContext<Translations | null>(null);

export function I18nProvider({
  language,
  children,
}: {
  language: Language;
  children: ReactNode;
}) {
  const t = useMemo(() => dictionaries[language], [language]);
  return <I18nContext.Provider value={t}>{children}</I18nContext.Provider>;
}

/** Every visible string in the app reads from this. */
export function useT(): Translations {
  const t = useContext(I18nContext);
  if (!t) throw new Error("useT() used outside <I18nProvider>");
  return t;
}

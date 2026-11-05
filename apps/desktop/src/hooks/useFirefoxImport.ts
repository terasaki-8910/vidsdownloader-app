import { useCallback, useEffect, useRef, useState } from "react";
import { useT } from "../i18n/I18nContext";
import { ipc } from "../ipc";
import type { FirefoxImportResult } from "../ipc/contract";
import { IS_WINDOWS } from "../lib/platform";
import { errorMessage } from "../lib/url";

export interface FirefoxImport {
  /**
   * Firefox's profiles. Empty until they are loaded, when Firefox has none, and off
   * Windows (the import only exists there), so "nothing to offer" is `length === 0`.
   */
  profiles: string[];
  /** The profile the import will read; Firefox's default one until the person picks another. */
  profile: string;
  chooseProfile: (profile: string) => void;
  running: boolean;
  /** The last finished import; null before one and while the next one runs. */
  result: FirefoxImportResult | null;
  /** The sentence for `result` (see i18n importLogins.result); null when there is none. */
  resultLine: string | null;
  /** The last failure, ready to show; null when there is none. */
  errorLine: string | null;
  /** Runs the import; resolves to its result, or null when it failed or did not start. */
  start: () => Promise<FirefoxImportResult | null>;
}

/**
 * Importing the logins of a Firefox profile into the in-app browser's accounts. The
 * browser's one-time offer and Settings > Sites both use it, so they list the same
 * profiles and say the same thing afterwards. Cookies stay in Rust: only counts get here.
 */
export function useFirefoxImport(): FirefoxImport {
  const t = useT();
  const [profiles, setProfiles] = useState<string[]>([]);
  const [profile, setProfile] = useState("");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<FirefoxImportResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const runningRef = useRef(false);

  useEffect(() => {
    if (!IS_WINDOWS) return;
    let cancelled = false;
    ipc.call("firefox_profiles_info", {}).then(
      (info) => {
        if (cancelled) return;
        setProfiles(info.profiles);
        setProfile(info.defaultProfile ?? info.profiles[0] ?? "");
      },
      // Firefox cannot be read: nothing to offer.
      () => {},
    );
    return () => {
      cancelled = true;
    };
  }, []);

  const start = useCallback(async () => {
    if (runningRef.current || profile === "") return null;
    runningRef.current = true;
    setRunning(true);
    setResult(null);
    setError(null);
    try {
      const imported = await ipc.call("firefox_import", { profile });
      setResult(imported);
      return imported;
    } catch (e) {
      setError(errorMessage(e));
      return null;
    } finally {
      runningRef.current = false;
      setRunning(false);
    }
  }, [profile]);

  const alreadySignedIn = result?.skipped.filter((site) => site.reason === "loggedIn").length ?? 0;

  return {
    profiles,
    profile,
    chooseProfile: setProfile,
    running,
    result,
    resultLine: result ? t.importLogins.result(result.imported.length, alreadySignedIn) : null,
    errorLine: error !== null ? t.sites.failed(error) : null,
    start,
  };
}

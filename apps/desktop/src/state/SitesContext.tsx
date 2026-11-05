import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { BrowserState, JobRow, Site, SiteProbeReport } from "../ipc/contract";
import { ipc } from "../ipc";
import { nextFrame } from "../lib/frame";
import { errorMessage } from "../lib/url";
import { selectedAccount } from "../lib/sites";
import { useJobs } from "./JobsContext";

const IDLE_BROWSER: BrowserState = {
  url: null,
  loading: false,
  mode: "browse",
  siteId: null,
  accountId: null,
};

/** Where a login started from a queue row ended up. */
export type JobLoginOutcome = "browser" | "settings";

/** What the report box of a site added in this run shows. */
export type SiteReportEntry =
  | { status: "probing"; url: string }
  | { status: "done"; url: string; report: SiteProbeReport };

/** Starting 集める is asked for the page opening at this URL (see BrowserView). */
export interface CollectRequest {
  url: string;
}

/** A request nobody picked up (the page never loaded, the person went elsewhere) lapses. */
const COLLECT_REQUEST_TTL_MS = 60_000;

interface SitesContextValue {
  sites: Site[];
  browser: BrowserState;
  /** The last failure of an action on the browser destination; null when none. */
  browserError: string | null;
  dismissBrowserError: () => void;
  /**
   * The add-site report of each site added in this run, by site id; an entry goes when its
   * site is removed, and none survives a restart.
   */
  siteReports: Record<number, SiteReportEntry>;
  /**
   * Asks yt-dlp what it makes of the URL and keeps the answer in siteReports. The first probe
   * of a site goes without its login; `withLogin` gives the run the selected account's login.
   */
  probeSite: (siteId: number, url: string, withLogin?: boolean) => Promise<void>;
  /** The pending request to start 集める on the page being opened; null when none. */
  collectRequest: CollectRequest | null;
  /** Asks BrowserView to start 集める once the page at `url` has loaded. */
  requestCollect: (url: string) => void;
  clearCollectRequest: () => void;
  /** Opens a normalized http(s) URL in the browser with the site's selected account. */
  openUrl: (url: string) => Promise<void>;
  /** Closes the page and shows the site list (not while a login is in progress). */
  showStartPage: () => Promise<void>;
  /** Opens the site's login page for the account. */
  startLogin: (siteId: number, accountId: number) => Promise<void>;
  /** Saves the login and, if jobs were waiting for the site, retries them. */
  finishLogin: () => Promise<void>;
  cancelLogin: () => Promise<void>;
  /**
   * Starts a login for a job that stopped with LoginRequired and remembers the
   * job, so it is retried when that site's login finishes. beforeBrowser runs
   * right before the browser opens (the caller switches to that destination,
   * so the native page is never shown over another screen). Resolves to
   * "settings" when the site's account does not use the in-app browser: the
   * caller should send the person to Settings > Sites instead.
   */
  loginForJob: (job: JobRow, beforeBrowser: () => void) => Promise<JobLoginOutcome>;
}

const SitesContext = createContext<SitesContextValue | null>(null);

/** Built-in sites first, in the order the app adds them, then the ones the user added. */
function inAddedOrder(list: Site[]): Site[] {
  return [...list].sort((a, b) => a.id - b.id);
}

export function SitesProvider({ children }: { children: ReactNode }) {
  const { jobs, retry } = useJobs();
  const [sites, setSites] = useState<Site[]>([]);
  const [browser, setBrowser] = useState<BrowserState>(IDLE_BROWSER);
  const [browserError, setBrowserError] = useState<string | null>(null);
  const [siteReports, setSiteReports] = useState<Record<number, SiteReportEntry>>({});
  const [collectRequest, setCollectRequest] = useState<CollectRequest | null>(null);

  // Latest values for callbacks that must not change identity with them.
  const sitesRef = useRef(sites);
  const browserRef = useRef(browser);
  const jobsRef = useRef(jobs);
  useEffect(() => {
    sitesRef.current = sites;
    browserRef.current = browser;
    jobsRef.current = jobs;
  });

  // Jobs that stopped for a login, by site id; retried when that login finishes.
  const waitingJobs = useRef(new Map<number, Set<number>>());
  // The newest probe of each site: an older one that ends later is ignored.
  const probeRuns = useRef(new Map<number, number>());
  const collectTimer = useRef<number | undefined>(undefined);

  /** Drops the reports of sites that are no longer in the list. */
  const pruneReports = useCallback((list: Site[]) => {
    const present = new Set(list.map((site) => site.id));
    for (const id of [...probeRuns.current.keys()]) {
      if (!present.has(id)) probeRuns.current.delete(id);
    }
    setSiteReports((previous) => {
      const kept = Object.entries(previous).filter(([id]) => present.has(Number(id)));
      return kept.length === Object.keys(previous).length ? previous : Object.fromEntries(kept);
    });
  }, []);

  useEffect(() => {
    // An event carries the whole list, so one that arrives before the snapshot
    // is newer than it and the snapshot must not overwrite it.
    let eventSeen = false;

    const offSites = ipc.on("sites-updated", (list) => {
      eventSeen = true;
      setSites(inAddedOrder(list));
      pruneReports(list);
    });
    const offBrowser = ipc.on("browser-state", setBrowser);

    void ipc.call("sites_list", {}).then((list) => {
      if (!eventSeen) setSites(inAddedOrder(list));
    });

    return () => {
      offSites();
      offBrowser();
    };
  }, [pruneReports]);

  useEffect(() => () => window.clearTimeout(collectTimer.current), []);

  const probeSite = useCallback(async (siteId: number, url: string, withLogin = false) => {
    const run = (probeRuns.current.get(siteId) ?? 0) + 1;
    probeRuns.current.set(siteId, run);
    setSiteReports((previous) => ({ ...previous, [siteId]: { status: "probing", url } }));
    let report: SiteProbeReport;
    try {
      report = await ipc.call("site_probe", { url, withLogin });
    } catch (error) {
      report = { kind: "failed", category: "Unknown", message: errorMessage(error) };
    }
    // A newer probe of the site started meanwhile, or the site was removed.
    if (probeRuns.current.get(siteId) !== run) return;
    setSiteReports((previous) =>
      siteId in previous ? { ...previous, [siteId]: { status: "done", url, report } } : previous,
    );
  }, []);

  const clearCollectRequest = useCallback(() => {
    window.clearTimeout(collectTimer.current);
    setCollectRequest(null);
  }, []);

  const requestCollect = useCallback((url: string) => {
    window.clearTimeout(collectTimer.current);
    setCollectRequest({ url });
    collectTimer.current = window.setTimeout(() => setCollectRequest(null), COLLECT_REQUEST_TTL_MS);
  }, []);

  const dismissBrowserError = useCallback(() => setBrowserError(null), []);

  const showStartPage = useCallback(async () => {
    if (browserRef.current.mode === "login") return;
    try {
      await ipc.call("browser_open", { url: null, accountId: null });
    } catch (error) {
      setBrowserError(errorMessage(error));
    }
  }, []);

  const openUrl = useCallback(async (url: string) => {
    setBrowserError(null);
    try {
      await ipc.call("browser_open", { url, accountId: null });
    } catch (error) {
      setBrowserError(errorMessage(error));
    }
  }, []);

  const startLogin = useCallback(async (siteId: number, accountId: number) => {
    setBrowserError(null);
    try {
      await ipc.call("login_start", { siteId, accountId });
    } catch (error) {
      setBrowserError(errorMessage(error));
    }
  }, []);

  const retryWaiting = useCallback(
    async (siteId: number) => {
      const ids = waitingJobs.current.get(siteId);
      waitingJobs.current.delete(siteId);
      if (!ids) return;
      for (const id of ids) {
        // A row retried by hand in the meantime is no longer failed; leave it.
        const job = jobsRef.current.find((j) => j.id === id);
        if (job?.state !== "Failed") continue;
        try {
          await retry(id);
        } catch (error) {
          setBrowserError(errorMessage(error));
        }
      }
    },
    [retry],
  );

  const finishLogin = useCallback(async () => {
    const siteId = browserRef.current.siteId;
    setBrowserError(null);
    try {
      await ipc.call("login_finish", {});
    } catch (error) {
      setBrowserError(errorMessage(error));
      return;
    }
    if (siteId != null) await retryWaiting(siteId);
  }, [retryWaiting]);

  const cancelLogin = useCallback(async () => {
    setBrowserError(null);
    try {
      await ipc.call("login_cancel", {});
    } catch (error) {
      setBrowserError(errorMessage(error));
    }
  }, []);

  const loginForJob = useCallback(
    async (job: JobRow, beforeBrowser: () => void): Promise<JobLoginOutcome> => {
      setBrowserError(null);
      try {
        const knownId = await ipc.call("site_for_url", { url: job.url });
        let site: Site | undefined;
        if (knownId == null) {
          // The site's start page, not the video: the user finds the login link there.
          site = await ipc.call("site_add", { url: `${new URL(job.url).origin}/` });
        } else {
          site =
            sitesRef.current.find((s) => s.id === knownId) ??
            (await ipc.call("sites_list", {})).find((s) => s.id === knownId);
        }
        const account = site ? selectedAccount(site) : null;
        if (!site || !account) throw new Error("site has no account");
        if (account.method.kind !== "inApp") return "settings";

        let waiting = waitingJobs.current.get(site.id);
        if (!waiting) {
          waiting = new Set();
          waitingJobs.current.set(site.id, waiting);
        }
        waiting.add(job.id);

        beforeBrowser();
        // Let the browser destination mount and report its bounds before the
        // native page is created.
        await nextFrame();
        await ipc.call("login_start", { siteId: site.id, accountId: account.id });
      } catch (error) {
        setBrowserError(errorMessage(error));
        beforeBrowser();
      }
      return "browser";
    },
    [],
  );

  const value = useMemo<SitesContextValue>(
    () => ({
      sites,
      browser,
      browserError,
      dismissBrowserError,
      siteReports,
      probeSite,
      collectRequest,
      requestCollect,
      clearCollectRequest,
      openUrl,
      showStartPage,
      startLogin,
      finishLogin,
      cancelLogin,
      loginForJob,
    }),
    [
      sites,
      browser,
      browserError,
      dismissBrowserError,
      siteReports,
      probeSite,
      collectRequest,
      requestCollect,
      clearCollectRequest,
      openUrl,
      showStartPage,
      startLogin,
      finishLogin,
      cancelLogin,
      loginForJob,
    ],
  );

  return <SitesContext.Provider value={value}>{children}</SitesContext.Provider>;
}

export function useSites(): SitesContextValue {
  const ctx = useContext(SitesContext);
  if (!ctx) throw new Error("useSites() used outside <SitesProvider>");
  return ctx;
}

import { useEffect, useRef, useState } from "react";
import "./styles/tokens.css";
import "./styles/global.css";
import { BrowserView } from "./components/BrowserView";
import { DetailPanel } from "./components/DetailPanel";
import { DetailSheet } from "./components/DetailSheet";
import { DownloadsView } from "./components/DownloadsView";
import { LibraryView } from "./components/LibraryView";
import { MethodsView } from "./components/MethodsView";
import { PageMediaChoiceDialog } from "./components/PageMediaChoiceDialog";
import { Sidebar, type View } from "./components/Sidebar";
import { SettingsView, type SettingsSection } from "./components/SettingsView";
import { SourceFirstDialog } from "./components/SourceFirstDialog";
import { SourcesView } from "./components/SourcesView";
import { UpdateDialogHost } from "./components/UpdateDialog";
import { useMediaQuery } from "./hooks/useMediaQuery";
import { I18nProvider } from "./i18n/I18nContext";
import { ipc } from "./ipc";
import type { JobRow } from "./ipc/contract";
import { readDebugHash } from "./lib/debugHash";
import { nextFrame } from "./lib/frame";
import { activeJobCount } from "./lib/jobStats";
import { revealPath } from "./lib/reveal";
import { JobsProvider, useJobs } from "./state/JobsContext";
import { NavigationContext } from "./state/NavigationContext";
import { PlayerProvider } from "./state/PlayerContext";
import { SettingsProvider, useSettings } from "./state/SettingsContext";
import { SitesProvider, useSites } from "./state/SitesContext";
import { SourcesProvider, useSources } from "./state/SourcesContext";
import { ToolsProvider } from "./state/ToolsContext";
import { UpdatesProvider, useUpdates } from "./state/UpdatesContext";
import styles from "./App.module.css";

export default function App() {
  return (
    <SettingsProvider>
      <JobsProvider>
        <SitesProvider>
          <ToolsProvider>
            <PlayerProvider>
              <SourcesProvider>
                <UpdatesProvider>
                  <LocalizedShell />
                </UpdatesProvider>
              </SourcesProvider>
            </PlayerProvider>
          </ToolsProvider>
        </SitesProvider>
      </JobsProvider>
    </SettingsProvider>
  );
}

/** Wraps the shell in i18n once Settings has told us which language to use. */
function LocalizedShell() {
  const { settings } = useSettings();
  return (
    <I18nProvider language={settings?.language ?? "ja"}>
      <AppShell />
    </I18nProvider>
  );
}

function AppShell() {
  const { settings, updateSettings } = useSettings();
  const { jobs, cancel, retry, choosePageMedia, saveAgain, presetFor } = useJobs();
  const { browser, loginForJob, startLogin, showStartPage, openUrl, requestCollect } = useSites();
  const { firstCheck } = useSources();
  const { dialog: updateDialog } = useUpdates();

  const [view, setView] = useState<View>("downloads");
  const [selectedJobId, setSelectedJobId] = useState<number | null>(null);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [manualClosed, setManualClosed] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("general");
  const [mediaChoiceJob, setMediaChoiceJob] = useState<JobRow | null>(null);
  const [flowRequest, setFlowRequest] = useState<{ id: number; seq: number } | null>(null);
  const flowRequestSeq = useRef(0);

  const urlFieldRef = useRef<HTMLTextAreaElement>(null);
  const appliedSettingsHash = useRef(false);
  const appliedSheetHash = useRef(false);

  // SPEC "GUI": layout slides to B (detail panel) from ~1320px wide.
  const isWide = useMediaQuery("(min-width: 1320px)");
  const prefersDark = useMediaQuery("(prefers-color-scheme: dark)");

  // UI2: a panel closed by hand while wide stays closed until the *next*
  // crossing of 1320px, in either direction -- this only resets the flag when
  // isWide's value actually flips (matchMedia's "change" event, under the
  // hood, only fires on a real crossing), never on an unrelated re-render.
  useEffect(() => {
    setManualClosed(false);
  }, [isWide]);

  // tokens.css keys dark mode off :root[data-theme], and :root is <html> --
  // not some nested app div -- so the attribute has to go on the document.
  useEffect(() => {
    const root = document.documentElement;
    if (settings?.theme === "light") root.setAttribute("data-theme", "light");
    else if (settings?.theme === "dark") root.setAttribute("data-theme", "dark");
    else root.removeAttribute("data-theme");
  }, [settings?.theme]);

  useEffect(() => {
    document.documentElement.lang = settings?.language ?? "ja";
  }, [settings?.language]);

  // The page in the browser is a native child webview drawn above all web
  // content, so it must be hidden whenever anything else is on screen: another
  // destination or the detail sheet. This is the only place that decides it.
  // A page opening re-sends the value because Rust resets its flag when a
  // login ends or is cancelled. The first-check question of a source registered from the
  // collect tray is a dialog too, and the page would be drawn over it; so is the update dialog
  // (asking again once the queue is idle can open it on any destination).
  const browserVisible = view === "browser" && !sheetOpen && firstCheck === null && updateDialog === null;
  const browserPageOpen = browser.url !== null;
  useEffect(() => {
    ipc.call("browser_set_visible", { visible: browserVisible }).catch(() => {});
  }, [browserVisible, browserPageOpen]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if ((e.ctrlKey || e.metaKey) && e.key === ",") {
        e.preventDefault();
        setView("settings");
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  // Debug-only (see lib/debugHash.ts): apply #view=/#theme=/#language=/
  // #settings= once settings have loaded, so screenshots can reach any state
  // without scripted clicks. Not real routing.
  useEffect(() => {
    if (appliedSettingsHash.current || !settings) return;
    appliedSettingsHash.current = true;
    const hash = readDebugHash();
    if (hash.view) setView(hash.view);
    if (hash.theme) updateSettings({ theme: hash.theme });
    if (hash.language) updateSettings({ language: hash.language });
    if (hash.settingsSection) {
      setSettingsSection(hash.settingsSection);
      setView("settings");
    }
  }, [settings, updateSettings]);

  // #sheet=1 / #detailJob=<id> need a job to select, so they wait for the
  // queue snapshot to have loaded.
  useEffect(() => {
    if (appliedSheetHash.current || jobs.length === 0) return;
    const hash = readDebugHash();
    if (hash.openSheet || hash.detailJob != null) {
      appliedSheetHash.current = true;
      const job = hash.detailJob != null ? jobs.find((j) => j.id === hash.detailJob) : jobs[0];
      if (job) {
        setSelectedJobId(job.id);
        if (hash.openSheet) setSheetOpen(true);
      }
    }
  }, [jobs]);

  // A job that was erased (its saved item was removed from the library) cannot stay selected.
  useEffect(() => {
    if (selectedJobId != null && !jobs.some((j) => j.id === selectedJobId)) {
      setSelectedJobId(null);
      setSheetOpen(false);
    }
  }, [jobs, selectedJobId]);

  if (!settings) return null;

  const selectedJob =
    (selectedJobId != null ? jobs.find((j) => j.id === selectedJobId) : undefined) ??
    (isWide ? jobs[0] : undefined) ??
    null;
  const detailPreset = selectedJob ? presetFor(selectedJob.id) : undefined;

  const effectiveDark = settings.theme === "dark" || (settings.theme !== "light" && prefersDark);

  function openSettings(targetSection?: SettingsSection) {
    if (targetSection) setSettingsSection(targetSection);
    setView("settings");
  }

  /** Opens the browser on an account's login page (from Settings > Sites). */
  async function handleStartLogin(siteId: number, accountId: number) {
    setSheetOpen(false);
    setView("browser");
    // Let the browser destination mount and report its bounds first.
    await nextFrame();
    await startLogin(siteId, accountId);
  }

  /** Shows the browser at a URL (from the add-site report); with `collect`, 集める starts once it has loaded. */
  async function handleOpenInBrowser(
    url: string,
    options?: { collect?: boolean; collectShape?: string; flowId?: number },
  ) {
    setSheetOpen(false);
    if (options?.flowId == null) {
      setFlowRequest(null);
    } else {
      flowRequestSeq.current += 1;
      setFlowRequest({ id: options.flowId, seq: flowRequestSeq.current });
    }
    setView("browser");
    // Let the browser destination mount and report its bounds first.
    await nextFrame();
    if (options?.collect) requestCollect(url);
    await openUrl(url);
    if (options?.collectShape) {
      await ipc.call("collect_start", { shape: options.collectShape, maxScrolls: 10 });
    }
  }

  /** A queue row stopped for a login: log in, then the row is retried by itself. */
  async function handleLoginJob(job: JobRow) {
    const outcome = await loginForJob(job, () => {
      setSheetOpen(false);
      setView("browser");
    });
    // The site's account takes its login from Firefox or a cookies.txt file:
    // nothing to do in the browser, the settings are where to fix it.
    if (outcome === "settings") openSettings("sites");
  }

  function handleAddUrl() {
    setView("downloads");
    requestAnimationFrame(() => urlFieldRef.current?.focus());
  }

  function handleFocusJob(jobId: number) {
    setSelectedJobId(jobId);
    if (isWide) {
      setManualClosed(false);
    }
  }

  function handleOpenDetail(jobId: number) {
    handleFocusJob(jobId);
    if (!isWide) setSheetOpen(true);
  }

  return (
    <NavigationContext.Provider value={{ openInBrowser: handleOpenInBrowser, flowRequest }}>
      <div className={styles.shell}>
        <Sidebar
          view={view}
          onSelectView={(next) => {
            // Choosing the browser again while it is shown returns to its site list.
            if (next === "browser" && view === "browser") void showStartPage();
            setView(next);
          }}
          activeJobCount={activeJobCount(jobs)}
          onAddUrl={handleAddUrl}
          isEffectiveDark={effectiveDark}
          onToggleTheme={() => updateSettings({ theme: effectiveDark ? "light" : "dark" })}
        />

        <main className={styles.main}>
          {view === "downloads" ? (
            <DownloadsView
              urlFieldRef={urlFieldRef}
              isWide={isWide}
              onFocusJob={handleFocusJob}
              onOpenDetail={handleOpenDetail}
              onOpenToolsSettings={() => openSettings("tools")}
              onLogin={(job) => void handleLoginJob(job)}
              onChooseMedia={setMediaChoiceJob}
            />
          ) : view === "library" ? (
            <LibraryView />
          ) : view === "sources" ? (
            <SourcesView />
          ) : view === "methods" ? (
            <MethodsView />
          ) : view === "browser" ? (
            <BrowserView onOpenSites={() => openSettings("sites")} />
          ) : (
            <SettingsView
              section={settingsSection}
              onChangeSection={setSettingsSection}
              onLogin={(siteId, accountId) => void handleStartLogin(siteId, accountId)}
            />
          )}

          {view === "downloads" && (
            <DetailPanel
              job={selectedJob}
              preset={detailPreset}
              open={isWide && !manualClosed && selectedJob != null}
              onClose={() => setManualClosed(true)}
              onCancel={() => selectedJob && void cancel(selectedJob.id)}
              onRetry={() => selectedJob && void retry(selectedJob.id)}
              onReveal={(path) => void revealPath(path)}
              onLogin={() => selectedJob && void handleLoginJob(selectedJob)}
              onSaveAgain={() => selectedJob && void saveAgain(selectedJob.id)}
              onChooseMedia={() => selectedJob && setMediaChoiceJob(selectedJob)}
            />
          )}
        </main>

        <SourceFirstDialog />
        <UpdateDialogHost />
        {mediaChoiceJob && (
          <PageMediaChoiceDialog
            job={mediaChoiceJob}
            onCancel={() => setMediaChoiceJob(null)}
            onSave={async (url) => {
              await choosePageMedia(mediaChoiceJob.id, url);
              setMediaChoiceJob(null);
            }}
          />
        )}

        {view === "downloads" && (
          <DetailSheet
            job={selectedJob}
            preset={detailPreset}
            open={!isWide && sheetOpen}
            onClose={() => setSheetOpen(false)}
            onCancel={() => selectedJob && void cancel(selectedJob.id)}
            onRetry={() => selectedJob && void retry(selectedJob.id)}
            onReveal={(path) => void revealPath(path)}
            onLogin={() => selectedJob && void handleLoginJob(selectedJob)}
            onSaveAgain={() => selectedJob && void saveAgain(selectedJob.id)}
            onChooseMedia={() => selectedJob && setMediaChoiceJob(selectedJob)}
          />
        )}
      </div>
    </NavigationContext.Provider>
  );
}

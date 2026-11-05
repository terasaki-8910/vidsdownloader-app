import {
  ArrowLeft,
  ArrowRight,
  Check,
  Globe,
  KeyRound,
  LayoutGrid,
  LayoutList,
  LocateFixed,
  Lock,
  Plus,
  RotateCw,
  X,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import type { CollectState } from "../ipc/contract";
import { ipc, usesMockIpc } from "../ipc";
import { useT } from "../i18n/I18nContext";
import { IMAGES_SHAPE, PLAYED_SHAPE } from "../lib/collect";
import { readDebugHash } from "../lib/debugHash";
import { nextFrame } from "../lib/frame";
import { hasLoginInfo, selectedAccount, siteHomeUrl, siteStartStatus } from "../lib/sites";
import { registerFailureText } from "../lib/sources";
import { errorMessage, normalizeUrl } from "../lib/url";
import { useJobs } from "../state/JobsContext";
import { useSettings } from "../state/SettingsContext";
import { useSites } from "../state/SitesContext";
import { useSources } from "../state/SourcesContext";
import { IconButton } from "./IconButton";
import { ImportOffer } from "./ImportOffer";
import { PageImagesTray } from "./PageImagesTray";
import styles from "./BrowserView.module.css";

/** How long a loaded page is left alone before an asked-for 集める starts (pages that draw late). */
const COLLECT_SETTLE_MS = 500;

interface BrowserViewProps {
  /** Opens Settings > Sites (to add a site). */
  onOpenSites: () => void;
}

/**
 * The Browser destination: a toolbar, an optional login banner and a stage. The
 * page itself is a native child webview that Rust draws over the stage (it is
 * not part of this DOM), so the stage reports its rectangle with
 * browser_set_bounds. With no page open the stage shows the site list instead.
 */
export function BrowserView({ onOpenSites }: BrowserViewProps) {
  const t = useT();
  const { settings } = useSettings();
  const { enqueue, enqueuePageImages, enqueuePageMedia } = useJobs();
  const { sites, browser, browserError, openUrl, showStartPage, finishLogin, cancelLogin, collectRequest, clearCollectRequest } =
    useSites();
  const { sources, phase: sourcePhase, registerFromCollect } = useSources();

  const stageRef = useRef<HTMLDivElement>(null);
  const startRef = useRef<HTMLDivElement>(null);
  const lastBounds = useRef("");
  const [draft, setDraft] = useState(browser.url ?? "");
  const [localError, setLocalError] = useState<string | null>(null);
  const [added, setAdded] = useState(false);
  const [finishing, setFinishing] = useState(false);
  const [collect, setCollect] = useState<CollectState | null>(null);
  const [selectedUrls, setSelectedUrls] = useState<Record<string, boolean>>({});
  // Items the page has dropped since they were found (virtualized lists), so "show on page" is off.
  const [goneUrls, setGoneUrls] = useState<Record<string, true>>({});
  const [showRuleForm, setShowRuleForm] = useState(false);
  const [ruleName, setRuleName] = useState("");
  const [matchPattern, setMatchPattern] = useState("");
  const [ruleSaved, setRuleSaved] = useState(false);
  const [showXNotice, setShowXNotice] = useState(false);
  const [reloadFinding, setReloadFinding] = useState(false);
  const [reloadFindStatus, setReloadFindStatus] = useState<{ found: number } | { error: string } | null>(null);
  /** The page and link shape registered as a source from this tray, and whether it existed already. */
  const [sourceDone, setSourceDone] = useState<{ key: string; existed: boolean } | null>(null);
  const [registeringSource, setRegisteringSource] = useState(false);
  const [sourceNotice, setSourceNotice] = useState<string | null>(null);
  const warnedPages = useRef(new Set<string>());
  const startCollectRef = useRef<() => Promise<void>>(async () => {});
  const debugCollectApplied = useRef(false);
  const allCheckboxRef = useRef<HTMLInputElement>(null);

  const isLogin = browser.mode === "login";
  const pageOpen = browser.url !== null;
  const site = sites.find((s) => s.id === browser.siteId) ?? null;
  const selectedCount = collect?.items.filter((item) => selectedUrls[item.url] !== false).length ?? 0;
  const allSelected = Boolean(collect?.items.length) && selectedCount === collect?.items.length;
  // The page's own images are saved directly (one job for the page), not queued as links.
  const imagesChosen = collect?.shape === IMAGES_SHAPE;
  const playedChosen = collect?.shape === PLAYED_SHAPE;
  // A rule source is a page with a link shape: registering one is per page and per shape.
  const sourceKey = browser.url && collect?.shape && !playedChosen ? `${browser.url}
${collect.shape}` : null;
  const sourceExists =
    sourceKey !== null &&
    sources.some((s) => s.kind === "rule" && s.url === browser.url && s.ruleShape === collect?.shape);

  // Reports the stage's rectangle (CSS px, the logical px Rust expects) when it
  // changes. Sent only on change, so measuring on every render is cheap.
  const sendBounds = useCallback(() => {
    const el = stageRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    const key = `${rect.x}|${rect.y}|${rect.width}|${rect.height}`;
    if (key === lastBounds.current) return;
    lastBounds.current = key;
    ipc.call("browser_set_bounds", { x: rect.x, y: rect.y, width: rect.width, height: rect.height }).catch(() => {
      lastBounds.current = "";
    });
  }, []);

  // A banner or a wrapped toolbar moves the stage without resizing the window.
  useLayoutEffect(() => {
    sendBounds();
  });

  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const observer = new ResizeObserver(sendBounds);
    observer.observe(el);
    window.addEventListener("resize", sendBounds);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", sendBounds);
    };
  }, [sendBounds]);

  // Follow the page's address unless the person is typing a new one.
  useEffect(() => {
    setDraft(browser.url ?? "");
    setLocalError(null);
  }, [browser.url]);

  useEffect(() => {
    if (!added) return;
    const timer = window.setTimeout(() => setAdded(false), 1600);
    return () => window.clearTimeout(timer);
  }, [added]);

  useEffect(
    () =>
      ipc.on("collect-state", (state) => {
        setCollect(state);
        setSelectedUrls((previous) => {
          const next: Record<string, boolean> = {};
          for (const item of state.items) next[item.url] = previous[item.url] ?? true;
          return next;
        });
      }),
    [],
  );

  useEffect(() => {
    if (allCheckboxRef.current) {
      allCheckboxRef.current.indeterminate = selectedCount > 0 && !allSelected;
    }
  }, [allSelected, selectedCount]);

  // 集める asked for by the page being opened (the add-site report's "open and collect"): start it
  // once that site's page has loaded and stayed loaded for a moment, as if it had been pressed. A
  // navigation in the meantime restarts the wait; a page of another site never starts it.
  useEffect(() => {
    startCollectRef.current = startCollect;
  });

  // The mock has already opened the requested page; start collecting once BrowserView
  // receives it so screenshot URLs need no scripted click.
  useEffect(() => {
    const debug = readDebugHash();
    if (!usesMockIpc || !debug.mockCollect || debugCollectApplied.current || !browser.url || browser.loading) return;
    debugCollectApplied.current = true;
    void startCollectRef.current();
  }, [browser.loading, browser.url]);
  useEffect(() => {
    if (!collectRequest || isLogin || browser.loading || !browser.url) return undefined;
    if (siteHost(browser.url) !== siteHost(collectRequest.url)) return undefined;
    const timer = window.setTimeout(() => {
      clearCollectRequest();
      void startCollectRef.current();
    }, COLLECT_SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [collectRequest, clearCollectRequest, isLogin, browser.loading, browser.url]);

  // The import offer's buttons vanish with it: the first site is where the keyboard goes on.
  async function focusStartList() {
    await nextFrame();
    startRef.current?.querySelector("button")?.focus();
  }

  function submitAddress(e: FormEvent) {
    e.preventDefault();
    if (isLogin) return;
    const url = normalizeUrl(draft);
    if (!url) {
      setLocalError(t.browser.invalidUrl);
      return;
    }
    setLocalError(null);
    void openUrl(url);
  }

  async function addPage() {
    if (!browser.url || !settings) return;
    try {
      await enqueue([browser.url], settings.defaultPreset);
      setLocalError(null);
      setAdded(true);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function startCollect() {
    if (!browser.url) return;
    const url = browser.url;
    setRuleName(site?.name ?? safeHost(url));
    setMatchPattern(ruleMatchPattern(url));
    setShowRuleForm(false);
    setRuleSaved(false);
    setSourceNotice(null);
    setReloadFindStatus(null);
    const host = safeHost(url).toLowerCase();
    const isX = host === "x.com" || host.endsWith(".x.com") || host === "twitter.com" || host.endsWith(".twitter.com");
    const showWarning = isX && !warnedPages.current.has(url);
    if (showWarning) warnedPages.current.add(url);
    setShowXNotice(showWarning);
    try {
      await ipc.call("collect_start", { shape: null, maxScrolls: 10 });
      setLocalError(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function toggleCollect() {
    if (!collect) return;
    try {
      if (collect.running) {
        await ipc.call("collect_stop", {});
      } else {
        await ipc.call("collect_start", { shape: collect.shape, maxScrolls: collect.maxScrolls });
      }
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function selectShape(shape: string) {
    try {
      await ipc.call("collect_select", { shape });
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function reloadFind() {
    setReloadFindStatus(null);
    setReloadFinding(true);
    try {
      const result = await ipc.call("collect_reload_find", {});
      setReloadFindStatus({ found: result.found });
    } catch (error) {
      setReloadFindStatus({ error: errorMessage(error) });
    } finally {
      setReloadFinding(false);
    }
  }

  async function closeCollect() {
    try {
      await ipc.call("collect_close", {});
      setCollect(null);
      setSelectedUrls({});
      setGoneUrls({});
      setShowRuleForm(false);
      setShowXNotice(false);
      setSourceNotice(null);
      setReloadFindStatus(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function addCollected() {
    if (!collect || !settings) return;
    const chosen = collect.items.filter((item) => selectedUrls[item.url] !== false);
    if (chosen.length === 0) return;
    try {
      if (playedChosen) {
        const pageUrl = browser.url;
        if (!pageUrl) return;
        await enqueuePageMedia({
          items: chosen.map((item) => ({ mediaUrl: item.url, pageUrl, title: item.title })),
          preset: settings.defaultPreset,
        });
      } else if (imagesChosen) {
        if (!browser.url) return;
        const page = collect.page;
        await enqueuePageImages({
          pageUrl: browser.url,
          title: page?.title || null,
          uploader: page?.uploader ?? null,
          publishedAt: page?.publishedAt ?? null,
          id: page?.id ?? null,
          images: chosen.map((item, position) => ({ url: item.url, index: item.index ?? position + 1 })),
        });
      } else {
        await enqueue(
          chosen.map((item) => item.url),
          settings.defaultPreset,
        );
      }
      setAdded(true);
      setLocalError(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  function setAllSelected(checked: boolean) {
    if (!collect) return;
    setSelectedUrls((previous) => {
      const next = { ...previous };
      for (const item of collect.items) next[item.url] = checked;
      return next;
    });
  }

  async function registerSource() {
    if (!sourceKey) return;
    setSourceNotice(null);
    setRegisteringSource(true);
    const outcome = await registerFromCollect();
    setRegisteringSource(false);
    if (outcome.kind === "registered") setSourceDone({ key: sourceKey, existed: false });
    else if (outcome.kind === "exists") setSourceDone({ key: sourceKey, existed: true });
    else if (outcome.kind === "failed") setSourceNotice(registerFailureText(t, outcome));
  }

  async function revealItem(url: string) {
    try {
      const found = await ipc.call("collect_reveal", { url });
      if (!found) setGoneUrls((previous) => ({ ...previous, [url]: true }));
      setLocalError(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function saveRule(e: FormEvent) {
    e.preventDefault();
    try {
      await ipc.call("collect_save_rule", { name: ruleName, matchPattern });
      setRuleSaved(true);
      setShowRuleForm(false);
      setLocalError(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function done() {
    if (finishing) return;
    setFinishing(true);
    try {
      await finishLogin();
    } finally {
      setFinishing(false);
    }
  }

  const account = site ? selectedAccount(site) : null;
  const chipBase = hasLoginInfo(account) ? t.browser.loginPresent : t.browser.loginAbsent;
  const chipText = site && account && site.accounts.length > 1 ? t.browser.chipWithAccount(chipBase, account.label) : chipBase;
  const loginSiteName = site?.name ?? (browser.url ? safeHost(browser.url) : "");
  const message = localError ?? (browserError ? t.browser.failed(browserError) : null);

  return (
    <section className={styles.browser}>
      <div className={styles.bar}>
        <IconButton label={t.browser.back} disabled={!pageOpen} onClick={() => void ipc.call("browser_back", {})}>
          <ArrowLeft size={18} aria-hidden />
        </IconButton>
        <IconButton label={t.browser.forward} disabled={!pageOpen} onClick={() => void ipc.call("browser_forward", {})}>
          <ArrowRight size={18} aria-hidden />
        </IconButton>
        <IconButton label={t.browser.reload} disabled={!pageOpen} onClick={() => void ipc.call("browser_reload", {})}>
          <RotateCw size={17} aria-hidden className={browser.loading ? styles.spin : undefined} />
        </IconButton>
        <IconButton label={t.browser.siteList} disabled={!pageOpen || isLogin} onClick={() => void showStartPage()}>
          <LayoutList size={18} aria-hidden />
        </IconButton>

        <form className={styles.addr} onSubmit={submitAddress}>
          {browser.url?.startsWith("https:") ? <Lock size={14} aria-hidden /> : <Globe size={14} aria-hidden />}
          <input
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value);
              setLocalError(null);
            }}
            readOnly={isLogin}
            placeholder={t.browser.addressPlaceholder}
            aria-label={t.browser.address}
            aria-invalid={localError === t.browser.invalidUrl || undefined}
            spellCheck={false}
            autoComplete="off"
          />
        </form>

        {pageOpen && !isLogin && site && (
          <span className={hasLoginInfo(account) ? styles.chipOn : styles.chip} title={chipText}>
            <KeyRound size={14} aria-hidden />
            <span className={styles.chipText}>{chipText}</span>
          </span>
        )}

        {pageOpen && !isLogin && (
          <button type="button" className={styles.secondary} onClick={() => void startCollect()}>
            <LayoutGrid size={16} aria-hidden />
            {t.browser.collect}
          </button>
        )}

        {pageOpen && (
          <button
            type="button"
            className={isLogin || collect ? styles.secondary : styles.primary}
            onClick={() => void addPage()}
            disabled={!settings}
            aria-live="polite"
          >
            {added ? <Check size={16} aria-hidden /> : <Plus size={16} aria-hidden />}
            {added ? t.browser.pageAdded : t.browser.addPage}
          </button>
        )}
      </div>

      {isLogin && (
        <div className={styles.loginBanner} role="status">
          <KeyRound size={16} aria-hidden className={styles.bannerIcon} />
          <span className={styles.bannerText}>{t.browser.loginBanner(loginSiteName)}</span>
          <button type="button" className={styles.ghost} onClick={() => void cancelLogin()}>
            {t.browser.loginCancel}
          </button>
          <button type="button" className={styles.primary} onClick={() => void done()} disabled={finishing}>
            {t.browser.loginDone}
          </button>
        </div>
      )}

      {message && (
        <div className={styles.error} role="alert">
          {message}
        </div>
      )}

      <div className={collect ? styles.collectWorkspace : styles.workspace}>
        <div className={styles.stage} ref={stageRef}>
          {!pageOpen ? (
          <div className={styles.start} ref={startRef}>
            <ImportOffer onGone={() => void focusStartList()} />
            <h1 className={styles.startTitle}>{t.browser.startTitle}</h1>
            {sites.length > 0 && (
              <ul className={styles.siteList}>
                {sites.map((s) => {
                  const status = siteStartStatus(t, s);
                  return (
                    <li key={s.id}>
                      <button type="button" className={styles.siteRow} onClick={() => void openUrl(siteHomeUrl(s))}>
                        <span className={styles.siteIcon} aria-hidden>
                          {s.name.slice(0, 1).toUpperCase()}
                        </span>
                        <span className={styles.siteName}>{s.name}</span>
                        <span className={styles.siteHost}>{s.domains[0]}</span>
                        <span className={status.present ? styles.siteLogin : styles.siteNoLogin}>{status.text}</span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
            <button type="button" className={styles.addSite} onClick={onOpenSites}>
              {t.browser.addSite}
            </button>
          </div>
          ) : (
            usesMockIpc &&
            (collect ? (
              <div className={styles.mockGrid} aria-hidden>
                {collect.items.map((item) => (
                  <div className={styles.mockCard} key={item.url}>
                    <span className={styles.mockCheck}>
                      <Check size={13} />
                    </span>
                    <div className={styles.mockThumb}>{item.index != null && <img src={item.url} alt="" />}</div>
                    {(item.index == null || item.title) && <strong>{item.title || item.url}</strong>}
                  </div>
                ))}
              </div>
            ) : (
              <div className={styles.placeholder}>{t.browser.pagePlaceholder}</div>
            ))
          )}
        </div>

        {collect && (
          <aside className={styles.tray} aria-label={`${t.browser.foundLabel} ${collect.items.length}`}>
            <div className={styles.trayHead}>
              <div className={styles.trayTitleRow}>
                <h2>
                  {t.browser.foundLabel} <span>{collect.items.length}</span>
                </h2>
                <IconButton label={t.browser.closeCollect} size="sm" onClick={() => void closeCollect()}>
                  <X size={16} aria-hidden />
                </IconButton>
              </div>
              <div className={styles.progressLine}>
                <span>{t.browser.scrollProgress(collect.scrolls, collect.maxScrolls)}</span>
                <button type="button" className={styles.textButton} onClick={() => void toggleCollect()}>
                  {collect.running ? t.browser.collectStop : t.browser.collectResume}
                </button>
              </div>
              {collect.groups.length > 0 && collect.shape && (
                <select
                  className={styles.groupSelect}
                  value={collect.shape}
                  aria-label={t.browser.matchPattern}
                  onChange={(e) => void selectShape(e.target.value)}
                >
                  {collect.groups.map((group) => (
                    <option key={group.shape} value={group.shape}>
                      {group.shape === IMAGES_SHAPE
                        ? t.browser.imagesGroup(group.count)
                        : group.shape === PLAYED_SHAPE
                          ? t.browser.playedGroup(group.count)
                        : t.browser.groupOption(group.shape, group.count)}
                    </option>
                  ))}
                </select>
              )}
              <div className={styles.reloadFindLine}>
                <button
                  type="button"
                  className={styles.textButton}
                  disabled={reloadFinding}
                  onClick={() => void reloadFind()}
                >
                  {reloadFinding ? t.browser.reloadFinding : t.browser.reloadFind}
                </button>
                {reloadFindStatus && (
                  <span>
                    {"error" in reloadFindStatus
                      ? t.browser.collectFailed(reloadFindStatus.error)
                      : reloadFindStatus.found > 0
                        ? t.browser.reloadFound(reloadFindStatus.found)
                        : t.browser.reloadFoundNone}
                  </span>
                )}
              </div>
              {collect.adsExcluded > 0 && (
                <p className={styles.adsNote}>{t.browser.adsExcluded(collect.adsExcluded)}</p>
              )}
              {showXNotice && <p className={styles.xNotice}>{t.browser.xCollectWarning}</p>}
              {collect.error && <p className={styles.collectError}>{t.browser.collectFailed(collect.error)}</p>}
            </div>

            {collect.items.length > 0 && (
              <label className={styles.allRow}>
                <input
                  ref={allCheckboxRef}
                  type="checkbox"
                  checked={allSelected}
                  onChange={() => setAllSelected(!allSelected)}
                />
                <span>{t.browser.all}</span>
                <span className={styles.allCount}>{t.browser.selectedOfTotal(selectedCount, collect.items.length)}</span>
              </label>
            )}

            {collect.items.length === 0 ? (
              <p className={styles.collectEmpty}>{playedChosen ? t.browser.playedEmpty : t.browser.collectEmpty}</p>
            ) : imagesChosen ? (
              <PageImagesTray
                items={collect.items}
                selected={selectedUrls}
                onToggle={(url, checked) => setSelectedUrls((previous) => ({ ...previous, [url]: checked }))}
                otherLinks={collect.otherLinks}
              />
            ) : (
              <ul className={styles.foundList}>
                {collect.items.map((item) => (
                  <li key={item.url}>
                    <label>
                      <input
                        type="checkbox"
                        checked={selectedUrls[item.url] !== false}
                        onChange={(e) =>
                          setSelectedUrls((previous) => ({ ...previous, [item.url]: e.target.checked }))
                        }
                      />
                      {item.thumbnail && <img src={item.thumbnail} alt="" />}
                      <div>
                        <span title={item.title || item.url}>{item.title || item.url}</span>
                        {item.played && (
                          <p className={styles.adsNote}>{playedMeta(item.url, item.played.kind, item.played.durationSeconds)}</p>
                        )}
                      </div>
                    </label>
                    {!playedChosen && (
                      <IconButton
                        label={goneUrls[item.url] ? t.browser.revealGone : t.browser.revealItem}
                        size="sm"
                        disabled={Boolean(goneUrls[item.url])}
                        onClick={() => void revealItem(item.url)}
                      >
                        <LocateFixed size={16} aria-hidden />
                      </IconButton>
                    )}
                  </li>
                ))}
              </ul>
            )}

            <div className={styles.trayFoot}>
              <button
                type="button"
                className={styles.primary}
                disabled={!settings || selectedCount === 0}
                onClick={() => void addCollected()}
              >
                {added ? <Check size={16} aria-hidden /> : <Plus size={16} aria-hidden />}
                {added ? t.browser.collectedAdded : t.browser.addCollected(selectedCount)}
              </button>
              {!playedChosen && (showRuleForm ? (
                <form className={styles.ruleForm} onSubmit={(e) => void saveRule(e)}>
                  <label>
                    <span>{t.browser.ruleName}</span>
                    <input value={ruleName} onChange={(e) => setRuleName(e.target.value)} required />
                  </label>
                  <label>
                    <span>{t.browser.matchPattern}</span>
                    <input value={matchPattern} onChange={(e) => setMatchPattern(e.target.value)} required />
                  </label>
                  <button type="submit" className={styles.secondary} disabled={!collect.shape}>
                    {t.browser.save}
                  </button>
                </form>
              ) : (
                <button
                  type="button"
                  className={styles.ghost}
                  disabled={!collect.shape}
                  onClick={() => {
                    setShowRuleForm(true);
                    setRuleSaved(false);
                  }}
                >
                  {ruleSaved || collect.ruleId ? t.browser.ruleSaved : t.browser.saveRule}
                </button>
              ))}
              {!playedChosen && <button
                type="button"
                className={styles.ghost}
                disabled={!sourceKey || sourceExists || sourceDone?.key === sourceKey || sourcePhase !== "idle"}
                onClick={() => void registerSource()}
              >
                {sourceDone?.key === sourceKey && !sourceDone.existed
                  ? t.sources.trayRegistered
                  : sourceExists || sourceDone?.key === sourceKey
                    ? t.sources.rowRegistered
                    : registeringSource && sourcePhase === "previewing"
                      ? t.sources.previewing
                      : t.sources.trayRegister}
              </button>}
              {!playedChosen && (sourceNotice ? (
                <p role="alert" className={styles.collectError}>
                  {sourceNotice}
                </p>
              ) : (
                <p className={styles.adsNote}>{t.sources.trayNote}</p>
              ))}
            </div>
          </aside>
        )}
      </div>
    </section>
  );
}

function playedMeta(url: string, kind: string, durationSeconds: number | null): string {
  const parts = [safeHost(url), kind];
  if (durationSeconds != null) parts.push(formatDuration(durationSeconds));
  return parts.join(" · ");
}

function formatDuration(totalSeconds: number): string {
  const seconds = Math.max(0, Math.round(totalSeconds));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const tail = String(seconds % 60).padStart(2, "0");
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${tail}`
    : `${minutes}:${tail}`;
}

/** A page's host without "www.", for telling whether two URLs are on the same site. */
function siteHost(url: string): string {
  return safeHost(url).toLowerCase().replace(/^www\./, "");
}

function safeHost(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}

function ruleMatchPattern(url: string): string {
  try {
    const parsed = new URL(url);
    parsed.search = "";
    parsed.hash = "";
    return `${parsed.toString()}*`;
  } catch {
    return `${url.split("?")[0]}*`;
  }
}

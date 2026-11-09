import {
  ArrowLeft,
  ArrowRight,
  Check,
  Crosshair,
  Globe,
  KeyRound,
  LayoutGrid,
  LayoutList,
  LocateFixed,
  Lock,
  MousePointerClick,
  Plus,
  Repeat2,
  RotateCw,
  X,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import type { CollectState, Flow, FlowReplayItem, FlowScope, SavedFlow, SavedPage } from "../ipc/contract";
import { ipc, usesMockIpc } from "../ipc";
import { useT } from "../i18n/I18nContext";
import type { Translations } from "../i18n/types";
import { IMAGES_SHAPE, PLAYED_SHAPE } from "../lib/collect";
import { readDebugHash } from "../lib/debugHash";
import { nextFrame } from "../lib/frame";
import { defaultSavedPageName, savedPageUrl } from "../lib/savedPages";
import { selectedAccount, siteForUrl, siteHomeUrl, siteStartStatus } from "../lib/sites";
import { registerFailureText } from "../lib/sources";
import { errorMessage, normalizeUrl } from "../lib/url";
import { useJobs } from "../state/JobsContext";
import { useNavigation } from "../state/NavigationContext";
import { useSettings } from "../state/SettingsContext";
import { useSites } from "../state/SitesContext";
import { useSources } from "../state/SourcesContext";
import { AddressTools } from "./AddressTools";
import { IconButton } from "./IconButton";
import { ImportOffer } from "./ImportOffer";
import { PageImagesTray } from "./PageImagesTray";
import { SavedPages } from "./SavedPages";
import styles from "./BrowserView.module.css";

/** How long a loaded page is left alone before an asked-for 集める starts (pages that draw late). */
const COLLECT_SETTLE_MS = 500;
const FLOW_MISMATCH_PREFIX = "flow-step-mismatch:";
/** How long the copy-link button shows that the link was copied. */
const LINK_COPIED_MS = 1600;
/** The tray's checks go to the page's badges at most this often. */
const SELECTION_SYNC_MS = 150;

/** Where the element picker is: not picking, waiting for a press in the page, or looking for a pressed video's stream. */
type PickPhase = "off" | "active" | "finding";

/**
 * A choice of group and checks made by the picker, applied once the tray lists that group: every
 * item is checked (`only` null) or only these URLs are.
 */
interface PickSelection {
  shape: string;
  only: string[] | null;
}

/** True once `state` lists the planned group, with the items the plan names (any, when it names none). */
function pickSelectionReady(plan: PickSelection, state: CollectState): boolean {
  if (state.shape !== plan.shape || state.items.length === 0) return false;
  return plan.only === null || plan.only.every((url) => state.items.some((item) => item.url === url));
}

function pickSelectionChecks(plan: PickSelection, state: CollectState): Record<string, boolean> {
  return Object.fromEntries(state.items.map((item) => [item.url, plan.only === null || plan.only.includes(item.url)]));
}

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
  const { flowRequest } = useNavigation();
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
  // The element picker ("pick from the page"): where it is, the link that was pressed (waiting for
  // "all of this shape" or "just this one"), and whether a pressed video's stream never showed.
  const [pickPhase, setPickPhase] = useState<PickPhase>("off");
  const [pickedLink, setPickedLink] = useState<{ url: string; shape: string; count: number } | null>(null);
  const [pickNotFound, setPickNotFound] = useState(false);
  const pickSelection = useRef<PickSelection | null>(null);
  const latestCollect = useRef<CollectState | null>(null);
  const lastSentUnselected = useRef("");
  const chooseGroupRef = useRef<(shape: string, only: string[] | null) => Promise<void>>(async () => {});
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
  const mockFlowApplied = useRef(false);
  const [recording, setRecording] = useState(false);
  const [recordedSteps, setRecordedSteps] = useState(0);
  const [flow, setFlow] = useState<Flow | null>(null);
  const [flowScopes, setFlowScopes] = useState<FlowScope[]>([]);
  const [savedFlows, setSavedFlows] = useState<SavedFlow[]>([]);
  const [savedFlowsDismissed, setSavedFlowsDismissed] = useState(false);
  const [flowName, setFlowName] = useState("");
  const [flowNameError, setFlowNameError] = useState<string | null>(null);
  const [flowSaving, setFlowSaving] = useState(false);
  const [flowSaved, setFlowSaved] = useState(false);
  /** The steps came from a stored flow, so nothing was saved just now. */
  const [flowFromStore, setFlowFromStore] = useState(false);
  const [savedFlowId, setSavedFlowId] = useState<number | null>(null);
  const appliedFlowRequestSeq = useRef<number | null>(null);
  const [flowMismatch, setFlowMismatch] = useState<string | null>(null);
  const [replaying, setReplaying] = useState(false);
  const [flowProgress, setFlowProgress] = useState({ done: 0, total: 0 });
  const [flowResults, setFlowResults] = useState<FlowReplayItem[]>([]);
  const [flowCapped, setFlowCapped] = useState(false);
  const [selectedFlowResults, setSelectedFlowResults] = useState<Record<number, boolean>>({});
  const [savedPages, setSavedPages] = useState<SavedPage[]>([]);
  /** The toolbar's copy-link button shows a check for a moment after the link was copied. */
  const [linkCopied, setLinkCopied] = useState(false);

  const isLogin = browser.mode === "login";
  const pageOpen = browser.url !== null;
  const site = sites.find((s) => s.id === browser.siteId) ?? null;
  const collectSelectedCount = collect?.items.filter((item) => selectedUrls[item.url] !== false).length ?? 0;
  const flowSelectableCount = flowResults.filter((item) => item.media !== null).length;
  const flowSelectedCount = flowResults.filter((item, index) => item.media && selectedFlowResults[index] !== false).length;
  const selectedCount = collectSelectedCount + flowSelectedCount;
  const selectableCount = (collect?.items.length ?? 0) + flowSelectableCount;
  const allSelected = selectableCount > 0 && selectedCount === selectableCount;
  const trayOpen = collect !== null || flow !== null || (savedFlows.length > 0 && !savedFlowsDismissed);
  const trayItemCount = (collect?.items.length ?? 0) + flowResults.length;
  const scrollAction = collect ? scrollButton(collect, t.browser) : null;
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
    let current = true;
    setSavedFlowsDismissed(false);
    setSavedFlows([]);
    if (!browser.url || isLogin) {
      setSavedFlows([]);
      return () => {
        current = false;
      };
    }
    void ipc.call("flow_list", { url: browser.url }).then(
      (flows) => {
        if (current) setSavedFlows(flows);
      },
      (error) => {
        if (current) {
          setSavedFlows([]);
          setLocalError(errorMessage(error));
        }
      },
    );
    return () => {
      current = false;
    };
  }, [browser.url, isLogin]);

  useEffect(() => {
    if (flowRequest === null || !browser.url) return;
    if (appliedFlowRequestSeq.current === flowRequest.seq) return;
    const saved = savedFlows.find((item) => item.id === flowRequest.id);
    if (!saved) return;
    appliedFlowRequestSeq.current = flowRequest.seq;
    useSavedFlow(saved);
  }, [browser.url, flowRequest, savedFlows]);

  useEffect(() => {
    if (!added) return;
    const timer = window.setTimeout(() => setAdded(false), 1600);
    return () => window.clearTimeout(timer);
  }, [added]);

  useEffect(() => {
    if (!linkCopied) return undefined;
    const timer = window.setTimeout(() => setLinkCopied(false), LINK_COPIED_MS);
    return () => window.clearTimeout(timer);
  }, [linkCopied]);

  useEffect(() => {
    let current = true;
    ipc.call("saved_page_list", {}).then(
      (pages) => {
        if (current) setSavedPages(pages);
      },
      (error) => {
        if (current) setLocalError(errorMessage(error));
      },
    );
    return () => {
      current = false;
    };
  }, []);

  useEffect(
    () =>
      ipc.on("collect-state", (state) => {
        latestCollect.current = state;
        setCollect(state);
        // A group the picker asked for gets its checks as soon as the tray lists it.
        const asked = pickSelection.current;
        const plan = asked !== null && pickSelectionReady(asked, state) ? asked : null;
        if (plan) pickSelection.current = null;
        setSelectedUrls((previous) => {
          if (plan) return pickSelectionChecks(plan, state);
          const next: Record<string, boolean> = {};
          for (const item of state.items) next[item.url] = previous[item.url] ?? true;
          return next;
        });
      }),
    [],
  );

  // Badges clicked in the page (the app polls it every half second): the tray's checks follow.
  useEffect(
    () =>
      ipc.on("collect-toggles", ({ toggles }) => {
        setSelectedUrls((previous) => {
          const next = { ...previous };
          for (const toggle of toggles) next[toggle.url] = toggle.selected;
          return next;
        });
      }),
    [],
  );

  useEffect(
    () =>
      ipc.on("collect-pick", (event) => {
        switch (event.kind) {
          case "active":
            setPickedLink(null);
            setPickNotFound(false);
            setPickPhase("active");
            break;
          case "ended":
            setPickPhase("off");
            break;
          case "finding":
            setPickNotFound(false);
            setPickPhase("finding");
            break;
          case "link":
            setPickPhase("off");
            setPickedLink({ url: event.url, shape: event.shape, count: event.count });
            break;
          case "image":
            setPickPhase("off");
            void chooseGroupRef.current(IMAGES_SHAPE, [event.url]);
            break;
          case "video":
            setPickPhase("off");
            void chooseGroupRef.current(PLAYED_SHAPE, event.urls);
            break;
          case "videoNotFound":
            setPickPhase("off");
            setPickNotFound(true);
            break;
        }
      }),
    [],
  );

  // The tray's checks go to the page's badges: the items that are not checked, a moment after the
  // last change. A list that has unchecked items is sent again with every state, since a page
  // that was loaded again has forgotten it.
  useEffect(() => {
    if (!collect) return undefined;
    const unselected = collect.items.filter((item) => selectedUrls[item.url] === false).map((item) => item.url);
    const timer = window.setTimeout(() => {
      if (unselected.length === 0 && lastSentUnselected.current === "") return;
      lastSentUnselected.current = unselected.join("\n");
      void ipc.call("collect_set_selection", { unselected }).catch(() => {});
    }, SELECTION_SYNC_MS);
    return () => window.clearTimeout(timer);
  }, [collect, selectedUrls]);

  // A pressed link or a missing video belongs to the page it was pressed on.
  useEffect(() => {
    setPickedLink(null);
    setPickNotFound(false);
  }, [browser.url]);

  // The picker waits for a press in the page; Escape in the page ends it there, and here too.
  useEffect(() => {
    if (pickPhase !== "active") return undefined;
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") void stopPick();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [pickPhase]);

  useEffect(
    () =>
      ipc.on("flow-record-progress", ({ steps }) => {
        setRecordedSteps(steps);
      }),
    [],
  );

  useEffect(
    () =>
      ipc.on("flow-progress", (progress) => {
        setFlowProgress(progress);
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
    chooseGroupRef.current = chooseGroup;
  });

  // The mock has already opened the requested page; start collecting once BrowserView
  // receives it so screenshot URLs need no scripted click.
  useEffect(() => {
    const debug = readDebugHash();
    if (!usesMockIpc || !(debug.mockCollect || debug.mockScroll || debug.mockPick) || debugCollectApplied.current || !browser.url || browser.loading) return;
    debugCollectApplied.current = true;
    void startCollectRef.current();
  }, [browser.loading, browser.url]);

  useEffect(() => {
    const debug = readDebugHash();
    if (!usesMockIpc || !debug.mockFlow || mockFlowApplied.current) return;
    mockFlowApplied.current = true;
    void ipc.call("flow_record_stop", {}).then((recorded) => {
      setFlow(recorded);
      setFlowScopes(recorded.steps.map((_, index) => (index === 0 ? "restOfRow" : "this")));
      setFlowName(defaultFlowName(recorded));
      setFlowSaved(false);
      setFlowFromStore(false);
    });
  }, []);
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

  async function copyLink() {
    if (!browser.url) return;
    try {
      await ipc.call("copy_text", { text: browser.url });
      setLocalError(null);
      setLinkCopied(true);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function openExternal() {
    if (!browser.url) return;
    try {
      await ipc.call("open_external_url", { url: browser.url });
      setLocalError(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  /** The star: saves the page as shown (query kept, no #fragment), or takes it off the list. */
  async function toggleSavedPage() {
    if (!browser.url) return;
    const url = savedPageUrl(browser.url);
    const existing = savedPages.find((page) => page.url === url);
    try {
      if (existing) {
        await ipc.call("saved_page_remove", { id: existing.id });
        setSavedPages((previous) => previous.filter((page) => page.id !== existing.id));
      } else {
        const page = await ipc.call("saved_page_add", { url, name: defaultSavedPageName(url) });
        setSavedPages((previous) => (previous.some((item) => item.id === page.id) ? previous : [...previous, page]));
      }
      setLocalError(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function renameSavedPage(id: number, name: string) {
    const page = await ipc.call("saved_page_rename", { id, name });
    setSavedPages((previous) => previous.map((item) => (item.id === id ? page : item)));
  }

  async function removeSavedPage(id: number) {
    await ipc.call("saved_page_remove", { id });
    setSavedPages((previous) => previous.filter((page) => page.id !== id));
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

  async function toggleFlowRecording() {
    try {
      if (recording) {
        const recorded = await ipc.call("flow_record_stop", {});
        setRecording(false);
        setFlow(recorded);
        setFlowScopes(recorded.steps.map(() => "this"));
        setFlowName(defaultFlowName(recorded));
        setFlowNameError(null);
        setFlowSaved(false);
        setFlowFromStore(false);
        setFlowMismatch(null);
      } else {
        await startFlowRecording();
      }
      setLocalError(null);
    } catch (error) {
      setRecording(false);
      setLocalError(errorMessage(error));
    }
  }

  async function startFlowRecording() {
    await ipc.call("flow_record_start", {});
    setRecording(true);
    setRecordedSteps(0);
    setFlow(null);
    setFlowScopes([]);
    setFlowName("");
    setFlowNameError(null);
    setFlowSaved(false);
    setFlowFromStore(false);
    setSavedFlowId(null);
    setFlowMismatch(null);
    setFlowResults([]);
    setFlowCapped(false);
  }

  function useSavedFlow(saved: SavedFlow) {
    setFlow({ startUrl: saved.startUrl, steps: saved.steps, media: null });
    setFlowScopes(saved.scopes);
    setFlowName(saved.name);
    setFlowNameError(null);
    setFlowSaved(true);
    setFlowFromStore(true);
    setSavedFlowId(saved.id);
    setFlowMismatch(null);
    setFlowResults([]);
    setFlowCapped(false);
    setFlowProgress({ done: 0, total: 0 });
  }

  async function saveFlow() {
    if (!flow || flowSaving) return;
    const name = flowName.trim();
    if (!name) {
      setFlowNameError(t.browser.flowNameEmpty);
      return;
    }
    setFlowSaving(true);
    setFlowNameError(null);
    try {
      const saved = await ipc.call("flow_save", { name, flow: { ...flow, scopes: flowScopes } });
      setSavedFlows((previous) => [saved, ...previous.filter((item) => item.id !== saved.id)]);
      setSavedFlowsDismissed(false);
      setFlowName(saved.name);
      setFlowSaved(true);
      setFlowFromStore(false);
      setSavedFlowId(saved.id);
      setLocalError(null);
    } catch (error) {
      setFlowNameError(errorMessage(error));
    } finally {
      setFlowSaving(false);
    }
  }

  function setFlowScope(index: number, scope: FlowScope) {
    setFlowScopes((previous) => previous.map((value, position) => (position === index ? scope : value)));
  }

  async function replayFlow() {
    if (!flow || replaying) return;
    setReplaying(true);
    setFlowMismatch(null);
    setFlowResults([]);
    setFlowCapped(false);
    setFlowProgress({ done: 0, total: replayCount(flow, flowScopes) });
    try {
      const result = await ipc.call("flow_replay", { flow, scopes: flowScopes });
      setFlowResults(result.results);
      setFlowCapped(result.capped);
      setSelectedFlowResults(
        Object.fromEntries(result.results.map((item, index) => [index, item.media !== null])),
      );
      setLocalError(null);
    } catch (error) {
      const message = errorMessage(error);
      if (message.startsWith(FLOW_MISMATCH_PREFIX)) {
        setFlowMismatch(message.slice(FLOW_MISMATCH_PREFIX.length));
        setLocalError(null);
      } else {
        setLocalError(message);
      }
    } finally {
      setReplaying(false);
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

  /** The group select: the person's own choice, which also drops a pick still waiting to be checked. */
  function chooseShape(shape: string) {
    pickSelection.current = null;
    void selectShape(shape);
  }

  /**
   * Chooses a group the way the group select does and checks `only` of its items (every item when
   * null). A group that is chosen and listed already needs no new choice; otherwise the checks wait
   * for the group's items to arrive.
   */
  async function chooseGroup(shape: string, only: string[] | null) {
    const plan: PickSelection = { shape, only };
    const latest = latestCollect.current;
    if (latest && pickSelectionReady(plan, latest)) {
      pickSelection.current = null;
      setSelectedUrls(pickSelectionChecks(plan, latest));
      return;
    }
    pickSelection.current = plan;
    await selectShape(shape);
  }

  async function startPick() {
    setPickedLink(null);
    setPickNotFound(false);
    try {
      await ipc.call("collect_pick_start", { language: settings?.language ?? "ja" });
      setLocalError(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function stopPick() {
    try {
      await ipc.call("collect_pick_stop", {});
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  /** The pressed link's two choices: every link of its shape, or only that one. */
  async function collectPickedLink(onlyThis: boolean) {
    if (!pickedLink) return;
    const link = pickedLink;
    setPickedLink(null);
    await chooseGroup(link.shape, onlyThis ? [link.url] : null);
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
      if (collect) await ipc.call("collect_close", {});
      if (replaying) await ipc.call("flow_replay_cancel", {});
      setCollect(null);
      latestCollect.current = null;
      pickSelection.current = null;
      lastSentUnselected.current = "";
      setPickPhase("off");
      setPickedLink(null);
      setPickNotFound(false);
      setSelectedUrls({});
      setGoneUrls({});
      setShowRuleForm(false);
      setShowXNotice(false);
      setSourceNotice(null);
      setReloadFindStatus(null);
      setFlow(null);
      setFlowScopes([]);
      setFlowResults([]);
      setFlowCapped(false);
      setSelectedFlowResults({});
      setSavedFlowsDismissed(true);
      setFlowMismatch(null);
      setSavedFlowId(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  async function addCollected() {
    if (!settings) return;
    const chosen = collect?.items.filter((item) => selectedUrls[item.url] !== false) ?? [];
    const chosenFlow = flowResults.filter((item, index) => item.media && selectedFlowResults[index] !== false);
    if (chosen.length === 0 && chosenFlow.length === 0) return;
    try {
      if (collect && playedChosen) {
        const pageUrl = browser.url;
        if (!pageUrl) return;
        await enqueuePageMedia({
          items: chosen.map((item) => ({ mediaUrl: item.url, pageUrl, title: item.title })),
          preset: settings.defaultPreset,
          method: collect.ruleId == null ? undefined : { kind: "rule", id: collect.ruleId },
        });
      } else if (collect && imagesChosen) {
        if (!browser.url) return;
        const page = collect.page;
        await enqueuePageImages({
          pageUrl: browser.url,
          title: page?.title || null,
          uploader: page?.uploader ?? null,
          publishedAt: page?.publishedAt ?? null,
          id: page?.id ?? null,
          images: chosen.map((item, position) => ({ url: item.url, index: item.index ?? position + 1 })),
          method: collect.ruleId == null ? undefined : { kind: "rule", id: collect.ruleId },
        });
      } else if (chosen.length > 0) {
        await enqueue(
          chosen.map((item) => item.url),
          settings.defaultPreset,
          collect?.ruleId == null ? undefined : { kind: "rule", id: collect.ruleId },
        );
      }
      if (flow && chosenFlow.length > 0) {
        await enqueuePageMedia({
          items: chosenFlow.map((item) => ({
            mediaUrl: item.media as string,
            pageUrl: flow.startUrl,
            title: item.replayLabel,
          })),
          preset: settings.defaultPreset,
          method: savedFlowId == null ? undefined : { kind: "flow", id: savedFlowId },
        });
      }
      setAdded(true);
      setLocalError(null);
    } catch (error) {
      setLocalError(errorMessage(error));
    }
  }

  function setAllSelected(checked: boolean) {
    if (collect) {
      setSelectedUrls((previous) => {
        const next = { ...previous };
        for (const item of collect.items) next[item.url] = checked;
        return next;
      });
    }
    setSelectedFlowResults((previous) => {
      const next = { ...previous };
      flowResults.forEach((item, index) => {
        if (item.media) next[index] = checked;
      });
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

  // The login key's status: the site's, with the account's name when the site has several.
  const account = site ? selectedAccount(site) : null;
  const loginStatus = site ? siteStartStatus(t, site) : null;
  const loginLabel =
    loginStatus && account && site && site.accounts.length > 1
      ? t.browser.chipWithAccount(loginStatus.text, account.label)
      : (loginStatus?.text ?? "");
  // The page's own actions sit in the address field; a login page has none of them.
  const pageActionsShown = pageOpen && !isLogin;
  const pageSaved = browser.url !== null && savedPages.some((page) => page.url === savedPageUrl(browser.url ?? ""));
  const loginSiteName = site?.name ?? (browser.url ? safeHost(browser.url) : "");
  const message = localError ?? (browserError ? t.browser.failed(browserError) : null);

  return (
    <section className={styles.browser}>
      <div className={styles.bar}>
        <div className={styles.navGroup}>
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
        </div>

        <div className={styles.addrRow}>
          <form className={`${styles.addr} ${pageActionsShown ? styles.addrWithTools : ""}`} onSubmit={submitAddress}>
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
            {pageActionsShown && (
              <AddressTools
                login={loginStatus ? { present: loginStatus.present, label: loginLabel } : null}
                copied={linkCopied}
                saved={pageSaved}
                onLogin={onOpenSites}
                onCopy={() => void copyLink()}
                onToggleSaved={() => void toggleSavedPage()}
                onOpenExternal={() => void openExternal()}
              />
            )}
          </form>
        </div>

        <div className={styles.actionGroup}>
          {pageOpen && !isLogin && (
            <button type="button" className={styles.secondary} onClick={() => void startCollect()}>
              <LayoutGrid size={16} aria-hidden />
              {t.browser.collect}
            </button>
          )}

          {pageOpen && !isLogin && (
            <div className={styles.flowRecordControl}>
              <button type="button" className={styles.ghost} onClick={() => void toggleFlowRecording()}>
                <MousePointerClick size={16} aria-hidden />
                {recording ? t.browser.flowRecordStop : t.browser.flowRecord}
              </button>
              {recording && <span aria-live="polite">{t.browser.flowRecording(recordedSteps)}</span>}
            </div>
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

      <div className={trayOpen ? styles.collectWorkspace : styles.workspace}>
        <div className={styles.stage} ref={stageRef}>
          {!pageOpen ? (
          <div className={styles.start} ref={startRef}>
            <ImportOffer onGone={() => void focusStartList()} />
            {savedPages.length > 0 && (
              <SavedPages
                pages={savedPages}
                letterFor={(page) => (siteForUrl(sites, page.url)?.name ?? siteHost(page.url)).slice(0, 1).toUpperCase()}
                onOpen={(url) => void openUrl(url)}
                onRename={renameSavedPage}
                onRemove={removeSavedPage}
                onEmptied={() => void focusStartList()}
              />
            )}
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
            (collect && collect.items.length > 0 ? (
              <div className={styles.mockGrid} aria-hidden>
                {collect.items.map((item, index) => (
                  <div className={styles.mockCard} key={item.url}>
                    <span className={styles.mockCheck}>
                      <Check size={13} />
                    </span>
                    <div className={styles.mockThumb}>{item.index != null && <img src={item.url} alt="" />}</div>
                    {(item.index == null || item.title) && <strong>{item.title || item.url}</strong>}
                    {pickPhase === "active" && index === 2 && (
                      <>
                        <span className={styles.pickOutline} />
                        <span className={styles.pickTag}>
                          {t.browser.pickLabel(
                            collect.shape ?? "",
                            collect.groups.find((group) => group.shape === collect.shape)?.count ?? collect.items.length,
                          )}
                        </span>
                      </>
                    )}
                  </div>
                ))}
              </div>
            ) : (
              <div className={styles.placeholder}>{t.browser.pagePlaceholder}</div>
            ))
          )}
        </div>

        {trayOpen && (
          <aside className={styles.tray} aria-label={`${t.browser.foundLabel} ${trayItemCount}`}>
            <div className={styles.trayHead}>
              <div className={styles.trayTitleRow}>
                <h2>
                  {t.browser.foundLabel} <span>{trayItemCount}</span>
                </h2>
                <IconButton label={t.browser.closeCollect} size="sm" onClick={() => void closeCollect()}>
                  <X size={16} aria-hidden />
                </IconButton>
              </div>
              {collect && <div className={styles.progressLine}>
                <span>{scrollLine(collect, t.browser)}</span>
                {scrollAction && (
                  <button type="button" className={styles.textButton} onClick={() => void toggleCollect()}>
                    {scrollAction}
                  </button>
                )}
              </div>}
              {collect && collect.groups.length > 0 && collect.shape && (
                <div className={styles.groupRow}>
                  <select
                    className={styles.groupSelect}
                    value={collect.shape}
                    aria-label={t.browser.matchPattern}
                    onChange={(e) => chooseShape(e.target.value)}
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
                  <IconButton
                    size="sm"
                    label={t.browser.pickButton}
                    pressed={pickPhase === "active"}
                    onClick={() => void (pickPhase === "active" ? stopPick() : startPick())}
                  >
                    <Crosshair size={16} aria-hidden />
                  </IconButton>
                </div>
              )}
              {collect && <div className={styles.reloadFindLine}>
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
              </div>}
              {collect && collect.adsExcluded > 0 && (
                <p className={styles.adsNote}>{t.browser.adsExcluded(collect.adsExcluded)}</p>
              )}
              {showXNotice && <p className={styles.xNotice}>{t.browser.xCollectWarning}</p>}
              {collect?.error && <p className={styles.collectError}>{t.browser.collectFailed(collect.error)}</p>}
            </div>

            {collect && pickPhase !== "off" && (
              <div className={styles.trayNote}>
                <div className={styles.pickBox} role="status">
                  <span>{pickPhase === "active" ? t.browser.pickActive : t.browser.pickFinding}</span>
                  {pickPhase === "active" && (
                    <button type="button" className={styles.textButton} onClick={() => void stopPick()}>
                      {t.browser.pickCancel}
                    </button>
                  )}
                </div>
              </div>
            )}

            {collect && pickPhase === "off" && pickedLink && (
              <div className={styles.trayNote}>
                <div className={styles.pickBox}>
                  <p className={styles.pickResult}>
                    {t.browser.pickedLead}
                    <span className={styles.pickShape}>{pickedLink.shape}</span>
                  </p>
                  <div className={styles.pickActions}>
                    <button type="button" className={styles.secondary} onClick={() => void collectPickedLink(false)}>
                      {t.browser.pickAll(pickedLink.count)}
                    </button>
                    <button type="button" className={styles.ghost} onClick={() => void collectPickedLink(true)}>
                      {t.browser.pickOnly}
                    </button>
                  </div>
                </div>
              </div>
            )}

            {collect && pickPhase === "off" && pickNotFound && (
              <div className={styles.trayNote}>
                <p className={styles.adsNote} role="status">
                  {t.browser.pickNotFound}
                </p>
              </div>
            )}

            {savedFlows.length > 0 && (
              <section className={styles.savedFlowSection} aria-labelledby="saved-flows-title">
                <h3 id="saved-flows-title">{t.browser.savedFlows}</h3>
                <ul className={styles.savedFlowList}>
                  {savedFlows.map((saved) => (
                    <li key={saved.id}>
                      <span>
                        <strong>{saved.name}</strong>
                        <small>{t.browser.savedFlowSteps(saved.steps.length)}</small>
                      </span>
                      <button type="button" className={styles.textButton} onClick={() => useSavedFlow(saved)}>
                        {t.browser.useSavedFlow}
                      </button>
                    </li>
                  ))}
                </ul>
              </section>
            )}

            {flow && (
              <section className={styles.flowSection} aria-labelledby="recorded-flow-title">
                <h3 id="recorded-flow-title">{t.browser.learnedFlow}</h3>
                <ol className={styles.flowSteps}>
                  {flow.steps.map((step, index) => {
                    const restCount = Math.max(1, step.rowCount - step.rowIndex);
                    return (
                      <li key={`${step.pattern}-${index}`}>
                        <div className={styles.flowStepLine}>
                          <span>{t.browser.flowStep(step.label)}</span>
                          {step.matchCount >= 2 && (
                            <select
                              value={flowScopes[index] ?? "this"}
                              aria-label={`${index + 1}. ${t.browser.flowStep(step.label)}`}
                              onChange={(event) => setFlowScope(index, event.target.value as FlowScope)}
                            >
                              <option value="this">{t.browser.flowScopeThis}</option>
                              <option value="restOfRow">{t.browser.flowScopeRow(restCount)}</option>
                              <option value="allOfKind">{t.browser.flowScopeAll(step.matchCount)}</option>
                            </select>
                          )}
                        </div>
                        {index + 1 === flow.steps.length && (flow.media || !flowFromStore) && (
                          <p className={styles.flowMedia} title={flow.media ?? undefined}>
                            {flow.media ? safeHost(flow.media) : t.browser.flowNoMedia}
                          </p>
                        )}
                      </li>
                    );
                  })}
                </ol>
                <div className={styles.flowSaveLine}>
                  <input
                    type="text"
                    value={flowName}
                    maxLength={80}
                    readOnly={flowSaved}
                    aria-label={t.browser.flowName}
                    aria-invalid={flowNameError !== null || undefined}
                    onChange={(event) => {
                      setFlowName(event.target.value);
                      setFlowNameError(null);
                    }}
                  />
                  {flowSaved ? (
                    <span role="status">{flowFromStore ? t.browser.flowStored : t.browser.flowSaved}</span>
                  ) : (
                    <button
                      type="button"
                      className={styles.textButton}
                      disabled={flowSaving}
                      onClick={() => void saveFlow()}
                    >
                      {t.browser.flowSave}
                    </button>
                  )}
                </div>
                {flowNameError && (
                  <p role="alert" className={styles.collectError}>
                    {flowNameError}
                  </p>
                )}
                <div className={styles.flowReplayLine}>
                  <button
                    type="button"
                    className={styles.secondary}
                    disabled={replaying || flow.steps.length === 0}
                    onClick={() => void replayFlow()}
                  >
                    <Repeat2 size={16} aria-hidden />
                    {replaying ? t.browser.flowReplaying : t.browser.flowReplay}
                  </button>
                  {(replaying || flowProgress.total > 0) && (
                    <span aria-live="polite">{t.browser.flowProgress(flowProgress.done, flowProgress.total)}</span>
                  )}
                </div>
                {flowMismatch && (
                  <div className={styles.flowMismatch} role="alert">
                    <span>{t.browser.flowMismatch(flowMismatch)}</span>
                    <button type="button" className={styles.textButton} onClick={() => void toggleFlowRecording()}>
                      {t.browser.flowRecordAgain}
                    </button>
                  </div>
                )}
                {flowCapped && <p className={styles.adsNote}>{t.browser.flowCapped}</p>}
              </section>
            )}

            {selectableCount > 0 && (
              <label className={styles.allRow}>
                <input
                  ref={allCheckboxRef}
                  type="checkbox"
                  checked={allSelected}
                  onChange={() => setAllSelected(!allSelected)}
                />
                <span>{t.browser.all}</span>
                <span className={styles.allCount}>{t.browser.selectedOfTotal(selectedCount, selectableCount)}</span>
              </label>
            )}

            {flowResults.length > 0 && (
              <section className={styles.flowResultSection} aria-labelledby="flow-results-title">
                <h3 id="flow-results-title">{t.browser.flowResults(flowResults.length)}</h3>
                <ul className={`${styles.foundList} ${styles.flowFoundList}`}>
                  {flowResults.map((item, index) => (
                    <li key={`${item.replayLabel}-${index}`}>
                      <label>
                        <input
                          type="checkbox"
                          disabled={!item.media}
                          checked={Boolean(item.media) && selectedFlowResults[index] !== false}
                          onChange={(event) =>
                            setSelectedFlowResults((previous) => ({ ...previous, [index]: event.target.checked }))
                          }
                        />
                        <div>
                          <span title={item.replayLabel}>{item.replayLabel}</span>
                          <p className={styles.adsNote} title={item.media ?? undefined}>
                            {item.media ? safeHost(item.media) : t.browser.flowNoMedia}
                          </p>
                        </div>
                      </label>
                    </li>
                  ))}
                </ul>
              </section>
            )}

            {collect && (collect.items.length === 0 ? (
              <div className={styles.emptyArea}>
                <p>{playedChosen ? t.browser.playedEmpty : t.browser.collectEmpty}</p>
                <p>{t.browser.pickEmptyHint}</p>
                <button
                  type="button"
                  className={styles.secondary}
                  disabled={pickPhase !== "off"}
                  onClick={() => void startPick()}
                >
                  <Crosshair size={16} aria-hidden />
                  {t.browser.pickButton}
                </button>
              </div>
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
            ))}

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
              {collect && !playedChosen && (showRuleForm ? (
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
              {collect && !playedChosen && <button
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
              {collect && !playedChosen && (sourceNotice ? (
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

/** The tray's status line for the auto-scroll: what it is doing, or why it is not, and how many items it found. */
function scrollLine(collect: CollectState, text: Translations["browser"]): string {
  const count = collect.items.length;
  if (collect.running) return text.scrollRunning(count);
  if (collect.stopReason === "end") return text.scrollEnd(count);
  if (collect.stopReason === "cap") return text.scrollCap;
  return text.scrollStopped(count);
}

/** The button beside that line; none once the most items a session keeps were collected. */
function scrollButton(collect: CollectState, text: Translations["browser"]): string | null {
  if (collect.running) return text.collectStop;
  if (collect.stopReason === "end") return text.collectAgain;
  if (collect.stopReason === "cap") return null;
  return text.collectContinue;
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

function replayCount(flow: Flow, scopes: FlowScope[]): number {
  const total = flow.steps.reduce((count, step, index) => {
    const scope = scopes[index] ?? "this";
    const choices = scope === "allOfKind" ? step.matchCount : scope === "restOfRow" ? step.rowCount - step.rowIndex : 1;
    return count * Math.max(1, choices);
  }, 1);
  return Math.min(30, total);
}

function defaultFlowName(flow: Flow): string {
  const label = flow.steps[flow.steps.length - 1]?.label || "Flow";
  return [...`${label}（${safeHost(flow.startUrl)}）`].slice(0, 80).join("");
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

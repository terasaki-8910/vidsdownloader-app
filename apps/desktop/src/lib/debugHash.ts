// Debug-only affordance so screenshots can reach any state without scripted
// clicks, e.g. #view=library, #view=settings, #theme=dark, #settings=adblock, #sheet=1, and for the
// mock backend #mockUrl=<url>, #mockLogin=<domain>, #mockMulti=1, #mockUpdate=available. The
// browser tray can be opened without clicks with #view=browser&mockCollect=1 (links) or
// #view=browser&mockCollect=images (page images). List
// screenshots can use #select=<id>,<id>, then #menu=<id> and #confirm=delete. This is
// not app routing -- real navigation is plain React state. Collect-method screenshots use
// #view=methods, #mockCollectMethods=empty, #methodsMenu=<kind>-<id>, and
// #methodsDialog=<kind>-<id>. Read once on mount.
import type { Language, Theme } from "../ipc/contract";

export interface DebugHashState {
  view?: "downloads" | "library" | "sources" | "methods" | "browser" | "settings";
  theme?: Theme;
  language?: Language;
  /** Opens the settings view on this section (the same as #view=settings plus the section). */
  settingsSection?: "general" | "sites" | "adblock" | "tools" | "updates";
  openSheet?: boolean;
  /** Selects a specific queue row (by id) for the detail panel/sheet, instead
   *  of the default "first row". Verification-only, e.g. to check the detail
   *  panel against a Done row's kv content. */
  detailJob?: number;
  /** Selects these ids in the currently shown list, focusing the last one. */
  select?: number[];
  /** Opens this row's menu under its ellipsis button after debug selection is applied. */
  menu?: number;
  /** Runs the selection's Delete shortcut after debug selection is applied. */
  confirmDelete?: boolean;
  /** Forces the clipboard banner open without needing a real window-focus
   *  event. Verification-only. */
  forceClipboardBanner?: boolean;
  /** Mock backend only: start with this page open in the browser. */
  mockUrl?: string;
  /** Mock backend only: open the comments conflict dialog in Saved videos. */
  commentsConflict?: boolean;
  /** Mock backend only: open a page and start collecting links or page images. */
  mockCollect?: "links" | "images" | "played";
  /** Mock backend only: show the recorded-flow prototype with a two-step flow. */
  mockFlow?: boolean;
  /** Mock backend only: start in login mode for the site with this domain. */
  mockLogin?: string;
  /** Mock backend only: YouTube gets a second account. */
  mockMulti?: boolean;
  /** Mock backend only: X uses Firefox and Bilibili a cookies.txt file. */
  mockMethods?: boolean;
  /** Mock backend only: remove every saved collect rule and recorded flow. */
  mockCollectMethods?: "empty";
  /** Opens one collect-method row's menu once, e.g. "rule-2". */
  methodsMenu?: { kind: "rule" | "flow"; id: number };
  /** Opens one collect-method row's destination and quality dialog once. */
  methodsDialog?: { kind: "rule" | "flow"; id: number };
  /** Mock backend only: trash / forget leave side files ("sideFiles") or traces ("traces") behind. */
  mockErase?: "sideFiles" | "traces";
  /**
   * Mock backend only: Firefox has no profile ("none"), three ("many"), or the import
   * fails the way the Rust command does when a site could not be imported ("failing").
   */
  mockFirefox?: "none" | "many" | "failing";
  /**
   * Mock backend only: no sources at all ("empty"), or the three sources plus one with a very
   * long name and a very long collect-rule shape ("long").
   */
  mockSources?: "empty" | "long";
  /**
   * Mock backend only: what the update check finds (with no value: nothing, the queue as usual).
   * "latest" nothing new; "available" 0.2.0 ("availableLong" with notes that scroll); "busy"
   * 0.2.0 while two downloads run and stay running; "draining" the same, but they finish after
   * about 6 s; "skipped" 0.2.0, which the settings skip; "failing" 0.2.0, whose download fails
   * part way; "offline" the check fails. Every value but "busy" / "draining" starts with an idle
   * queue, and every value with all tools installed (no tools notice behind the dialog).
   */
  mockUpdate?: MockUpdate;
}

export type MockUpdate =
  | "latest"
  | "available"
  | "availableLong"
  | "busy"
  | "draining"
  | "skipped"
  | "failing"
  | "offline";

const MOCK_UPDATES: MockUpdate[] = [
  "latest",
  "available",
  "availableLong",
  "busy",
  "draining",
  "skipped",
  "failing",
  "offline",
];

export function readDebugHash(): DebugHashState {
  if (!import.meta.env.DEV || typeof location === "undefined") return {};
  const params = new URLSearchParams(location.hash.replace(/^#/, ""));
  const state: DebugHashState = {};

  const view = params.get("view");
  if (
    view === "downloads" ||
    view === "library" ||
    view === "sources" ||
    view === "methods" ||
    view === "browser" ||
    view === "settings"
  ) {
    state.view = view;
  }

  const theme = params.get("theme");
  if (theme === "light" || theme === "dark" || theme === "system") state.theme = theme;

  const language = params.get("language");
  if (language === "ja" || language === "en") state.language = language;

  const settingsSection = params.get("settings");
  if (
    settingsSection === "general" ||
    settingsSection === "sites" ||
    settingsSection === "adblock" ||
    settingsSection === "tools" ||
    settingsSection === "updates"
  ) {
    state.settingsSection = settingsSection;
  }

  if (params.get("sheet") === "1") state.openSheet = true;

  const detailJob = params.get("detailJob");
  if (detailJob && /^\d+$/.test(detailJob)) state.detailJob = Number(detailJob);

  const select = params.get("select");
  if (select && /^\d+(,\d+)*$/.test(select)) state.select = select.split(",").map(Number);
  const menu = params.get("menu");
  if (menu && /^\d+$/.test(menu)) state.menu = Number(menu);
  if (params.get("confirm") === "delete") state.confirmDelete = true;

  if (params.get("forceClipboardBanner") === "1") state.forceClipboardBanner = true;

  const mockUrl = params.get("mockUrl");
  if (mockUrl) state.mockUrl = mockUrl;
  if (params.get("commentsConflict") === "1") state.commentsConflict = true;
  const mockCollect = params.get("mockCollect");
  if (mockCollect === "1") state.mockCollect = "links";
  else if (mockCollect === "images") state.mockCollect = "images";
  else if (mockCollect === "played") state.mockCollect = "played";
  if (params.get("mockFlow") === "1") state.mockFlow = true;
  const mockLogin = params.get("mockLogin");
  if (mockLogin) state.mockLogin = mockLogin;
  if (params.get("mockMulti") === "1") state.mockMulti = true;
  if (params.get("mockMethods") === "1") state.mockMethods = true;
  if (params.get("mockCollectMethods") === "empty") state.mockCollectMethods = "empty";
  const methodKey = /^(rule|flow)-(\d+)$/;
  const methodsMenu = params.get("methodsMenu")?.match(methodKey);
  if (methodsMenu) state.methodsMenu = { kind: methodsMenu[1] as "rule" | "flow", id: Number(methodsMenu[2]) };
  const methodsDialog = params.get("methodsDialog")?.match(methodKey);
  if (methodsDialog) {
    state.methodsDialog = { kind: methodsDialog[1] as "rule" | "flow", id: Number(methodsDialog[2]) };
  }
  const mockErase = params.get("mockErase");
  if (mockErase === "sideFiles" || mockErase === "traces") state.mockErase = mockErase;
  const mockFirefox = params.get("mockFirefox");
  if (mockFirefox === "none" || mockFirefox === "many" || mockFirefox === "failing") {
    state.mockFirefox = mockFirefox;
  }
  const mockSources = params.get("mockSources");
  if (mockSources === "empty" || mockSources === "long") state.mockSources = mockSources;
  const mockUpdate = params.get("mockUpdate");
  if (mockUpdate && (MOCK_UPDATES as string[]).includes(mockUpdate)) state.mockUpdate = mockUpdate as MockUpdate;

  return state;
}

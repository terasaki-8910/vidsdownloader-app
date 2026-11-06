// Saved collect rules and recorded flows, grouped by site.
import { Workflow } from "lucide-react";
import {
  Fragment,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { useT } from "../i18n/I18nContext";
import { ipc } from "../ipc";
import type { CollectMethod, FlowScope } from "../ipc/contract";
import { readDebugHash } from "../lib/debugHash";
import { folderName } from "../lib/sources";
import { errorMessage } from "../lib/url";
import { useNavigation } from "../state/NavigationContext";
import { useSettings } from "../state/SettingsContext";
import { useSites } from "../state/SitesContext";
import { ConfirmDialog } from "./ConfirmDialog";
import downloadsStyles from "./DownloadsView.module.css";
import emptyStyles from "./EmptyState.module.css";
import queueStyles from "./QueueRow.module.css";
import { RowMenu, type MenuEntry, type RowMenuHandle } from "./RowMenu";
import { SourceOptionsDialog } from "./SourceOptionsDialog";
import sourceStyles from "./SourceRow.module.css";
import sitesStyles from "./SitesSection.module.css";
import styles from "./MethodsView.module.css";

const MAX_NAME = 200;

interface MethodGroup {
  key: string;
  name: string;
  registered: boolean;
  methods: CollectMethod[];
}

function sortMethods(methods: CollectMethod[]): CollectMethod[] {
  return [...methods].sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === "rule" ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
}

export function MethodsView() {
  const t = useT();
  const { sites } = useSites();
  const [methods, setMethods] = useState<CollectMethod[] | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const focusIndex = useRef<number | null>(null);
  const debug = useRef(readDebugHash());

  async function load() {
    setMethods(await ipc.call("methods_list", {}));
  }

  useEffect(() => {
    void load();
  }, []);

  const groups = useMemo<MethodGroup[]>(() => {
    if (!methods) return [];
    const result: MethodGroup[] = [];
    for (const site of sites) {
      const rows = methods.filter((method) => method.siteId === site.id);
      if (rows.length > 0) {
        result.push({ key: `site-${site.id}`, name: site.name, registered: true, methods: sortMethods(rows) });
      }
    }
    const unregistered = new Map<string, CollectMethod[]>();
    for (const method of methods) {
      if (method.siteId !== null) continue;
      const rows = unregistered.get(method.siteName) ?? [];
      rows.push(method);
      unregistered.set(method.siteName, rows);
    }
    for (const name of [...unregistered.keys()].sort((a, b) => a.localeCompare(b))) {
      result.push({ key: `host-${name}`, name, registered: false, methods: sortMethods(unregistered.get(name) ?? []) });
    }
    return result;
  }, [methods, sites]);

  const ordered = groups.flatMap((group) => group.methods);
  const ruleCount = methods?.filter((method) => method.kind === "rule").length ?? 0;
  const flowCount = methods?.filter((method) => method.kind === "flow").length ?? 0;

  useLayoutEffect(() => {
    const index = focusIndex.current;
    if (index === null) return;
    focusIndex.current = null;
    const buttons = listRef.current?.querySelectorAll<HTMLButtonElement>("[aria-haspopup='menu']");
    buttons?.[Math.min(index, buttons.length - 1)]?.focus();
  }, [methods]);

  async function remove(method: CollectMethod) {
    focusIndex.current = ordered.findIndex((item) => item.kind === method.kind && item.id === method.id);
    try {
      await ipc.call("method_delete", { kind: method.kind, id: method.id });
      await load();
    } catch (error) {
      focusIndex.current = null;
      throw error;
    }
  }

  return (
    <section className={downloadsStyles.downloads}>
      <div className={downloadsStyles.head}>
        <h1>{t.methods.title}</h1>
        {methods && methods.length > 0 && (
          <div className={downloadsStyles.summary}>{t.methods.summary(ruleCount, flowCount)}</div>
        )}
      </div>

      {!methods ? null : methods.length === 0 ? (
        <div className={emptyStyles.empty}>
          <div className={emptyStyles.icon} aria-hidden>
            <Workflow size={32} />
          </div>
          <div className={emptyStyles.body}>{t.methods.emptyBody}</div>
        </div>
      ) : (
        <div className={styles.groups} ref={listRef}>
          {groups.map((group) => {
            const badge = Array.from(group.name)[0]?.toUpperCase() ?? "";
            return (
              <section key={group.key} className={styles.group} aria-labelledby={`${group.key}-title`}>
                <h2 id={`${group.key}-title`} className={styles.groupHeading}>
                  <span className={sitesStyles.icon} aria-hidden>{badge}</span>
                  <span>{group.name}</span>
                  {!group.registered && <span className={styles.unregistered}>{t.methods.notRegistered}</span>}
                </h2>
                <ul className={`${downloadsStyles.list} ${styles.list}`}>
                  {group.methods.map((method) => (
                    <MethodRow
                      key={`${method.kind}-${method.id}`}
                      method={method}
                      debugMenu={debug.current.methodsMenu?.kind === method.kind && debug.current.methodsMenu.id === method.id}
                      debugDialog={debug.current.methodsDialog?.kind === method.kind && debug.current.methodsDialog.id === method.id}
                      onRefresh={load}
                      onRemove={remove}
                    />
                  ))}
                </ul>
              </section>
            );
          })}
        </div>
      )}
    </section>
  );
}

interface MethodRowProps {
  method: CollectMethod;
  debugMenu: boolean;
  debugDialog: boolean;
  onRefresh: () => Promise<void>;
  onRemove: (method: CollectMethod) => Promise<void>;
}

function MethodRow({ method, debugMenu, debugDialog, onRefresh, onRemove }: MethodRowProps) {
  const t = useT();
  const { settings } = useSettings();
  const { openInBrowser } = useNavigation();
  const menuRef = useRef<RowMenuHandle>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const debugApplied = useRef(false);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState("");
  const [dialog, setDialog] = useState<"options" | "delete" | null>(debugDialog ? "options" : null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [focusTick, setFocusTick] = useState(0);

  useEffect(() => {
    if (focusTick > 0) menuRef.current?.focus();
  }, [focusTick]);

  useEffect(() => {
    if (!renaming) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [renaming]);

  useEffect(() => {
    if (debugApplied.current) return;
    debugApplied.current = true;
    if (debugMenu) menuRef.current?.open();
  }, [debugMenu]);

  function fail(error: unknown) {
    const message = errorMessage(error);
    const source = message.startsWith("rule-in-use:") ? message.slice("rule-in-use:".length) : null;
    setError(source ? t.methods.ruleInUse(source) : t.library.failed(message));
  }

  function startRename() {
    setError(null);
    setDraft(method.name);
    setRenaming(true);
  }

  function stopRename(returnFocus: boolean) {
    setRenaming(false);
    setError(null);
    if (returnFocus) setFocusTick((value) => value + 1);
  }

  async function submitRename() {
    if (busy) return;
    const name = draft.trim();
    if (!name) {
      setError(t.library.nameEmpty);
      return;
    }
    if (name === method.name) {
      stopRename(true);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await ipc.call("method_rename", { kind: method.kind, id: method.id, name });
      await onRefresh();
      setRenaming(false);
      setFocusTick((value) => value + 1);
    } catch (error) {
      fail(error);
    } finally {
      setBusy(false);
    }
  }

  function onRenameKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key === "Enter") {
      event.preventDefault();
      void submitRename();
    } else if (event.key === "Escape") {
      event.preventDefault();
      if (!busy) stopRename(true);
    }
  }

  async function useMethod() {
    setError(null);
    try {
      if (method.kind === "rule" && method.shape) {
        await openInBrowser(method.pageUrl, { collectShape: method.shape });
      } else {
        await openInBrowser(method.pageUrl, { flowId: method.id });
      }
    } catch (error) {
      fail(error);
    }
  }

  async function remove() {
    setDialog(null);
    setBusy(true);
    setError(null);
    try {
      await onRemove(method);
    } catch (error) {
      fail(error);
      setFocusTick((value) => value + 1);
    } finally {
      setBusy(false);
    }
  }

  const entries: MenuEntry[] = [
    { kind: "item", label: t.library.rename, onSelect: startRename },
    { kind: "item", label: t.methods.options, onSelect: () => setDialog("options") },
    { kind: "separator" },
    { kind: "item", label: t.common.delete, danger: true, onSelect: () => setDialog("delete") },
  ];

  const scopes = (method.scopes ?? []).map((scope: FlowScope) => {
    if (scope === "this") return t.browser.flowScopeThis;
    if (scope === "restOfRow") return t.browser.flowScopeRow();
    return t.browser.flowScopeAll();
  });
  const detailParts = [
    method.destDir ? t.methods.destination(folderName(method.destDir)) : t.methods.defaultDestination,
    method.preset ? t.methods.quality(t.presets[method.preset]) : null,
    method.lastUsedAt == null ? t.methods.neverUsed : t.methods.lastUsed(t.methods.date(method.lastUsedAt, new Date())),
  ].filter((part): part is string => part !== null);

  return (
    <li
      className={sourceStyles.row}
      aria-busy={busy}
      onContextMenu={(event) => {
        event.preventDefault();
        if (!busy) menuRef.current?.openAt(event.clientX, event.clientY);
      }}
      onKeyDown={(event) => {
        if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
          event.preventDefault();
          if (!busy) menuRef.current?.open();
        }
      }}
    >
      <span aria-hidden />
      <div className={sourceStyles.body}>
        {renaming ? (
          <input
            ref={inputRef}
            type="text"
            className={sourceStyles.renameInput}
            value={draft}
            readOnly={busy}
            maxLength={MAX_NAME}
            spellCheck={false}
            autoComplete="off"
            aria-label={t.library.newName}
            aria-invalid={error !== null}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={onRenameKeyDown}
            onBlur={() => {
              if (!busy) stopRename(false);
            }}
          />
        ) : (
          <div className={sourceStyles.title}>{method.name}</div>
        )}

        <div className={sourceStyles.meta}>
          {method.kind === "rule" ? (
            <>
              {t.methods.ruleKind} · <span className={sourceStyles.shape}>{method.shape}</span>
            </>
          ) : (
            t.methods.flowMeta(method.stepCount ?? 0, scopes.join(document.documentElement.lang === "ja" ? "、" : ", "))
          )}
        </div>
        <div className={sourceStyles.meta}>
          {detailParts.map((part, index) => (
            <Fragment key={part}>{index > 0 && " · "}{part}</Fragment>
          ))}
        </div>
        {error && <p role="alert" className={sourceStyles.error}>{error}</p>}
      </div>

      <div className={sourceStyles.actions}>
        <button
          type="button"
          className={`${queueStyles.textAction} ${sourceStyles.check}`}
          disabled={busy}
          onClick={() => void useMethod()}
        >
          {t.methods.use}
        </button>
        <RowMenu
          ref={menuRef}
          label={t.library.moreActions}
          entries={entries}
          disabled={busy}
          buttonTabIndex={0}
          onReturnFocus={() => menuRef.current?.focus()}
        />
      </div>

      {dialog === "options" && settings && (
        <SourceOptionsDialog
          title={t.methods.optionsTitle(method.name)}
          destDir={method.destDir}
          preset={method.preset}
          defaultDir={settings.downloadDir}
          defaultPreset={settings.defaultPreset}
          changeFolderLabel={t.methods.chooseFolder}
          onSave={async (destDir, preset) => {
            await ipc.call("method_set_options", { kind: method.kind, id: method.id, destDir, preset });
            await onRefresh();
          }}
          onClose={() => {
            setDialog(null);
            setFocusTick((value) => value + 1);
          }}
        />
      )}
      {dialog === "delete" && (
        <ConfirmDialog
          title={t.methods.deleteTitle(method.name)}
          body={method.kind === "rule" ? t.methods.deleteRuleBody : t.methods.deleteFlowBody}
          cancelLabel={t.common.dialogCancel}
          confirmLabel={t.common.delete}
          onCancel={() => {
            setDialog(null);
            setFocusTick((value) => value + 1);
          }}
          onConfirm={() => void remove()}
        />
      )}
    </li>
  );
}

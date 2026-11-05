import { ChevronDown, Trash2 } from "lucide-react";
import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent, type MouseEvent } from "react";
import { useFirefoxImport } from "../hooks/useFirefoxImport";
import { ipc } from "../ipc";
import type { Account, LoginMethod, SavedFlow, Site } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { nextFrame } from "../lib/frame";
import { accountStatus, selectedAccount } from "../lib/sites";
import { errorMessage, normalizeUrl } from "../lib/url";
import { useSites } from "../state/SitesContext";
import { ConfirmDialog } from "./ConfirmDialog";
import { IconButton } from "./IconButton";
import { ImportProfile } from "./ImportProfile";
import { RowMenu, type MenuEntry, type RowMenuHandle } from "./RowMenu";
import { SiteReport } from "./SiteReport";
import settingsStyles from "./SettingsView.module.css";
import styles from "./SitesSection.module.css";

interface SitesSectionProps {
  /** Opens the in-app browser on the account's login page. */
  onLogin: (siteId: number, accountId: number) => void;
}

/** Settings > Sites: add a site, then where each site's login comes from, per account. */
export function SitesSection({ onLogin }: SitesSectionProps) {
  const t = useT();
  const { sites } = useSites();
  const [profiles, setProfiles] = useState<string[]>([]);

  useEffect(() => {
    void ipc.call("firefox_profiles", {}).then(setProfiles, () => setProfiles([]));
  }, []);

  return (
    <div className={styles.body}>
      <SiteAdd />
      <p className={styles.lead}>{t.sites.lead}</p>
      <ul className={styles.list}>
        {sites.map((site) => (
          <SiteRow key={site.id} site={site} profiles={profiles} onLogin={onLogin} />
        ))}
      </ul>
      <FirefoxImportBlock />
    </div>
  );
}

/**
 * Brings the logins of a Firefox profile into the sites that have none here: the same import
 * as the browser's one-time offer, kept for later. Not shown without Windows or a profile.
 */
function FirefoxImportBlock() {
  const t = useT();
  const headingId = useId();
  const imp = useFirefoxImport();
  const buttonRef = useRef<HTMLButtonElement>(null);
  const wasRunning = useRef(false);

  // The button is disabled while the import runs; the keyboard focus returns to it after.
  useEffect(() => {
    if (imp.running) {
      wasRunning.current = true;
    } else if (wasRunning.current) {
      wasRunning.current = false;
      buttonRef.current?.focus();
    }
  }, [imp.running]);

  if (imp.profiles.length === 0) return null;

  return (
    <section className={styles.importBlock} aria-labelledby={headingId}>
      <h2 id={headingId} className={styles.addHeading}>
        {t.importLogins.blockTitle}
      </h2>
      <p className={styles.lead}>{t.importLogins.blockLead}</p>
      <div className={styles.importRow}>
        <ImportProfile
          profiles={imp.profiles}
          value={imp.profile}
          onChange={imp.chooseProfile}
          disabled={imp.running}
        />
        <button
          type="button"
          className={styles.secondary}
          ref={buttonRef}
          disabled={imp.running}
          onClick={() => void imp.start()}
        >
          {imp.running ? t.importLogins.importing : t.importLogins.importButton}
        </button>
      </div>
      <div role="status">{imp.resultLine && <p className={styles.importResult}>{imp.resultLine}</p>}</div>
      {imp.errorLine && (
        <p role="alert" className={styles.addError}>
          {imp.errorLine}
        </p>
      )}
    </section>
  );
}

/** The badge (first letter), name and, when it differs from the name, the host of a site. */
export function SiteIdentity({ site }: { site: Site }) {
  const showHost = site.domains[0] != null && site.domains[0] !== site.name;
  return (
    <>
      <span className={styles.icon} aria-hidden>
        {site.name.slice(0, 1).toUpperCase()}
      </span>
      <span className={styles.nameBlock}>
        <span className={styles.name}>{site.name}</span>
        {showHost && <span className={styles.host}>{site.domains[0]}</span>}
      </span>
    </>
  );
}

function methodValue(method: LoginMethod): string {
  return method.kind === "firefox" ? `firefox:${method.profile}` : method.kind;
}

/** The number for the next default account label ("Account 2" after "Main"). */
function nextAccountNumber(accounts: Account[]): number {
  let highest = accounts.length;
  for (const account of accounts) {
    const match = /(\d+)$/.exec(account.label);
    if (match) highest = Math.max(highest, Number(match[1]));
  }
  return highest + 1;
}

function SiteRow({
  site,
  profiles,
  onLogin,
}: {
  site: Site;
  profiles: string[];
  onLogin: (siteId: number, accountId: number) => void;
}) {
  const t = useT();
  const groupName = useId();
  const deleteCellRef = useRef<HTMLDivElement>(null);
  const deleteMenuRef = useRef<RowMenuHandle>(null);
  const [pending, setPending] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [showFlows, setShowFlows] = useState(false);
  const [flows, setFlows] = useState<SavedFlow[]>([]);
  const [flowsLoading, setFlowsLoading] = useState(false);
  const [flowBusyId, setFlowBusyId] = useState<number | null>(null);
  const [renamingFlowId, setRenamingFlowId] = useState<number | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [confirmingFlowDelete, setConfirmingFlowDelete] = useState<SavedFlow | null>(null);
  const [flowError, setFlowError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { siteReports } = useSites();

  const selected = selectedAccount(site);
  const multi = site.accounts.length > 1;
  const report = siteReports[site.id];

  async function run(action: () => Promise<void>) {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(t.sites.failed(errorMessage(e)));
    } finally {
      setPending(false);
    }
  }

  async function setMethod(account: Account, method: LoginMethod) {
    await ipc.call("account_set_method", { accountId: account.id, method });
  }

  function changeMethod(account: Account, value: string) {
    void run(async () => {
      if (value === "inApp") {
        await setMethod(account, { kind: "inApp" });
      } else if (value.startsWith("firefox:")) {
        await setMethod(account, { kind: "firefox", profile: value.slice("firefox:".length) });
      } else {
        await pickCookiesFile(account);
      }
    });
  }

  async function pickCookiesFile(account: Account) {
    const path = await ipc.call("pick_cookies_file", {});
    if (path) await setMethod(account, { kind: "cookiesTxt", path });
  }

  function addAccount() {
    void run(async () => {
      const account = await ipc.call("account_add", {
        siteId: site.id,
        label: t.sites.accountName(nextAccountNumber(site.accounts)),
      });
      onLogin(site.id, account.id);
    });
  }

  function accountAction(account: Account) {
    if (account.method.kind !== "inApp") return null;
    if (account.cookieCount != null) {
      return (
        <button
          type="button"
          className={styles.link}
          disabled={pending}
          onClick={() => void run(() => ipc.call("account_logout", { accountId: account.id }).then(() => undefined))}
        >
          {t.common.logout}
        </button>
      );
    }
    return (
      <button type="button" className={styles.secondary} disabled={pending} onClick={() => onLogin(site.id, account.id)}>
        {t.common.login}
      </button>
    );
  }

  async function removeSite() {
    setConfirmingDelete(false);
    await run(() => ipc.call("site_remove", { siteId: site.id }).then(() => undefined));
    // Removed: the row is gone and so is the cell. Failed: the dialog took the focus
    // with it, so hand it back to the trash button.
    await nextFrame();
    deleteCellRef.current?.querySelector("button")?.focus();
  }

  async function openFlows() {
    setShowFlows(true);
    setFlowsLoading(true);
    setFlowError(null);
    try {
      setFlows(await ipc.call("flow_list_for_site", { siteId: site.id }));
    } catch (e) {
      setFlowError(t.sites.failed(errorMessage(e)));
    } finally {
      setFlowsLoading(false);
    }
  }

  function startFlowRename(flow: SavedFlow) {
    setRenamingFlowId(flow.id);
    setRenameDraft(flow.name);
    setFlowError(null);
  }

  async function renameFlow(flow: SavedFlow) {
    const name = renameDraft.trim();
    if (!name) {
      setFlowError(t.sites.flowNameEmpty);
      return;
    }
    setFlowBusyId(flow.id);
    setFlowError(null);
    try {
      await ipc.call("flow_rename", { id: flow.id, name });
      setFlows((previous) =>
        previous.map((item) => (item.id === flow.id ? { ...item, name, updatedAt: Math.floor(Date.now() / 1000) } : item)),
      );
      setRenamingFlowId(null);
    } catch (e) {
      setFlowError(t.sites.failed(errorMessage(e)));
    } finally {
      setFlowBusyId(null);
    }
  }

  async function deleteFlow() {
    const flow = confirmingFlowDelete;
    if (!flow) return;
    setConfirmingFlowDelete(null);
    setFlowBusyId(flow.id);
    setFlowError(null);
    try {
      await ipc.call("flow_delete", { id: flow.id });
      setFlows((previous) => previous.filter((item) => item.id !== flow.id));
    } catch (e) {
      setFlowError(t.sites.failed(errorMessage(e)));
    } finally {
      setFlowBusyId(null);
    }
  }

  const selectedStatus = selected ? accountStatus(t, selected) : null;
  const deleteEntries: MenuEntry[] = [];
  if (site.flowCount > 0) {
    deleteEntries.push({ kind: "item", label: t.sites.viewFlows, onSelect: () => void openFlows() });
  }
  if (!site.builtin) {
    deleteEntries.push({ kind: "item", label: t.sites.deleteSiteMenu, danger: true, onSelect: () => setConfirmingDelete(true) });
  }

  function openContextMenu(e: MouseEvent<HTMLLIElement>) {
    if (deleteEntries.length === 0 || pending) return;
    // Form fields keep their own right-click; buttons have none in the app, so the row's menu
    // opens over them too (the trash icon included).
    if (e.target instanceof Element && e.target.closest("input, select, textarea")) return;
    e.preventDefault();
    deleteMenuRef.current?.openAt(e.clientX, e.clientY);
  }

  function openKeyboardMenu(e: KeyboardEvent<HTMLLIElement>) {
    if (deleteEntries.length === 0 || pending || (e.key !== "ContextMenu" && !(e.shiftKey && e.key === "F10"))) return;
    e.preventDefault();
    deleteMenuRef.current?.open();
  }

  return (
    <li className={styles.row} onContextMenu={openContextMenu} onKeyDown={openKeyboardMenu}>
      <div className={`${styles.head} ${styles.headAction}`}>
        <SiteIdentity site={site} />
        {selected && (
          <label className={styles.methodWrap}>
            <select
              className={styles.method}
              aria-label={`${site.name}: ${t.sites.methodLabel}`}
              value={methodValue(selected.method)}
              disabled={pending}
              onChange={(e) => changeMethod(selected, e.target.value)}
            >
              <option value="inApp">{t.sites.inApp}</option>
              {profiles.map((profile) => (
                <option key={profile} value={`firefox:${profile}`}>
                  {t.sites.firefox(profile)}
                </option>
              ))}
              {selected.method.kind === "firefox" && !profiles.includes(selected.method.profile) && (
                <option value={methodValue(selected.method)}>{t.sites.firefox(selected.method.profile)}</option>
              )}
              <option value="cookiesTxt">
                {selected.method.kind === "cookiesTxt" ? t.sites.cookiesTxt : t.sites.cookiesTxtPick}
              </option>
            </select>
            <ChevronDown size={14} aria-hidden className={styles.methodChevron} />
          </label>
        )}
        {/* A fourth column on every row so the selects line up; built-in sites cannot be removed. */}
        <div className={styles.actionCell} ref={deleteCellRef}>
          {!site.builtin && (
            <IconButton
              label={t.sites.deleteSite(site.name)}
              disabled={pending}
              onClick={() => setConfirmingDelete(true)}
            >
              <Trash2 size={16} aria-hidden />
            </IconButton>
          )}
          {deleteEntries.length > 0 && (
            <RowMenu
              ref={deleteMenuRef}
              label={t.sites.deleteSite(site.name)}
              entries={deleteEntries}
              disabled={pending}
              anchorRef={deleteCellRef}
              hideButton={!site.builtin}
              onReturnFocus={() => deleteCellRef.current?.querySelector("button")?.focus()}
            />
          )}
        </div>
      </div>
      {site.flowCount > 0 && <p className={styles.flowCount}>{t.sites.flowCount(site.flowCount)}</p>}
      {confirmingDelete && (
        <ConfirmDialog
          title={t.sites.deleteConfirmTitle(site.name)}
          body={t.sites.deleteConfirmBody(site.accounts.length)}
          cancelLabel={t.common.dialogCancel}
          confirmLabel={t.common.delete}
          onCancel={() => setConfirmingDelete(false)}
          onConfirm={() => void removeSite()}
        />
      )}
      {confirmingFlowDelete && (
        <ConfirmDialog
          title={t.sites.deleteFlowTitle(confirmingFlowDelete.name)}
          body={t.sites.deleteFlowBody}
          cancelLabel={t.common.dialogCancel}
          confirmLabel={t.common.delete}
          onCancel={() => setConfirmingFlowDelete(null)}
          onConfirm={() => void deleteFlow()}
        />
      )}

      {showFlows && (
        <section className={styles.flowPanel} aria-label={t.sites.flowsTitle(site.name)}>
          <div className={styles.flowPanelHead}>
            <h3>{t.sites.flowsTitle(site.name)}</h3>
            <button type="button" className={styles.link} onClick={() => setShowFlows(false)}>
              {t.common.close}
            </button>
          </div>
          {!flowsLoading && (
            <ul className={styles.flowList}>
              {flows.map((flow) => (
                <li key={flow.id}>
                  <div className={styles.flowName}>
                    {renamingFlowId === flow.id ? (
                      <input
                        autoFocus
                        type="text"
                        maxLength={80}
                        value={renameDraft}
                        readOnly={flowBusyId === flow.id}
                        aria-label={t.sites.flowNameLabel}
                        aria-invalid={flowError !== null || undefined}
                        onChange={(event) => {
                          setRenameDraft(event.target.value);
                          setFlowError(null);
                        }}
                        onBlur={() => {
                          if (flowBusyId !== flow.id) setRenamingFlowId(null);
                        }}
                        onKeyDown={(event) => {
                          if (event.nativeEvent.isComposing || event.keyCode === 229) return;
                          if (event.key === "Enter") {
                            event.preventDefault();
                            void renameFlow(flow);
                          } else if (event.key === "Escape") {
                            event.preventDefault();
                            if (flowBusyId !== flow.id) setRenamingFlowId(null);
                          }
                        }}
                      />
                    ) : (
                      <span>{flow.name}</span>
                    )}
                    <small>{t.sites.flowSteps(flow.steps.length)}</small>
                  </div>
                  <div className={styles.flowActions}>
                    <button
                      type="button"
                      className={styles.link}
                      disabled={flowBusyId !== null}
                      onClick={() => startFlowRename(flow)}
                    >
                      {t.sites.renameFlow}
                    </button>
                    <button
                      type="button"
                      className={styles.link}
                      disabled={flowBusyId !== null}
                      onClick={() => setConfirmingFlowDelete(flow)}
                    >
                      {t.common.delete}
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
          {flowError && (
            <p role="alert" className={styles.error}>
              {flowError}
            </p>
          )}
        </section>
      )}

      {report && (
        <SiteReport
          siteId={site.id}
          entry={report}
          onLogin={selected?.method.kind === "inApp" ? () => onLogin(site.id, selected.id) : null}
        />
      )}

      {multi ? (
        <div className={styles.accounts}>
          {site.accounts.map((account) => {
            const status = accountStatus(t, account);
            const inputId = `${groupName}-${account.id}`;
            return (
              <div key={account.id} className={styles.account}>
                <input
                  id={inputId}
                  type="radio"
                  name={groupName}
                  className={styles.radio}
                  checked={account.id === selected?.id}
                  disabled={pending}
                  aria-label={t.sites.selectAccount(account.label)}
                  onChange={() =>
                    void run(() => ipc.call("account_select", { siteId: site.id, accountId: account.id }).then(() => undefined))
                  }
                />
                <label htmlFor={inputId} className={styles.accountLabel}>
                  {account.label}
                </label>
                <span className={status.present ? styles.status : styles.none}>{status.text}</span>
                {accountAction(account)}
                <button
                  type="button"
                  className={styles.link}
                  disabled={pending}
                  onClick={() =>
                    void run(() => ipc.call("account_remove", { accountId: account.id }).then(() => undefined))
                  }
                >
                  {t.common.delete}
                </button>
              </div>
            );
          })}
          <div className={styles.accountsFoot}>
            <button type="button" className={styles.secondary} disabled={pending} onClick={addAccount}>
              {t.sites.addAccount}
            </button>
          </div>
        </div>
      ) : (
        selected &&
        selectedStatus && (
          <div className={styles.foot}>
            <span className={selectedStatus.present ? styles.status : styles.none}>{selectedStatus.text}</span>
            {selected.method.kind === "cookiesTxt" && (
              <button
                type="button"
                className={styles.link}
                disabled={pending}
                onClick={() => void run(() => pickCookiesFile(selected))}
              >
                {t.sites.changeFile}
              </button>
            )}
            {accountAction(selected)}
            <button type="button" className={styles.link} disabled={pending} onClick={addAccount}>
              {t.sites.addAccount}
            </button>
          </div>
        )
      )}

      {error && (
        <p role="alert" className={styles.error}>
          {error}
        </p>
      )}
    </li>
  );
}

/**
 * Adds a site by URL. It does not start a login: a site that needs none can be added
 * too, and the row's login button does it later.
 */
function SiteAdd() {
  const t = useT();
  const { probeSite } = useSites();
  const headingId = useId();
  const noteId = useId();
  const [text, setText] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (pending) return;
    const url = normalizeUrl(text);
    if (!url) {
      setError(t.browser.invalidUrl);
      return;
    }
    setPending(true);
    setError(null);
    try {
      const site = await ipc.call("site_add", { url });
      setText("");
      // The row shows what yt-dlp makes of the URL; the check runs on without this form.
      void probeSite(site.id, url);
    } catch (err) {
      setError(t.sites.failed(errorMessage(err)));
    } finally {
      setPending(false);
    }
  }

  return (
    <form className={styles.add} onSubmit={(e) => void submit(e)}>
      <h2 id={headingId} className={styles.addHeading}>
        {t.sites.addSiteTitle}
      </h2>
      <div className={styles.addRow}>
        <input
          type="text"
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          className={styles.addInput}
          value={text}
          placeholder={t.sites.addSitePlaceholder}
          aria-labelledby={headingId}
          aria-describedby={noteId}
          onChange={(e) => {
            setText(e.target.value);
            setError(null);
          }}
        />
        <button type="submit" className={styles.secondary} disabled={pending || text.trim() === ""}>
          {t.sites.addSiteButton}
        </button>
      </div>
      <p id={noteId} className={settingsStyles.note}>
        {t.sites.addSiteNote}
      </p>
      {error && (
        <p role="alert" className={styles.addError}>
          {error}
        </p>
      )}
    </form>
  );
}

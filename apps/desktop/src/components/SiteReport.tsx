// What the app found out about the URL just added in Settings > Sites: a box inside the new
// site's row, under its header and above its accounts line. Check = it works, X = it does not,
// Minus = nothing is needed. The buttons are SitesSection's own. The report is read without a
// login (see site_probe), so "needs a login" is what the site's page says to a stranger.
import { Check, Minus, X, type LucideIcon } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { SiteProbeReport } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import type { Translations } from "../i18n/types";
import { registerFailureText } from "../lib/sources";
import { errorMessage } from "../lib/url";
import { useJobs } from "../state/JobsContext";
import { useNavigation } from "../state/NavigationContext";
import { useSettings } from "../state/SettingsContext";
import { useSites, type SiteReportEntry } from "../state/SitesContext";
import { useSources } from "../state/SourcesContext";
import stateStyles from "./JobStateLine.module.css";
import sitesStyles from "./SitesSection.module.css";
import styles from "./SiteReport.module.css";

type Mark = "ok" | "no" | "na";

const MARKS: Record<Mark, { icon: LucideIcon; className: string }> = {
  ok: { icon: Check, className: styles.ok },
  no: { icon: X, className: styles.no },
  na: { icon: Minus, className: styles.na },
};

interface Line {
  mark: Mark;
  text: string;
}

/** The login line: needed (and used) when the run was made while logged in, else not needed. */
function loginLine(t: Translations, loggedIn: boolean): Line {
  return loggedIn
    ? { mark: "ok", text: t.siteReport.loginUsed }
    : { mark: "na", text: t.siteReport.noLoginNeeded };
}

/** The marked lines of a finished report. */
function reportLines(t: Translations, report: SiteProbeReport): Line[] {
  const r = t.siteReport;
  switch (report.kind) {
    case "list":
      return [
        { mark: "ok", text: r.list(report.count) },
        loginLine(t, report.loggedIn),
        report.videos === "supported"
          ? { mark: "ok", text: r.videosSupported }
          : { mark: "na", text: r.videosUnknown },
      ];
    case "video":
      return [{ mark: "ok", text: r.video }, loginLine(t, report.loggedIn)];
    case "loginRequired":
      return [
        { mark: "no", text: r.loginRequired },
        { mark: "ok", text: r.loginThenRetry },
      ];
    case "unsupported":
      return [
        { mark: "no", text: r.unsupported },
        { mark: "ok", text: r.unsupportedHint },
      ];
    case "failed":
      return [{ mark: "no", text: r.failed(t.errors[report.category]) }];
  }
}

interface SiteReportProps {
  siteId: number;
  entry: SiteReportEntry;
  /** Starts the login of the site's selected account; null when it has no in-app account to log in to. */
  onLogin: (() => void) | null;
}

export function SiteReport({ siteId, entry, onLogin }: SiteReportProps) {
  const t = useT();
  const headingId = useId();
  const boxRef = useRef<HTMLElement>(null);
  const { probeSite } = useSites();
  const probing = entry.status === "probing";

  // The new site is at the end of a long list: show its row when a check starts here, and again
  // when the result (taller than the progress bar) arrives. Coming back to Settings to a finished
  // report leaves the scroll position alone.
  const checkedHere = useRef(false);
  useEffect(() => {
    if (probing) checkedHere.current = true;
    else if (!checkedHere.current) return;
    boxRef.current?.closest("li")?.scrollIntoView({ block: "nearest" });
  }, [probing]);

  /**
   * The "check again" button leaves with the old report: keep the keyboard focus in the box.
   * After a login wall the check is made with the site's login; after a failure it is the
   * same check again.
   */
  function recheck(withLogin: boolean) {
    boxRef.current?.focus();
    void probeSite(siteId, entry.url, withLogin);
  }

  return (
    <section ref={boxRef} tabIndex={-1} className={styles.box} aria-labelledby={headingId} aria-busy={probing}>
      <h3 id={headingId} className={styles.heading}>
        {t.siteReport.heading}
      </h3>
      <div className={styles.url}>{entry.url}</div>

      {/* One live region for both states, so the result is announced when it replaces the bar. */}
      <div role="status">
        {entry.status === "probing" ? (
          <div className={`${stateStyles.prog} ${styles.probing}`}>
            <div className={`${stateStyles.bar} ${stateStyles.indeterminate}`}>
              <span />
            </div>
            <div className={stateStyles.pstat}>
              <span>{t.siteReport.probing}</span>
            </div>
          </div>
        ) : (
          <ul className={styles.lines}>
            {reportLines(t, entry.report).map((line) => {
              const { icon: Icon, className } = MARKS[line.mark];
              return (
                <li key={line.text} className={styles.line}>
                  <Icon size={16} aria-hidden className={`${styles.mark} ${className}`} />
                  <span>{line.text}</span>
                </li>
              );
            })}
          </ul>
        )}
      </div>
      {entry.status === "done" && (
        <ReportActions url={entry.url} report={entry.report} onLogin={onLogin} onRecheck={recheck} />
      )}
    </section>
  );
}

function ReportActions({
  url,
  report,
  onLogin,
  onRecheck,
}: {
  url: string;
  report: SiteProbeReport;
  onLogin: (() => void) | null;
  onRecheck: (withLogin: boolean) => void;
}) {
  const t = useT();
  const r = t.siteReport;
  const { openInBrowser } = useNavigation();
  const { settings } = useSettings();
  const { enqueue } = useJobs();
  const { sources, phase, register } = useSources();
  const [registeredHere, setRegisteredHere] = useState(false);
  const [queued, setQueued] = useState(false);
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const alreadySource = sources.some((source) => source.kind === "list" && source.url === url);

  async function registerSource() {
    if (pending) return;
    setPending(true);
    setProblem(null);
    try {
      const outcome = await register(url);
      if (outcome.kind === "registered") setRegisteredHere(true);
      setProblem(registerFailureText(t, outcome));
    } finally {
      setPending(false);
    }
  }

  async function addToDownloads() {
    if (pending || !settings) return;
    setPending(true);
    setProblem(null);
    try {
      await enqueue([url], settings.defaultPreset);
      setQueued(true);
    } catch (error) {
      setProblem(t.sites.failed(errorMessage(error)));
    } finally {
      setPending(false);
    }
  }

  const openButton = (
    <button type="button" className={sitesStyles.link} onClick={() => void openInBrowser(url)}>
      {r.openInBrowser}
    </button>
  );

  let buttons;
  switch (report.kind) {
    case "list":
      buttons = (
        <>
          <button
            type="button"
            className={sitesStyles.secondary}
            disabled={alreadySource || pending || phase !== "idle"}
            onClick={() => void registerSource()}
          >
            {alreadySource ? (registeredHere ? r.registeredNow : r.registeredAlready) : r.registerSource}
          </button>
          {openButton}
        </>
      );
      break;
    case "video":
      buttons = (
        <>
          <button
            type="button"
            className={sitesStyles.secondary}
            disabled={queued || pending || !settings}
            onClick={() => void addToDownloads()}
          >
            {queued ? r.addedToDownloads : r.addToDownloads}
          </button>
          {openButton}
        </>
      );
      break;
    case "loginRequired":
      buttons = (
        <>
          {onLogin && (
            <button type="button" className={sitesStyles.secondary} onClick={onLogin}>
              {t.common.login}
            </button>
          )}
          <button type="button" className={sitesStyles.link} onClick={() => onRecheck(true)}>
            {r.recheck}
          </button>
        </>
      );
      break;
    case "unsupported":
      buttons = (
        <button
          type="button"
          className={sitesStyles.secondary}
          onClick={() => void openInBrowser(url, { collect: true })}
        >
          {r.openAndCollect}
        </button>
      );
      break;
    case "failed":
      buttons = (
        <button
          type="button"
          className={`${sitesStyles.link} ${styles.loneLink}`}
          onClick={() => onRecheck(false)}
        >
          {r.recheck}
        </button>
      );
      break;
  }

  return (
    <>
      <div className={styles.actions}>{buttons}</div>
      {problem && (
        <p role="alert" className={styles.error}>
          {problem}
        </p>
      )}
    </>
  );
}

import { Check } from "lucide-react";
import { useEffect, useId, type ReactNode } from "react";
import type { Language, Preset, SourceAutoCheck, Theme, ToolProgress, ToolStatus } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import type { Translations } from "../i18n/types";
import { useSettings } from "../state/SettingsContext";
import { useTools } from "../state/ToolsContext";
import { IS_MAC } from "../lib/platform";
import { AD_BLOCK_AVAILABLE, AdBlockSection } from "./AdBlockSection";
import { NotifyGroup } from "./NotifyGroup";
import { SitesSection } from "./SitesSection";
import { Switch } from "./Switch";
import { UpdatesSection } from "./UpdatesSection";
import styles from "./SettingsView.module.css";

export type SettingsSection = "general" | "sites" | "adblock" | "tools" | "updates";

interface SettingsViewProps {
  section: SettingsSection;
  onChangeSection: (section: SettingsSection) => void;
  /** Opens the in-app browser on an account's login page. */
  onLogin: (siteId: number, accountId: number) => void;
}

const PRESETS: Preset[] = ["best", "upTo1080", "upTo720", "compatibilityH264", "audioM4a", "audioMp3"];
const CONCURRENCIES = [1, 2, 3, 4, 5];
const AUTO_CHECKS: SourceAutoCheck[] = ["off", "hourly", "sixHours", "daily"];

/**
 * Settings, a destination in the main window (sidebar, Ctrl+,/Cmd+,): a page title,
 * a section nav (a row above the content below 900px) and the section itself, which
 * scrolls inside the main area. Ad blocking is Windows-only for now, so its section is
 * neither listed nor shown elsewhere.
 */
export function SettingsView({ section, onChangeSection, onLogin }: SettingsViewProps) {
  const t = useT();
  const titleId = useId();

  const items: Array<{ id: SettingsSection; label: string }> = [
    { id: "general", label: t.settings.generalSection },
    { id: "sites", label: t.settings.sitesSection },
    ...(AD_BLOCK_AVAILABLE ? [{ id: "adblock" as const, label: t.settings.adBlockSection }] : []),
    { id: "tools", label: t.settings.toolsSection },
    { id: "updates", label: t.settings.updatesSection },
  ];
  const current = items.some((item) => item.id === section) ? section : "general";

  return (
    <section className={styles.page} aria-labelledby={titleId}>
      <div className={styles.head}>
        <h1 id={titleId}>{t.settings.title}</h1>
      </div>

      <div className={styles.cols}>
        <nav className={styles.nav} aria-label={t.settings.title}>
          {items.map((item) => (
            <button
              key={item.id}
              type="button"
              className={styles.sectionItem}
              aria-current={current === item.id ? "page" : undefined}
              onClick={() => onChangeSection(item.id)}
            >
              {item.label}
            </button>
          ))}
        </nav>

        <div className={styles.scroll} key={current}>
          <div className={styles.content}>
            {current === "general" ? (
              <GeneralSection />
            ) : current === "sites" ? (
              <SitesSection onLogin={onLogin} />
            ) : current === "adblock" ? (
              <AdBlockSection />
            ) : current === "updates" ? (
              <UpdatesSection />
            ) : (
              <ToolsSection />
            )}
          </div>
        </div>
      </div>
    </section>
  );
}

function Field({
  label,
  hint,
  hintId,
  children,
}: {
  label: string;
  hint?: string;
  hintId?: string;
  children: ReactNode;
}) {
  return (
    <div className={styles.field}>
      <div className={styles.fieldLabel}>{label}</div>
      {children}
      {hint && (
        <p id={hintId} className={styles.note}>
          {hint}
        </p>
      )}
    </div>
  );
}

function ThemeControl({ value, onChange }: { value: Theme; onChange: (theme: Theme) => void }) {
  const t = useT();
  const options: Array<{ value: Theme; label: string }> = [
    { value: "system", label: t.settings.themeSystem },
    { value: "light", label: t.settings.themeLight },
    { value: "dark", label: t.settings.themeDark },
  ];
  return (
    <div className={styles.segmented} role="radiogroup" aria-label={t.settings.theme}>
      {options.map((opt) => (
        <label key={opt.value} className={styles.segment}>
          <input
            type="radio"
            name="theme"
            className={styles.segmentInput}
            checked={value === opt.value}
            onChange={() => onChange(opt.value)}
          />
          <span>{opt.label}</span>
        </label>
      ))}
    </div>
  );
}

function autoCheckLabel(t: Translations, value: SourceAutoCheck): string {
  switch (value) {
    case "hourly":
      return t.sources.autoCheckHourly;
    case "sixHours":
      return t.sources.autoCheckSixHours;
    case "daily":
      return t.sources.autoCheckDaily;
    default:
      return t.sources.autoCheckOff;
  }
}

function GeneralSection() {
  const t = useT();
  const avoidAv1HintId = useId();
  const autoCheckHintId = useId();
  const { settings, updateSettings, pickDownloadDir } = useSettings();
  if (!settings) return null;

  return (
    <div className={styles.sectionBody}>
      <Field label={t.settings.language}>
        <select
          className={styles.select}
          value={settings.language}
          onChange={(e) => updateSettings({ language: e.target.value as Language })}
        >
          <option value="ja">{t.settings.languageJa}</option>
          <option value="en">{t.settings.languageEn}</option>
        </select>
      </Field>

      <Field label={t.settings.theme}>
        <ThemeControl value={settings.theme} onChange={(theme) => updateSettings({ theme })} />
      </Field>

      <Field label={t.settings.concurrency}>
        <select
          className={styles.select}
          value={settings.concurrency}
          onChange={(e) => updateSettings({ concurrency: Number(e.target.value) })}
        >
          {CONCURRENCIES.map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
      </Field>

      <Field label={t.sources.autoCheck} hint={t.sources.autoCheckNote} hintId={autoCheckHintId}>
        <select
          className={styles.select}
          value={settings.sourceAutoCheck}
          aria-label={t.sources.autoCheck}
          aria-describedby={autoCheckHintId}
          onChange={(e) => updateSettings({ sourceAutoCheck: e.target.value as SourceAutoCheck })}
        >
          {AUTO_CHECKS.map((value) => (
            <option key={value} value={value}>
              {autoCheckLabel(t, value)}
            </option>
          ))}
        </select>
      </Field>

      <Field label={t.settings.clipboardBanner}>
        <Switch
          checked={settings.clipboardBanner}
          onChange={(v) => updateSettings({ clipboardBanner: v })}
          label={t.settings.clipboardBanner}
        />
      </Field>

      <Field label={t.settings.collectIncludeAds}>
        <Switch
          checked={settings.collectIncludeAds}
          onChange={(v) => updateSettings({ collectIncludeAds: v })}
          label={t.settings.collectIncludeAds}
        />
      </Field>

      <Field label={t.settings.downloadFolder}>
        <div className={styles.folderControl}>
          <div className={`${styles.readonlyBox} ${styles.folderPathBox}`}>{settings.downloadDir}</div>
          <button type="button" className={styles.secondaryButton} onClick={() => void pickDownloadDir()}>
            {t.settings.changeDownloadFolder}
          </button>
        </div>
      </Field>

      <Field label={t.settings.keepSubtitleFiles}>
        <Switch
          checked={settings.keepSubtitleFiles}
          onChange={(v) => updateSettings({ keepSubtitleFiles: v })}
          label={t.settings.keepSubtitleFiles}
        />
      </Field>

      <Field label={t.settings.defaultPreset}>
        <select
          className={styles.select}
          value={settings.defaultPreset}
          onChange={(e) => updateSettings({ defaultPreset: e.target.value as Preset })}
        >
          {PRESETS.map((p) => (
            <option key={p} value={p}>
              {t.presets[p]}
            </option>
          ))}
        </select>
      </Field>

      <Field label={t.settings.avoidAv1} hint={t.settings.avoidAv1Hint} hintId={avoidAv1HintId}>
        <Switch
          checked={settings.avoidAv1}
          onChange={(v) => updateSettings({ avoidAv1: v })}
          label={t.settings.avoidAv1}
          describedBy={avoidAv1HintId}
        />
      </Field>

      {/* The backend shows notifications on Windows only for now. */}
      {!IS_MAC && <NotifyGroup />}
    </div>
  );
}

function toolStatusText(
  t: Translations,
  status: ToolStatus,
  progress: ToolProgress | undefined,
): { text: string; isError: boolean } {
  if (progress && (progress.phase === "downloading" || progress.phase === "installing")) {
    if (progress.phase === "downloading") {
      const percent = progress.totalBytes
        ? Math.round((progress.downloadedBytes / progress.totalBytes) * 100)
        : null;
      return { text: t.tools.downloading(percent), isError: false };
    }
    return { text: t.tools.installing, isError: false };
  }
  if (progress?.phase === "failed") {
    return { text: t.tools.installFailed, isError: true };
  }
  if (status.installed) {
    if (status.version !== status.expectedVersion) {
      return { text: t.tools.updateAvailable(status.expectedVersion), isError: false };
    }
    return { text: t.tools.installed(status.version ?? "?"), isError: false };
  }
  return { text: t.tools.notInstalled, isError: false };
}

function ToolsSection() {
  const t = useT();
  const { status, progress, installing, install, refresh } = useTools();
  useEffect(() => {
    refresh();
  }, [refresh]);
  const hasMissing = status.some((s) => !s.installed);
  const hasOutdated = status.some((s) => s.installed && s.version !== s.expectedVersion);
  const allUpToDate = status.length > 0 && !hasMissing && !hasOutdated;

  return (
    <div className={styles.sectionBody}>
      <ul className={styles.toolsList}>
        {status.map((s) => {
          const { text, isError } = toolStatusText(t, s, progress[s.tool]);
          return (
            <li key={s.tool} className={styles.toolRow}>
              <span className={styles.toolName}>{t.tools.names[s.tool]}</span>
              <span className={isError ? styles.toolError : styles.toolStatus}>{text}</span>
            </li>
          );
        })}
      </ul>

      {allUpToDate ? (
        <div className={styles.allInstalled}>
          <Check size={16} aria-hidden />
          <span>{t.tools.allInstalled}</span>
        </div>
      ) : (
        <button
          type="button"
          className={`${styles.secondaryButton} ${styles.installButton}`}
          onClick={() => void install()}
          disabled={installing}
        >
          {!hasMissing && hasOutdated ? t.tools.updateButton : t.tools.installButton}
        </button>
      )}
    </div>
  );
}

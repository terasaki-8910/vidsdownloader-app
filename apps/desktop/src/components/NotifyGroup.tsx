import { useId } from "react";
import type { NotifySettings } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { useSettings } from "../state/SettingsContext";
import { Switch } from "./Switch";
import settingsStyles from "./SettingsView.module.css";
import styles from "./NotifyGroup.module.css";

type NotifyKind = Exclude<keyof NotifySettings, "enabled">;

const KINDS: NotifyKind[] = ["done", "failed", "found"];

interface SwitchRow {
  id: string;
  label: string;
  checked: boolean;
  disabled: boolean;
  onChange: (value: boolean) => void;
}

/**
 * Settings > General > Notifications: one switch that turns every notification on or off and
 * one per kind. With the first off the three kinds are disabled, not hidden, and keep their own
 * values for when it is turned back on.
 */
export function NotifyGroup() {
  const t = useT();
  const { settings, updateSettings } = useSettings();
  const headingId = useId();
  const noteId = useId();
  const labelPrefix = useId();
  if (!settings) return null;

  const { notify } = settings;
  const change = (patch: Partial<NotifySettings>) => updateSettings({ notify: { ...notify, ...patch } });

  const rows: SwitchRow[] = [
    {
      id: "enabled",
      label: t.notify.enabled,
      checked: notify.enabled,
      disabled: false,
      onChange: (enabled) => change({ enabled }),
    },
    ...KINDS.map((kind) => ({
      id: kind,
      label: t.notify[kind],
      checked: notify[kind],
      disabled: !notify.enabled,
      onChange: (value: boolean) => change({ [kind]: value }),
    })),
  ];

  return (
    <section className={styles.group} aria-labelledby={headingId}>
      <h2 id={headingId} className={styles.heading}>
        {t.notify.heading}
      </h2>
      <ul className={styles.rows}>
        {rows.map((row) => (
          <li key={row.id} className={styles.row}>
            <span
              id={`${labelPrefix}-${row.id}`}
              className={`${styles.label} ${row.disabled ? styles.labelOff : ""}`}
            >
              {row.label}
            </span>
            <Switch
              checked={row.checked}
              onChange={row.onChange}
              label={row.label}
              labelledBy={`${labelPrefix}-${row.id}`}
              describedBy={noteId}
              disabled={row.disabled}
            />
          </li>
        ))}
      </ul>
      <p id={noteId} className={settingsStyles.note}>
        {t.notify.note}
      </p>
    </section>
  );
}

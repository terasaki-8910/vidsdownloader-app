import { useT } from "../i18n/I18nContext";
import settingsStyles from "./SettingsView.module.css";
import styles from "./ImportProfile.module.css";

interface ImportProfileProps {
  profiles: string[];
  value: string;
  onChange: (profile: string) => void;
  disabled?: boolean;
}

/**
 * Which Firefox profile to import: a select, or just its name as text when Firefox has
 * only one (a select with a single choice asks nothing).
 */
export function ImportProfile({ profiles, value, onChange, disabled = false }: ImportProfileProps) {
  const t = useT();
  if (profiles.length <= 1) {
    return <span className={styles.single}>{t.importLogins.profileOption(profiles[0] ?? value)}</span>;
  }
  return (
    <select
      className={settingsStyles.select}
      aria-label={t.importLogins.profileLabel}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value)}
    >
      {profiles.map((profile) => (
        <option key={profile} value={profile}>
          {t.importLogins.profileOption(profile)}
        </option>
      ))}
    </select>
  );
}

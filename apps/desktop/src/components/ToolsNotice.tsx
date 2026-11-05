import { AlertTriangle } from "lucide-react";
import { useT } from "../i18n/I18nContext";
import styles from "./ToolsNotice.module.css";

interface ToolsNoticeProps {
  onInstall: () => void;
}

export function ToolsNotice({ onInstall }: ToolsNoticeProps) {
  const t = useT();
  return (
    <div className={styles.notice} role="status">
      <AlertTriangle size={16} aria-hidden className={styles.icon} />
      <span className={styles.message}>{t.tools.missingNotice}</span>
      <button type="button" className={styles.action} onClick={onInstall}>
        {t.tools.installAction}
      </button>
    </div>
  );
}

import { Clipboard, X } from "lucide-react";
import { useT } from "../i18n/I18nContext";
import { IconButton } from "./IconButton";
import styles from "./ClipboardBanner.module.css";

interface ClipboardBannerProps {
  url: string;
  onAdd: () => void;
  onDismiss: () => void;
}

export function ClipboardBanner({ url, onAdd, onDismiss }: ClipboardBannerProps) {
  const t = useT();
  return (
    <div className={styles.banner} role="status">
      <Clipboard size={16} aria-hidden className={styles.icon} />
      <span className={styles.message}>{t.clipboard.message}</span>
      <code className={styles.url}>{url}</code>
      <button type="button" className={styles.addButton} onClick={onAdd}>
        {t.clipboard.add}
      </button>
      <IconButton label={t.clipboard.dismiss} size="sm" onClick={onDismiss}>
        <X size={16} aria-hidden />
      </IconButton>
    </div>
  );
}

import { X } from "lucide-react";
import type { JobRow, Preset } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import type { RowActionHandlers } from "../lib/rowAction";
import { DetailBody } from "./DetailBody";
import { IconButton } from "./IconButton";
import styles from "./DetailPanel.module.css";

interface DetailPanelProps extends RowActionHandlers {
  job: JobRow | null;
  preset: Preset | undefined;
  open: boolean;
  onClose: () => void;
}

/**
 * The >=1320px persistent detail panel (layout B). Width animates between 0
 * and 360px so it reads as sliding in/out, not popping; see useBreakpointPanel
 * in App.tsx for the "only on crossing, and closing by hand sticks until the
 * next crossing" logic (SPEC UI2).
 */
export function DetailPanel({
  job,
  preset,
  open,
  onClose,
  onCancel,
  onRetry,
  onReveal,
  onLogin,
  onSaveAgain,
  onChooseMedia,
}: DetailPanelProps) {
  const t = useT();
  const showContent = open && job;

  return (
    <aside className={`${styles.detail} ${open ? styles.open : ""}`} aria-hidden={!showContent}>
      {showContent && job && (
        <>
          <div className={styles.header}>
            <IconButton label={t.common.close} onClick={onClose}>
              <X size={16} aria-hidden />
            </IconButton>
          </div>
          <div className={styles.content}>
            <DetailBody
              job={job}
              preset={preset}
              onCancel={onCancel}
              onRetry={onRetry}
              onReveal={onReveal}
              onLogin={onLogin}
              onSaveAgain={onSaveAgain}
              onChooseMedia={onChooseMedia}
            />
          </div>
        </>
      )}
    </aside>
  );
}

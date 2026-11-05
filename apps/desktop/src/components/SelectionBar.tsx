import { X } from "lucide-react";
import { IconButton } from "./IconButton";
import queueStyles from "./QueueRow.module.css";
import styles from "./SelectionBar.module.css";

export interface SelectionBarAction {
  label: string;
  danger?: boolean;
  onClick: () => void;
}

export function SelectionBar({
  summary,
  clearLabel,
  actions,
  onClear,
}: {
  summary: string;
  clearLabel: string;
  actions: SelectionBarAction[];
  onClear: () => void;
}) {
  return (
    <div className={styles.bar}>
      <span className={styles.summary}>{summary}</span>
      {actions.map((action) => (
        <button
          key={action.label}
          type="button"
          className={`${queueStyles.textAction} ${action.danger ? styles.danger : ""}`}
          onClick={action.onClick}
        >
          {action.label}
        </button>
      ))}
      <IconButton label={clearLabel} className={styles.clear} onClick={onClear}>
        <X size={16} aria-hidden />
      </IconButton>
    </div>
  );
}

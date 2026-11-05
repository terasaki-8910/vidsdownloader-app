import type { ReactNode } from "react";
import styles from "./EmptyState.module.css";

interface EmptyStateProps {
  icon: ReactNode;
  title?: string;
  body: string;
}

/** Text (and a decorative icon) only -- no controls with nothing to act on. */
export function EmptyState({ icon, title, body }: EmptyStateProps) {
  return (
    <div className={styles.empty}>
      <div className={styles.icon} aria-hidden>
        {icon}
      </div>
      {title && <div className={styles.title}>{title}</div>}
      <div className={styles.body}>{body}</div>
    </div>
  );
}

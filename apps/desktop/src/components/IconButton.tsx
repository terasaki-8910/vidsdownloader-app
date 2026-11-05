import type { MouseEventHandler, ReactNode } from "react";
import styles from "./IconButton.module.css";

interface IconButtonProps {
  /** Used as both the accessible name and the native hover tooltip -- every icon button needs one. */
  label: string;
  onClick?: MouseEventHandler<HTMLButtonElement>;
  disabled?: boolean;
  children: ReactNode;
  className?: string;
  size?: "sm" | "md";
  tabIndex?: number;
}

export function IconButton({
  label,
  onClick,
  disabled,
  children,
  className,
  size = "md",
  tabIndex,
}: IconButtonProps) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      disabled={disabled}
      tabIndex={tabIndex}
      className={`${styles.button} ${size === "sm" ? styles.sm : ""} ${className ?? ""}`}
    >
      {children}
    </button>
  );
}

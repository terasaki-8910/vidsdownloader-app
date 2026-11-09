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
  /** A toggle button's state (aria-pressed); the pressed button is tinted with the accent. */
  pressed?: boolean;
}

export function IconButton({
  label,
  onClick,
  disabled,
  children,
  className,
  size = "md",
  tabIndex,
  pressed,
}: IconButtonProps) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      onClick={onClick}
      disabled={disabled}
      tabIndex={tabIndex}
      className={`${styles.button} ${size === "sm" ? styles.sm : ""} ${className ?? ""}`}
    >
      {children}
    </button>
  );
}

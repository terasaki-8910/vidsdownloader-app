import type { MouseEventHandler, ReactNode, Ref } from "react";
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
  /** For a button that opens a popover: its open state and kind. */
  expanded?: boolean;
  popup?: "dialog" | "menu";
  ref?: Ref<HTMLButtonElement>;
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
  expanded,
  popup,
  ref,
}: IconButtonProps) {
  return (
    <button
      ref={ref}
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      aria-expanded={expanded}
      aria-haspopup={popup}
      onClick={onClick}
      disabled={disabled}
      tabIndex={tabIndex}
      className={`${styles.button} ${size === "sm" ? styles.sm : ""} ${className ?? ""}`}
    >
      {children}
    </button>
  );
}

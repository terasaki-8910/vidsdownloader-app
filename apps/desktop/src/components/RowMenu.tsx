// A "..." icon button that opens a short action menu (the saved items in the library).
// The same WAI-ARIA menu button as FolderMenu (same markup, same styles, same keys):
// Enter / Space / ArrowDown open it on the first item, ArrowUp opens it on the last,
// ArrowUp / ArrowDown / Home / End move, Escape closes and returns to the button, Tab and
// an outside click close.
// The button looks like IconButton (it shares its CSS module); IconButton itself takes no
// aria-haspopup / aria-expanded, so the <button> is written out here.
// The popover is positioned against the window (position: fixed) so a scrolling list or a
// short one never clips it; it opens upward near the bottom of the window and closes when
// the page scrolls or resizes.
import { Ellipsis } from "lucide-react";
import {
  useEffect,
  useId,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type Ref,
  type RefObject,
} from "react";
import folderStyles from "./FolderMenu.module.css";
import iconStyles from "./IconButton.module.css";
import styles from "./RowMenu.module.css";

export type MenuEntry =
  | { kind: "item"; label: string; hint?: string; disabled?: boolean; danger?: boolean; onSelect: () => void }
  | { kind: "separator" };

/** What the owner can ask of the menu button. */
export interface RowMenuHandle {
  focus: () => void;
  open: () => void;
  openAt: (x: number, y: number) => void;
}

interface RowMenuProps {
  /** Accessible name and tooltip of the "..." button. */
  label: string;
  entries: MenuEntry[];
  disabled?: boolean;
  onBeforeOpen?: () => void;
  /** Restores focus to the owning row when the row, rather than this button, opened the menu. */
  onReturnFocus?: () => void;
  /** An external control to position against when this menu has no visible trigger. */
  anchorRef?: RefObject<HTMLElement | null>;
  /** Omits the ellipsis button for menus opened only through an owning row. */
  hideButton?: boolean;
  /** Source rows keep this out of the tab order; destinations that expose every action can opt in. */
  buttonTabIndex?: 0 | -1;
  ref?: Ref<RowMenuHandle>;
}

/** Gap between the button and the popover, and the room kept to the window's edge. */
const GAP = 6;
const EDGE = 12;

export function RowMenu({
  label,
  entries,
  disabled = false,
  onBeforeOpen,
  onReturnFocus,
  anchorRef,
  hideButton = false,
  buttonTabIndex = -1,
  ref,
}: RowMenuProps) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<CSSProperties | undefined>(undefined);
  const wrapRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const buttonId = useId();
  const menuId = useId();
  const focusTarget = useRef<"first" | "last">("first");
  const pointer = useRef<{ x: number; y: number } | null>(null);
  const openedFrom = useRef<"button" | "row">("button");

  useImperativeHandle(
    ref,
    () => ({
      focus: () => buttonRef.current?.focus(),
      open: () => {
        onBeforeOpen?.();
        openedFrom.current = "row";
        pointer.current = null;
        focusTarget.current = "first";
        setOpen(true);
      },
      openAt: (x, y) => {
        onBeforeOpen?.();
        openedFrom.current = "row";
        pointer.current = { x, y };
        focusTarget.current = "first";
        setOpen(true);
      },
    }),
    [onBeforeOpen, onReturnFocus],
  );

  function items(): HTMLButtonElement[] {
    return Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("[role='menuitem']:not(:disabled)") ?? []);
  }

  function close(returnFocus: boolean) {
    setOpen(false);
    if (!returnFocus) return;
    if (openedFrom.current === "row") onReturnFocus?.();
    else buttonRef.current?.focus({ preventScroll: true });
  }

  // Under the button, right edges aligned; above it when the window has no room below.
  useLayoutEffect(() => {
    if (!open) {
      setPosition(undefined);
      return;
    }
    const popover = popoverRef.current;
    if (!popover) return;
    if (pointer.current) {
      const { x, y } = pointer.current;
      const width = popover.offsetWidth;
      const height = popover.offsetHeight;
      const left = Math.max(EDGE, Math.min(x + width + EDGE > window.innerWidth ? x - width : x, window.innerWidth - width - EDGE));
      const top = Math.max(EDGE, Math.min(y + height + EDGE > window.innerHeight ? y - height : y, window.innerHeight - height - EDGE));
      setPosition({ left, top });
      return;
    }
    const anchor = anchorRef?.current ?? buttonRef.current;
    if (!anchor) return;
    const rect = anchor.getBoundingClientRect();
    const need = popover.offsetHeight + EDGE;
    const flip = window.innerHeight - rect.bottom < need && rect.top > need;
    const right = document.documentElement.clientWidth - rect.right;
    setPosition(flip ? { right, bottom: window.innerHeight - rect.top + GAP } : { right, top: rect.bottom + GAP });
  }, [open]);

  useEffect(() => {
    if (!open) return undefined;
    const list = items();
    const target = focusTarget.current === "last" ? list[list.length - 1] : list[0];
    target?.focus({ preventScroll: true });
    focusTarget.current = "first";

    function onPointerDown(e: MouseEvent) {
      if (!wrapRef.current?.contains(e.target as Node)) {
        e.preventDefault();
        close(true);
      }
    }
    function onMoved(e: Event) {
      // The popover is fixed, so a scroll or resize would leave it away from its button.
      if (e.target instanceof Node && menuRef.current?.contains(e.target)) return;
      close(true);
    }
    document.addEventListener("mousedown", onPointerDown);
    window.addEventListener("scroll", onMoved, true);
    window.addEventListener("resize", onMoved);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      window.removeEventListener("scroll", onMoved, true);
      window.removeEventListener("resize", onMoved);
    };
  }, [open]);

  function onButtonKeyDown(e: KeyboardEvent<HTMLButtonElement>) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      onBeforeOpen?.();
      openedFrom.current = "button";
      focusTarget.current = e.key === "ArrowUp" ? "last" : "first";
      setOpen(true);
    }
  }

  function onMenuKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const list = items();
    const index = list.indexOf(document.activeElement as HTMLButtonElement);
    let next: number | null = null;
    if (e.key === "ArrowDown") next = (index + 1) % list.length;
    else if (e.key === "ArrowUp") next = (index - 1 + list.length) % list.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = list.length - 1;
    else if (e.key === "Escape") {
      e.preventDefault();
      close(true);
      return;
    } else if (e.key === "Tab") {
      e.preventDefault();
      close(true);
      return;
    }
    if (next !== null) {
      e.preventDefault();
      list[next]?.focus();
    }
  }

  return (
    <div className={styles.wrap} ref={wrapRef}>
      {!hideButton && (
        <button
          ref={buttonRef}
          id={buttonId}
          type="button"
          className={iconStyles.button}
          aria-label={label}
          title={label}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-controls={open ? menuId : undefined}
          disabled={disabled}
          tabIndex={buttonTabIndex}
          onClick={() => {
            onBeforeOpen?.();
            openedFrom.current = "button";
            pointer.current = null;
            setOpen((v) => !v);
          }}
          onKeyDown={onButtonKeyDown}
        >
          <Ellipsis size={16} aria-hidden />
        </button>
      )}

      {open && (
        <div ref={popoverRef} className={`${folderStyles.popover} ${styles.popover}`} style={position}>
          <div
            id={menuId}
            role="menu"
            aria-label={hideButton ? label : undefined}
            aria-labelledby={hideButton ? undefined : buttonId}
            ref={menuRef}
            className={styles.menu}
            onKeyDown={onMenuKeyDown}
          >
            {entries.map((entry, index) =>
              entry.kind === "separator" ? (
                <div key={`separator-${index}`} role="separator" className={folderStyles.separator} />
              ) : (
                <button
                  key={entry.label}
                  type="button"
                  role="menuitem"
                  tabIndex={-1}
                  disabled={entry.disabled}
                  className={`${folderStyles.item} ${entry.danger ? styles.danger : ""}`}
                  onClick={() => {
                    close(true);
                    entry.onSelect();
                  }}
                >
                  <span className={folderStyles.path}>{entry.label}</span>
                  {entry.hint && <span className={styles.hint}>{entry.hint}</span>}
                </button>
              ),
            )}
          </div>
        </div>
      )}
    </div>
  );
}

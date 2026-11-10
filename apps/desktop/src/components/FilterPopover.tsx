// The tray's "filter and sort" popover. A non-modal dialog under the head's filter button, as wide
// as the tray's content. Escape and a press outside close it and give focus back to the button;
// every change applies at once (the list and counts follow). A press in the page itself is a press
// in a native view of its own, which this document never sees: the page leaves the popover open.
import { useEffect, useId, useRef, useState, type RefObject } from "react";
import { useT } from "../i18n/I18nContext";
import {
  NO_FILTER,
  SORT_KEYS,
  filterActive,
  sortLabel,
  type SortKey,
  type TrayFilter,
  type UploaderCount,
} from "../lib/collectFilter";
import styles from "./FilterPopover.module.css";

/** How many uploaders show before the "N more" button. */
const UPLOADERS_SHOWN = 5;

interface FilterPopoverProps {
  filter: TrayFilter;
  onChange: (next: TrayFilter) => void;
  uploaders: UploaderCount[];
  total: number;
  shown: number;
  /** The head's filter button: a press on it is its own toggle, and focus goes back to it. */
  buttonRef: RefObject<HTMLButtonElement | null>;
  onClose: () => void;
}

export function FilterPopover({ filter, onChange, uploaders, total, shown, buttonRef, onClose }: FilterPopoverProps) {
  const t = useT();
  const rootRef = useRef<HTMLDivElement>(null);
  const [allUploaders, setAllUploaders] = useState(false);
  const titleId = useId();

  useEffect(() => {
    rootRef.current?.focus({ preventScroll: true });
    function close() {
      onClose();
      buttonRef.current?.focus({ preventScroll: true });
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
    }
    function onPointerDown(event: MouseEvent) {
      const target = event.target as Element;
      if (rootRef.current?.contains(target) || buttonRef.current?.contains(target)) return;
      onClose();
      // A press on something that takes focus keeps it there; a press on plain page area would
      // drop focus to the document, so it goes back to the button once the press has settled.
      if (!target.closest?.("button, input, select, textarea, a[href], [tabindex]")) {
        window.setTimeout(() => buttonRef.current?.focus({ preventScroll: true }), 0);
      }
    }
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("mousedown", onPointerDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("mousedown", onPointerDown);
    };
  }, [onClose, buttonRef]);

  const visible = allUploaders ? uploaders : uploaders.slice(0, UPLOADERS_SHOWN);
  const hiddenUploaders = uploaders.length - UPLOADERS_SHOWN;

  function toggleUploader(name: string, checked: boolean) {
    onChange({
      ...filter,
      uploaders: checked ? [...filter.uploaders, name] : filter.uploaders.filter((value) => value !== name),
    });
  }

  return (
    <div ref={rootRef} className={styles.popover} role="dialog" aria-label={t.browser.filterButton} tabIndex={-1}>
      <div className={styles.field}>
        <label className={styles.label} htmlFor={`${titleId}-sort`}>
          {t.browser.filterSort}
        </label>
        <select
          id={`${titleId}-sort`}
          className={styles.select}
          value={filter.sort}
          onChange={(event) => onChange({ ...filter, sort: event.target.value as SortKey })}
        >
          {SORT_KEYS.map((key) => (
            <option key={key} value={key}>
              {sortLabel(t, key)}
            </option>
          ))}
        </select>
      </div>

      <div className={styles.field} role="group" aria-labelledby={`${titleId}-length`}>
        <span className={styles.label} id={`${titleId}-length`}>
          {t.browser.filterLength}
        </span>
        <div className={styles.lengthRow}>
          <input
            type="number"
            min={0}
            inputMode="numeric"
            className={styles.number}
            value={filter.minMinutes}
            placeholder={t.browser.filterMin}
            aria-label={`${t.browser.filterLength} ${t.browser.filterMin}`}
            onChange={(event) => onChange({ ...filter, minMinutes: event.target.value })}
          />
          <span>{t.browser.filterMinutes}</span>
          <span className={styles.sep} aria-hidden>
            〜
          </span>
          <input
            type="number"
            min={0}
            inputMode="numeric"
            className={styles.number}
            value={filter.maxMinutes}
            placeholder={t.browser.filterMax}
            aria-label={`${t.browser.filterLength} ${t.browser.filterMax}`}
            onChange={(event) => onChange({ ...filter, maxMinutes: event.target.value })}
          />
          <span>{t.browser.filterMinutes}</span>
        </div>
      </div>

      {uploaders.length > 0 && (
        <div className={styles.field} role="group" aria-labelledby={`${titleId}-uploader`}>
          <span className={styles.label} id={`${titleId}-uploader`}>
            {t.browser.filterUploader}
          </span>
          <ul className={styles.uploaders}>
            {visible.map((uploader) => (
              <li key={uploader.name}>
                <label className={styles.uploader}>
                  <input
                    type="checkbox"
                    checked={filter.uploaders.includes(uploader.name)}
                    onChange={(event) => toggleUploader(uploader.name, event.target.checked)}
                  />
                  <span className={styles.uploaderName}>{uploader.name}</span>
                  <span className={styles.uploaderCount}>{uploader.count}</span>
                </label>
              </li>
            ))}
          </ul>
          {hiddenUploaders > 0 && (
            <button type="button" className={styles.textButton} onClick={() => setAllUploaders((value) => !value)}>
              {allUploaders ? t.browser.uploadersLess : t.browser.uploadersMore(hiddenUploaders)}
            </button>
          )}
        </div>
      )}

      <div className={styles.field} role="group" aria-labelledby={`${titleId}-date`}>
        <span className={styles.label} id={`${titleId}-date`}>
          {t.browser.filterDate}
        </span>
        <div className={styles.dateRow}>
          <input
            type="date"
            className={styles.date}
            value={filter.from}
            aria-label={t.browser.filterDateFrom}
            onChange={(event) => onChange({ ...filter, from: event.target.value })}
          />
          <span className={styles.sep} aria-hidden>
            〜
          </span>
          <input
            type="date"
            className={styles.date}
            value={filter.to}
            aria-label={t.browser.filterDateTo}
            onChange={(event) => onChange({ ...filter, to: event.target.value })}
          />
        </div>
      </div>

      <div className={styles.foot}>
        <button
          type="button"
          className={styles.textButton}
          disabled={!filterActive(filter)}
          onClick={() => onChange(NO_FILTER)}
        >
          {t.browser.filterClear}
        </button>
        <span className={styles.count} aria-live="polite">
          {t.browser.filterCount(shown, total)}
        </span>
      </div>
    </div>
  );
}

import { ChevronDown, ChevronRight } from "lucide-react";
import { useId, useState } from "react";
import type { CollectItem, CollectLinks } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import styles from "./PageImagesTray.module.css";

interface PageImagesTrayProps {
  items: CollectItem[];
  /** A missing entry counts as checked, like the tray's own list. */
  selected: Record<string, boolean>;
  onToggle: (url: string, checked: boolean) => void;
  /** The best-ranked link group of the page, listed collapsed below the images; null for none. */
  otherLinks: CollectLinks | null;
}

/**
 * The body of the browser's tray when the chosen group is the page's own images: a 3-column
 * grid of the images, each with a checkbox, and under it the page's other links, collapsed.
 * The tray's header and footer stay the ones of the link list.
 */
export function PageImagesTray({ items, selected, onToggle, otherLinks }: PageImagesTrayProps) {
  const t = useT();
  const [linksOpen, setLinksOpen] = useState(false);
  const imagesId = useId();
  const linksId = useId();

  return (
    <div className={styles.groups}>
      <section aria-labelledby={imagesId}>
        <h3 id={imagesId} className={styles.groupTitle}>
          {t.browser.imagesGroup(items.length)}
        </h3>
        <ul className={styles.grid}>
          {items.map((item, position) => {
            const checked = selected[item.url] !== false;
            return (
              <li key={item.url} className={styles.cell} data-checked={checked}>
                <label className={styles.cellLabel}>
                  <img
                    src={item.thumbnail ?? item.url}
                    alt=""
                    title={item.title || undefined}
                    className={styles.image}
                    draggable={false}
                  />
                  <input
                    type="checkbox"
                    className={styles.check}
                    aria-label={t.browser.imageLabel(item.index ?? position + 1)}
                    checked={checked}
                    onChange={(e) => onToggle(item.url, e.target.checked)}
                  />
                </label>
              </li>
            );
          })}
        </ul>
      </section>

      {otherLinks && otherLinks.urls.length > 0 && (
        <section className={styles.linksGroup}>
          <button
            type="button"
            className={styles.groupToggle}
            aria-expanded={linksOpen}
            aria-controls={linksOpen ? linksId : undefined}
            onClick={() => setLinksOpen((open) => !open)}
          >
            {linksOpen ? <ChevronDown size={16} aria-hidden /> : <ChevronRight size={16} aria-hidden />}
            <span>{t.browser.otherLinks(otherLinks.urls.length)}</span>
          </button>
          {linksOpen && (
            <ul id={linksId} className={styles.links}>
              {otherLinks.urls.map((url) => (
                <li key={url} title={url}>
                  {url}
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
    </div>
  );
}

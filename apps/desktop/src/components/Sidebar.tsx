import { Download, Folder, Globe, Moon, Plus, Rss, SlidersHorizontal, Sun, Workflow } from "lucide-react";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { useT } from "../i18n/I18nContext";
import { IconButton } from "./IconButton";
import styles from "./Sidebar.module.css";

export type View = "downloads" | "library" | "sources" | "methods" | "browser" | "settings";

interface SidebarProps {
  view: View;
  onSelectView: (view: View) => void;
  activeJobCount: number;
  onAddUrl: () => void;
  isEffectiveDark: boolean;
  onToggleTheme: () => void;
}

/**
 * Below 720px window width this becomes an icon-only rail (never fully
 * hidden). useMediaQuery's underlying matchMedia "change" event only fires
 * when the breakpoint is actually crossed, so this never re-collapses mid-
 * resize while already on the same side of 720px.
 */
export function Sidebar({
  view,
  onSelectView,
  activeJobCount,
  onAddUrl,
  isEffectiveDark,
  onToggleTheme,
}: SidebarProps) {
  const t = useT();
  const isRail = !useMediaQuery("(min-width: 720px)");
  const labelClass = isRail ? styles.srOnly : styles.navLabel;

  return (
    <aside className={styles.sidebar} data-rail={isRail || undefined}>
      {!isRail && <div className={styles.brand}>{t.sidebar.brand}</div>}

      <button type="button" className={styles.addUrl} onClick={onAddUrl} title={t.sidebar.addUrl}>
        <Plus size={18} aria-hidden />
        <span className={labelClass}>{t.sidebar.addUrl}</span>
      </button>

      <nav className={styles.nav} aria-label={t.sidebar.downloads}>
        <button
          type="button"
          className={styles.navItem}
          aria-current={view === "downloads" ? "page" : undefined}
          onClick={() => onSelectView("downloads")}
          title={t.sidebar.downloads}
        >
          <Download size={18} aria-hidden />
          <span className={labelClass}>{t.sidebar.downloads}</span>
          {!isRail && activeJobCount > 0 && <span className={styles.count}>{activeJobCount}</span>}
        </button>
        <button
          type="button"
          className={styles.navItem}
          aria-current={view === "library" ? "page" : undefined}
          onClick={() => onSelectView("library")}
          title={t.sidebar.library}
        >
          <Folder size={18} aria-hidden />
          <span className={labelClass}>{t.sidebar.library}</span>
        </button>
        <button
          type="button"
          className={styles.navItem}
          aria-current={view === "sources" ? "page" : undefined}
          onClick={() => onSelectView("sources")}
          title={t.sources.title}
        >
          <Rss size={18} aria-hidden />
          <span className={labelClass}>{t.sources.title}</span>
        </button>
        <button
          type="button"
          className={styles.navItem}
          aria-current={view === "methods" ? "page" : undefined}
          onClick={() => onSelectView("methods")}
          title={t.methods.title}
        >
          <Workflow size={18} aria-hidden />
          <span className={labelClass}>{t.methods.title}</span>
        </button>
        <button
          type="button"
          className={styles.navItem}
          aria-current={view === "browser" ? "page" : undefined}
          onClick={() => onSelectView("browser")}
          title={t.sidebar.browser}
        >
          <Globe size={18} aria-hidden />
          <span className={labelClass}>{t.sidebar.browser}</span>
        </button>
      </nav>

      <div className={styles.foot}>
        <button
          type="button"
          className={styles.settingsLink}
          aria-current={view === "settings" ? "page" : undefined}
          onClick={() => onSelectView("settings")}
          title={t.sidebar.settings}
        >
          <SlidersHorizontal size={18} aria-hidden />
          <span className={labelClass}>{t.sidebar.settings}</span>
        </button>
        <IconButton
          label={isEffectiveDark ? t.common.toggleThemeToLight : t.common.toggleThemeToDark}
          onClick={onToggleTheme}
        >
          {isEffectiveDark ? <Sun size={18} aria-hidden /> : <Moon size={18} aria-hidden />}
        </IconButton>
      </div>
    </aside>
  );
}

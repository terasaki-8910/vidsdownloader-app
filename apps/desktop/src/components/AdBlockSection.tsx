import { useState } from "react";
import { ipc } from "../ipc";
import type { Site } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { errorMessage } from "../lib/url";
import { useSites } from "../state/SitesContext";
import settingsStyles from "./SettingsView.module.css";
import { SiteIdentity } from "./SitesSection";
import sitesStyles from "./SitesSection.module.css";
import { Switch } from "./Switch";
import styles from "./AdBlockSection.module.css";

/** Ad blocking (uBlock Origin Lite) exists on Windows only for now; elsewhere the section is hidden. */
export const AD_BLOCK_AVAILABLE = navigator.userAgent.includes("Windows");

/** Settings > Ad blocking: one switch per site, in the order of Settings > Sites. */
export function AdBlockSection() {
  const t = useT();
  const { sites } = useSites();

  return (
    <div className={sitesStyles.body}>
      <p className={`${sitesStyles.lead} ${styles.lead}`}>{t.adBlock.lead}</p>
      <ul className={sitesStyles.list}>
        {sites.map((site) => (
          <AdBlockRow key={site.id} site={site} />
        ))}
      </ul>
      <p className={settingsStyles.note}>{t.adBlock.note}</p>
    </div>
  );
}

function AdBlockRow({ site }: { site: Site }) {
  const t = useT();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function setAdBlock(enabled: boolean) {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await ipc.call("site_set_ad_block", { siteId: site.id, enabled });
    } catch (e) {
      setError(t.sites.failed(errorMessage(e)));
    } finally {
      setPending(false);
    }
  }

  return (
    <li className={`${sitesStyles.row} ${styles.row}`}>
      <div className={sitesStyles.head}>
        <SiteIdentity site={site} />
        <span className={styles.switchCell}>
          <Switch
            checked={site.adBlock}
            disabled={pending}
            label={t.adBlock.siteSwitch(site.name)}
            onChange={(enabled) => void setAdBlock(enabled)}
          />
        </span>
      </div>
      {error && (
        <p role="alert" className={sitesStyles.error}>
          {error}
        </p>
      )}
    </li>
  );
}

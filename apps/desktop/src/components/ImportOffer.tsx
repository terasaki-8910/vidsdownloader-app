import { useEffect, useId, useRef, useState } from "react";
import { useFirefoxImport } from "../hooks/useFirefoxImport";
import { useT } from "../i18n/I18nContext";
import { useSettings } from "../state/SettingsContext";
import { ImportProfile } from "./ImportProfile";
import styles from "./ImportOffer.module.css";

interface ImportOfferProps {
  /** Called when the card has been dismissed, so the page can put the keyboard focus somewhere. */
  onGone: () => void;
}

/**
 * A one-time card above the browser's site list, offering to bring the logins of a Firefox
 * profile into the in-app browser (Windows; Chrome and Edge keep theirs behind Windows'
 * own protection, which the card says). It goes away for good once the person has
 * dismissed it or imported: the setting importOfferDismissed. Settings > Sites keeps the
 * same import available.
 */
export function ImportOffer({ onGone }: ImportOfferProps) {
  const t = useT();
  const titleId = useId();
  const { settings, updateSettings } = useSettings();
  const imp = useFirefoxImport();
  const [closed, setClosed] = useState(false);
  const importRef = useRef<HTMLButtonElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const wasRunning = useRef(false);

  // The button the person pressed is disabled while the import runs, and (on success) gone
  // after it: hand the keyboard focus to what is there afterwards.
  useEffect(() => {
    if (imp.running) {
      wasRunning.current = true;
      return;
    }
    if (!wasRunning.current) return;
    wasRunning.current = false;
    (imp.result ? closeRef : importRef).current?.focus();
  }, [imp.running, imp.result]);

  // Not before the settings (is it dismissed?) and Firefox's profiles (is there any?) are
  // known. A dismissed card still shows the outcome of the import that dismissed it.
  if (!settings || imp.profiles.length === 0 || closed) return null;
  if (settings.importOfferDismissed && !imp.result) return null;

  async function runImport() {
    const result = await imp.start();
    if (result) updateSettings({ importOfferDismissed: true });
  }

  function notNow() {
    updateSettings({ importOfferDismissed: true });
    onGone();
  }

  function close() {
    setClosed(true);
    onGone();
  }

  return (
    <section className={styles.card} aria-labelledby={titleId}>
      <h2 id={titleId} className={styles.title}>
        {t.importLogins.offerTitle}
      </h2>
      <div role="status">
        <p className={styles.body}>{imp.resultLine ?? t.importLogins.offerBody}</p>
      </div>
      <div className={styles.row}>
        {imp.result ? (
          <button type="button" className={styles.textButton} ref={closeRef} onClick={close}>
            {t.importLogins.close}
          </button>
        ) : (
          <>
            <ImportProfile
              profiles={imp.profiles}
              value={imp.profile}
              onChange={imp.chooseProfile}
              disabled={imp.running}
            />
            <button
              type="button"
              className={styles.importButton}
              ref={importRef}
              disabled={imp.running}
              onClick={() => void runImport()}
            >
              {imp.running ? t.importLogins.importing : t.importLogins.importButton}
            </button>
            <button type="button" className={styles.textButton} disabled={imp.running} onClick={notNow}>
              {t.importLogins.notNow}
            </button>
          </>
        )}
      </div>
      {imp.errorLine && (
        <p role="alert" className={styles.error}>
          {imp.errorLine}
        </p>
      )}
    </section>
  );
}

// Settings > アップデート (round 11, as sampled): the running version, アップデートを確認 and the
// result of the last check right under it, written with SettingsView's own styles:
//   checking  確認しています…
//   latest    最新の版です（11月4日 18:40 に確認）
//   found     0.2.0 があります  [更新する]
//   skipped   0.2.0 はスキップしました  [スキップをやめる]
//   failed    確認できませんでした（<message>）
// The launch check fills the same line; one that failed leaves it empty.
import { Check } from "lucide-react";
import { useT } from "../i18n/I18nContext";
import { useUpdates } from "../state/UpdatesContext";
import settingsStyles from "./SettingsView.module.css";
import styles from "./UpdatesSection.module.css";

export function UpdatesSection() {
  const t = useT();
  const { appVersion, check, skipped, checkNow, install, unskip } = useUpdates();
  const checking = check.status === "checking";

  return (
    <div className={settingsStyles.sectionBody}>
      <div className={settingsStyles.field}>
        <div className={settingsStyles.fieldLabel}>{t.updates.currentVersion}</div>
        <div className={styles.version}>{appVersion ?? ""}</div>
      </div>

      <div className={styles.check}>
        <button
          type="button"
          className={`${settingsStyles.secondaryButton} ${styles.checkButton}`}
          // aria-disabled, not disabled, so focus stays on the button while it checks.
          aria-disabled={checking ? "true" : undefined}
          onClick={() => {
            if (!checking) checkNow();
          }}
        >
          {t.updates.check}
        </button>
        <p className={styles.result} role="status">
          {check.status === "checking" ? (
            <span className={styles.quiet}>{t.updates.checking}</span>
          ) : check.status === "latest" ? (
            <>
              <Check size={16} aria-hidden className={styles.tick} />
              <span>{t.updates.upToDate(t.updates.checkedAt(check.checkedAt, new Date()))}</span>
            </>
          ) : check.status === "found" && !skipped ? (
            <>
              <span className={styles.strong}>{t.updates.found(check.update.version)}</span>
              <button type="button" className={styles.primary} onClick={install}>
                {t.updates.install}
              </button>
            </>
          ) : check.status === "found" ? (
            <>
              <span className={styles.quiet}>{t.updates.skipped(check.update.version)}</span>
              <button type="button" className={styles.link} onClick={unskip}>
                {t.updates.unskip}
              </button>
            </>
          ) : check.status === "failed" ? (
            <span className={styles.error}>{t.updates.checkFailed(check.message)}</span>
          ) : null}
        </p>
      </div>
    </div>
  );
}

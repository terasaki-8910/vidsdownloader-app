import { Play } from "lucide-react";
import type { JobRow, Preset } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { joinMeta, formatDateTime } from "../lib/format";
import { getRowAction, type RowActionHandlers } from "../lib/rowAction";
import { isPlayablePath } from "../lib/media";
import { useNavigation } from "../state/NavigationContext";
import { JobStateLine } from "./JobStateLine";
import { MiniPlayer } from "./MiniPlayer";
import styles from "./DetailBody.module.css";

interface DetailBodyProps extends RowActionHandlers {
  job: JobRow;
  preset: Preset | undefined;
}

/** The content shared by the wide detail panel and the narrow overlay sheet. */
export function DetailBody({ job, preset, onCancel, onRetry, onReveal, onLogin, onSaveAgain, onChooseMedia }: DetailBodyProps) {
  const t = useT();
  const { openInBrowser } = useNavigation();
  const title = job.details?.title ?? job.url;
  const sub = joinMeta([
    job.details?.uploader,
    job.details?.site,
    job.details?.publishedAt != null ? formatDateTime(job.details.publishedAt) : null,
  ]);
  const action = getRowAction(job, t, {
    onCancel,
    onRetry,
    onReveal,
    onLogin,
    onSaveAgain,
    onChooseMedia,
    onOpenInBrowser: () => void openInBrowser(job.url),
  });

  return (
    <div className={styles.body}>
      {job.state === "Done" &&
      job.outputPath &&
      isPlayablePath(job.outputPath) &&
      !job.skipped &&
      job.listCount == null ? (
        <MiniPlayer key={job.id} job={job} title={title} />
      ) : (
        <div className={styles.thumbLg}>
          {job.details?.thumbnailUrl ? (
            <img src={job.details.thumbnailUrl} alt="" />
          ) : (
            <Play size={28} aria-hidden />
          )}
        </div>
      )}

      <div>
        <h2 className={styles.title}>{title}</h2>
        {sub && <div className={styles.sub}>{sub}</div>}
      </div>

      <JobStateLine job={job} showPath={job.state !== "Done"} />

      {(preset || job.outputPath) && (
        <dl className={styles.kv}>
          {preset && (
            <>
              <dt>{t.detail.quality}</dt>
              <dd>{t.presets[preset]}</dd>
            </>
          )}
          {job.outputPath && (
            <>
              <dt>{t.detail.savedTo}</dt>
              <dd className={styles.path}>{job.outputPath}</dd>
            </>
          )}
        </dl>
      )}

      {action && (
        <div className={styles.actions}>
          <button
            type="button"
            className={action.kind === "login" ? styles.loginButton : styles.actionButton}
            onClick={action.onClick}
          >
            {action.icon}
            {action.label}
          </button>
        </div>
      )}
    </div>
  );
}

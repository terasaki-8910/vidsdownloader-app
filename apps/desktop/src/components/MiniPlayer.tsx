import { ExternalLink, PictureInPicture2, Play } from "lucide-react";
import { useEffect, useRef } from "react";
import { useT } from "../i18n/I18nContext";
import { ipc } from "../ipc";
import type { JobRow } from "../ipc/contract";
import { usePlayer } from "../state/PlayerContext";
import styles from "./MiniPlayer.module.css";

export function MiniPlayer({ job, title }: { job: JobRow; title: string }) {
  const t = useT();
  const slotRef = useRef<HTMLDivElement>(null);
  const player = usePlayer();
  const loaded = player.jobId === job.id;

  useEffect(() => {
    const slot = slotRef.current;
    if (slot) player.attach(job.id, slot);
    return () => player.release(job.id);
  }, [job.id, player.attach, player.release]);

  const pipLabel = player.inPip ? t.player.pipExit : t.player.pip;

  return (
    <div
      className={`${styles.box} ${loaded && !player.error ? styles.stage : ""}`}
      role="group"
      aria-label={title}
    >
      <div ref={slotRef} className={styles.slot} />

      {!loaded && (
        <>
          {job.details?.thumbnailUrl && <img className={styles.poster} src={job.details.thumbnailUrl} alt="" />}
          <button
            type="button"
            className={styles.play}
            aria-label={t.player.play}
            onClick={() => slotRef.current && void player.play(job, slotRef.current)}
          >
            <Play size={22} fill="currentColor" aria-hidden />
          </button>
        </>
      )}

      {loaded && !player.error && document.pictureInPictureEnabled && (
        <button
          type="button"
          className={`${styles.pip} ${player.inPip ? styles.pipActive : ""}`}
          aria-label={pipLabel}
          title={pipLabel}
          onClick={() => void player.togglePip()}
        >
          <PictureInPicture2 size={18} aria-hidden />
        </button>
      )}

      {loaded && player.error === "unsupported" && (
        <div className={styles.message}>
          <span>{t.player.unsupported}</span>
          <button type="button" className={styles.openButton} onClick={() => void ipc.call("open_job_file", { id: job.id })}>
            <ExternalLink size={15} aria-hidden />
            {t.player.openWith}
          </button>
        </div>
      )}

      {loaded && player.error === "missing" && <div className={styles.message}>{t.player.missing}</div>}
    </div>
  );
}

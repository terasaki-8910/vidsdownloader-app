import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ipc } from "../ipc";
import type { JobRow } from "../ipc/contract";
import { mediaUrl } from "../lib/media";
import styles from "../components/MiniPlayer.module.css";

type PlayerError = "unsupported" | "missing" | null;

interface PlayerContextValue {
  jobId: number | null;
  inPip: boolean;
  error: PlayerError;
  play: (job: JobRow, slot: HTMLElement) => Promise<void>;
  attach: (jobId: number, slot: HTMLElement) => void;
  release: (jobId: number) => void;
  togglePip: () => Promise<void>;
}

const PlayerContext = createContext<PlayerContextValue | null>(null);

export function PlayerProvider({ children }: { children: ReactNode }) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const holderRef = useRef<HTMLDivElement | null>(null);
  const jobIdRef = useRef<number | null>(null);
  const inPipRef = useRef(false);
  const [jobId, setJobId] = useState<number | null>(null);
  const [inPip, setInPip] = useState(false);
  const [error, setError] = useState<PlayerError>(null);

  const freeVideo = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    video.pause();
    video.removeAttribute("src");
    video.removeAttribute("poster");
    video.load();
    video.remove();
    jobIdRef.current = null;
    setJobId(null);
    setError(null);
  }, []);

  useEffect(() => {
    const video = document.createElement("video");
    video.controls = true;
    video.playsInline = true;
    video.preload = "metadata";
    video.setAttribute("controlslist", "nodownload");
    video.className = styles.video;

    const holder = document.createElement("div");
    holder.style.position = "fixed";
    holder.style.width = "1px";
    holder.style.height = "1px";
    holder.style.overflow = "hidden";
    holder.style.opacity = "0";
    holder.style.pointerEvents = "none";
    holder.setAttribute("aria-hidden", "true");
    document.body.append(holder);

    const enterPip = () => {
      inPipRef.current = true;
      setInPip(true);
    };
    const leavePip = () => {
      inPipRef.current = false;
      setInPip(false);
      if (video.parentElement === holder) freeVideo();
    };
    const mediaError = () => {
      const failedJobId = jobIdRef.current;
      if (failedJobId == null) return;
      void ipc.call("job_file_exists", { id: failedJobId }).then((exists) => {
        if (jobIdRef.current !== failedJobId) return;
        setError(exists ? "unsupported" : "missing");
        video.remove();
      });
    };
    video.addEventListener("enterpictureinpicture", enterPip);
    video.addEventListener("leavepictureinpicture", leavePip);
    video.addEventListener("error", mediaError);
    videoRef.current = video;
    holderRef.current = holder;

    return () => {
      video.removeEventListener("enterpictureinpicture", enterPip);
      video.removeEventListener("leavepictureinpicture", leavePip);
      video.removeEventListener("error", mediaError);
      video.pause();
      video.remove();
      holder.remove();
      videoRef.current = null;
      holderRef.current = null;
    };
  }, [freeVideo]);

  // The playing job's saved item was erased: stop, leave picture-in-picture, free the video.
  useEffect(
    () =>
      ipc.on("job-removed", ({ id }) => {
        if (jobIdRef.current !== id) return;
        void (async () => {
          if (document.pictureInPictureElement) {
            try {
              await document.exitPictureInPicture();
            } catch {
              // Already left.
            }
          }
          if (jobIdRef.current === id) freeVideo();
          inPipRef.current = false;
          setInPip(false);
        })();
      }),
    [freeVideo],
  );

  const play = useCallback(async (job: JobRow, slot: HTMLElement) => {
    const video = videoRef.current;
    if (!video) return;
    if (jobIdRef.current !== job.id && inPipRef.current && document.pictureInPictureElement) {
      await document.exitPictureInPicture();
    }
    if (jobIdRef.current !== job.id) {
      video.src = mediaUrl(job.id);
      video.poster = job.details?.thumbnailUrl ?? "";
      video.setAttribute("aria-label", job.details?.title ?? job.url);
      jobIdRef.current = job.id;
      setJobId(job.id);
      setError(null);
    }
    slot.append(video);
    try {
      await video.play();
    } catch {
      // The media element's error event distinguishes a missing file from an unsupported format.
    }
    video.focus();
  }, []);

  const attach = useCallback((targetJobId: number, slot: HTMLElement) => {
    const video = videoRef.current;
    if (video && jobIdRef.current === targetJobId) slot.append(video);
  }, []);

  const release = useCallback(
    (targetJobId: number) => {
      const video = videoRef.current;
      if (!video || jobIdRef.current !== targetJobId) return;
      if (inPipRef.current) {
        holderRef.current?.append(video);
      } else {
        freeVideo();
      }
    },
    [freeVideo],
  );

  const togglePip = useCallback(async () => {
    const video = videoRef.current;
    if (!video) return;
    try {
      if (document.pictureInPictureElement) await document.exitPictureInPicture();
      else await video.requestPictureInPicture();
    } catch {
      // Refused (e.g. metadata not loaded yet); the button simply does nothing.
    }
  }, []);

  const value = useMemo(
    () => ({ jobId, inPip, error, play, attach, release, togglePip }),
    [jobId, inPip, error, play, attach, release, togglePip],
  );
  return <PlayerContext.Provider value={value}>{children}</PlayerContext.Provider>;
}

export function usePlayer(): PlayerContextValue {
  const context = useContext(PlayerContext);
  if (!context) throw new Error("usePlayer() used outside <PlayerProvider>");
  return context;
}

// A source's destination folder and quality: a modal <dialog> in ConfirmDialog's look (so
// Escape, focus containment and the backdrop come for free), with the folder box and select
// of Settings. Light dismiss by Escape and a backdrop click; focus starts on the first
// control. Both choices default to the app's current ones, shown in the "default" entries.
import { useEffect, useId, useRef, useState, type MouseEvent } from "react";
import { ipc } from "../ipc";
import type { Preset, SourceRow } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { errorMessage } from "../lib/url";
import dialogStyles from "./ConfirmDialog.module.css";
import settingsStyles from "./SettingsView.module.css";
import styles from "./SourceOptionsDialog.module.css";

const PRESETS: Preset[] = ["best", "upTo1080", "upTo720", "compatibilityH264", "audioM4a", "audioMp3"];

interface SourceOptionsDialogProps {
  source: SourceRow;
  /** The download folder and quality preset a source without its own uses. */
  defaultDir: string;
  defaultPreset: Preset;
  onSave: (destDir: string | null, preset: Preset | null) => Promise<void>;
  /** The dialog was closed, saved or not. */
  onClose: () => void;
}

export function SourceOptionsDialog({ source, defaultDir, defaultPreset, onSave, onClose }: SourceOptionsDialogProps) {
  const t = useT();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const firstRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const qualityId = useId();
  const [dir, setDir] = useState<string | null>(source.destDir);
  const [preset, setPreset] = useState<Preset | null>(source.preset);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog || dialog.open) return;
    dialog.showModal();
    firstRef.current?.focus();
  }, []);

  const close = () => dialogRef.current?.close();

  async function pickFolder() {
    setError(null);
    try {
      const picked = await ipc.call("pick_folder", {});
      if (picked !== null) setDir(picked);
    } catch (e) {
      setError(t.library.failed(errorMessage(e)));
    }
  }

  async function save() {
    if (busy) return;
    if (dir === source.destDir && preset === source.preset) {
      close();
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSave(dir, preset);
      close();
    } catch (e) {
      setError(t.library.failed(errorMessage(e)));
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={dialogRef}
      aria-modal="true"
      aria-labelledby={titleId}
      className={`${dialogStyles.dialog} ${styles.dialog}`}
      onClose={onClose}
      onClick={(e: MouseEvent<HTMLDialogElement>) => {
        if (e.target === dialogRef.current) close();
      }}
    >
      <div className={dialogStyles.inner}>
        <h2 id={titleId} className={dialogStyles.title}>
          {t.sources.options}
        </h2>

        <div className={styles.fields}>
          <div className={settingsStyles.field}>
            <div className={settingsStyles.fieldLabel}>{t.sources.optionsFolder}</div>
            <div className={styles.folderControl}>
              <div
                className={`${settingsStyles.readonlyBox} ${styles.folderBox}`}
                title={dir ?? defaultDir}
              >
                {dir ?? t.sources.optionsFolderDefault(defaultDir)}
              </div>
              <button
                ref={firstRef}
                type="button"
                className={settingsStyles.secondaryButton}
                disabled={busy}
                onClick={() => void pickFolder()}
              >
                {t.sources.optionsChangeFolder}
              </button>
            </div>
            <button
              type="button"
              className={styles.reset}
              disabled={busy || dir === null}
              onClick={() => setDir(null)}
            >
              {t.sources.optionsResetFolder}
            </button>
          </div>

          <div className={settingsStyles.field}>
            <label htmlFor={qualityId} className={settingsStyles.fieldLabel}>
              {t.sources.optionsQuality}
            </label>
            <select
              id={qualityId}
              className={`${settingsStyles.select} ${styles.select}`}
              value={preset ?? ""}
              disabled={busy}
              onChange={(e) => setPreset(e.target.value === "" ? null : (e.target.value as Preset))}
            >
              <option value="">{t.sources.optionsQualityDefault(t.presets[defaultPreset])}</option>
              {PRESETS.map((p) => (
                <option key={p} value={p}>
                  {t.presets[p]}
                </option>
              ))}
            </select>
          </div>
        </div>

        {error && (
          <p role="alert" className={styles.error}>
            {error}
          </p>
        )}

        <div className={dialogStyles.buttons}>
          <button type="button" className={styles.cancel} onClick={close}>
            {t.common.dialogCancel}
          </button>
          <button type="button" className={styles.save} disabled={busy} onClick={() => void save()}>
            {t.sources.optionsSave}
          </button>
        </div>
      </div>
    </dialog>
  );
}

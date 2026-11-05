import { ChevronDown, Plus } from "lucide-react";
import { useRef, useState, type DragEvent, type KeyboardEvent, type RefObject } from "react";
import type { Preset } from "../ipc/contract";
import { useT } from "../i18n/I18nContext";
import { FolderMenu } from "./FolderMenu";
import styles from "./UrlBar.module.css";

const PRESETS: Preset[] = ["best", "upTo1080", "upTo720", "compatibilityH264", "audioM4a", "audioMp3", "commentsOnly"];

function parseUrls(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

interface UrlBarProps {
  textareaRef: RefObject<HTMLTextAreaElement | null>;
  defaultPreset: Preset;
  downloadDir: string;
  recentDownloadDirs: string[];
  onAdd: (urls: string[], preset: Preset) => void;
  onChooseDownloadDir: (path: string) => void;
  onPickDownloadDir: () => void;
}

export function UrlBar({
  textareaRef,
  defaultPreset,
  downloadDir,
  recentDownloadDirs,
  onAdd,
  onChooseDownloadDir,
  onPickDownloadDir,
}: UrlBarProps) {
  const t = useT();
  const [text, setText] = useState("");
  // Seeded once from Settings' default preset; changing it here is a per-add
  // choice, not a write back to the saved default (see report).
  const [preset, setPreset] = useState<Preset>(defaultPreset);
  const wrapRef = useRef<HTMLDivElement>(null);

  const urls = parseUrls(text);
  const canAdd = urls.length > 0;

  function autosize(el: HTMLTextAreaElement) {
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 120)}px`;
  }

  function submit() {
    if (!canAdd) return;
    onAdd(urls, preset);
    setText("");
    const el = textareaRef.current;
    if (el) el.style.height = "auto";
  }

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      submit();
    }
  }

  function handleDrop(e: DragEvent<HTMLDivElement>) {
    e.preventDefault();
    const dropped = e.dataTransfer.getData("text/uri-list") || e.dataTransfer.getData("text/plain");
    if (!dropped.trim()) return;
    setText((prev) => (prev.trim().length > 0 ? `${prev}\n${dropped}` : dropped));
  }

  return (
    <div className={styles.urlbar} onDragOver={(e) => e.preventDefault()} onDrop={handleDrop}>
      <div className={styles.field} ref={wrapRef}>
        <Plus size={18} aria-hidden className={styles.fieldIcon} />
        <textarea
          ref={textareaRef}
          className={styles.textarea}
          rows={1}
          value={text}
          placeholder={t.urlbar.placeholder}
          onChange={(e) => {
            setText(e.target.value);
            autosize(e.target);
          }}
          onKeyDown={handleKeyDown}
          aria-label={t.urlbar.placeholder}
        />
      </div>

      {/* One group, so a wrap moves all three below the field together. */}
      <div className={styles.controls}>
        <label className={styles.select}>
          <span className={styles.selectLabel}>{t.urlbar.qualityLabel}</span>
          <select
            className={styles.selectInput}
            value={preset}
            onChange={(e) => setPreset(e.target.value as Preset)}
          >
            {PRESETS.map((p) => (
              <option key={p} value={p}>
                {t.presets[p]}
              </option>
            ))}
          </select>
          <ChevronDown size={16} aria-hidden className={styles.selectChevron} />
        </label>

        <FolderMenu
          downloadDir={downloadDir}
          recentDownloadDirs={recentDownloadDirs}
          onChoose={onChooseDownloadDir}
          onPickOther={onPickDownloadDir}
        />

        <button type="button" className={styles.addButton} onClick={submit} disabled={!canAdd}>
          {t.urlbar.add}
        </button>
      </div>
    </div>
  );
}

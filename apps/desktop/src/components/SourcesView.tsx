// The Sources destination: registered channels, lists and collect-rule pages that are checked
// for new items. Composed from the app's own parts: the Downloads title block and URL bar,
// Settings' secondary button, the empty state. Registering goes through SourcesContext, which
// the 集める tray and the Downloads list rows use too.
import { Rss } from "lucide-react";
import { useLayoutEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { useT } from "../i18n/I18nContext";
import { newestCheck, registerFailureText } from "../lib/sources";
import { errorMessage, normalizeUrl } from "../lib/url";
import { useSources } from "../state/SourcesContext";
import downloadsStyles from "./DownloadsView.module.css";
import emptyStyles from "./EmptyState.module.css";
import settingsStyles from "./SettingsView.module.css";
import { SourceRow } from "./SourceRow";
import urlbarStyles from "./UrlBar.module.css";
import styles from "./SourcesView.module.css";

export function SourcesView() {
  const t = useT();
  const { sources, loaded, phase, register, checkAll, remove } = useSources();
  const fieldRef = useRef<HTMLTextAreaElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  /** Where the row that was just removed sat, so keyboard focus can stay near it. */
  const focusIndex = useRef<number | null>(null);
  const [text, setText] = useState("");
  /** The line under the field: why a registration or a check of everything did not work. */
  const [message, setMessage] = useState<string | null>(null);

  const previewing = phase === "previewing";
  const canRegister = text.trim() !== "" && phase === "idle";
  const allBusy = sources.length > 0 && sources.every((source) => source.status !== "idle");

  // A removed row takes the keyboard focus with it; hand it to the row that took its place
  // (or the field when the list is now empty).
  useLayoutEffect(() => {
    const index = focusIndex.current;
    if (index === null) return;
    focusIndex.current = null;
    const buttons = listRef.current?.querySelectorAll<HTMLButtonElement>("[aria-haspopup='menu']");
    if (buttons && buttons.length > 0) buttons[Math.min(index, buttons.length - 1)]?.focus();
    else fieldRef.current?.focus();
  }, [sources]);

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    if (!canRegister) return;
    const url = normalizeUrl(text);
    if (!url) {
      setMessage(t.browser.invalidUrl);
      return;
    }
    setMessage(null);
    const outcome = await register(url);
    if (outcome.kind === "registered") setText("");
    setMessage(registerFailureText(t, outcome));
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    // A URL has no line breaks: Enter and Ctrl+Enter both submit (not while an IME converts).
    if (e.key !== "Enter" || e.shiftKey || e.nativeEvent.isComposing || e.keyCode === 229) return;
    e.preventDefault();
    void submit();
  }

  async function runCheckAll() {
    setMessage(null);
    try {
      await checkAll();
    } catch (error) {
      setMessage(t.library.failed(errorMessage(error)));
    }
  }

  async function removeSource(id: number) {
    focusIndex.current = sources.findIndex((source) => source.id === id);
    try {
      await remove(id);
    } catch (error) {
      focusIndex.current = null;
      throw error;
    }
  }

  return (
    <section className={downloadsStyles.downloads}>
      <div className={downloadsStyles.head}>
        <h1>{t.sources.title}</h1>
        {sources.length > 0 && (
          <div className={downloadsStyles.summary}>
            {t.sources.summary(sources.length, newestCheck(sources), new Date())}
          </div>
        )}
        {sources.length > 0 && (
          <button
            type="button"
            className={`${settingsStyles.secondaryButton} ${styles.checkAll}`}
            disabled={allBusy}
            onClick={() => void runCheckAll()}
          >
            {t.sources.checkAll}
          </button>
        )}
      </div>

      <form className={urlbarStyles.urlbar} onSubmit={(e) => void submit(e)}>
        <div className={urlbarStyles.field}>
          <Rss size={18} aria-hidden className={urlbarStyles.fieldIcon} />
          <textarea
            ref={fieldRef}
            className={urlbarStyles.textarea}
            rows={1}
            value={text}
            placeholder={t.sources.urlPlaceholder}
            aria-label={t.sources.urlPlaceholder}
            aria-invalid={message !== null || undefined}
            spellCheck={false}
            onChange={(e) => {
              setText(e.target.value);
              setMessage(null);
            }}
            onKeyDown={onKeyDown}
          />
        </div>
        <div className={urlbarStyles.controls}>
          <button type="submit" className={urlbarStyles.addButton} disabled={!canRegister}>
            {previewing ? t.sources.previewing : t.sources.register}
          </button>
        </div>
      </form>

      {message && (
        <p role="alert" className={styles.message}>
          {message}
        </p>
      )}

      {!loaded ? null : sources.length === 0 ? (
        <div className={emptyStyles.empty}>
          <div className={emptyStyles.icon} aria-hidden>
            <Rss size={32} />
          </div>
          <div className={`${emptyStyles.body} ${styles.emptyBody}`}>{t.sources.emptyBody}</div>
        </div>
      ) : (
        <ul className={downloadsStyles.list} ref={listRef}>
          {sources.map((source) => (
            <SourceRow key={source.id} source={source} onRemove={removeSource} />
          ))}
        </ul>
      )}
    </section>
  );
}

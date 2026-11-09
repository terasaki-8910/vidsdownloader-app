// The browser toolbar's page actions, inside the address field after the input (they replaced
// the login chip): login status, copy link, save page, open in the default browser.
import { Check, ExternalLink, KeyRound, Link2, Star } from "lucide-react";
import { useT } from "../i18n/I18nContext";
import { IconButton } from "./IconButton";
import styles from "./AddressTools.module.css";

interface AddressToolsProps {
  /** The login status of the page's site; null (hidden) when the page belongs to no registered site. */
  login: { present: boolean; label: string } | null;
  copied: boolean;
  saved: boolean;
  onLogin: () => void;
  onCopy: () => void;
  onToggleSaved: () => void;
  onOpenExternal: () => void;
}

export function AddressTools({ login, copied, saved, onLogin, onCopy, onToggleSaved, onOpenExternal }: AddressToolsProps) {
  const t = useT();
  return (
    <div className={styles.tools}>
      {login && (
        <IconButton
          size="sm"
          label={login.label}
          className={login.present ? styles.loginOn : styles.loginOff}
          onClick={onLogin}
        >
          <KeyRound size={16} aria-hidden />
        </IconButton>
      )}
      <IconButton size="sm" label={copied ? t.browser.linkCopied : t.browser.copyLink} onClick={onCopy}>
        {copied ? <Check size={16} aria-hidden /> : <Link2 size={16} aria-hidden />}
      </IconButton>
      <IconButton
        size="sm"
        label={saved ? t.browser.unsavePage : t.browser.savePage}
        className={saved ? styles.starOn : undefined}
        onClick={onToggleSaved}
      >
        <Star size={16} aria-hidden />
      </IconButton>
      <IconButton size="sm" label={t.browser.openExternal} onClick={onOpenExternal}>
        <ExternalLink size={16} aria-hidden />
      </IconButton>
    </div>
  );
}

import type { Account, Site } from "../ipc/contract";
import type { Translations } from "../i18n/types";
import { formatMonthDay } from "./format";

/** The account a download from this site uses: the selected one, else the first. */
export function selectedAccount(site: Site): Account | null {
  return site.accounts.find((a) => a.id === site.selectedAccountId) ?? site.accounts[0] ?? null;
}

/** True when downloads from the site have login info to use. */
export function hasLoginInfo(account: Account | null): boolean {
  if (!account) return false;
  return account.method.kind === "inApp" ? account.cookieCount != null : true;
}

export function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** The status line shown for an account in Settings > Sites. */
export function accountStatus(t: Translations, account: Account): { text: string; present: boolean } {
  const date = account.updatedAt != null ? formatMonthDay(account.updatedAt) : null;
  switch (account.method.kind) {
    case "firefox":
      return { text: t.sites.statusFirefox, present: true };
    case "cookiesTxt":
      return { text: t.sites.statusFile(fileName(account.method.path), date), present: true };
    case "inApp":
      return account.cookieCount != null
        ? { text: t.sites.statusCookies(account.cookieCount, date), present: true }
        : { text: t.sites.statusNone, present: false };
  }
}

/** The status line shown for a site on the browser start page. */
export function siteStartStatus(t: Translations, site: Site): { text: string; present: boolean } {
  const account = selectedAccount(site);
  if (!hasLoginInfo(account) || !account) return { text: t.browser.loginAbsent, present: false };
  if (account.method.kind !== "inApp") return accountStatus(t, account);
  const date = account.updatedAt != null ? formatMonthDay(account.updatedAt) : null;
  return { text: t.browser.siteLoginPresent(date), present: true };
}

/** The registered site a page belongs to (the longest matching domain), or null. */
export function siteForUrl(sites: Site[], url: string): Site | null {
  let host: string;
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return null;
  }
  let best: { site: Site; length: number } | null = null;
  for (const site of sites) {
    for (const domain of site.domains) {
      if ((host === domain || host.endsWith(`.${domain}`)) && (!best || domain.length > best.length)) {
        best = { site, length: domain.length };
      }
    }
  }
  return best?.site ?? null;
}

/** The first domain as a URL to open in the browser. */
export function siteHomeUrl(site: Site): string {
  return `https://${site.domains[0] ?? ""}/`;
}

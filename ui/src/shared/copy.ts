// User-visible wording built from the model, using upstream's localization keys
// (UsageWindow.name, Unavailability.message, UsageDetailCard.resetText).
import { locale, t } from "./i18n";
import type { UsageWindow } from "./model";

export function windowName(w: UsageWindow): string {
  if (w.label) return w.label;
  let base: string;
  switch (w.kind.type) {
    case "fiveHour": base = t("5-hour limit"); break;
    case "weekly": base = t("Weekly limit"); break;
    case "spend": base = t("Spend limit"); break;
    case "balance": base = t("Balance"); break;
    case "daily": base = t("Daily limit"); break;
    case "messages": base = t("Message allowance"); break;
    case "monthly": base = t("Monthly limit"); break;
    case "topUp": base = t("Top-up pack"); break;
    case "credits": base = t("Credit allowance"); break;
    case "sharedCredits": base = t("Team credits"); break;
    case "other": {
      const s = w.kind.seconds;
      base = s >= 86_400 && s % 86_400 === 0
        ? t("%@-day limit", s / 86_400)
        : t("%@-hour limit", Math.max(Math.round(s / 3600), 1));
    }
  }
  const scoped = w.scope ? `${base} · ${w.scope}` : base;
  const estimate = w.estimate && { planPrice: t("estimated"), sinceTopUp: t("since top-up"), yourBudget: t("of your budget") }[w.estimate];
  return estimate ? `${scoped} · ${estimate}` : scoped;
}

export function lengthText(w: UsageWindow): string {
  const hours = w.windowSeconds / 3600;
  return hours >= 24 ? t("%@ days", Math.round(hours / 24)) : t("%@ hours", Math.round(hours));
}

const isToday = (d: Date) => d.toDateString() === new Date().toDateString();

function intlLocale() {
  return { "zh-Hans": "zh-CN", "zh-Hant": "zh-TW" }[locale()] ?? locale();
}

/** Upstream template "jmm" (today) / "MMMdjmm". */
export function formatResetTime(d: Date): string {
  const opts: Intl.DateTimeFormatOptions = isToday(d)
    ? { hour: "numeric", minute: "2-digit" }
    : { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" };
  return new Intl.DateTimeFormat(intlLocale(), opts).format(d);
}

export function resetText(w: UsageWindow): string {
  if (w.nextExpiry && (!w.resetsAt || Date.parse(w.nextExpiry.at) < Date.parse(w.resetsAt))) {
    const at = new Date(w.nextExpiry.at);
    const day = new Intl.DateTimeFormat(intlLocale(), isToday(at) ? { hour: "numeric", minute: "2-digit" } : { month: "short", day: "numeric" }).format(at);
    const amount = new Intl.NumberFormat(intlLocale(), { maximumFractionDigits: 1 }).format(w.nextExpiry.amount);
    return t("%@: %@ credits expire", day, amount);
  }
  if (!w.resetsAt) return w.reportsLength ? lengthText(w) : "";
  return t("Resets %@", formatResetTime(new Date(w.resetsAt)));
}

export function relativeTime(iso: string): string {
  const seconds = (Date.parse(iso) - Date.now()) / 1000;
  const rtf = new Intl.RelativeTimeFormat(intlLocale(), { numeric: "auto" });
  const abs = Math.abs(seconds);
  if (abs < 60) return rtf.format(Math.round(seconds), "second");
  if (abs < 3600) return rtf.format(Math.round(seconds / 60), "minute");
  if (abs < 86_400) return rtf.format(Math.round(seconds / 3600), "hour");
  return rtf.format(Math.round(seconds / 86_400), "day");
}

/** Upstream `ProviderUsage.Unavailability.message`, keyed by the serialized case name. */
const unavailableKeys: Record<string, string> = {
  loading: "Loading…",
  notConnected: "Connect Claude Code in Settings to see usage.",
  awaitingResponse: "Waiting for the next Claude Code response.",
  noLimitsReported: "No limits reported.",
  signInRequired: "Sign in to Codex to see usage.",
  claudeSignInRequired: "Sign in to Claude Code to see usage.",
  claudeLoginExpired: "Claude Code's saved login expired. Use Claude Code, or connect the status line.",
  codexNotInstalled: "Codex isn't installed.",
  codexServerFailed: "Couldn't start the Codex helper.",
  kiroNotInstalled: "Kiro CLI isn't installed.",
  kiroVersionUnsupported: "Update Kiro CLI to read subscription usage.",
  kiroSignInRequired: "Sign in to Kiro CLI to see usage.",
  antigravityNotRunning: "Open Antigravity to see its usage.",
  antigravityNotAnswering: "Antigravity is open but didn't answer. Restarting it usually helps.",
  cursorSignInRequired: "Sign in to Cursor to see usage.",
  cursorLoginExpired: "Cursor's saved login was refused. Open Cursor to renew it.",
  grokSignInRequired: "Sign in to Grok to see usage.",
  grokLoginExpired: "Grok's saved login expired. Use Grok to renew it.",
  signedOut: "Sign in to this account again in Settings.",
  notSignedIn: "Sign in from Settings to see usage.",
  zaiNoCodingPlan: "That key works. The account has no Coding Plan running on it.",
  serverAddressMissing: "Add the server address in Settings.",
  serverAddressRefused: "That address can't be used. It needs https://, unless the server is on your own network.",
  apiKeyMissing: "Add an API key in Settings.",
  apiKeyRefused: "That key was refused. Check it in Settings.",
  unreachable: "The service didn't respond.",
  unreadableReply: "Couldn't read the reply.",
  rateLimited: "Checking too often — easing off.",
  serverError: "The service returned an error.",
  sessionMissing: "Read a browser session in Settings.",
  sessionExpired: "The browser session expired. Sign in on the website, then read it again in Settings.",
  localLoginMissing: "Sign in with this service's own app or command-line tool first.",
  localLoginExpired: "The saved login has expired. Sign in again with the service's own app or tool.",
  localAppMissing: "Nothing saved on this Mac yet. Open the service's app once, then retry.",
  noPlan: "This account has no plan with usage limits.",
};

export function unavailableMessage(reason: string): string {
  const key = unavailableKeys[reason];
  return key ? t(key) : reason;
}

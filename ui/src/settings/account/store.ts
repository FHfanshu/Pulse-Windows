// Per-account setting accessors (upstream AppSettings+Providers / +Rings / +Panel / +Accounts), as patches
// for `updateSettings`. Maps are replaced whole, so a cleared entry is one that is left out.
import { invoke } from "@tauri-apps/api/core";
import { updateSettings, type AppSettings } from "../../shared/settings";

/** `map` with `id` set to `value`, or removed when `value` is null/undefined. */
export function withEntry<V>(map: Record<string, V>, id: string, value: V | null | undefined): Record<string, V> {
  const next = { ...map };
  if (value == null) delete next[id];
  else next[id] = value;
  return next;
}

const withMember = (list: string[], id: string, on: boolean) =>
  on ? [...list.filter((a) => a !== id), id] : list.filter((a) => a !== id);

export const setEnabled = (s: AppSettings, id: string, on: boolean) =>
  updateSettings({ enabledAccounts: withMember(s.enabledAccounts, id, on) });

export const setDetailedCard = (s: AppSettings, id: string, on: boolean) =>
  updateSettings({ detailedCards: withMember(s.detailedCards, id, on) });

export const setSplit = (s: AppSettings, id: string, on: boolean) =>
  updateSettings({ splitAccounts: withMember(s.splitAccounts, id, on) });

export const setPinnedWindow = (s: AppSettings, id: string, window: string | null) =>
  updateSettings({ pinnedWindows: withEntry(s.pinnedWindows, id, window) });

export const setRingTint = (s: AppSettings, id: string, hex: string | null) =>
  updateSettings({ ringTints: withEntry(s.ringTints, id, hex) });

/** A route the provider does not offer, or "automatic", is stored as an absence (upstream `setSource`). */
export const setSource = (s: AppSettings, id: string, source: string) =>
  updateSettings({ sources: withEntry(s.sources, id, source === "automatic" ? null : source) });

export const setSessionBrowser = (s: AppSettings, id: string, browser: string | null) =>
  updateSettings({ sessionBrowsers: withEntry(s.sessionBrowsers, id, browser) });

/** Blank removes the entry rather than storing an empty string. */
export const setServerAddress = (s: AppSettings, id: string, address: string) =>
  updateSettings({ serverAddresses: withEntry(s.serverAddresses, id, address.trim() || null) });

export const setBalanceBasis = (s: AppSettings, id: string, basis: string) =>
  updateSettings({ balanceBases: withEntry(s.balanceBases, id, basis === "sinceTopUp" ? null : basis) });

export const setBalanceBudget = (s: AppSettings, id: string, budget: number | null) =>
  updateSettings({ balanceBudgets: withEntry(s.balanceBudgets, id, budget) });

export const setLowBalanceAlert = (s: AppSettings, id: string, amount: number | null) =>
  updateSettings({ lowBalanceAlerts: withEntry(s.lowBalanceAlerts, id, amount) });

export const renameAccount = (s: AppSettings, id: string, name: string) =>
  updateSettings({ extraAccounts: s.extraAccounts.map((a) => (a.id === id ? { ...a, name } : a)) });

/**
 * Forgets an added account and everything stored against it (upstream `removeAccount`): a later account
 * must never inherit a removed one's pinned window, route or colour. Add every new per-account store here.
 */
export async function removeAccount(s: AppSettings, id: string, provider: string) {
  await invoke("set_secret", { id, value: null });
  // The set refuses to go empty upstream; if this was the only account on the rail the provider's own
  // first account takes its place, so the rail is never left naming an account that is gone.
  const remaining = s.enabledAccounts.filter((a) => a !== id);
  await updateSettings({
    extraAccounts: s.extraAccounts.filter((a) => a.id !== id),
    enabledAccounts: remaining.length ? remaining : [provider],
    providerOrder: s.providerOrder.filter((a) => a !== id),
    pinnedWindows: withEntry(s.pinnedWindows, id, null),
    sources: withEntry(s.sources, id, null),
    ringTints: withEntry(s.ringTints, id, null),
    sessionBrowsers: withEntry(s.sessionBrowsers, id, null),
    serverAddresses: withEntry(s.serverAddresses, id, null),
    lowBalanceAlerts: withEntry(s.lowBalanceAlerts, id, null),
    balanceBases: withEntry(s.balanceBases, id, null),
    balanceBudgets: withEntry(s.balanceBudgets, id, null),
    detailedCards: s.detailedCards.filter((a) => a !== id),
    splitAccounts: s.splitAccounts.filter((a) => a !== id),
    trayAccount: s.trayAccount === id ? null : s.trayAccount,
  });
}

/**
 * A figure typed into a settings field, or null for anything that is not one (upstream `money`): digits, `.`
 * and `,` only; a comma decimal separator is read; the currency symbol somebody types out of habit is
 * ignored; infinities and non-positive numbers are refused, because an infinite denominator made the ring NaN.
 */
export function parseMoney(typed: string): number | null {
  const kept = typed.replace(/[^0-9.,-]/g, "");
  if (!kept) return null;
  // "1,5" is one and a half; "1,234.50" and "1.234,50" take the last separator as the decimal one.
  const lastComma = kept.lastIndexOf(",");
  const lastDot = kept.lastIndexOf(".");
  let text = kept;
  if (lastComma >= 0 && lastDot >= 0) {
    const decimal = Math.max(lastComma, lastDot);
    text = kept.slice(0, decimal).replace(/[.,]/g, "") + "." + kept.slice(decimal + 1);
  } else if (lastComma >= 0) {
    // Several commas are thousands separators; one is a decimal comma.
    text = (kept.match(/,/g)?.length ?? 0) > 1 ? kept.replace(/,/g, "") : kept.replace(",", ".");
  }
  const value = Number(text);
  return Number.isFinite(value) && value > 0 ? value : null;
}

/** The stored figure back in the field, without a trailing `.0`. */
export const moneyText = (amount: number | null | undefined): string =>
  amount == null ? "" : String(Math.round(amount * 100) / 100);

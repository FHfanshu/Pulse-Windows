// Mirror of crates/pulse-core/src/model.rs (serde camelCase). Rules ported from
// upstream Usage/ProviderUsage.swift.

export interface AccountKey {
  provider: string;
  slot: string;
}

export type WindowKind =
  | { type: "fiveHour" | "weekly" | "spend" | "balance" | "daily" | "messages" | "monthly" | "topUp" | "credits" | "sharedCredits" }
  | { type: "other"; seconds: number };

export type Estimate = "planPrice" | "sinceTopUp" | "yourBudget";

export interface UsageWindow {
  id: string;
  kind: WindowKind;
  scope: string | null;
  usedFraction: number;
  windowSeconds: number;
  resetsAt: string | null;
  reportsLength: boolean;
  estimate: Estimate | null;
  isExhausted: boolean;
  nextExpiry: { amount: number; at: string } | null;
  label: string | null;
}

export type UsageState = { kind: "live" } | { kind: "stale" } | { kind: "unavailable"; reason: string };

export interface ProviderUsage {
  account: AccountKey;
  windows: UsageWindow[];
  observedAt: string | null;
  state: UsageState;
  plan: string | null;
  creditBalance: string | null;
  creditRemaining: { amount: number; currency: string } | null;
  origin: string | null;
  isCached: boolean;
}

export const accountId = (a: AccountKey) => (a.slot ? `${a.provider}#${a.slot}` : a.provider);

/** Whole percentage that never rounds "some" to 0 or "not all" to 100. */
export function figure(fraction: number): number {
  if (!Number.isFinite(fraction)) return 0;
  const percent = Math.min(Math.max(fraction, 0), 1) * 100;
  if (percent <= 0) return 0;
  if (percent >= 100) return 100;
  return Math.min(Math.max(Math.round(percent), 1), 99);
}

export const remainingFraction = (w: UsageWindow) => Math.min(Math.max(1 - w.usedFraction, 0), 1);
export const percentText = (w: UsageWindow, remaining = false) =>
  `${figure(remaining ? remainingFraction(w) : w.usedFraction)}%`;
export const isSpent = (w: UsageWindow | null | undefined) => !!w && (w.isExhausted || w.usedFraction >= 1);

function maxByUsed(ws: UsageWindow[]): UsageWindow | null {
  let best: UsageWindow | null = null;
  for (const w of ws) if (!best || w.usedFraction > best.usedFraction) best = w;
  return best;
}

export function headlineWindow(u: ProviderUsage, preferring?: string | null): UsageWindow | null {
  if (preferring) {
    const pinned = u.windows.find((w) => w.id === preferring);
    if (pinned) return pinned;
  }
  return maxByUsed(u.windows);
}

export function secondWindow(u: ProviderUsage, preferring?: string | null): UsageWindow | null {
  const headline = headlineWindow(u, preferring);
  if (!headline || u.windows.length < 2) return null;
  const rest = u.windows.filter((w) => w.id !== headline.id);
  const same = rest.filter((w) => w.scope === headline.scope);
  return maxByUsed(same.length ? same : rest);
}

export function elapsedFraction(w: UsageWindow, now = Date.now()): number | null {
  if (!w.reportsLength || !w.resetsAt || w.windowSeconds <= 0) return null;
  const remaining = (Date.parse(w.resetsAt) - now) / 1000;
  return Math.min(Math.max(1 - remaining / w.windowSeconds, 0), 1);
}

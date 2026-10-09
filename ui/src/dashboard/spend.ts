// Ported from upstream MenuDashboardModel (ledgers, ledgerReadAt, readingLedger): what an account tab's spend
// section has read. Lives outside React so a read started by a tab that is then left still lands.
import { invoke } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";

export interface SpendFigure {
  tokens: number;
  /** None where none of the work had a price. */
  cost: number | null;
}

export interface DashboardSpend {
  today: SpendFigure;
  busiest: SpendFigure;
  recent: SpendFigure;
  allTime: SpendFigure;
  /** The last 31 days, oldest first; empty for a history of a single day. */
  bars: { date: string; tokens: number }[];
  currency: string | null;
}

interface Entry {
  /** The last figures, kept on screen while a new read runs. */
  spend: DashboardSpend | null;
  reading: boolean;
  readAt: number | null;
}

/** Long enough that flicking between tabs does not rescan, short enough that "Today" is today's. */
const LIFETIME_MS = 5 * 60 * 1000;
const EMPTY: Entry = { spend: null, reading: false, readAt: null };

let entries = new Map<string, Entry>();
const listeners = new Set<() => void>();

function patch(provider: string, change: Partial<Entry>) {
  entries = new Map(entries).set(provider, { ...(entries.get(provider) ?? EMPTY), ...change });
  listeners.forEach((l) => l());
}

/** Reads the provider's ledger unless it was read in the last few minutes. */
export function readSpend(provider: string) {
  const entry = entries.get(provider) ?? EMPTY;
  if (entry.reading) return;
  if (entry.readAt !== null && Date.now() - entry.readAt < LIFETIME_MS) return;
  patch(provider, { reading: true });
  invoke<DashboardSpend | null>("dashboard_spend", { provider })
    .then((spend) => patch(provider, { spend, reading: false, readAt: Date.now() }))
    .catch(() => patch(provider, { reading: false, readAt: Date.now() }));
}

/** Forget everything read, e.g. once Token spend is switched off. */
export function forgetSpend() {
  entries = new Map();
  listeners.forEach((l) => l());
}

export function useSpend(provider: string): Entry {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => void listeners.delete(l);
    },
    () => entries.get(provider) ?? EMPTY,
  );
}

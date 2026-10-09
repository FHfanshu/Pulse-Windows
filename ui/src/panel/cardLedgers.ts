// Ported from upstream Panel/CardLedgers.swift: the detailed card's history for a provider, read when a card
// that shows one opens. Kept for the session and read again once `lifetime` has passed, so sweeping the rail
// does not rescan a store at every ring.
//
// Lives outside React on purpose. A read started from a component's effect is cancelled by the next ring's
// card and would come back as "no history"; here the request always runs to the end and lands in the store.
import { useEffect, useState, useSyncExternalStore } from "react";
import { cardSpend, estimatedValues, promptCache, type CardSpend, type PromptCacheReading, type WindowValue } from "../shared/spend";

/** Providers whose records the card reads (`spend::supports`): Claude Code's transcripts and Codex's rollouts. */
export const spendProviders: ReadonlySet<string> = new Set(["claudeCode", "codex"]);

/** Long enough that moving between rings does not rescan, short enough that "Today" is today's. */
export const LIFETIME_MS = 5 * 60 * 1000;

export interface CardLedger {
  /** The last figures, kept while a new read runs so the section does not blank out and come back. */
  spend: CardSpend | null;
  reading: boolean;
  /** Asked and not answered. */
  failed: boolean;
  readAt: number | null;
  /** When each live session's prompt cache lapses, read on every opening. */
  promptCache: PromptCacheReading | null;
}

const EMPTY: CardLedger = { spend: null, reading: false, failed: false, readAt: null, promptCache: null };
let entries = new Map<string, CardLedger>();
/** Bumped by a reset, so an answer to a question asked before it is dropped. */
let generation = 0;
const listeners = new Set<() => void>();

function patch(provider: string, change: Partial<CardLedger>) {
  entries = new Map(entries).set(provider, { ...(entries.get(provider) ?? EMPTY), ...change });
  listeners.forEach((l) => l());
}

/** Reads the provider's history unless it was read in the last few minutes. */
export function readCardSpend(provider: string) {
  const entry = entries.get(provider) ?? EMPTY;
  if (entry.reading) return;
  if (entry.readAt !== null && Date.now() - entry.readAt < LIFETIME_MS) return;
  patch(provider, { reading: true });
  const asked = generation;
  cardSpend(provider)
    .then((spend) => asked === generation && patch(provider, { spend, failed: false, reading: false, readAt: Date.now() }))
    .catch(() => asked === generation && patch(provider, { failed: true, reading: false, readAt: Date.now() }));
}

/** The latest session's cache: a directory listing and the tail of one file, cheap enough for every opening. */
export function readPromptCache(provider: string) {
  promptCache(provider)
    .then((reading) => patch(provider, { promptCache: reading }))
    .catch(() => {});
}

/**
 * What each limit of an account is worth, for the detailed card's "Estimated value" line. Worked from the
 * reading's percentages and the moment they were read, so it is asked once per reading and kept: sweeping the
 * rail past a ring again does not scan its records again.
 */
const valueAnswers = new Map<string, WindowValue[]>();
const valueAsked = new Set<string>();
const valueListeners = new Set<() => void>();

export function useEstimatedValues(account: string, reading: string, enabled: boolean): WindowValue[] {
  const key = `${account}|${reading}`;
  const [, bump] = useState(0);
  useEffect(() => {
    const listener = () => bump((n) => n + 1);
    valueListeners.add(listener);
    return () => void valueListeners.delete(listener);
  }, []);
  useEffect(() => {
    if (!enabled || valueAnswers.has(key) || valueAsked.has(key)) return;
    valueAsked.add(key);
    const asked = generation;
    estimatedValues(account, true)
      .then((values) => {
        if (asked !== generation) return;
        valueAnswers.set(key, values);
        valueListeners.forEach((l) => l());
      })
      .catch(() => valueAsked.delete(key));
  }, [account, key, enabled]);
  return enabled ? valueAnswers.get(key) ?? [] : [];
}

/** Forget everything, for when Token spend is switched: the old answers were for the other setting. */
export function resetCardLedgers() {
  entries = new Map();
  valueAnswers.clear();
  valueAsked.clear();
  generation += 1;
  listeners.forEach((l) => l());
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => void listeners.delete(listener);
};

export function useCardLedger(provider: string): CardLedger {
  return useSyncExternalStore(subscribe, () => entries.get(provider) ?? EMPTY);
}

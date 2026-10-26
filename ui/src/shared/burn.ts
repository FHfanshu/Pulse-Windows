// Ported from upstream Usage/BurnRate.swift (also in Rust as pulse-core `spend::burn`; the card works it out at
// the moment it is drawn, as upstream does, so it uses the reading and the clock it has).
// How a limit is being spent against its own window: whether it will last, and, only when the evidence carries
// it, when it runs out. Everything comes from one reading: how much is gone and how far through the window the
// clock is. No history is kept.
import { t } from "./i18n";
import { elapsedFraction, type UsageWindow } from "./model";
import { intlLocale } from "./spend";

/** Nothing is said about a window barely open: a single burst early on reads as a rate that empties it by lunch. */
export const MINIMUM_ELAPSED = 0.03;
/** A prediction further out than this (seconds) is not shown: past two hours it is not actionable. */
export const HORIZON = 2 * 3600;

export interface BurnReading {
  /** Whether it is on course to run out before the window resets. */
  exhaustsBeforeReset: boolean;
  /** Seconds until it runs out: only when that is before the reset and inside the horizon. Null far more often than not. */
  timeToExhaustion: number | null;
}

/** What may be said about a window, or null when nothing may be (upstream `BurnRate.reading(for:now:)`). */
export function burnReading(w: UsageWindow, now = Date.now()): BurnReading | null {
  // `elapsedFraction` already refuses a window whose length was chosen to sort by rather than reported.
  const elapsed = elapsedFraction(w, now);
  if (elapsed == null || elapsed < MINIMUM_ELAPSED || !w.resetsAt) return null;
  const used = Math.min(Math.max(w.usedFraction, 0), 1);
  const untilReset = (Date.parse(w.resetsAt) - now) / 1000;
  if (!(untilReset > 0)) return null;
  // The window's own length, from the two figures that describe it.
  const elapsedSeconds = (untilReset / (1 - elapsed)) * elapsed;
  if (!(elapsedSeconds > 0) || !Number.isFinite(elapsedSeconds)) return null;
  const perHour = (used / elapsedSeconds) * 3600;
  if (used >= 1 || perHour <= 0) return { exhaustsBeforeReset: used >= 1, timeToExhaustion: used >= 1 ? 0 : null };
  const untilEmpty = ((1 - used) / perHour) * 3600;
  const first = untilEmpty < untilReset;
  // Both filters: if the window resets first there is nothing to predict, and past two hours it is a guess.
  return { exhaustsBeforeReset: first, timeToExhaustion: first && untilEmpty <= HORIZON ? untilEmpty : null };
}

/** "about an hour", "about 45 minutes": rounded, because the precision is not real (upstream `BurnRate.approximate`). */
export function approximateDuration(seconds: number): string {
  const minutes = Math.round(seconds / 60);
  if (minutes < 15) return t("under 15 minutes");
  // Under an hour in quarters; past that in halves of an hour. The cut is at 68: five quarters is an hour and a quarter.
  if (minutes < 68) {
    const quarters = Math.max(Math.round(minutes / 15), 1);
    return quarters === 4 ? t("about an hour") : t("about %@ minutes", quarters * 15);
  }
  const hours = Math.round((minutes / 60) * 2) / 2;
  // The reader's chosen language, as every other number: a comma-decimal locale prints "about 1,5 hours".
  return t("about %@ hours", new Intl.NumberFormat(intlLocale(), { maximumFractionDigits: 1 }).format(hours));
}

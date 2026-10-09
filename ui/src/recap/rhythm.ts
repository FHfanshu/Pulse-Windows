// Ported from upstream Usage/Recap.swift (`Recap.busiestHours`).
import type { Report } from "./types";

/**
 * The four hours in a row, wrapping past midnight, that held the most of the period's tokens:
 * where this person's day actually bunched. One band drives the clock's highlight, the sentence
 * beneath it and the timetable's figure.
 *
 * It replaced a fixed 21:00-04:59 band, which read as a claim about when *you* worked ("8% ...
 * between 9 PM and 5 AM") while saying the same hours for everyone. `lateShare` still measures
 * that band for `--recap`.
 */
export interface RhythmBand {
  /** The first hour, 0..<24. */
  from: number;
  length: number;
  /** Of the period's hour tokens, 0...1. */
  share: number;
}

export const BAND_LENGTH = 4;

export function hourInBand(hour: number, band: RhythmBand): boolean {
  return (((hour - band.from) % 24) + 24) % 24 < band.length;
}

/** The hour the band stops at, exclusive: 10 for one starting at 6. */
export const bandEnd = (band: RhythmBand) => (band.from + band.length) % 24;

/** The earliest start among equals, so a tie reads the same twice: the rule `peakHour` follows.
 *  Null without 24 hours or without any work. */
export function rhythmBand(recap: Pick<Report, "hours">): RhythmBand | null {
  const hours = recap.hours;
  if (!hours || hours.length !== 24) return null;
  const total = hours.reduce((a, b) => a + b, 0);
  if (total <= 0) return null;
  let best = 0;
  let bestTokens = -1;
  for (let start = 0; start < 24; start++) {
    let tokens = 0;
    for (let i = 0; i < BAND_LENGTH; i++) tokens += hours[(start + i) % 24];
    if (tokens > bestTokens) {
      best = start;
      bestTokens = tokens;
    }
  }
  return { from: best, length: BAND_LENGTH, share: bestTokens / total };
}

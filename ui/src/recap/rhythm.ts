import type { Report } from "./types";

/** One band drives both the clock's highlight and the sentence beneath it. */
export interface RhythmBand {
  from: number;
  length: number;
  share: number;
}

export function hourInBand(hour: number, band: RhythmBand): boolean {
  return (hour - band.from + 24) % 24 < band.length;
}

export function rhythmBand(recap: Pick<Report, "hours" | "persona">): RhythmBand | null {
  const hours = recap.hours;
  if (!hours || hours.length !== 24) return null;
  const total = hours.reduce((a, b) => a + b, 0);
  if (total <= 0) return null;
  const share = (from: number, length: number) => {
    let sum = 0;
    for (let i = 0; i < length; i++) sum += hours[(from + i) % 24];
    return sum / total;
  };
  let from: number;
  let length: number;
  switch (recap.persona) {
    case "nightOwl": from = 21; length = 8; break;
    case "earlyBird": from = 5; length = 5; break;
    case "dayShift": from = 10; length = 8; break;
    default:
      from = 0;
      length = 4;
      for (let h = 1; h < 24; h++) if (share(h, length) > share(from, length)) from = h;
  }
  return { from, length, share: share(from, length) };
}

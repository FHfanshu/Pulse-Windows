// The monthly subscription price typed into the recap window (upstream RecapPrice; the Rust twin
// in crates/pulse-core/src/recap/periods.rs carries the tests).
//
// Strict, and no locale guessing: after trimming whitespace and one leading "$", the text is
// either plain digits with an optional "." or "," and one or two decimals ("20", "12.5", "12,5" is
// 12.5), or digits grouped by "," in valid groups of three with an optional ".dd" ("1,200",
// "1,200.50"). Anything else is refused. Empty clears the price, and so does 0.

/** A month's price in US dollars. */
export const MAXIMUM_PRICE = 10_000;

export type PriceEntry = { kind: "none" } | { kind: "amount"; value: number } | { kind: "refused" };

const digits = (text: string) => /^[0-9]+$/.test(text);

function plain(text: string): string | null {
  const at = text.search(/[.,]/);
  if (at < 0) return digits(text) ? text : null;
  const whole = text.slice(0, at);
  const decimals = text.slice(at + 1);
  return digits(whole) && digits(decimals) && decimals.length <= 2 ? `${whole}.${decimals}` : null;
}

function grouped(text: string): string | null {
  const [integer, decimals, extra] = text.split(".");
  if (extra !== undefined) return null;
  const groups = integer.split(",");
  if (groups.length < 2 || groups[0].length === 0 || groups[0].length > 3 || !groups.every(digits)) return null;
  if (groups.slice(1).some((g) => g.length !== 3)) return null;
  let number = groups.join("");
  if (decimals !== undefined) {
    if (!digits(decimals) || decimals.length > 2) return null;
    number += `.${decimals}`;
  }
  return number;
}

export function priceEntry(typed: string): PriceEntry {
  let text = typed.trim();
  if (!text) return { kind: "none" };
  if (text.startsWith("$")) text = text.slice(1).trim();
  const number = plain(text) ?? grouped(text);
  if (number === null) return { kind: "refused" };
  const value = Number(number);
  if (!Number.isFinite(value) || value > MAXIMUM_PRICE) return { kind: "refused" };
  return value === 0 ? { kind: "none" } : { kind: "amount", value };
}

/** The figure as the field shows it: no grouping, at most two decimals. */
export function priceText(amount: number | null | undefined): string {
  if (amount === null || amount === undefined || !Number.isFinite(amount) || amount <= 0 || amount > MAXIMUM_PRICE) return "";
  return String(Math.round(amount * 100) / 100);
}

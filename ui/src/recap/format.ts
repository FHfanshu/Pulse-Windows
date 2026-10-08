// Numbers, money, hours and dates as the recap cards print them (upstream RecapFormat and
// RecapWords). Everything speaks the app's language; the calendar is Gregorian with weeks from
// Monday, so what a card prints and what Rust counted never differ.
import { locale, t } from "../shared/i18n";

export interface Figure {
  number: string;
  unit: string;
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/** The BCP-47 tag Intl should speak. */
function tag(): string {
  return locale();
}

function language(): string {
  return tag().split("-")[0];
}

// A "YYYY-MM-DD" date at noon UTC, so no zone moves it to another day.
function utc(date: string): Date {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d, 12));
}

function formatted(date: Date, options: Intl.DateTimeFormatOptions): string {
  return new Intl.DateTimeFormat(tag(), { ...options, timeZone: "UTC", calendar: "gregory" } as Intl.DateTimeFormatOptions).format(date);
}

// MARK: Tokens

/** 万/亿 in Simplified Chinese, 萬/億 in Traditional Chinese, 万/億 in Japanese, 만/억 in Korean. */
function myriadUnits(): [string, string] | null {
  switch (tag()) {
    case "zh-Hans":
      return ["万", "亿"];
    case "zh-Hant":
      return ["萬", "億"];
    case "ja":
      return ["万", "億"];
    case "ko":
      return ["만", "억"];
    default:
      return null;
  }
}

/** "4.10" and "4.00" both read as a mistake; trim what adds nothing. */
function trimmedFixed(value: number, decimals: number): string {
  const text = value.toFixed(decimals);
  return text.includes(".") ? text.replace(/0+$/, "").replace(/\.$/, "") : text;
}

/** "5.9" "B", "4.19" "亿", "812" "": the one rule for where 万 and 亿 begin. */
export function tokenParts(tokens: number, units: [string, string] | null): Figure {
  if (units) {
    if (tokens >= 100_000_000) {
      const scaled = tokens / 100_000_000;
      return { number: trimmedFixed(scaled, scaled < 10 ? 2 : scaled < 100 ? 1 : 0), unit: units[1] };
    }
    if (tokens >= 10_000) {
      const scaled = tokens / 10_000;
      return { number: trimmedFixed(scaled, scaled < 10 ? 1 : 0), unit: units[0] };
    }
    return { number: String(tokens), unit: "" };
  }
  if (tokens >= 1e9) return { number: trimmedFixed(tokens / 1e9, tokens < 1e10 ? 1 : 0), unit: "B" };
  if (tokens >= 1e6) return { number: trimmedFixed(tokens / 1e6, tokens < 1e7 ? 1 : 0), unit: "M" };
  if (tokens >= 1e3) return { number: trimmedFixed(tokens / 1e3, tokens < 1e4 ? 1 : 0), unit: "K" };
  return { number: String(tokens), unit: "" };
}

/** "4764" -> "4,764", "8.4" -> "8.4". */
function separated(number: string): string {
  const [head, fraction] = number.split(".");
  if (head.length <= 3) return number;
  const grouped = head.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return fraction === undefined ? grouped : `${grouped}.${fraction}`;
}

export function tokens(count: number): Figure {
  const parts = tokenParts(Math.round(count), myriadUnits());
  return { number: separated(parts.number), unit: parts.unit };
}

export const figureText = (figure: Figure) => figure.number + figure.unit;

// MARK: Money

function currencyFormat(currency: string, digits: number): Intl.NumberFormat {
  return new Intl.NumberFormat(currency === "USD" ? "en-US" : tag(), {
    style: "currency",
    currency,
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

/** Whole units from ten up ("$1,342", "$47"), cents below it ("$3.40"), and "< $0.01" for a
 *  positive amount under a cent, so a sliver of spend is never "$0". */
export function money(amount: number, currency: string): string {
  if (amount > 0 && amount < 0.01) return t("< %@", currencyFormat(currency, 2).format(0.01));
  return currencyFormat(currency, amount >= 10 ? 0 : 2).format(amount);
}

/** A ruler label: whole units unless the step between labels is under one. */
export function axisMoney(amount: number, step: number, currency: string): string {
  return currencyFormat(currency, step < 1 ? 2 : 0).format(amount);
}

// MARK: Ratios

/** "41%", and "<1%" for a share that is there but would round to nothing. */
export function percent(share: number): string {
  const rounded = Math.round(share * 100);
  if (rounded === 0 && share > 0) return "<1%";
  return `${rounded}%`;
}

/** The same share split for a figure with its sign set small: ("41", "%"), ("<1", "%"). */
export function percentFigure(share: number): Figure {
  return { number: percent(share).slice(0, -1), unit: "%" };
}

/** "6.7" under ten, "42" from ten up, "<0.1" for a positive multiple that would round to "0.0". */
export function multiple(value: number): string {
  if (value > 0 && value < 0.05) return "<0.1";
  return value < 10 ? value.toFixed(1) : value.toFixed(0);
}

// MARK: Hours and clock times

/** Only English counts the day in twelves, and only where its locale does. */
function usesTwelveHourClock(): boolean {
  if (language() !== "en") return false;
  return new Intl.DateTimeFormat(tag(), { hour: "numeric" }).resolvedOptions().hour12 === true;
}

/** One hour of the day: ("23:00", "") or ("11", "PM"). */
export function hour(h: number): Figure {
  if (!usesTwelveHourClock()) return { number: `${pad2(h)}:00`, unit: "" };
  return { number: String(h % 12 === 0 ? 12 : h % 12), unit: h < 12 ? "AM" : "PM" };
}

/** The same hour on one line, for rows: "23:00", "11 PM". */
export function hourLabel(h: number): string {
  const figure = hour(h);
  return figure.unit ? `${figure.number} ${figure.unit}` : figure.number;
}

/** Minutes after midnight as a time of day, on the same clock as `hour`. */
export function clockTime(minutes: number): string {
  const wrapped = ((minutes % 1440) + 1440) % 1440;
  if (!usesTwelveHourClock()) return `${pad2(Math.floor(wrapped / 60))}:${pad2(wrapped % 60)}`;
  return formatted(new Date(Date.UTC(1970, 0, 1, 0, wrapped)), { hour: "numeric", minute: "2-digit" }).replace(/ /g, " ");
}

/** 21:00 to 04:59, the same band the report's late share measures. */
export const isLate = (h: number) => h >= 21 || h < 5;

// MARK: Dates

/** "September", "九月", "9月", "9월". */
export function monthName(month: number): string {
  return formatted(new Date(Date.UTC(2026, month - 1, 15)), { month: "long" });
}

/** "Sep", "9月", "9월": the short month for the twelve-bar chart. */
export function shortMonthName(month: number): string {
  return formatted(new Date(Date.UTC(2026, month - 1, 15)), { month: "short" });
}

// 2024-01-01 is a Monday.
const mondayFirst = (index: number) => new Date(Date.UTC(2024, 0, 1 + index, 12));

/** Weekday headings, Monday first: one character where the language has one ("一", "月", "월"),
 *  the short name elsewhere (because "T" and "T" are Tuesday and Thursday). */
export function weekdayHeadings(): string[] {
  const compact = ["zh", "ja", "ko"].includes(language());
  return Array.from({ length: 7 }, (_, i) => formatted(mondayFirst(i), { weekday: compact ? "narrow" : "short" }));
}

/** The weekdays' short names, Monday first: "Mon", "周一", "月", "월". */
export function weekdayNames(): string[] {
  return Array.from({ length: 7 }, (_, i) => formatted(mondayFirst(i), { weekday: "short" }));
}

export function weekdayName(date: string): string {
  return formatted(utc(date), { weekday: "short" });
}

/** "2026", "2026年", "2026년": how the language says which year. */
export function yearName(year: number): string {
  return formatted(new Date(Date.UTC(year, 5, 15)), { year: "numeric" });
}

/** "September 2026", "2026年9月". */
export function monthYear(date: string): string {
  return formatted(utc(date), { year: "numeric", month: "long" });
}

/** "Thu, Sep 17", "9月17日周四". */
export function dayWithWeekday(date: string): string {
  return formatted(utc(date), { month: "short", day: "numeric", weekday: "short" });
}

/** "9/21": month and day with no name, for a range or a chain. */
export function shortDay(date: string): string {
  return formatted(utc(date), { month: "numeric", day: "numeric" });
}

/** "9/21-9/27", and a single day alone. */
export function dayRange(first: string, last: string): string {
  const start = shortDay(first);
  return first === last ? start : `${start}–${shortDay(last)}`;
}

/** "Sep 17", "9月17日". */
export function day(date: string): string {
  return formatted(utc(date), { month: "short", day: "numeric" });
}

/** "Sep 17 · Thu". */
export function dayAndWeekday(date: string): string {
  return `${day(date)} · ${weekdayName(date)}`;
}

export function dayOfMonth(date: string): number {
  return Number(date.slice(8, 10));
}

export function monthOf(date: string): number {
  return Number(date.slice(5, 7));
}

// MARK: Words (the choice between a singular and a plural key is made here, outside the key)

export const Words = {
  days: (n: number) => (n === 1 ? t("1 day") : t("%@ days", n)),
  daysUnit: (n: number) => (n === 1 ? t("day") : t("days")),
  sessions: (n: number) => (n === 1 ? t("1 session") : t("%@ sessions", n)),
  agents: (n: number) => (n === 1 ? t("1 agent") : t("%@ agents", n)),
  activeDays: (n: number) => (n === 1 ? t("1 active day") : t("%@ active days", n)),
  toolsUnit: (n: number) => (n === 1 ? t("tool") : t("tools")),
  nightsPastMidnight: (n: number) => (n === 1 ? t("One night ran past midnight") : t("%@ nights ran past midnight", n)),
};

// MARK: Emphasis (a figure inside a sentence is marked, not split)

export const EMPHASIS_OPEN = "";
export const EMPHASIS_CLOSE = "";

export const mark = (value: string) => EMPHASIS_OPEN + value + EMPHASIS_CLOSE;

export function segments(text: string): { text: string; emphasised: boolean }[] {
  const out: { text: string; emphasised: boolean }[] = [];
  let buffer = "";
  let emphasised = false;
  for (const ch of text) {
    if (ch === EMPHASIS_OPEN) {
      if (buffer) out.push({ text: buffer, emphasised: false });
      buffer = "";
      emphasised = true;
    } else if (ch === EMPHASIS_CLOSE) {
      if (buffer) out.push({ text: buffer, emphasised: true });
      buffer = "";
      emphasised = false;
    } else {
      buffer += ch;
    }
  }
  if (buffer) out.push({ text: buffer, emphasised });
  return out;
}

export const plain = (text: string) => text.split(EMPHASIS_OPEN).join("").split(EMPHASIS_CLOSE).join("");

export { pad2 };

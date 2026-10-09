// The cards a report gets, in order, and the wording the cards share (upstream RecapDeck,
// RecapCopy and RecapScoreData). The rules that need no price are Rust's (`report.deck`); what is
// here joins them to the reader's price and the privacy switch, both of which change live.
import { t } from "../shared/i18n";
import * as F from "./format";
import type { CardId, Persona, Report, ScoreBar } from "./types";

export interface Payback {
  used: number;
  monthlyPrice: number;
  /** Months of subscription the period covers; prorated by days while it runs. */
  months: number;
  isToDate: boolean;
  paid: number;
  multiple: number;
}

export interface Change {
  arrow: string;
  percent: string;
  versus: string;
}

export interface Deck {
  recap: Report;
  monthlyPrice: number | null;
  hidesProjects: boolean;
  payback: Payback | null;
  cards: CardId[];
  stories: CardId[];
  isYear: boolean;
  page(card: CardId): { number: number; count: number } | null;
  projectName(index: number, name: string): string;
  periodName: string;
  periodLabel: string;
  previousName: string;
  nextName: string;
  workedThroughLine: string;
  activeDaysLine: string;
  change: Change | null;
  tokensPerDay: number | null;
  costSeries: number[];
  streak: { days: number; isCurrent: boolean } | null;
  provenance: string[];
  costIsFloor: boolean;
  cacheSavings: number | null;
  scoreStamp: string;
  lastDay: string | null;
  sessionsPerActiveDay: number | null;
  scoreBars: ScoreBar[];
  scoreAxis: { index: number; label: string }[];
  scoreBusiestNote: string | null;
}

export function personaTitle(persona: Persona): string {
  switch (persona) {
    case "nightOwl":
      return t("Night owl");
    case "earlyBird":
      return t("Early bird");
    case "dayShift":
      return t("Day shift");
    case "allDay":
      return t("All day");
  }
}

export function periodTitle(key: string): string {
  const [year, month] = key.split("-").map(Number);
  return month ? F.monthYear(`${key}-01`) : F.yearName(year);
}

export function makeDeck(recap: Report, monthlyPrice: number | null, hidesProjects: boolean): Deck {
  const isYear = !recap.period.includes("-");
  const year = Number(recap.period.slice(0, 4));
  const month = isYear ? 0 : Number(recap.period.slice(5, 7));

  const price = monthlyPrice && monthlyPrice > 0 ? monthlyPrice : null;
  const months = recap.deck.paybackMonths;
  const payback: Payback | null =
    price && months && recap.cost && recap.cost > 0
      ? {
          used: recap.cost,
          monthlyPrice: price,
          months,
          isToDate: recap.isInProgress,
          paid: price * months,
          multiple: recap.cost / (price * months),
        }
      : null;
  const cards = payback ? recap.deck.cardsWithPayback : recap.deck.cards;
  const stories: CardId[] = cards.filter((c) => c !== "poster");

  const periodName = isYear ? F.yearName(year) : F.monthName(month);
  const previousName = isYear ? F.yearName(year - 1) : F.monthName(month === 1 ? 12 : month - 1);
  const nextName = isYear ? F.yearName(year + 1) : F.monthName(month === 12 ? 1 : month + 1);

  const lastDay = recap.days.length ? recap.days[recap.days.length - 1].date : null;

  let change: Change | null = null;
  if (recap.previousTokens && recap.previousTokens > 0) {
    const delta = Math.round((recap.tokens / recap.previousTokens - 1) * 100);
    change = {
      arrow: delta > 0 ? "↑" : delta < 0 ? "↓" : "→",
      percent: `${Math.abs(delta)}%`,
      versus: t("vs same period in %@", previousName),
    };
  }

  const provenance = [
    recap.cost !== undefined
      ? t("Counted from this Mac's local records · money estimated at API prices")
      : t("Counted from this Mac's local records"),
  ];
  if (recap.isInProgress) provenance.push(t("Figures are to date."));
  if (recap.recordsBegin) provenance.push(t("Records on this Mac begin on %@; days before it are not counted.", F.day(recap.recordsBegin)));
  if (recap.deck.costIsFloor) provenance.push(t("Some work had no published price, so the money is a floor."));
  if (recap.isPartial) provenance.push(t("Some tools' counts may be missing, so the total is a floor."));

  const bars = recap.deck.scoreBars;
  const scoreAxis: { index: number; label: string }[] = [];
  if (bars.length) {
    if (isYear) {
      recap.months.forEach((m, index) => scoreAxis.push({ index, label: F.shortMonthName(m.month) }));
    } else {
      [...new Set([0, 9, 19, bars.length - 1])]
        .filter((i) => i < bars.length)
        .sort((a, b) => a - b)
        .forEach((i) => scoreAxis.push({ index: i, label: String(i + 1) }));
    }
  }

  let scoreBusiestNote: string | null = null;
  if (isYear) {
    const maximum = Math.max(0, ...recap.months.map((m) => m.tokens));
    const busiest = maximum > 0 ? recap.months.find((m) => m.tokens === maximum) : undefined;
    if (busiest) scoreBusiestNote = t("Busiest %@ · %@", F.monthName(busiest.month), F.figureText(F.tokens(busiest.tokens)));
  } else if (recap.busiestDay && recap.busiestDay.tokens > 0) {
    scoreBusiestNote = t("Busiest %@ · %@", F.day(recap.busiestDay.date), F.figureText(F.tokens(recap.busiestDay.tokens)));
  }

  return {
    recap,
    monthlyPrice: price,
    hidesProjects,
    payback,
    cards,
    stories,
    isYear,
    page: (card) => {
      const index = stories.indexOf(card);
      return index < 0 ? null : { number: index + 1, count: stories.length };
    },
    projectName: (index, name) => (hidesProjects ? t("Project %@", index + 1) : name),
    periodName,
    // The label in the running head: "SEPTEMBER 2026", "2026".
    periodLabel: isYear ? String(year) : F.monthYear(recap.start).toUpperCase(),
    previousName,
    nextName,
    workedThroughLine: t("In %@, you and AI worked through", periodName),
    activeDaysLine:
      // "of 1 days" on a running month's first day: the singular is its own key.
      recap.observedDays === 1
        ? t("Active %@ of 1 day", F.mark(String(recap.activeDays)))
        : t("Active %@ of %@ days", F.mark(String(recap.activeDays)), recap.observedDays),
    change,
    // Over the days of the period Pulse could see.
    tokensPerDay: recap.observedDays > 0 ? Math.trunc(recap.tokens / recap.observedDays) : null,
    costSeries: recap.deck.costSeries,
    streak: recap.deck.streak ? { days: recap.deck.streak[0], isCurrent: recap.deck.streak[1] } : null,
    provenance,
    costIsFloor: recap.deck.costIsFloor,
    cacheSavings: recap.deck.cacheSavings ?? null,
    scoreStamp: isYear ? `NO. ${year}` : `NO. ${year}·${F.pad2(month)}`,
    lastDay,
    sessionsPerActiveDay: recap.deck.sessionsPerActiveDay ?? null,
    scoreBars: bars,
    scoreAxis,
    scoreBusiestNote,
  };
}

/** "pulse-recap-2026-09-02-opener.png": the period, the card's place in the deck, its name. */
export function fileName(card: CardId, deck: Deck): string {
  const place = Math.max(deck.cards.indexOf(card), 0) + 1;
  return `pulse-recap-${deck.recap.period}-${F.pad2(place)}-${card}.png`;
}

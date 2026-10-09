// The recap report as `recap_report` sends it (crates/pulse-core/src/recap, serde camelCase).
// Days are local calendar dates, "YYYY-MM-DD". A figure Pulse cannot stand behind is absent.

export type Persona = "nightOwl" | "earlyBird" | "dayShift" | "allDay";

/** "2026-09" for a month, "2026" for a year. */
export type PeriodKey = string;

export interface Day {
  date: string;
  tokens: number;
  cost?: number;
}

export interface MonthRow {
  /** 1...12 */
  month: number;
  tokens: number;
  cost?: number;
  activeDays: number;
}

export interface ModelShare {
  name: string;
  tokens: number;
  share: number;
  cost?: number;
}

export interface AgentShare {
  agent: string;
  name: string;
  /** Stem of the mark in assets/icons, where the agent has one. */
  icon?: string;
  tokens: number;
  share: number;
  activeDays: number;
  cost?: number;
  activeDates: string[];
}

export interface ProjectShare {
  name: string;
  tokens: number;
  share: number;
  sessions: number;
}

export type Mark = "used" | "other" | "quietWeekend" | "quiet" | "toCome";

export interface Week {
  first: string;
  last: string;
  tokens: number;
}

export interface Chain {
  days: string[];
  runStart: number;
  runEnd: number;
  first: string;
  last: string;
}

export interface Insights {
  weekdayTokens: (number | null)[];
  busiestWeekday?: number;
  workSplit?: {
    weekdayTokens: number;
    weekendTokens: number;
    weekdayDays: number;
    weekendDays: number;
    weekdayActive: number;
    weekendActive: number;
  };
  tokensPerActiveDay?: number;
  weeks: Week[];
  busiestWeek?: number;
  longestRun?: { first: string; last: string; length: number };
  streakChain?: Chain;
  firstHour?: number;
  quarters?: { from: number; tokens: number; share: number }[];
  leadingQuarter?: number;
  costPerMillion?: number;
  costPerActiveDay?: number;
  costliestDay?: Day;
  costBars?: (number | null)[];
  monthsToCome: boolean[];
  agentMarks: Mark[][];
}

export type CardId = "poster" | "opener" | "calendar" | "yearCalendar" | "months" | "timetable" | "payback" | "scorecard";

export interface ScoreBar {
  slot: "active" | "quiet" | "future";
  fraction: number;
  isBusiest: boolean;
}

export interface ScoreAgent {
  /** Absent for the grouped rest ("Other"). */
  name?: string;
  share: number;
  tone: "lime" | "ink" | "grey";
}

export interface DeckFacts {
  cards: CardId[];
  cardsWithPayback: CardId[];
  paybackMonths?: number;
  costIsFloor: boolean;
  cacheSavings?: number;
  costSeries: number[];
  streak?: [number, boolean];
  sessionsPerActiveDay?: number;
  scoreBars: ScoreBar[];
  scoreAgents: ScoreAgent[];
}

export interface Report {
  period: PeriodKey;
  start: string;
  end: string;
  isInProgress: boolean;
  tokens: number;
  cost?: number;
  unpricedTokens: number;
  previousTokens?: number;
  activeDays: number;
  elapsedDays: number;
  sessions: number;
  days: Day[];
  months: MonthRow[];
  hours?: number[];
  peakHour?: number;
  lateShare?: number;
  latestMinute?: number;
  lateNights: number;
  persona?: Persona;
  models: ModelShare[];
  agents: AgentShare[];
  projects: ProjectShare[];
  cacheHitRate?: number;
  cacheSavings?: number;
  currentStreak: number;
  longestStreak: number;
  busiestDay?: Day;
  currency: string;
  isPartial: boolean;
  /** The first day this PC has any record for, when it falls inside the period after its first day. */
  recordsBegin?: string;
  /** The period's days Pulse could have seen work on, from `recordsBegin` (or the start). */
  observedDays: number;
  insights: Insights;
  deck: DeckFacts;
}

export interface Offer {
  months: PeriodKey[];
  years: PeriodKey[];
  defaultMonth: PeriodKey;
  defaultYear: PeriodKey;
}

export interface RecapPayload {
  offer: Offer;
  report: Report;
}

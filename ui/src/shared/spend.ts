// Mirror of the Token spend IPC payloads (src-tauri/src/spend_ipc.rs and crates/pulse-core/src/spend,
// serde camelCase; chrono dates arrive as ISO strings) plus the number and money formatting the
// Token spend pane and the detailed card share (upstream TokenCount, SpendFormat, AccountUsageCard.money).
import { invoke } from "@tauri-apps/api/core";
import { locale, t } from "./i18n";

// MARK: - Payloads

/** An agent's id: the `raw()` of a source registered in crates/pulse-core/src/spend/sources (`claudeCode`, `openCode`...). */
export type SpendAgent = string;

/** What the pane needs to name and draw one agent; the list comes from Rust, so a new source needs no change here. */
export interface AgentInfo {
  id: SpendAgent;
  name: string;
  /** File stem of its mark in assets/icons, where the set has one. */
  icon: string | null;
}

/** Sparse on the wire: a kind that is zero is absent. */
export interface TokenTally {
  input?: number;
  cacheWrite?: number;
  cacheRead?: number;
  output?: number;
  cacheWrite1h?: number;
  repliesWithoutCacheFields?: number;
}

export interface TokenCost {
  input: number;
  cacheWrite: number;
  cacheRead: number;
  output: number;
}

export interface AgentShare {
  agent: SpendAgent;
  tokens: number;
  cost: number;
  unpricedTokens: number;
}

export interface ModelShare {
  name: string;
  tokens: number;
  agents: SpendAgent[];
  share: number;
}

export interface SummaryDay {
  date: string;
  tokens: number;
  cost: number;
  tally: TokenTally;
  unpricedTokens: number;
  unclassifiedTokens: number;
  hasInvalidCategories: boolean;
}

export interface SpendSummary {
  tokens: number;
  cost: number;
  tally: TokenTally;
  unclassifiedTokens: number;
  hasTokenBreakdown: boolean;
  unpricedTokens: number;
  agents: AgentShare[];
  models: ModelShare[];
  days: SummaryDay[];
  unpricedModels: string[];
  hasAggregateTiming: boolean;
  hasPartialCounts: boolean;
  months: SummaryDay[];
  /** Tokens by hour of the local day; keys are "0"-"23". */
  hours: Record<string, number>;
  currentStreak: number;
  longestStreak: number;
}

export interface ActivityDay {
  date: string;
  tokens: number;
}

export interface ActivityWeek {
  start: string;
  days: (ActivityDay | null)[];
  unrecorded: boolean[];
}

export interface TokenActivity {
  weeks: ActivityWeek[];
  total: number;
  activeDays: number;
  cuts: number[];
  hasPartialCounts: boolean;
}

export interface ModelDay {
  date: string;
  tokens: number;
  tally: TokenTally | null;
  costBreakdown: TokenCost | null;
  unpricedTokens: number;
  unclassifiedTokens: number;
}

export interface ModelAgent {
  agent: SpendAgent;
  tokens: number;
  cost: number | null;
  unpricedTokens: number;
}

export interface ModelSpendSummary {
  name: string;
  tokens: number;
  tally: TokenTally | null;
  unclassifiedTokens: number;
  days: ModelDay[];
  agents: ModelAgent[];
  hours: Record<string, number> | null;
  costBreakdown: TokenCost | null;
  unpricedTokens: number;
  hasAggregateTiming: boolean;
  hasPartialCounts: boolean;
}

/** One transcript in the session list; the quarter-hours stay in Rust. */
export interface SessionRow {
  agent: SpendAgent;
  title: string | null;
  isReview: boolean;
  /** The project as the list shows it. */
  project: string | null;
  end: string;
  tokens: number;
  cost: number;
  unpricedTokens: number;
}

export interface ProjectRow {
  name: string;
  tokens: number;
  cost: number;
  unpricedTokens: number;
  sessions: number;
  lastUsed: string;
}

/** The project and session lists of the summary on screen. */
export interface SpendLists {
  projects: ProjectRow[];
  sessions: SessionRow[];
}

export interface SpendOverview {
  summary: SpendSummary;
  activity: TokenActivity;
  agent: SpendSummary | null;
  model: ModelSpendSummary | null;
  /** Absent while a model is open: it draws neither list. */
  lists: SpendLists | null;
  /** Present sources that produced no records at all. */
  noRecords: SpendAgent[];
  /** Every agent the pane can name; registered for `agentName` and `agentIcon` as the overview arrives. */
  agents: AgentInfo[];
  hasReadLimitations: boolean;
  /** The recap period keys the two buttons open: "2026-09" and "2026". */
  recap: { month: string; year: string };
}

/** "Reading Codex…", 1/2: which agent a scan is on. */
export interface SpendProgress {
  agent: SpendAgent;
  /** The product name, so the row can be drawn before the first overview has arrived. */
  name: string;
  index: number;
  total: number;
}

export interface SpendFigure {
  tokens: number;
  cost: number;
}

export interface LedgerDay {
  date: string;
  tokens: number;
  cost: number;
  unpricedTokens: number;
}

export interface CardSpend {
  today: SpendFigure;
  week: SpendFigure;
  month: SpendFigure;
  /** 31 days, oldest first, today last. */
  days: LedgerDay[];
  topModel: [string, number] | null;
  cacheHitRate: number | null;
  currency: string | null;
}

export interface PromptCacheLapse {
  lastRequest: string;
  /** Seconds. */
  lifetime: number;
  isMinimum: boolean;
  expiresAt: string;
}

export interface PromptCacheSession {
  id: string;
  title: string | null;
  isReview: boolean;
  project: string | null;
  lapse: PromptCacheLapse;
}

export interface PromptCacheReading {
  live: PromptCacheSession[];
  lastLapsed: PromptCacheLapse | null;
}

export const spendOverview = (overLast: number | null, agent: SpendAgent | null, model: string | null, rescan: boolean) =>
  invoke<SpendOverview | null>("spend_overview", { overLast, agent, model, rescan }).then((overview) => {
    if (overview) registerAgents(overview.agents);
    return overview;
  });
/** Lets the kept scan go: Token spend was switched off, or the Settings window closed. */
/** What one limit is worth by this PC's reckoning (`estimated_value`); `elsewhere` limits have no figure. */
export interface WindowEstimate {
  window: string;
  spent: number;
  full: number;
  elsewhere: boolean;
}
/** `card` is the detailed card's asking: it leaves out windows seen spent elsewhere and providers that state their limits in money. */
export const estimatedValue = (account: string, card: boolean) => invoke<WindowEstimate[]>("estimated_value", { account, card });
export const spendRelease = () => invoke<void>("spend_release");
export const cardSpend = (provider: string) => invoke<CardSpend | null>("card_spend", { provider });
export const promptCache = (provider: string) => invoke<PromptCacheReading | null>("prompt_cache", { provider });

const names = new Map<SpendAgent, string>();
const icons = new Map<SpendAgent, string | null>();

/** Remember how the agents are named and drawn (the Rust registry's answer). */
export function registerAgents(agents: AgentInfo[]) {
  for (const { id, name, icon } of agents) {
    names.set(id, name);
    if (icon !== null || !icons.has(id)) icons.set(id, icon);
  }
}

/** Product names, left untranslated; an agent not heard of yet is shown by its id. */
export const agentName: Record<SpendAgent, string> = new Proxy({} as Record<SpendAgent, string>, {
  get: (_, id) => names.get(String(id)) ?? String(id),
});

/** The stem of an agent's mark in assets/icons, or null where the icon set has none (the row then draws no mark). */
export const agentIcon = (id: SpendAgent): string | null => icons.get(id) ?? null;

// MARK: - Tally helpers (upstream TokenTally)

export const tallyTotal = (x: TokenTally) => (x.input ?? 0) + (x.cacheWrite ?? 0) + (x.cacheRead ?? 0) + (x.output ?? 0);
/** New input, cache writes included. */
export const tallyFresh = (x: TokenTally) => (x.input ?? 0) + (x.cacheWrite ?? 0);
/** Whether the recorded kinds plus an explicit unclassified count account for the reported total (`TokenTally::accounts_for`). */
export const accountsFor = (x: TokenTally, tokens: number, unclassified: number) =>
  [x.input ?? 0, x.cacheWrite ?? 0, x.cacheRead ?? 0, x.output ?? 0, unclassified].every((v) => v >= 0) &&
  tallyTotal(x) + unclassified === tokens;
export const reportsNoCache = (x: TokenTally) =>
  (x.repliesWithoutCacheFields ?? 0) > 0 && (x.cacheRead ?? 0) === 0 && (x.cacheWrite ?? 0) === 0;

// MARK: - Formatting

export function intlLocale(): string {
  const l = locale();
  return l === "zh-Hans" ? "zh-CN" : l === "zh-Hant" ? "zh-TW" : l;
}

/** Languages that count by 10^4 and 10^8, with the characters each spells them with. */
function myriadUnits(): [string, string] | null {
  switch (locale()) {
    case "zh-Hans": return ["万", "亿"];
    case "zh-Hant": return ["萬", "億"];
    case "ja": return ["万", "億"];
    case "ko": return ["만", "억"];
    default: return null;
  }
}

function trimmed(value: number, decimals: number): string {
  const text = value.toFixed(decimals);
  return text.includes(".") ? text.replace(/0+$/, "").replace(/\.$/, "") : text;
}

/** "5.9B", "119M", or "4.19亿", "4764万" where numbers group by ten thousands (upstream `TokenCount.short`). */
export function tokenCount(tokens: number): string {
  const units = myriadUnits();
  if (units) {
    if (tokens >= 1e8) {
      const scaled = tokens / 1e8;
      return trimmed(scaled, scaled < 10 ? 2 : scaled < 100 ? 1 : 0) + units[1];
    }
    if (tokens >= 1e4) {
      const scaled = tokens / 1e4;
      return trimmed(scaled, scaled < 10 ? 1 : 0) + units[0];
    }
    return String(tokens);
  }
  if (tokens >= 1e9) return trimmed(tokens / 1e9, tokens < 1e10 ? 1 : 0) + "B";
  if (tokens >= 1e6) return trimmed(tokens / 1e6, tokens < 1e7 ? 1 : 0) + "M";
  if (tokens >= 1e3) return trimmed(tokens / 1e3, tokens < 1e4 ? 1 : 0) + "K";
  return String(tokens);
}

/** "12.3M tokens". */
export const tokensText = (tokens: number) => t("%@ tokens", tokenCount(tokens));

/** The exact count, grouped the reader's way (upstream `SpendFormat.tokens`), for a title attribute. */
export const exactTokens = (tokens: number) =>
  t("%@ tokens", new Intl.NumberFormat(intlLocale()).format(tokens));

export const unpricedText = (tokens: number) =>
  t("%@ tokens unpriced", new Intl.NumberFormat(intlLocale()).format(tokens));

function currencyFormat(currency: string, fractionDigits: number | { significant: number }) {
  const options: Intl.NumberFormatOptions = { style: "currency", currency };
  if (typeof fractionDigits === "number") {
    options.minimumFractionDigits = fractionDigits;
    options.maximumFractionDigits = fractionDigits;
  } else {
    options.maximumSignificantDigits = fractionDigits.significant;
  }
  try {
    return new Intl.NumberFormat(intlLocale(), options);
  } catch {
    return new Intl.NumberFormat(intlLocale(), { ...options, currency: "USD" });
  }
}

/**
 * An estimated amount: dollars unless the ledger names another currency, two places or none from a
 * thousand up. A positive amount under a cent is "< $0.01", never "$0.00".
 */
export function money(amount: number, currency?: string | null): string {
  const code = currency || "USD";
  if (amount > 0 && amount < 0.01) return t("< %@", currencyFormat(code, 2).format(0.01));
  return currencyFormat(code, amount >= 1000 ? 0 : 2).format(amount);
}

/** The same figure for a title attribute, where the visible rounding hides digits. */
export function moneyExact(amount: number, currency?: string | null): string {
  if (amount === 0) return money(amount, currency);
  return currencyFormat(currency || "USD", { significant: 15 }).format(amount);
}

/** "Oct 6, 2025": a hover reads without the span picker beside it, so the year is in. */
export function chartDate(iso: string): string {
  return new Intl.DateTimeFormat(intlLocale(), { year: "numeric", month: "short", day: "numeric" }).format(new Date(iso));
}

export function shortDate(iso: string): string {
  return new Intl.DateTimeFormat(intlLocale(), { month: "short", day: "numeric" }).format(new Date(iso));
}

export const hourText = (hour: number) => t("%@ o'clock", hour);

/** "≈$220": whole dollars from a hundred up, because the figure is an estimate and cents would claim a precision it does not have (upstream `BudgetEstimator.approximate`). */
export function approximate(amount: number): string {
  const digits = amount >= 100 ? 0 : 2;
  return "≈" + new Intl.NumberFormat(intlLocale(), { style: "currency", currency: "USD", minimumFractionDigits: digits, maximumFractionDigits: digits }).format(amount);
}

/** "1 hr", "1 hr, 5 min", "38 min": whole minutes, rounded up, so the last minute still reads as one (upstream `PromptCacheLapse.duration`). */
export function duration(seconds: number): string {
  const minutes = Math.max(Math.ceil(seconds / 60), 1);
  const unit = (n: number, name: "hour" | "minute") =>
    new Intl.NumberFormat(intlLocale(), { style: "unit", unit: name, unitDisplay: "short" }).format(n);
  if (seconds >= 3600) {
    const parts = [unit(Math.floor(minutes / 60), "hour")];
    if (minutes % 60) parts.push(unit(minutes % 60, "minute"));
    return new Intl.ListFormat(intlLocale(), { type: "unit", style: "short" }).format(parts);
  }
  return unit(minutes, "minute");
}

// MARK: - Session labels (upstream SessionLabel)

/**
 * What a session is called wherever one is listed: its own title where it has one; a review session Codex
 * ran by itself says so; else the project it ran in; else "Untitled conversation". Never the file name.
 */
export function sessionLabel(s: { title: string | null; isReview: boolean }, project: string | null): string {
  if (s.title) return s.title;
  if (s.isReview) return t("Codex review");
  return project ?? t("Untitled conversation");
}

/** Whether the label is the session's own name rather than its project, which is when a subtitle adds the project. */
export const sessionNamesItself = (s: { title: string | null; isReview: boolean }) => !!s.title || s.isReview;

/** "Sep 23, 11:08": when a session ran. */
export function dateTime(iso: string): string {
  return new Intl.DateTimeFormat(intlLocale(), { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }).format(new Date(iso));
}

/** A clock time, with the date too unless it is today (the prompt-cache list's `time`). */
export function timeText(iso: string): string {
  const date = new Date(iso);
  const sameDay = date.toDateString() === new Date().toDateString();
  return new Intl.DateTimeFormat(
    intlLocale(),
    sameDay ? { hour: "numeric", minute: "2-digit" } : { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" },
  ).format(date);
}

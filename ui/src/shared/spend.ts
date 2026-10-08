// Mirror of the Token spend IPC payloads (src-tauri/src/spend_ipc.rs and crates/pulse-core/src/spend,
// serde camelCase; chrono dates arrive as ISO strings) plus the number and money formatting the
// Token spend pane and the detailed card share (upstream TokenCount, SpendFormat, AccountUsageCard.money).
import { invoke } from "@tauri-apps/api/core";
import { locale, t } from "./i18n";

// MARK: - Payloads

export type SpendAgent = "claudeCode" | "codex";

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

export interface SpendOverview {
  summary: SpendSummary;
  activity: TokenActivity;
  agent: SpendSummary | null;
  model: ModelSpendSummary | null;
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

export const spendOverview = (overLast: number | null, agent: SpendAgent | null, model: string | null) =>
  invoke<SpendOverview | null>("spend_overview", { overLast, agent, model });
export const cardSpend = (provider: string) => invoke<CardSpend | null>("card_spend", { provider });
export const promptCache = (provider: string) => invoke<PromptCacheReading | null>("prompt_cache", { provider });

export const agentName: Record<SpendAgent, string> = { claudeCode: "Claude Code", codex: "Codex" };

// MARK: - Tally helpers (upstream TokenTally)

export const tallyTotal = (x: TokenTally) => (x.input ?? 0) + (x.cacheWrite ?? 0) + (x.cacheRead ?? 0) + (x.output ?? 0);
/** New input, cache writes included. */
export const tallyFresh = (x: TokenTally) => (x.input ?? 0) + (x.cacheWrite ?? 0);
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

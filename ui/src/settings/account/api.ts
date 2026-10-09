// Payloads and commands of the account pane's history, service status and low-balance row
// (src-tauri/src/history_ipc.rs, status_ipc.rs, notifications_ipc.rs; serde camelCase, chrono dates as ISO strings).
import { invoke } from "@tauri-apps/api/core";

// MARK: - Usage history (pulse_core::history)

export type HistoryReadKind = "answered" | "failed" | "notConfigured" | "notAsked" | "unsupported";
export type LedgerOrigin = "localTranscripts" | "importedRecords" | "providerStatistics" | "providerLogs";

export interface HistoryFigure {
  tokens: number;
  /** Null where none of the work had a published price: a dash, never a confident zero. */
  cost: number | null;
}

export interface HistoryModelRow {
  name: string;
  share: number | null;
  cacheRate: number | null;
  speed: number | null;
  firstToken: number | null;
}

export interface AccountHistory {
  provider: string;
  read: HistoryReadKind;
  origin: LedgerOrigin;
  currency: string | null;
  hasDays: boolean;
  today: HistoryFigure;
  last31Days: HistoryFigure;
  busiestDay: HistoryFigure;
  allTime: HistoryFigure;
  last7Days: HistoryFigure;
  /** The last 31 days, oldest first. */
  days: { date: string; tokens: number }[];
  models: HistoryModelRow[];
  overallCacheRate: number | null;
  hasPartialCounts: boolean;
  unpricedModels: string[];
  timesReplies: boolean;
  speedsEmpty: boolean;
}

export const accountHistory = (id: string) => invoke<AccountHistory>("account_history", { id });

// MARK: - Service status (pulse_core::status)

export type StatusPageId = "openAi" | "claude" | "deepSeek";
export type StatusState = "operational" | "degraded" | "partialOutage" | "fullOutage" | "maintenance" | "unrecognised";

export interface StatusDay {
  /** "2026-10-04": the day as the page counts it. */
  date: string;
  state: StatusState | null;
  /** The colour the page drew, 0xRRGGBB. */
  rgb: number | null;
}

export interface StatusComponent {
  id: string;
  name: string;
  state: StatusState;
  days: StatusDay[];
  /** The page's own figure, in percent. */
  uptime: number | null;
}

export interface StatusView {
  page: StatusPageId;
  address: string;
  host: string;
  company: string;
  dayCount: number;
  /** Null when the page couldn't be read. */
  status: { page: StatusPageId; components: StatusComponent[] } | null;
}

export const serviceStatus = (provider: string, force = false) => invoke<StatusView | null>("service_status", { provider, force });

export const openStatusPage = (provider: string) => invoke<void>("open_status_page", { provider });

// MARK: - Notifications

export const reportsSpendableBalance = (provider: string) => invoke<boolean>("reports_spendable_balance", { provider });

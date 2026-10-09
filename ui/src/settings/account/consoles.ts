// Payloads and commands of the window starter, Codex signals and console groups
// (src-tauri/src/codex_signals_ipc.rs, console_ipc.rs; serde camelCase, chrono dates as ISO strings).
import { invoke } from "@tauri-apps/api/core";
import type { AccountHistory } from "./api";

// MARK: - Codex signals (pulse_core::codex_signals)

export interface TruncationRow {
  model: string;
  /** Responses with any reasoning at all. */
  responses: number;
  /** Responses whose reasoning reached 516, the first lattice point. */
  reachedLattice: number;
  /** Of those, the ones that stopped exactly on a lattice point. */
  onLattice: number;
  share: number | null;
  /** Enough responses reached the lattice to say anything about it. */
  isMeasurable: boolean;
  isSuspicious: boolean;
}

export type SettingsChange =
  | { kind: "model"; asked: string; ran: string }
  | { kind: "effort"; asked: string; ran: string }
  | { kind: "contextWindow"; was: number; now: number };

export type CodexChange = SettingsChange & { date: string; session: string; order: number };

export interface CodexSignals {
  truncation: TruncationRow[];
  changes: CodexChange[];
  /** Sessions in the span, and how many of them were new enough to judge for changed settings. */
  sessions: number;
  judgedSessions: number;
}

/** Null when a newer request replaced this one. */
export const codexSignals = (days: number | null) => invoke<CodexSignals | null>("codex_signals", { days });

// MARK: - The consoles' sign-in (src-tauri/src/console_ipc.rs)

export type ConsoleProvider = "deepSeek" | "openCodeGo";

export type ConsoleRead =
  | { kind: "read"; workspace: string | null }
  | { kind: "unanswered" }
  | { kind: "notFound" }
  | { kind: "busy" };

export const consoleSession = (provider: ConsoleProvider) => invoke<boolean>("console_session", { provider });
export const consoleRead = (provider: ConsoleProvider) => invoke<ConsoleRead>("console_read", { provider });
export const consoleRemove = (provider: ConsoleProvider) => invoke<void>("console_remove", { provider });

/** The history of a provider that is asked rather than scanned: DeepSeek's console, OpenCode Go's request log, Z.ai's statistics. */
export const providerHistory = (id: string, currency: string | null = null) =>
  invoke<AccountHistory>("provider_history", { id, currency });

/** Fired on `window` when a console's session is kept or removed, so a history card can read again. */
export const CONSOLE_SESSION_EVENT = "pulse-console-session";

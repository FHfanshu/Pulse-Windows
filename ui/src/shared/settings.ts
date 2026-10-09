// Mirror of crates/pulse-core/src/settings.rs (serde camelCase) plus the IPC hooks
// both windows use.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { ProviderUsage } from "./model";

export type PanelSize = "small" | "standard" | "large";
export type RailSpacing = "compact" | "standard" | "roomy";
export type AppLanguage = "system" | "en" | "zh-Hans" | "zh-Hant" | "ja" | "ko";
export type RefreshInterval = { type: "adaptive" } | { type: "fixed"; minutes: number };

export interface AppSettings {
  hidesTrayIcon: boolean;
  trayAccount: string | null;
  showsUsageInTray: boolean;
  trayStyle: "figure" | "ring" | "split";
  showsMenuDashboard: boolean;
  openSettingsShortcut: { accelerator: string } | null;
  togglePanelShortcut: { accelerator: string } | null;
  language: AppLanguage;
  launchAtLogin: boolean;
  isPanelVisible: boolean;
  hidesInFullScreen: boolean;
  followsActiveDisplay: boolean;
  panelSize: PanelSize;
  railSpacing: RailSpacing;
  topRailShowsPercentages: boolean;
  sideRailShowsPercentages: boolean;
  labelAboveRing: boolean;
  freeAcrossFiguresBeside: boolean;
  usesRoundEnds: boolean;
  usesGlass: boolean;
  glassTransparency: number;
  usesLightPanel: boolean;
  autoCollapse: boolean;
  detailedCards: string[];
  showsWindowClock: boolean;
  windowClockDirection: "elapsed" | "remaining";
  showsForecast: boolean;
  showsRemaining: boolean;
  warningThreshold: number;
  dockShowsAlertColor: boolean;
  showsSecondRing: boolean;
  animatesRingActivity: boolean;
  splitAccounts: string[];
  pinnedWindows: Record<string, string>;
  ringTints: Record<string, string>;
  enabledAccounts: string[];
  extraAccounts: { id: string; provider: string; name: string }[];
  providerOrder: string[];
  offeredProviders: string[];
  sources: Record<string, string>;
  serverAddresses: Record<string, string>;
  balanceBases: Record<string, string>;
  balanceBudgets: Record<string, number>;
  sessionBrowsers: Record<string, string>;
  refreshInterval: RefreshInterval;
  networkProxy: { enabled: boolean; scheme: string; host: string; port: number };
  readsTokenSpend: boolean;
  recapMonthlyPrice: number | null;
  recapHidesProjects: boolean;
  recapAnnouncedMonth: string | null;
  /** Window starter (off by default): provider ids, the hours it may act in, and its last attempt per provider. */
  primedProviders: string[];
  primerHours: { start: number; end: number };
  primerRunOutcomes: Record<string, string>;
  /** Seconds since 1970. */
  primerRunTimes: Record<string, number>;
  lowBalanceAlerts: Record<string, number>;
  alertThreshold: number | null;
  alertsOnReset: boolean;
  alertsOnFailure: boolean;
  alertsOnOutage: boolean;
  alertsOnRecap: boolean;
}

export interface ProviderInfo {
  id: string;
  name: string;
  icon: string;
  multipleAccounts: boolean;
}

export interface UsagePayload {
  usages: ProviderUsage[];
  refreshing: string[];
}

export const updateSettings = (patch: Partial<AppSettings>) => invoke<AppSettings>("update_settings", { patch });

/** Live settings: loaded once, then kept current from `settings-changed`. */
export function useSettings(): AppSettings | null {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  useEffect(() => {
    invoke<AppSettings>("get_settings").then(setSettings);
    const un = listen<AppSettings>("settings-changed", (e) => setSettings(e.payload));
    return () => void un.then((f) => f());
  }, []);
  return settings;
}

/** Live readings from `usage-changed`. */
export function useUsage(): UsagePayload {
  const [payload, setPayload] = useState<UsagePayload>({ usages: [], refreshing: [] });
  useEffect(() => {
    invoke<UsagePayload>("get_snapshot").then(setPayload);
    const un = listen<UsagePayload>("usage-changed", (e) => setPayload(e.payload));
    return () => void un.then((f) => f());
  }, []);
  return payload;
}

/** The account whose card shows this PC's records for `provider`: the first account, the one the records
 *  belong to, while it is on the panel; otherwise the first of the provider's accounts that is (a Windows
 *  difference: upstream shows them on the first account only, so a panel of added accounts showed none). */
export function recordsAccount(settings: Pick<AppSettings, "enabledAccounts">, provider: string): string | null {
  if (settings.enabledAccounts.includes(provider)) return provider;
  return settings.enabledAccounts.find((id) => id.split("#")[0] === provider) ?? null;
}

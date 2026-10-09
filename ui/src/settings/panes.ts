// The sidebar's contents: fixed panes, then one pane per account, grouped as upstream does
// (Docs/ui/settings.md, "Panes"). Titles are computed on demand so they follow the language.
import { t } from "../shared/i18n";
import { providerNames } from "../panel/Icon";
import type { AppSettings, ProviderInfo } from "../shared/settings";
import type { GlyphName } from "./Glyph";

export type FixedPane =
  | "appearance" | "rings" | "placement" | "spend" | "general" | "notifications" | "network" | "about";

/** A pane id: a fixed pane, or `account:<account id>`. */
export type PaneId = string;

export const accountPaneId = (id: string) => `account:${id}`;
export const accountOf = (pane: PaneId): string | null => (pane.startsWith("account:") ? pane.slice(8) : null);

export type PaneIcon = { glyph: GlyphName } | { provider: string };

export interface SidebarItem {
  id: PaneId;
  title: string;
  icon: PaneIcon;
  /** Extra text the search also matches (row titles of the pane). */
  terms: string[];
}

export interface SidebarSection {
  title: string | null;
  items: SidebarItem[];
}

/** Providers filed under "API and pay-as-you-go"; everything else is a subscription. */
export const apiProviders = new Set(["moonshot", "openAIPlatform", "deepSeek"]);

function fixed(id: FixedPane): SidebarItem {
  switch (id) {
    case "appearance":
      return { id, title: t("Appearance"), icon: { glyph: "appearance" }, terms: ["Size", "Spacing", "Round ends", "Panel colour", "Liquid Glass", "Transparency", "Ring activity animation"].map((k) => t(k)) };
    case "rings":
      return { id, title: t("Rings and figures"), icon: { glyph: "rings" }, terms: ["Percentages at the side", "Percentages across", "Figures beside the rings", "Figure above the ring", "Show what's left", "Forecast", "Second limit inside the ring", "Time until reset", "Time ring direction", "Turn red at", "Alert colour when docked"].map((k) => t(k)) };
    case "placement":
      return { id, title: t("Position and behavior"), icon: { glyph: "placement" }, terms: ["Show floating panel", "Hide in full screen", "Hide until pointed at", "Position", "Follow the active display", "Order"].map((k) => t(k)) };
    case "spend":
      return { id, title: t("Token spend"), icon: { glyph: "spend" }, terms: ["Read local usage records", "Span", "Token activity"].map((k) => t(k)) };
    case "general":
      return { id, title: t("General"), icon: { glyph: "general" }, terms: ["Open at login", "Hide menu bar icon", "Interface language"].map((k) => t(k)) };
    case "notifications":
      return { id, title: t("Notifications"), icon: { glyph: "bell" }, terms: ["Warn at", "When a limit comes back", "When a reading stops arriving", "When a service is down", "When a recap is ready"].map((k) => t(k)) };
    case "network":
      return { id, title: t("Network and refresh"), icon: { glyph: "network" }, terms: ["Check every", "Proxy"].map((k) => t(k)) };
    case "about":
      return { id, title: t("About"), icon: { glyph: "about" }, terms: [] };
  }
}

export interface AccountEntry {
  id: string;
  provider: string;
  /** The user's label for an added account, else the provider's name. */
  label: string;
}

export const providerName = (id: string, providers: ProviderInfo[]) =>
  providers.find((p) => p.id === id)?.name ?? providerNames[id] ?? id;

/** Every account Pulse knows: each provider's first, plus added logins. */
export function allAccounts(settings: AppSettings, providers: ProviderInfo[]): AccountEntry[] {
  const primary = providers.map((p) => ({ id: p.id, provider: p.id, label: p.name }));
  const extra = settings.extraAccounts.map((a) => ({ id: a.id, provider: a.provider, label: a.name || providerName(a.provider, providers) }));
  return [...primary, ...extra];
}

/** Enabled accounts in rail order: the saved order first, then whatever it does not mention. */
export function enabledInOrder(settings: AppSettings): string[] {
  const enabled = new Set(settings.enabledAccounts);
  const ordered = settings.providerOrder.filter((id) => enabled.has(id));
  const rest = settings.enabledAccounts.filter((id) => !ordered.includes(id));
  return [...ordered, ...[...rest].sort()];
}

/** Row titles every account pane has (its Panel group). */
const ACCOUNT_TERMS = ["Show in panel", "Detailed card", "Ring colour", "Ring shows"];

export function buildSections(settings: AppSettings, providers: ProviderInfo[]): SidebarSection[] {
  const accounts = allAccounts(settings, providers);
  const byId = new Map(accounts.map((a) => [a.id, a]));
  const item = (a: AccountEntry): SidebarItem => ({
    id: accountPaneId(a.id),
    title: a.label,
    icon: { provider: a.provider },
    // Typing the provider's name finds an account the user has relabelled; the Panel group's rows
    // find every account, since each has its own.
    terms: [providerName(a.provider, providers), ...ACCOUNT_TERMS.map((k) => t(k))],
  });

  const enabledIds = enabledInOrder(settings).filter((id) => byId.has(id));
  const enabled = new Set(enabledIds);
  const rest = accounts
    .filter((a) => !enabled.has(a.id))
    .sort((a, b) => a.label.localeCompare(b.label));

  return [
    { title: t("Panel"), items: (["appearance", "rings", "placement", "spend"] as FixedPane[]).map(fixed) },
    { title: t("Application"), items: (["general", "notifications", "network"] as FixedPane[]).map(fixed) },
    { title: t("Enabled"), items: enabledIds.map((id) => item(byId.get(id)!)) },
    { title: t("Subscriptions"), items: rest.filter((a) => !apiProviders.has(a.provider)).map(item) },
    { title: t("API and pay-as-you-go"), items: rest.filter((a) => apiProviders.has(a.provider)).map(item) },
    { title: null, items: [fixed("about")] },
  ].filter((s) => s.items.length > 0);
}

/** Case-insensitive match on the pane's title or any of its terms. */
export function matches(item: SidebarItem, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return [item.title, ...item.terms].some((s) => s.toLowerCase().includes(q));
}

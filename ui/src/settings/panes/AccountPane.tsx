// Ported from upstream Settings/AccountPane.swift: one account's pane, its cards in order. What each card says and
// does is in ui/src/settings/account/.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { useUsage, type AppSettings, type ProviderInfo } from "../../shared/settings";
import { AccountsGroup } from "../account/AccountsGroup";
import "../account/account.css";
import { ConnectionGroup } from "../account/ConnectionGroup";
import { LowBalanceRow } from "../account/ConnectionRows";
import { DiagnosticsGroup } from "../account/DiagnosticsGroup";
import { EstimatedValueGroup } from "../account/EstimatedValueGroup";
import { LiveUsageGroup } from "../account/LiveUsageGroup";
import { accountParts, metaOf, pc, usageOf } from "../account/meta";
import { PanelGroup } from "../account/PanelGroup";
import { Group } from "../Group";

let providerList: Promise<ProviderInfo[]> | null = null;
/** `list_providers`, asked once for the window's lifetime. */
function useProviders(): ProviderInfo[] {
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  useEffect(() => {
    providerList ??= invoke<ProviderInfo[]>("list_providers");
    let live = true;
    providerList.then((p) => { if (live) setProviders(p); }).catch(() => { providerList = null; });
    return () => { live = false; };
  }, []);
  return providers;
}

/** One account: whether it is on the panel, where its figures come from, what it currently reads. */
export function AccountPane({ id, settings, onNavigate }: {
  id: string;
  settings: AppSettings;
  /** Called with a pane id, e.g. "appearance" once the account being shown has been removed. */
  onNavigate?: (pane: string) => void;
}) {
  const { provider, primary } = accountParts(id, settings);
  const providers = useProviders();
  const { usages, refreshing } = useUsage();
  const usage = usageOf(usages, id);
  const enabled = settings.enabledAccounts.includes(id);
  const meta = metaOf(provider);
  const multipleAccounts = providers.find((p) => p.id === provider)?.multipleAccounts ?? false;
  // Nothing chosen yet (upstream `needsProviderSelection`): no rail, no fetching, nothing to connect.
  const needsProviderSelection = settings.enabledAccounts.length === 0;

  // An added account that has just been removed: the shell is about to move to another pane.
  if (!primary && !settings.extraAccounts.some((a) => a.id === id)) return null;

  return (
    <div className="pane-stack">
      {/* What switching it on will do is exactly what somebody should read first. */}
      {!enabled && primary && meta.access && <p className="note">{pc(meta.access)}</p>}

      <PanelGroup id={id} provider={provider} primary={primary} settings={settings} usage={usage} />

      {/* Upstream places the window starter here, for Claude Code and Codex first accounts. */}

      {!needsProviderSelection && (
        <>
          <ConnectionGroup id={id} provider={provider} primary={primary} settings={settings} usage={usage} />
          <DiagnosticsGroup
            id={id}
            provider={provider}
            primary={primary}
            settings={settings}
            usage={usage}
            enabled={enabled}
            refreshing={refreshing.includes(id)}
            anyRefreshing={refreshing.length > 0}
          />
          <AccountsGroup
            id={id}
            provider={provider}
            primary={primary}
            settings={settings}
            multipleAccounts={multipleAccounts}
            onNavigate={(pane) => onNavigate?.(pane)}
          />
          <LiveUsageGroup id={id} settings={settings} usage={usage} enabled={enabled} refreshing={refreshing.length > 0} />
        </>
      )}

      {/* Upstream places the service status here. */}

      {/* Its own group rather than a row under Connection, which is about credentials and routes. This is a
          notification, and the figure is per account: the providers that report a balance do not price in the same
          currency. */}
      {primary && meta.reportsBalance && (
        <Group title={t("Notifications")}>
          <LowBalanceRow id={id} settings={settings} />
        </Group>
      )}

      {/* Upstream places the OpenCode Go and DeepSeek console groups here. */}

      {/* The estimate is money, and money needs the token split only a transcript carries. */}
      {primary && meta.keepsLocalTranscripts && (
        <EstimatedValueGroup id={id} usage={usage} readsTokenSpend={settings.readsTokenSpend} />
      )}

      {/* mount: history, status, starter, signals, console */}
    </div>
  );
}

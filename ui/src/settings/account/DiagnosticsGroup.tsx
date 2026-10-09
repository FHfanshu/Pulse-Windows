// Ported from upstream Settings/ConnectionDiagnosticsView.swift, ConnectionDiagnostic (report, message) and
// ConnectionRepairModel (what each remedy does). pulse-core keeps no per-route attempt log, so the "Route checks"
// disclosure is not drawn; everything else is derived from the account's current reading.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { unavailableMessage } from "../../shared/copy";
import { t } from "../../shared/i18n";
import type { ProviderUsage } from "../../shared/model";
import type { AppSettings } from "../../shared/settings";
import { intlLocale } from "../../shared/spend";
import { Button } from "../controls";
import { Group, Row } from "../Group";
import { helpUrl, remedyFor, type Remedy } from "./meta";

/** When each account's last pass finished, as this window saw it. Outlives the pane, not the window. */
const checkedAt = new Map<string, number>();

function useCheckedAt(id: string, refreshing: boolean, usage: ProviderUsage | undefined): number | null {
  const was = useRef(refreshing);
  useEffect(() => {
    if (was.current && !refreshing) checkedAt.set(id, Date.now());
    was.current = refreshing;
  }, [id, refreshing]);
  const seen = checkedAt.get(id);
  if (seen != null) return seen;
  // A live reading came out of a check that succeeded, and is the best account of when.
  return usage?.state.kind === "live" && usage.observedAt ? Date.parse(usage.observedAt) : null;
}

const originTitle = (usage: ProviderUsage): string | null => {
  switch (usage.origin) {
    case "endpoint":
    case "apiKey": return t("Usage endpoint");
    case "statusLine": return t("Claude Code status line");
    case "localLogin":
    case "localStore": return t("The app's saved plan");
    case "browserSession": return t("Signed-in web page");
    case "helper":
      return { codex: t("Codex app server"), kiro: t("Kiro CLI ACP"), antigravity: t("Local language server") }[usage.account.provider] ?? t("Provider tooling");
    default: return null;
  }
};

const stamp = (ms: number | null) =>
  ms == null
    ? t("Not recorded")
    : new Intl.DateTimeFormat(intlLocale(), { year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "2-digit", second: "2-digit" }).format(ms);

/** Upstream `ConnectionDiagnostic.message`. */
function stateMessage(usage: ProviderUsage): string {
  switch (usage.state.kind) {
    case "live": return t("Reading received");
    case "stale": return t("Older reading received");
    case "unavailable": return unavailableMessage(usage.state.reason);
  }
}

/** Upstream `ConnectionDiagnostic.report`: only source, status and timestamps; no account details or secrets. */
async function report(provider: string, primary: boolean, preference: string, usage: ProviderUsage, checked: number | null): Promise<string> {
  const iso = (ms: number | null | string) => (ms == null ? "unknown" : new Date(ms).toISOString());
  const token = usage.state.kind === "unavailable" ? usage.state.reason : usage.state.kind;
  let version = "development";
  try {
    version = await (await import("@tauri-apps/api/app")).getVersion();
  } catch { /* running outside the app */ }
  return [
    "Pulse connection diagnostic (v1)",
    `version: ${version}`,
    `generatedAt: ${iso(Date.now())}`,
    `provider: ${provider}`,
    `accountType: ${primary ? "primary" : "added"}`,
    `preference: ${primary ? preference : "endpoint"}`,
    `checkedAt: ${iso(checked)}`,
    `checkResult: ${checked == null ? "notChecked" : token}`,
    `displayState: ${token}`,
    `displayOrigin: ${usage.origin ?? "unknown"}`,
    `displayUsesCache: ${usage.isCached}`,
    `observedAt: ${iso(usage.observedAt ? Date.parse(usage.observedAt) : null)}`,
    `lastSuccessfulReadingAt: ${iso(usage.observedAt ? Date.parse(usage.observedAt) : null)}`,
  ].join("\n");
}

const remedyTitle = (r: Remedy): string => {
  switch (r.type) {
    case "signIn": return t("Sign in again…");
    case "editCredential": return t("Edit credential");
    case "editAddress": return t("Edit address");
    case "readBrowser": return t("Read from browser");
    case "connectStatusLine": return t("Connect status line");
    case "openApp": return t("Open %@", r.name);
    case "copyCommand": return t("Copy login command");
    case "retry": return t("Retry");
    case "help": return t("Setup help");
  }
};

/** Brings the reader to a field further down the connection card and puts the cursor there. */
function focusField(field: string) {
  document.getElementById("connection")?.scrollIntoView({ behavior: "smooth", block: "start" });
  window.setTimeout(() => document.getElementById(field)?.focus(), 250);
}

export function DiagnosticsGroup({ id, provider, primary, settings, usage, enabled, refreshing, anyRefreshing }: {
  id: string;
  provider: string;
  primary: boolean;
  settings: AppSettings;
  usage: ProviderUsage | undefined;
  enabled: boolean;
  /** This account is being asked right now. */
  refreshing: boolean;
  anyRefreshing: boolean;
}) {
  const checked = useCheckedAt(id, refreshing, usage);
  const [copied, setCopied] = useState(false);
  const [repairMessage, setRepairMessage] = useState<string | null>(null);
  useEffect(() => { setRepairMessage(null); setCopied(false); }, [id]);

  const reason = usage?.state.kind === "unavailable" ? usage.state.reason : null;
  const remedy = reason ? remedyFor(reason, primary) : null;

  const repair = async (r: Remedy) => {
    setRepairMessage(null);
    switch (r.type) {
      case "editCredential": return focusField("account-credential");
      case "editAddress": return focusField("account-address");
      case "readBrowser": return focusField("account-credential");
      case "connectStatusLine": {
        const ok = await invoke<boolean>("set_status_line", { connected: true }).catch(() => false);
        setRepairMessage(ok ? t("Connected. Use Claude Code to send a new reading.") : t("Couldn't connect the status line. Open setup help."));
        void invoke("refresh", { account: id });
        return;
      }
      case "copyCommand":
        await navigator.clipboard.writeText(r.command).catch(() => {});
        setRepairMessage(t("Copied — run %@ in your terminal, then retry.", r.command));
        return;
      case "retry": return void invoke("refresh", { account: id });
      case "help": return void invoke("open_external", { url: helpUrl(provider) });
      // An added account signs in again through Pulse's own flow; the primary one belongs to its CLI.
      case "signIn":
        if (!primary) void invoke("signin_start", { provider, replacing: id });
        return;
      // Launching another program (openApp) has no Windows equivalent yet.
      default: return;
    }
  };
  const unavailableRemedy = remedy && ((remedy.type === "signIn" && primary) || remedy.type === "openApp");

  return (
    <Group title={t("Connection diagnostics")}>
      <Row title={t("Latest check")} subtitle={usage ? stateMessage(usage) : t("No completed check since launch.")}>
        <Button disabled={anyRefreshing || !enabled} onClick={() => void invoke("refresh", { account: id })}>{t("Retry")}</Button>
      </Row>
      <Row title={t("Checked at")}><span className="value-text small-text selectable">{stamp(checked)}</span></Row>
      <Row title={t("Last successful reading")}>
        <span className="value-text small-text selectable">{stamp(usage?.observedAt ? Date.parse(usage.observedAt) : null)}</span>
      </Row>
      <Row
        title={t("Displayed figures")}
        subtitle={usage?.isCached ? t("Showing a saved reading; its original source is retained.") : null}
      >
        <span className="value-text small-text">{(usage && originTitle(usage)) ?? t("Source not recorded")}</span>
      </Row>
      {reason && remedy && remedy.type !== "retry" ? (
        <Row title={t("Next step")} subtitle={repairMessage ?? unavailableMessage(reason)}>
          <Button disabled={!!unavailableRemedy} onClick={() => void repair(remedy)}>{remedyTitle(remedy)}</Button>
        </Row>
      ) : (
        repairMessage && <Row title={repairMessage} />
      )}
      <Row title={t("Share diagnostics")} subtitle={t("Only source, status and timestamps; no account details or secrets.")}>
        <Button
          disabled={!usage}
          onClick={() => {
            if (!usage) return;
            void report(provider, primary, settings.sources[id] ?? "automatic", usage, checked).then((text) =>
              navigator.clipboard.writeText(text).then(() => setCopied(true)).catch(() => setCopied(false)),
            );
          }}
        >
          {copied ? t("Copied") : t("Copy diagnostics")}
        </Button>
      </Row>
      <Row title={t("Setup help")}>
        <Button onClick={() => void invoke("open_external", { url: helpUrl(provider) })}>{t("Open")}</Button>
      </Row>
    </Group>
  );
}

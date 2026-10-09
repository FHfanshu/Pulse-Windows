// Ported from upstream Settings/AccountLiveUsageGroup.swift: an account's Current usage card, when it was last read
// and each limit's figure and reset.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { relativeTime, resetText, unavailableMessage, windowName } from "../../shared/copy";
import { t } from "../../shared/i18n";
import { percentText, type ProviderUsage } from "../../shared/model";
import type { AppSettings } from "../../shared/settings";
import { intlLocale } from "../../shared/spend";
import { Button } from "../controls";
import { Group, Row } from "../Group";

/** Re-renders every second so "1 min, 0 sec" keeps counting (upstream's `Text(style: .relative)` does on its own). */
function useTick(ms: number) {
  const [, setTick] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), ms);
    return () => clearInterval(timer);
  }, [ms]);
}

/** "1 min, 0 sec" like upstream's relative style, falling back to a coarser phrase where the runtime has no duration format. */
function elapsedText(iso: string): string {
  const seconds = Math.max(Math.round((Date.now() - Date.parse(iso)) / 1000), 0);
  const Duration = (Intl as unknown as { DurationFormat?: new (l: string, o: object) => { format(d: object): string } }).DurationFormat;
  if (Duration && seconds < 3600) {
    try {
      return new Duration(intlLocale(), { style: "short" }).format({
        minutes: Math.floor(seconds / 60),
        seconds: seconds % 60,
      });
    } catch { /* fall through */ }
  }
  return relativeTime(iso);
}

export function LiveUsageGroup({ id, settings, usage, enabled, refreshing }: {
  id: string;
  settings: AppSettings;
  usage: ProviderUsage | undefined;
  enabled: boolean;
  /** Any pass at all, not just this account's: during a background one the press would only queue. */
  refreshing: boolean;
}) {
  useTick(1000);

  if (!enabled) {
    return (
      <Group title={t("Current usage")}>
        <Row title={t("Not shown")} subtitle={t("Enable a service in its settings to start monitoring.")} />
      </Group>
    );
  }

  const reason = usage?.state.kind === "unavailable" ? unavailableMessage(usage.state.reason) : null;
  return (
    <Group title={t("Current usage")}>
      {/* Says how current these figures are, and offers to make them current. */}
      <Row title={t("Last read")}>
        <span className="value-text num">{usage?.observedAt ? elapsedText(usage.observedAt) : t("Not yet")}</span>
        <Button disabled={refreshing} onClick={() => void invoke("refresh", { account: id })}>{t("Refresh")}</Button>
      </Row>
      {!usage || usage.windows.length === 0 ? (
        <Row title={t("No reading")} subtitle={reason} />
      ) : (
        usage.windows.map((w) => (
          <Row key={w.id} title={windowName(w)} subtitle={resetText(w) || null}>
            <span className="value-strong">{percentText(w, settings.showsRemaining)}</span>
          </Row>
        ))
      )}
      {usage?.plan && (
        <Row title={t("Plan")}><span className="value-text">{usage.plan}</span></Row>
      )}
      {usage?.creditBalance && (
        <Row title={t("Credit balance")}><span className="value-text">{usage.creditBalance}</span></Row>
      )}
    </Group>
  );
}

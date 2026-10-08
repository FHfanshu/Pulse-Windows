// Ported from upstream Settings/TokenSpendPane.swift and SpendPaneModel.swift: the pane whose subject is
// every agent's spending added up. The switch turns reading on; nothing is read while it is off.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { spendOverview, type SpendAgent, type SpendOverview } from "../../shared/spend";
import { updateSettings, type AppSettings } from "../../shared/settings";
import { Button } from "../controls";
import { Group, Row } from "../Group";
import { TokenSpendView, spanDays, type SpendSpan } from "../spend/SpendView";
import type { ActivityView } from "../spend/TokenActivity";
import { ToggleRow } from "./ToggleRow";

/** A choice the reader's own pick keeps for the next visit and launch; a stored value no longer offered falls back. */
function usePersisted<T extends string>(key: string, fallback: T, allowed: readonly T[]): [T, (v: T) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const stored = localStorage.getItem(key) as T | null;
      return stored && allowed.includes(stored) ? stored : fallback;
    } catch {
      return fallback;
    }
  });
  const set = (next: T) => {
    setValue(next);
    try { localStorage.setItem(key, next); } catch { /* storage can be unavailable */ }
  };
  return [value, set];
}

const spans = Object.keys(spanDays) as SpendSpan[];
const activityViews = ["daily", "weekly", "cumulative"] as const;

export function TokenSpendPane({ settings }: { settings: AppSettings }) {
  const enabled = settings.readsTokenSpend;
  const [span, setSpan] = usePersisted<SpendSpan>("settings.spendSpan", "week", spans);
  const [activityView, setActivityView] = usePersisted<ActivityView>("settings.spendActivityView", "daily", activityViews);
  // Where the pane has drilled into. Not a setting: it is a way of reading the page in front of you.
  const [agent, setAgent] = useState<SpendAgent | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [rescan, setRescan] = useState(0);
  const [shown, setShown] = useState<{ key: string; overview: SpendOverview | null } | null>(null);
  const [loading, setLoading] = useState(false);

  const requestKey = `${span}|${agent ?? ""}|${model ?? ""}`;

  useEffect(() => {
    if (!enabled) {
      setShown(null);
      setAgent(null);
      setModel(null);
      return;
    }
    let live = true;
    setLoading(true);
    spendOverview(spanDays[span], agent, model)
      .then((overview) => live && setShown({ key: requestKey, overview }))
      .catch(() => live && setShown({ key: requestKey, overview: null }))
      .finally(() => live && setLoading(false));
    // A newer request supersedes this one: its answer must not replace the current figures.
    return () => { live = false; };
  }, [enabled, span, agent, model, rescan]); // eslint-disable-line react-hooks/exhaustive-deps

  // A model opened under one agent means nothing under another.
  const pickAgent = (next: SpendAgent | null) => {
    setModel(null);
    setAgent(next);
  };

  return (
    <div className="pane-stack">
      <Group title={t("Token spend")}>
        <ToggleRow
          title="Read local usage records"
          subtitle="When enabled, scans local records and exports."
          checked={enabled}
          onChange={(readsTokenSpend) => updateSettings({ readsTokenSpend })}
        />
        {enabled && (
          <Row title={t("Monthly and Yearly Recap")} subtitle={t("Shareable cards for a month or a year, from this Mac's records.")}>
            <Button onClick={() => invoke("open_recap", { period: "month" })}>{t("Monthly Recap")}</Button>
            <Button onClick={() => invoke("open_recap", { period: "year" })}>{t("Yearly Recap")}</Button>
          </Row>
        )}
      </Group>
      {enabled && (
        <TokenSpendView
          overview={shown?.overview ?? null}
          pending={shown?.key !== requestKey}
          loading={loading}
          span={span}
          onSpan={setSpan}
          agent={agent}
          onAgent={pickAgent}
          model={model}
          onModel={setModel}
          activityView={activityView}
          onActivityView={setActivityView}
          onRescan={() => setRescan((n) => n + 1)}
        />
      )}
    </div>
  );
}

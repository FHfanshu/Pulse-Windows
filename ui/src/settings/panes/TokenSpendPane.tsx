// Ported from upstream Settings/TokenSpendPane.swift and SpendPaneModel.swift: the pane whose subject is
// every agent's spending added up. The switch turns reading on; nothing is read while it is off.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { monthName } from "../../recap/format";
import { t } from "../../shared/i18n";
import { agentName, registerAgents, spendOverview, spendRelease, type SpendAgent, type SpendOverview, type SpendProgress } from "../../shared/spend";
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
  // The scan is kept in Rust between visits; only a Rescan press (a newer count than the last one read) forces a new one.
  const consumedRescan = useRef(0);
  // The window coming back after it was hidden: its scan was let go, so the figures are read again.
  const [revisit, setRevisit] = useState(0);
  const [shown, setShown] = useState<{ key: string; overview: SpendOverview | null } | null>(null);
  const [loading, setLoading] = useState(false);
  const [progress, setProgress] = useState<SpendProgress | null>(null);

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
    const force = rescan !== consumedRescan.current;
    consumedRescan.current = rescan;
    spendOverview(spanDays[span], agent, model, force)
      .then((overview) => live && setShown({ key: requestKey, overview }))
      .catch(() => live && setShown({ key: requestKey, overview: null }))
      .finally(() => {
        if (!live) return;
        setLoading(false);
        setProgress(null);
      });
    // A newer request supersedes this one: its answer must not replace the current figures.
    return () => { live = false; };
  }, [enabled, span, agent, model, rescan, revisit]); // eslint-disable-line react-hooks/exhaustive-deps

  // "Reading Claude Code…  1/2" while a scan runs; a quiet reread behind figures already on screen sends none.
  useEffect(() => {
    if (!enabled) return;
    const un = listen<SpendProgress>("spend-progress", (e) => {
      registerAgents([{ id: e.payload.agent, name: e.payload.name, icon: null }]);
      setProgress(e.payload);
    });
    return () => { void un.then((off) => off()); };
  }, [enabled]);

  // The kept scan is let go when Token spend is switched off and when the window goes (upstream `release`).
  useEffect(() => {
    if (!enabled) {
      void spendRelease();
      return;
    }
    const onVisibility = () => {
      if (document.hidden) void spendRelease();
      else setRevisit((n) => n + 1);
    };
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("pagehide", spendRelease);
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("pagehide", spendRelease);
    };
  }, [enabled]);

  // A model opened under one agent means nothing under another.
  const pickAgent = (next: SpendAgent | null) => {
    setModel(null);
    setAgent(next);
  };

  // The periods the recap window would open on by itself; the buttons name them ("View September recap").
  const recapMonth = shown?.overview?.recap.month ?? null;
  const recapYear = shown?.overview?.recap.year ?? null;
  const monthNumber = recapMonth ? Number(recapMonth.split("-")[1]) : NaN;
  const recapMonthTitle = Number.isFinite(monthNumber) ? t("View %@ recap", monthName(monthNumber)) : t("Monthly Recap");
  // The year as a plain string, never grouped ("2,026").
  const recapYearTitle = recapYear ? t("View %@ yearly recap", recapYear) : t("Yearly Recap");

  return (
    <div className="pane-stack">
      <Group title={t("Token spend")}>
        <ToggleRow
          title="Read local usage records"
          subtitle="When enabled, scans local records and exports."
          checked={enabled}
          onChange={(readsTokenSpend) => updateSettings({ readsTokenSpend })}
        />
        {enabled && loading && progress && (
          <Row title={t("Reading %@…", agentName[progress.agent])}>
            <span className="num">{progress.index + 1}/{progress.total}</span>
          </Row>
        )}
        {enabled && (
          <Row title={t("Monthly and Yearly Recap")} subtitle={t("Shareable cards for a month or a year, from this Mac's records.")}>
            {/* Both are visible here, so the yearly recap is not something found only inside the window. */}
            <Button onClick={() => invoke("open_recap", { period: recapMonth ?? "month" })}>{recapMonthTitle}</Button>
            <Button onClick={() => invoke("open_recap", { period: recapYear ?? "year" })}>{recapYearTitle}</Button>
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

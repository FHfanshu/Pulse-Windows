// Ported from upstream Panel/FloatingUsagePanelContent.swift and UsageDockView.swift.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { setLanguage } from "../shared/i18n";
import { elapsedFraction, headlineWindow, isSpent, percentText, secondWindow } from "../shared/model";
import { recordsAccount, useSettings, useUsage } from "../shared/settings";
import { Card } from "./Card";
import { readCardSpend, readPromptCache, resetCardLedgers, useCardSpendProviders } from "./cardLedgers";
import { providerNames } from "./Icon";
import { axisOf, defaultMetrics, detailCardLayout, dockLayout, panelSize, PanelSizeScale, RailSpacingScale, type Edge, type PanelMetrics } from "./layout";
import { Ring } from "./Ring";
import { railSlots } from "./slots";
import { berthPath } from "./shapes";
import { GlassLook } from "./glass";
import { ink, LightPanel, Surface } from "./scheme";
import { pulseColors, spring, usageColor } from "./tint";

interface Rect { x: number; y: number; w: number; h: number }
interface PanelLayout { frame: Rect; rail: Rect; edge: Edge; docked: boolean }
interface PointerEvent { point: [number, number] | null; pressed: boolean; dragging: boolean }

const SLACK = 8;
const contains = (r: Rect, x: number, y: number) => x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;

export function App() {
  const { usages, refreshing: refreshingIds } = useUsage();
  const stored = useSettings();
  const [layout, setLayout] = useState<PanelLayout | null>(null);
  const [pointer, setPointer] = useState<[number, number] | null>(null);
  const [dragging, setDragging] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [cardHeight, setCardHeight] = useState<number | null>(null);
  const refreshing = useMemo(() => new Set(refreshingIds), [refreshingIds]);
  const hoveredItem = useRef<string | null>(null);
  // One ring per account, or per model group for a split account (upstream `RailSlot.rail`).
  const slots = useMemo(() => railSlots(usages, stored?.splitAccounts ?? []), [usages, stored?.splitAccounts]);
  const slotsRef = useRef(slots);
  slotsRef.current = slots;

  const settings = {
    showsRemaining: stored?.showsRemaining ?? false,
    warningAt: (stored?.warningThreshold ?? 75) / 100,
    usesGlass: stored?.usesGlass ?? false,
    glassTransparency: stored?.glassTransparency ?? 0.5,
    glassGlitter: stored?.glassGlitter ?? true,
    // Light only for the solid surface: glass is always drawn dark (upstream `panelScheme`).
    light: (stored?.usesLightPanel ?? false) && !(stored?.usesGlass ?? false),
    autoCollapse: stored?.autoCollapse ?? false,
    dockShowsAlertColor: stored?.dockShowsAlertColor ?? true,
    showsSecondRing: stored?.showsSecondRing ?? false,
    showsWindowClock: stored?.showsWindowClock ?? false,
    clockRemaining: stored?.windowClockDirection === "remaining",
    animatesActivity: stored?.animatesRingActivity ?? true,
    pinned: stored?.pinnedWindows ?? {},
    tints: stored?.ringTints ?? {},
  };
  if (stored) setLanguage(stored.language);

  useEffect(() => {
    const un = [
      listen<PanelLayout>("panel-layout", (e) => setLayout(e.payload)),
      listen<PointerEvent>("pointer", (e) => {
        setPointer(e.payload.point);
        setDragging(e.payload.dragging);
      }),
      listen<[number, number]>("rail-click", (e) => clickAt(e.payload[0], e.payload[1])),
    ];
    return () => un.forEach((p) => p.then((f) => f()));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const metrics: PanelMetrics = useMemo(
    () => ({
      ...defaultMetrics,
      scale: PanelSizeScale[stored?.panelSize ?? "standard"],
      spacing: RailSpacingScale[stored?.railSpacing ?? "standard"],
      topRailShowsPercentages: stored?.topRailShowsPercentages ?? false,
      sideRailShowsPercentages: stored?.sideRailShowsPercentages ?? true,
      labelAboveRing: stored?.labelAboveRing ?? false,
      freeAcrossFiguresBeside: stored?.freeAcrossFiguresBeside ?? false,
      usesRoundEnds: stored?.usesRoundEnds ?? false,
      showsWindowClock: stored?.showsWindowClock ?? false,
      showsForecast: stored?.showsForecast ?? false,
      showsDetailedCard: (stored?.detailedCards.length ?? 0) > 0,
      railCapacity: Math.max(slots.length, 1),
    }),
    [stored, slots.length],
  );
  const D = dockLayout(metrics);
  const C = detailCardLayout(metrics);

  // Hand Rust the sizes for every axis/docking so it can place and drag the window.
  useEffect(() => {
    if (!slots.length) return;
    const shapes = (edge: Edge, docked: boolean) => {
      const panel = panelSize(metrics, edge, docked);
      const rail = D.size(slots.length, axisOf(edge), docked);
      return { panel: { w: panel.w, h: panel.h }, rail: { w: rail.w, h: rail.h } };
    };
    invoke("set_geometry", {
      geometry: {
        verticalDocked: shapes("right", true),
        verticalFree: shapes("right", false),
        horizontalDocked: shapes("top", true),
        horizontalFree: shapes("top", false),
      },
    });
  }, [metrics, slots.length]); // eslint-disable-line react-hooks/exhaustive-deps

  const edge = layout?.edge ?? "right";
  const axis = axisOf(edge);
  const docked = layout?.docked ?? true;
  const vertical = axis === "vertical";
  const rail = layout?.rail ?? { x: 0, y: 0, w: 0, h: 0 };

  const entries = slots.map(({ id, account, usage: u }) => {
    const pin = settings.pinned[account];
    return { id, account, usage: u, title: providerNames[u.account.provider] ?? u.account.provider, headline: headlineWindow(u, pin), second: settings.showsSecondRing ? secondWindow(u, pin) : null };
  });
  const selectedIndex = entries.findIndex((e) => e.id === selected);
  const selectedEntry = selectedIndex >= 0 ? entries[selectedIndex] : null;

  // The detailed card's recent activity: this account's detailed switch is on, it is the account this PC's
  // records are shown on (`recordsAccount`), and Token spend is on (upstream `showsSpend`).
  const readsSpend = stored?.readsTokenSpend ?? false;
  const cardSpendProviders = useCardSpendProviders();
  const selectedProvider = selectedEntry?.usage.account.provider ?? null;
  const selectedIsDetailed = !!selectedEntry && (stored?.detailedCards.includes(selectedEntry.account) ?? false);
  const selectedShowsSpend =
    selectedIsDetailed && readsSpend && !!selectedProvider && !!stored && recordsAccount(stored, selectedProvider) === selectedEntry?.account && cardSpendProviders.has(selectedProvider);
  // Fetched on selection and at most every five minutes per provider; the store lives outside React, so a
  // fast sweep across the rings cannot cancel a read.
  useEffect(() => {
    if (!selectedShowsSpend || !selectedProvider) return;
    readCardSpend(selectedProvider);
    readPromptCache(selectedProvider);
  }, [selectedShowsSpend, selectedProvider, selected]);
  useEffect(() => resetCardLedgers, [readsSpend]);

  const ringCentre = (i: number) => D.firstRingAlong(docked, axis) + i * D.ringStep(axis, docked);
  const ringCentreAcross = D.ringCentreAcross(axis, docked);
  const panelAlong = vertical ? layout?.frame.h ?? 0 : layout?.frame.w ?? 0;
  const railAlong = vertical ? rail.y : rail.x;
  const cardAlong = vertical ? cardHeight ?? C.estimatedHeight : C.width;
  const cardPadding = (i: number) => {
    const raw = ringCentre(i) - cardAlong / 2;
    const first = -railAlong;
    const last = Math.max(panelAlong - railAlong - cardAlong, first);
    return Math.min(Math.max(raw, first), last);
  };
  const pointerCentre = (i: number) => {
    const raw = ringCentre(i) - cardPadding(i);
    const inset = C.cornerRadius + C.pointerHeight / 2;
    const first = Math.min(inset, cardAlong / 2);
    const last = Math.max(cardAlong - inset, first);
    return Math.min(Math.max(raw, first), last);
  };

  /** Item rects in window coordinates, for hover selection (upstream PointerEntryReporter). */
  const itemRect = (i: number): Rect => {
    const start = D.endPadding(docked) + i * D.ringStep(axis, docked);
    const len = D.itemLength(axis, docked);
    const across = D.thickness(axis, docked);
    return vertical
      ? { x: rail.x, y: rail.y + start, w: across, h: len }
      : { x: rail.x + start, y: rail.y, w: len, h: across };
  };

  const cardBand = (): Rect | null => {
    if (selectedIndex < 0 || !layout) return null;
    const start = railAlong + cardPadding(selectedIndex) - SLACK;
    const length = cardAlong + SLACK * 2;
    return vertical
      ? { x: 0, y: start, w: layout.frame.w, h: length }
      : { x: start, y: 0, w: length, h: layout.frame.h };
  };

  // Pointer → hover selection and deselection (upstream pointerMoved / isOverContent).
  useEffect(() => {
    if (!layout || dragging) {
      if (dragging) setSelected(null);
      return;
    }
    if (!pointer) {
      hoveredItem.current = null;
      setSelected(null);
      return;
    }
    const [x, y] = pointer;
    const band = cardBand();
    const over = contains(rail, x, y) || (band ? contains(band, x, y) : false);
    if (!over) {
      hoveredItem.current = null;
      setSelected(null);
      return;
    }
    const hit = entries.findIndex((_, i) => contains(itemRect(i), x, y));
    const id = hit >= 0 ? entries[hit].id : null;
    if (id && id !== hoveredItem.current) setSelected(id);
    hoveredItem.current = id;
  }, [pointer, dragging, layout]); // eslint-disable-line react-hooks/exhaustive-deps

  // Tell Rust where input belongs; everything else stays click-through.
  useEffect(() => {
    if (!layout) return;
    const band = cardBand();
    invoke("set_hit_rects", { rects: band ? [rail, band] : [rail], grab: rail });
  }, [layout, selected, cardHeight]); // eslint-disable-line react-hooks/exhaustive-deps

  const clickAt = useCallback((x: number, y: number) => {
    setLayout((current) => {
      if (!current) return current;
      // Ring hit test: within 1.08 × radius of a ring centre (upstream PanelHitArea.slot).
      {
        const list = slotsRef.current;
        const ax = axisOf(current.edge);
        const radius = (D.ringDiameter / 2) * 1.08;
        const across = D.ringCentreAcross(ax, current.docked);
        list.forEach((u, i) => {
          const along = D.firstRingAlong(current.docked, ax) + i * D.ringStep(ax, current.docked);
          const cx = current.rail.x + (ax === "vertical" ? across : along);
          const cy = current.rail.y + (ax === "vertical" ? along : across);
          if ((x - cx) ** 2 + (y - cy) ** 2 <= radius * radius) {
            invoke("refresh", { account: u.account });
          }
        });
      }
      return current;
    });
  }, [D]);

  const alert = useMemo(() => {
    if (!settings.dockShowsAlertColor) return null;
    let worst: ReturnType<typeof headlineWindow> = null;
    for (const e of entries) if (e.headline && (!worst || e.headline.usedFraction > worst.usedFraction)) worst = e.headline;
    if (!worst || !(worst.isExhausted || worst.usedFraction >= settings.warningAt)) return null;
    return usageColor(worst.usedFraction, worst.isExhausted, settings.warningAt, settings.light);
  }, [entries]); // eslint-disable-line react-hooks/exhaustive-deps
  void alert; // used by the collapsed sliver (auto-collapse), phase 1b

  if (!layout || !slots.length) return null;

  const railSize = D.size(entries.length, axis, docked);
  const berth = berthPath(railSize.w, railSize.h, edge, docked, 1, {
    flareHeight: D.flareHeight, flareWidth: D.flareWidth, cornerRadius: D.cornerRadius,
    collapsedWidth: D.collapsedWidth, usesRoundEnds: metrics.usesRoundEnds,
  });

  // Card placement relative to the rail (upstream cardAlignment + cardOffset + padding).
  const reach = C.width + C.pointerWidth + C.horizontalGap;
  const cardStyle = (i: number): { left: number; top: number } => {
    const along = cardPadding(i);
    switch (edge) {
      case "right": return { left: rail.x - reach, top: rail.y + along };
      case "left": return { left: rail.x + rail.w + C.horizontalGap, top: rail.y + along };
      case "top": return { left: rail.x + along, top: rail.y + rail.h + C.horizontalGap };
      case "bottom": return { left: rail.x + along, top: rail.y - C.horizontalGap - (cardHeight ?? C.estimatedHeight) - C.pointerWidth };
    }
  };
  const direction = edge === "left" || edge === "top" ? 1 : -1;
  const revealOrigin = (index: number) => {
    const gap = cardPadding(index);
    const box = gap + cardAlong;
    const alongRatio = box > 0 ? Math.min(Math.max((gap + cardAlong / 2) / box, 0), 1) : 0.5;
    const acrossRatio = edge === "left" || edge === "top" ? 0 : 1;
    return vertical ? `${acrossRatio * 100}% ${alongRatio * 100}%` : `${alongRatio * 100}% ${acrossRatio * 100}%`;
  };
  const slide = vertical ? { x: 10 * direction } : { y: 10 * direction };

  return (
    <LightPanel.Provider value={settings.light}>
    <GlassLook.Provider value={{ glitter: settings.glassGlitter }}>
    <div className={`panel${settings.light ? " light" : ""}`} onContextMenu={(e) => { e.preventDefault(); invoke("open_settings"); }}>
      <div
        className="rail"
        style={{ left: rail.x, top: rail.y, width: railSize.w, height: railSize.h }}
        onPointerDown={(e) => { if (e.button === 0) invoke("rail_press", { x: e.clientX, y: e.clientY }); }}
      >
        <svg className="berth" width={railSize.w} height={railSize.h}>
          <Surface
            d={berth}
            width={railSize.w}
            height={railSize.h}
            usesGlass={settings.usesGlass}
            glassTransparency={settings.glassTransparency}
            light={settings.light}
            screenEdge={docked ? edge : null}
          />
        </svg>
        <div
          className="rings"
          style={{
            flexDirection: vertical ? "column" : "row",
            gap: D.gap(axis, docked),
            padding: vertical
              ? `${D.endPadding(docked)}px ${D.crossPadding(axis, docked)}px`
              : `${D.crossPadding(axis, docked)}px ${D.endPadding(docked)}px`,
          }}
        >
          {entries.map((e) => {
            const second = e.second;
            const clock = settings.showsWindowClock && e.headline ? elapsedFraction(e.headline) : null;
            const isSel = selected === e.id;
            const label = D.showsPercentages(axis) ? (
              <div
                className="percent"
                style={{
                  fontSize: D.percentFontSize,
                  height: D.percentTextHeight,
                  lineHeight: `${D.percentTextHeight}px`,
                  color: isSpent(e.headline) ? pulseColors(settings.light).exhausted : ink(settings.light, e.headline ? 1 : 0.4),
                  opacity: refreshing.has(e.account) ? 0.4 : 1,
                }}
              >
                {e.headline ? percentText(e.headline, settings.showsRemaining) : "—"}
              </div>
            ) : null;
            return (
              <div
                key={e.id}
                className="item"
                style={{
                  flexDirection: "column",
                  gap: D.ringToTextSpacing,
                  [vertical ? "height" : "width"]: D.itemLength(axis, docked),
                }}
              >
                {D.labelLeads && label}
                <motion.div animate={{ scale: isSel ? 1.06 : 1 }} transition={spring(0.28, 0.84)}>
                  <Ring
                    provider={e.usage.account.provider}
                    usedFraction={e.headline?.usedFraction ?? null}
                    hasReading={!!e.headline}
                    isSpent={isSpent(e.headline)}
                    showsRemaining={settings.showsRemaining}
                    diameter={D.ringDiameter}
                    lineWidth={D.ringLineWidth}
                    scale={metrics.scale}
                    isRefreshing={refreshing.has(e.account)}
                    animatesActivity={settings.animatesActivity}
                    chosenTint={settings.tints[e.account] ?? null}
                    windowClockFraction={clock == null ? null : settings.clockRemaining ? 1 - clock : clock}
                    highlight={isSel}
                    secondFraction={second?.usedFraction ?? null}
                    secondIsSpent={isSpent(second)}
                    secondDiameter={D.secondRingDiameter}
                    secondLineWidth={D.secondRingLineWidth}
                    warningAt={settings.warningAt}
                  />
                </motion.div>
                {!D.labelLeads && label}
              </div>
            );
          })}
        </div>
      </div>

      <AnimatePresence>
        {selectedEntry && (
          <motion.div
            key="card"
            className="card-host"
            style={{ position: "absolute", transformOrigin: revealOrigin(selectedIndex) }}
            initial={{ opacity: 0, scale: 0.88, ...slide, ...cardStyle(selectedIndex) }}
            animate={{ opacity: 1, scale: 1, x: 0, y: 0, ...cardStyle(selectedIndex) }}
            exit={{ opacity: 0, scale: 0.88, ...slide }}
            transition={spring(0.28, 0.84)}
          >
            <Card
              usage={selectedEntry.usage}
              title={selectedEntry.title}
              edge={edge}
              metrics={metrics}
              pointerCenter={pointerCentre(selectedIndex)}
              showsRemaining={settings.showsRemaining}
              warningAt={settings.warningAt}
              usesGlass={settings.usesGlass}
              glassTransparency={settings.glassTransparency}
              detailed={selectedIsDetailed}
              showsSpend={selectedShowsSpend}
              onHeight={(h) => setCardHeight((old) => (old != null && Math.abs(old - h) < 0.5 ? old : h))}
            />
          </motion.div>
        )}
      </AnimatePresence>
      <span hidden>{ringCentreAcross}</span>
    </div>
    </GlassLook.Provider>
    </LightPanel.Provider>
  );
}

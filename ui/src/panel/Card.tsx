// Ported from upstream Panel/UsageDetailCard.swift. A detailed card adds the plan, the "Updated" line and the
// activity section (Activity.tsx).
import { animate, AnimatePresence, motion, type AnimationPlaybackControls } from "motion/react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { relativeTime, resetText, unavailableMessage, windowName } from "../shared/copy";
import { t } from "../shared/i18n";
import { accountId, isSpent, percentText, remainingFraction, type ProviderUsage, type UsageWindow } from "../shared/model";
import { approximate, intlLocale } from "../shared/spend";
import { ActivitySection, activityState } from "./Activity";
import { useCardLedger, useEstimatedValues } from "./cardLedgers";
import { ProviderIcon } from "./Icon";
import type { Edge } from "./layout";
import { detailCardLayout, type PanelMetrics } from "./layout";
import { useCardOutline } from "./nativeShapes";
import { bubblePath } from "./shapes";
import { ink, Surface, useLightPanel } from "./scheme";
import { pulseColors, spring, usageColor } from "./tint";

export interface CardProps {
  usage: ProviderUsage;
  title: string;
  edge: Edge;
  metrics: PanelMetrics;
  pointerCenter: number;
  showsRemaining: boolean;
  warningAt: number;
  usesGlass: boolean;
  glassTransparency: number;
  /** The native acrylic is drawn under the card (else the fallback gradient). */
  nativeBackdrop: boolean;
  /** The tallest the card may be, pointer excluded, or null for no limit: the room the screen leaves it
   *  (upstream `UsageDetailCard.maxHeight`). A taller card scrolls inside the same outline. */
  maxHeight: number | null;
  /** The detailed card: the plan, how fresh the figures are and (below) recent activity. Set per account. */
  detailed: boolean;
  /** Whether this account has records to show: detailed, the account the records are shown on, Token spend on. */
  showsSpend: boolean;
  onHeight: (h: number) => void;
}

/** "Updated 3 min. ago". */
function updatedText(iso: string): string {
  const seconds = (Date.parse(iso) - Date.now()) / 1000;
  if (-seconds < 60) return t("Updated just now");
  const rtf = new Intl.RelativeTimeFormat(intlLocale(), { numeric: "auto", style: "short" });
  const abs = Math.abs(seconds);
  const ago = abs < 3600 ? rtf.format(Math.round(seconds / 60), "minute")
    : abs < 86_400 ? rtf.format(Math.round(seconds / 3600), "hour")
    : rtf.format(Math.round(seconds / 86_400), "day");
  return t("Updated %@", ago);
}

const rowTransition = {
  initial: { opacity: 0, y: -6 },
  animate: { opacity: 1, y: 0, transition: { duration: 0.14, delay: 0.06, ease: "easeOut" } },
  exit: { opacity: 0, transition: { duration: 0.06, ease: "easeOut" } },
} as const;

export function Card(p: CardProps) {
  const L = detailCardLayout(p.metrics);
  const light = useLightPanel();
  const ref = useRef<HTMLDivElement>(null);
  const vertical = p.edge === "left" || p.edge === "right";
  const pointerSide = { left: "paddingLeft", right: "paddingRight", top: "paddingTop", bottom: "paddingBottom" }[p.edge];

  // offsetHeight, not getBoundingClientRect: the reveal animation scales the
  // card, and a transformed measurement would size the outline too short.
  const [contentH, setContentH] = useState<number>(L.estimatedHeight);
  // Upstream `bounded`: the contents scroll **only when the screen cannot hold them**, inside the same
  // outline, which is then as tall as the room.
  const limit = p.maxHeight == null ? Infinity : Math.max(p.maxHeight, 0);
  const bounded = contentH > limit + 0.5;
  const targetH = bounded ? limit : contentH;
  const limitRef = useRef(limit);
  limitRef.current = limit;
  const scroller = useRef<HTMLDivElement>(null);
  // The outline's height. Hovering from one ring to another swaps the content under the same card,
  // and the bubble grows or shrinks to the new content on the spring that moves the card (upstream
  // animates the shape with the selection), rather than snapping while the card is still sliding.
  const [shownH, setShownH] = useState<number>(Math.min(L.estimatedHeight, limit));
  const shownRef = useRef(shownH);
  const measured = useRef(false);
  const running = useRef<AnimationPlaybackControls | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const report = () => {
      const h = el.offsetHeight;
      // The first measurement is the card appearing: it is drawn at its size straight away.
      if (!measured.current) {
        measured.current = true;
        shownRef.current = Math.min(h, limitRef.current);
        setShownH(shownRef.current);
      }
      setContentH((old) => (Math.abs(old - h) < 0.5 ? old : h));
    };
    report();
    const ro = new ResizeObserver(report);
    ro.observe(el);
    return () => ro.disconnect();
  });
  useEffect(() => {
    // Stop the run towards an earlier target first (the estimate, before the first measurement).
    running.current?.stop();
    if (Math.abs(shownRef.current - targetH) < 0.5) return;
    running.current = animate(shownRef.current, targetH, {
      ...spring(0.28, 0.84),
      onUpdate: (v) => {
        shownRef.current = v;
        setShownH(v);
      },
    });
  }, [targetH]);
  useEffect(() => () => running.current?.stop(), []);
  // A changing work area and spring overshoot must never draw beyond the available room.
  const drawnH = Math.min(Math.max(shownH, 0), limit);
  // Placement, hit testing and both outlines follow the height actually drawn, including during the spring.
  const reported = useRef(p.onHeight);
  reported.current = p.onHeight;
  useLayoutEffect(() => reported.current(drawnH), [drawnH]);
  // Another ring's card starts at its top.
  const cardKey = `${p.usage.account.provider}|${p.usage.account.slot}|${p.usage.windows.map((w) => w.id).join(",")}`;
  useEffect(() => {
    if (scroller.current) scroller.current.scrollTop = 0;
  }, [cardKey]);

  const totalW = L.width + (vertical ? L.pointerWidth : 0);
  const totalH = drawnH + (vertical ? 0 : L.pointerWidth);
  const path = bubblePath(totalW, totalH, p.edge, p.pointerCenter, {
    cornerRadius: L.cornerRadius,
    pointerWidth: L.pointerWidth,
    pointerHeight: L.pointerHeight,
    usesRoundEnds: p.metrics.usesRoundEnds,
  });

  const surface = useRef<SVGSVGElement>(null);
  useCardOutline(surface, path, totalW);

  const u = p.usage;
  const unavailable = u.state.kind === "unavailable" ? u.state.reason : null;
  const saysNothing = u.windows.length === 0 && u.creditBalance == null && !unavailable;
  const footnote = u.state.kind === "stale"
    ? (u.observedAt ? t("As of %@", relativeTime(u.observedAt)) : t("Reading may be out of date"))
    : null;

  const font = (size: number, weight = 400) => ({ fontSize: size, fontWeight: weight, lineHeight: `${Math.round(size * 1.2)}px` });

  // The detailed card's extras. Read here, not in the section, so a card with nothing to say has no gap for it.
  const ledger = useCardLedger(u.account.provider);
  const activity = p.detailed && p.showsSpend ? activityState(ledger) : null;
  const plan = p.detailed && u.plan ? u.plan : null;
  // What a limit is worth, under the limit: this PC's spend since the window opened over the percentage the
  // provider reports. Withheld wherever the estimator withholds it, and for a window seen spent elsewhere.
  const values = useEstimatedValues(
    accountId(u.account),
    `${u.observedAt ?? ""}|${u.windows.map((w) => `${w.id}:${w.usedFraction}`).join(",")}`,
    p.detailed && p.showsSpend && u.state.kind === "live",
  );
  const valueText = (w: UsageWindow): string | null => {
    const v = values.find((x) => x.window === w.id);
    return v && !v.elsewhere
      ? t("Estimated value %@ · %@ used", approximate(v.full), approximate(v.spent))
      : null;
  };
  const updated = p.detailed && u.state.kind === "live" && u.observedAt ? updatedText(u.observedAt) : null;
  const head = (
    <>
      <ProviderIcon provider={u.account.provider} size={L.headerIconSize} />
      <span className="card-title" style={font(L.titleFontSize, 600)}>
        {t("%@ Usage", p.title)}
      </span>
      {/* The plan as the provider names it. It gives way to the title, which says whose card this is. */}
      {plan && <span className="card-plan" style={font(L.footnoteFontSize, 500)}>{plan}</span>}
    </>
  );

  return (
    <div className={`card ${p.usesGlass ? "glass" : ""}`} style={{ width: totalW, height: totalH, [pointerSide]: L.pointerWidth } as React.CSSProperties}>
      <svg ref={surface} className="card-surface" width={totalW} height={totalH}>
        <Surface d={path} width={totalW} height={totalH} usesGlass={p.usesGlass} glassTransparency={p.glassTransparency} light={light} nativeBackdrop={p.nativeBackdrop} />
      </svg>
      <div className="card-clip" style={{ clipPath: `path("${path}")`, width: totalW, height: totalH, position: "absolute", left: 0, top: 0 }}>
        <div
          ref={scroller}
          className={`card-scroll${bounded ? " scrolls" : ""}`}
          style={{
            width: L.width,
            height: drawnH,
            position: "absolute",
            left: p.edge === "left" ? L.pointerWidth : 0,
            top: p.edge === "top" ? L.pointerWidth : 0,
          }}
        >
        <div
          ref={ref}
          className="card-content"
          style={{
            // The scrollbar takes its share of the width: the contents fit what is left.
            width: bounded ? undefined : L.width,
            padding: L.padding,
            gap: L.contentSpacing,
            position: "relative",
          }}
        >
          {/* Hovering to another ring swaps the whole content as one layer: the old one fades where
              it is, lifted out of the layout so it neither pushes the new one down nor counts in the
              measured height, while the new one fades in at the top (the upstream demo, ~130 ms). */}
          <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={`${u.account.provider}|${u.account.slot}`}
            className="card-layer"
            style={{ display: "flex", flexDirection: "column", gap: L.contentSpacing, width: "100%" }}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1, transition: { duration: 0.14, ease: "easeOut" } }}
            exit={{ opacity: 0, transition: { duration: 0.12, ease: "easeOut" } }}
          >
          <div className="card-header" style={{ height: p.detailed ? undefined : L.headerHeight, minHeight: L.headerHeight, gap: 8 }}>
              <div className="card-header-line" style={p.detailed ? { flex: "1 1 auto" } : undefined}>
                {p.detailed ? (
                  <div className="card-header-stack" style={{ gap: L.headerLineSpacing }}>
                    <div className="card-header-top" style={{ minHeight: L.headerHeight }}>{head}</div>
                    {updated && (
                      // Level with the title, not the icon.
                      <div className="ellipsis" style={{ ...font(L.footnoteFontSize), opacity: 0.4, paddingLeft: L.headerIconSize + 8 }}>{updated}</div>
                    )}
                  </div>
                ) : head}
              </div>
          </div>

          <AnimatePresence initial={false}>
            {u.windows.map((w) => (
              <motion.div key={`${u.account.provider}|${w.id}`} {...rowTransition}>
                <MetricRow window={w} L={L} showsRemaining={p.showsRemaining} warningAt={p.warningAt} value={valueText(w)} />
              </motion.div>
            ))}
          </AnimatePresence>

          {u.windows.length === 0 && u.creditBalance != null && (
            <div className="value-row" style={font(L.rowFontSize)}>
              <span>{t("Credit balance")}</span>
              <span style={{ fontWeight: 500, opacity: 0.9 }}>{u.creditBalance}</span>
            </div>
          )}

          {saysNothing && <Message text={unavailableMessage("noLimitsReported")} size={L.messageFontSize} />}
          {unavailable && <Message text={unavailableMessage(unavailable)} size={L.messageFontSize} />}
          {footnote && <div style={{ ...font(L.footnoteFontSize), opacity: 0.4 }}>{footnote}</div>}

          <AnimatePresence initial={false}>
            {activity && (
              <motion.div key="activity" {...rowTransition}>
                <ActivitySection state={activity} L={L} provider={u.account.provider} promptCache={ledger.promptCache} />
              </motion.div>
            )}
          </AnimatePresence>
          </motion.div>
          </AnimatePresence>
        </div>
        </div>
      </div>
    </div>
  );
}

function Message({ text, size }: { text: string; size: number }) {
  return <div style={{ fontSize: size, lineHeight: 1.25, opacity: 0.55 }}>{text}</div>;
}

function MetricRow({ window: w, L, showsRemaining, warningAt, value }: { window: UsageWindow; L: ReturnType<typeof detailCardLayout>; showsRemaining: boolean; warningAt: number; value: string | null }) {
  const progress = showsRemaining ? remainingFraction(w) : Math.min(Math.max(w.usedFraction, 0), 1);
  const spent = isSpent(w);
  const light = useLightPanel();
  const accent = usageColor(w.usedFraction, w.isExhausted, warningAt, light);
  const figure = showsRemaining ? t("%@ Left", percentText(w, true)) : t("%@ Used", percentText(w));
  const line = { fontSize: L.rowFontSize, lineHeight: `${L.rowTextLineHeight}px`, height: L.rowTextLineHeight };
  return (
    <div className="metric-row" style={{ gap: L.rowInternalSpacing }}>
      <div className="ellipsis" style={line}>{windowName(w)}</div>
      <div className="bar" style={{ height: L.progressBarHeight }}>
        <motion.div
          className="bar-fill"
          style={{ background: accent }}
          initial={false}
          animate={{ width: `${progress * 100}%` }}
          transition={{ duration: 0.35, ease: "easeOut" }}
        />
      </div>
      <div className="metric-foot" style={line}>
        <span style={{ fontWeight: 500, color: spent ? pulseColors(light).exhausted : ink(light, 0.9) }}>{figure}</span>
        <span className="ellipsis" style={{ opacity: 0.45 }}>{resetText(w)}</span>
      </div>
      {/* Dimmer than the reported figures above it: it is the one number here the provider did not say. */}
      {value && <div className="ellipsis" style={{ ...line, opacity: 0.45 }}>{value}</div>}
    </div>
  );
}

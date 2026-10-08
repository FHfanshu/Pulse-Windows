// Ported from upstream Panel/UsageDetailCard.swift (compact card; the detailed
// activity section arrives with Token spend in phase 4).
import { AnimatePresence, motion } from "motion/react";
import { useLayoutEffect, useRef, useState } from "react";
import { relativeTime, resetText, unavailableMessage, windowName } from "../shared/copy";
import { t } from "../shared/i18n";
import { isSpent, percentText, remainingFraction, type ProviderUsage, type UsageWindow } from "../shared/model";
import { ProviderIcon } from "./Icon";
import type { Edge } from "./layout";
import { detailCardLayout, type PanelMetrics } from "./layout";
import { bubblePath } from "./shapes";
import { PulseColor, usageColor } from "./tint";

export interface CardProps {
  usage: ProviderUsage;
  title: string;
  edge: Edge;
  metrics: PanelMetrics;
  pointerCenter: number;
  showsRemaining: boolean;
  warningAt: number;
  usesGlass: boolean;
  onHeight: (h: number) => void;
}

const rowTransition = {
  initial: { opacity: 0, y: -6 },
  animate: { opacity: 1, y: 0, transition: { duration: 0.14, delay: 0.06, ease: "easeOut" } },
  exit: { opacity: 0, transition: { duration: 0.06, ease: "easeOut" } },
} as const;

export function Card(p: CardProps) {
  const L = detailCardLayout(p.metrics);
  const ref = useRef<HTMLDivElement>(null);
  const vertical = p.edge === "left" || p.edge === "right";
  const pointerSide = { left: "paddingLeft", right: "paddingRight", top: "paddingTop", bottom: "paddingBottom" }[p.edge];

  // offsetHeight, not getBoundingClientRect: the reveal animation scales the
  // card, and a transformed measurement would size the outline too short.
  const [contentH, setContentH] = useState<number>(L.estimatedHeight);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const report = () => {
      setContentH((old) => (Math.abs(old - el.offsetHeight) < 0.5 ? old : el.offsetHeight));
      p.onHeight(el.offsetHeight);
    };
    report();
    const ro = new ResizeObserver(report);
    ro.observe(el);
    return () => ro.disconnect();
  });

  const totalW = L.width + (vertical ? L.pointerWidth : 0);
  const totalH = contentH + (vertical ? 0 : L.pointerWidth);
  const path = bubblePath(totalW, totalH, p.edge, p.pointerCenter, {
    cornerRadius: L.cornerRadius,
    pointerWidth: L.pointerWidth,
    pointerHeight: L.pointerHeight,
    usesRoundEnds: p.metrics.usesRoundEnds,
  });

  const u = p.usage;
  const unavailable = u.state.kind === "unavailable" ? u.state.reason : null;
  const saysNothing = u.windows.length === 0 && u.creditBalance == null && !unavailable;
  const footnote = u.state.kind === "stale"
    ? (u.observedAt ? t("As of %@", relativeTime(u.observedAt)) : t("Reading may be out of date"))
    : null;

  const font = (size: number, weight = 400) => ({ fontSize: size, fontWeight: weight, lineHeight: `${Math.round(size * 1.2)}px` });

  return (
    <div className={`card ${p.usesGlass ? "glass" : ""}`} style={{ width: totalW, height: totalH, [pointerSide]: L.pointerWidth } as React.CSSProperties}>
      <svg className="card-surface" width={totalW} height={totalH}>
        <path d={path} fill={p.usesGlass ? "rgba(0,0,0,var(--glass-dim))" : "#000"} />
      </svg>
      <div className="card-clip" style={{ clipPath: `path("${path}")`, width: totalW, height: totalH, position: "absolute", left: 0, top: 0 }}>
        <div
          ref={ref}
          className="card-content"
          style={{
            width: L.width,
            padding: L.padding,
            gap: L.contentSpacing,
            position: "absolute",
            left: p.edge === "left" ? L.pointerWidth : 0,
            top: p.edge === "top" ? L.pointerWidth : 0,
          }}
        >
          <div className="card-header" style={{ height: L.headerHeight, gap: 8 }}>
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.div
                key={`${u.account.provider}|${u.account.slot}|${p.title}`}
                className="card-header-line"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1, transition: { duration: 0.1, ease: "easeOut" } }}
                exit={{ opacity: 0, transition: { duration: 0.06, ease: "easeOut" } }}
              >
                <ProviderIcon provider={u.account.provider} size={L.headerIconSize} />
                <span className="card-title" style={font(L.titleFontSize, 600)}>
                  {t("%@ Usage", p.title)}
                </span>
              </motion.div>
            </AnimatePresence>
          </div>

          <AnimatePresence initial={false}>
            {u.windows.map((w) => (
              <motion.div key={`${u.account.provider}|${w.id}`} {...rowTransition}>
                <MetricRow window={w} L={L} showsRemaining={p.showsRemaining} warningAt={p.warningAt} />
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
        </div>
      </div>
    </div>
  );
}

function Message({ text, size }: { text: string; size: number }) {
  return <div style={{ fontSize: size, lineHeight: 1.25, opacity: 0.55 }}>{text}</div>;
}

function MetricRow({ window: w, L, showsRemaining, warningAt }: { window: UsageWindow; L: ReturnType<typeof detailCardLayout>; showsRemaining: boolean; warningAt: number }) {
  const progress = showsRemaining ? remainingFraction(w) : Math.min(Math.max(w.usedFraction, 0), 1);
  const spent = isSpent(w);
  const accent = usageColor(w.usedFraction, w.isExhausted, warningAt);
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
        <span style={{ fontWeight: 500, color: spent ? PulseColor.exhausted : "rgba(255,255,255,0.9)" }}>{figure}</span>
        <span className="ellipsis" style={{ opacity: 0.45 }}>{resetText(w)}</span>
      </div>
    </div>
  );
}

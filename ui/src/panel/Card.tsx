// Ported from upstream Panel/UsageDetailCard.swift. A detailed card adds the plan, the "Updated" line and the
// activity section (Activity.tsx).
import { AnimatePresence, motion } from "motion/react";
import { useLayoutEffect, useRef, useState } from "react";
import { relativeTime, resetText, unavailableMessage, windowName } from "../shared/copy";
import { t } from "../shared/i18n";
import { isSpent, percentText, remainingFraction, type ProviderUsage, type UsageWindow } from "../shared/model";
import { intlLocale } from "../shared/spend";
import { ActivitySection, activityState } from "./Activity";
import { useCardLedger } from "./cardLedgers";
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
  /** The detailed card: the plan, how fresh the figures are and (below) recent activity. Set per account. */
  detailed: boolean;
  /** Whether this account has records to show: detailed, a primary account, Token spend on. */
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

  // The detailed card's extras. Read here, not in the section, so a card with nothing to say has no gap for it.
  const ledger = useCardLedger(u.account.provider);
  const activity = p.detailed && p.showsSpend ? activityState(ledger) : null;
  const plan = p.detailed && u.plan ? u.plan : null;
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
          <div className="card-header" style={{ height: p.detailed ? undefined : L.headerHeight, minHeight: L.headerHeight, gap: 8 }}>
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.div
                key={`${u.account.provider}|${u.account.slot}|${p.title}`}
                className="card-header-line"
                style={p.detailed ? { flex: "1 1 auto" } : undefined}
                initial={{ opacity: 0 }}
                animate={{ opacity: 1, transition: { duration: 0.1, ease: "easeOut" } }}
                exit={{ opacity: 0, transition: { duration: 0.06, ease: "easeOut" } }}
              >
                {p.detailed ? (
                  <div className="card-header-stack" style={{ gap: L.headerLineSpacing }}>
                    <div className="card-header-top">{head}</div>
                    {updated && (
                      // Level with the title, not the icon.
                      <div className="ellipsis" style={{ ...font(L.footnoteFontSize), opacity: 0.4, paddingLeft: L.headerIconSize + 8 }}>{updated}</div>
                    )}
                  </div>
                ) : head}
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

          <AnimatePresence initial={false}>
            {activity && (
              <motion.div key="activity" {...rowTransition}>
                <ActivitySection state={activity} L={L} provider={u.account.provider} promptCache={ledger.promptCache} />
              </motion.div>
            )}
          </AnimatePresence>
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

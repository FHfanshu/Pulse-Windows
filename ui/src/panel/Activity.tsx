// Ported from upstream Panel/UsageDetailCard.swift: the detailed card's last section (ActivitySection,
// DaysChart, PromptCacheRow). Every part is drawn at the height `detailCardLayout` names, so the panel's
// fixed frame and the drawing cannot drift apart.
//
// Tokens lead, money follows: the tokens are counted; the money is those counts at API prices, which a
// subscription does not pay, so it is the smaller, dimmer line, marked as approximate.
import { useEffect, useState } from "react";
import { t } from "../shared/i18n";
import {
  duration, money, sessionLabel, tokenCount, tokensText,
  type CardSpend, type LedgerDay, type PromptCacheLapse, type PromptCacheReading,
} from "../shared/spend";
import type { CardLedger } from "./cardLedgers";
import type { detailCardLayout } from "./layout";
import { ink, useLightPanel } from "./scheme";

type Layout = ReturnType<typeof detailCardLayout>;

/** The panel's size scale, which the layout's heights are all multiples of (a figure label is 13 at scale 1). */
const scaleOf = (L: Layout) => L.figureLabelHeight / 13;

/** The chart's span, and the longest of the three figures. */
const SPAN = 31;
/** Past this, a lapsed cache is not worth a line: the session is over. */
const STALE_AFTER_MS = 24 * 3600 * 1000;

/** What the card says for this provider; null when it has no section (Token spend off, or nothing to read). */
export type ActivityState =
  | { kind: "reading" }
  | { kind: "empty" }
  | { kind: "failed" }
  | { kind: "ledger"; spend: CardSpend };

export function activityState(ledger: CardLedger): ActivityState | null {
  if (ledger.spend) return ledger.spend.days.length === 0 ? { kind: "empty" } : { kind: "ledger", spend: ledger.spend };
  if (ledger.reading || ledger.readAt === null) return { kind: "reading" };
  // Answered with null: Token spend is off, or this provider keeps no records here.
  return ledger.failed ? { kind: "failed" } : null;
}


export function ActivitySection({ state, L, provider, promptCache }: {
  state: ActivityState;
  L: Layout;
  provider: string;
  promptCache: PromptCacheReading | null;
}) {
  const light = useLightPanel();
  const line = (size: number, height: number) => ({ fontSize: size, height, lineHeight: `${height}px` });
  const message = (text: string) => (
    <div className="ellipsis" style={{ ...line(L.rowFontSize, L.rowTextLineHeight), opacity: 0.45 }}>{text}</div>
  );
  return (
    <div className="activity" style={{ display: "flex", flexDirection: "column", gap: L.activitySpacing }}>
      <div style={{ height: 1, background: ink(light, 0.12) }} />
      <div style={{ ...line(L.footnoteFontSize, L.figureLabelHeight), fontWeight: 500, opacity: 0.5 }}>{t("On this Mac")}</div>
      {state.kind === "reading" && message(t("Reading local records…"))}
      {state.kind === "empty" && message(t("No history yet"))}
      {state.kind === "failed" && message(t("Couldn't read the history."))}
      {state.kind === "ledger" && <Ledger spend={state.spend} L={L} promptCache={promptCache} provider={provider} />}
    </div>
  );
}

function Ledger({ spend, L, promptCache }: { spend: CardSpend; L: Layout; promptCache: PromptCacheReading | null; provider: string }) {
  // No money line at all where nothing is priced, rather than a blank band under the figures.
  const priced = spend.month.cost > 0;
  const row = { display: "flex", alignItems: "baseline", gap: 8, fontSize: L.rowFontSize, height: L.rowTextLineHeight, lineHeight: `${L.rowTextLineHeight}px`, whiteSpace: "nowrap" } as const;
  const chartWidth = L.width - 2 * L.padding;
  const top = spend.topModel;

  return (
    <>
      <div style={{ display: "flex", alignItems: "flex-start", gap: 8, height: priced ? L.figuresHeight : L.unpricedFiguresHeight }}>
        <Figure label={t("Today")} {...spend.today} priced={priced} currency={spend.currency} L={L} />
        <Figure label={t("7 days")} {...spend.week} priced={priced} currency={spend.currency} L={L} />
        <Figure label={t("31 days")} {...spend.month} priced={priced} currency={spend.currency} L={L} />
      </div>

      <DaysChart days={spend.days} slots={SPAN} width={chartWidth} height={L.chartHeight} />

      <div style={row}>
        <span style={{ opacity: 0.45 }}>{t("Top model")}</span>
        <span style={{ flex: 1 }} />
        {top ? (
          // A model id can run to thirty characters; the share after it always stays.
          <span style={{ display: "flex", minWidth: 0, opacity: 0.9 }}>
            <span className="ellipsis">{top[0]}</span>
            <span style={{ flex: "none", whiteSpace: "pre" }}>{` · ${Math.round(top[1] * 100)}%`}</span>
          </span>
        ) : (
          <span style={{ opacity: 0.45 }}>—</span>
        )}
      </div>

      {spend.cacheHitRate !== null && (
        <div style={row}>
          <span style={{ opacity: 0.45 }}>{t("Cache hit rate")}</span>
          <span style={{ flex: 1 }} />
          <span style={{ opacity: 0.9 }}>{`${Math.round(spend.cacheHitRate * 100)}%`}</span>
        </div>
      )}

      {promptCache && <PromptCacheRow reading={promptCache} L={L} />}

      <div className="ellipsis" style={{ fontSize: L.footnoteFontSize, height: L.footnoteHeight, lineHeight: `${L.footnoteHeight}px`, opacity: 0.4 }}>
        {t("Costs are estimates at API prices.")}
      </div>
    </>
  );
}

/** No money line for work nobody priced, and "≈" for an estimate. */
function Figure({ label, tokens, cost, priced, currency, L }: {
  label: string;
  tokens: number;
  cost: number;
  priced: boolean;
  currency: string | null;
  L: Layout;
}) {
  return (
    <div
      style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", gap: scaleOf(L) * 2, whiteSpace: "nowrap", overflow: "hidden" }}
      role="group"
      aria-label={`${label}: ${tokensText(tokens)}`}
    >
      <div style={{ fontSize: L.footnoteFontSize, height: L.figureLabelHeight, lineHeight: `${L.figureLabelHeight}px`, opacity: 0.45 }}>{label}</div>
      <div style={{ fontSize: L.figureFontSize, fontWeight: 600, height: L.figureValueHeight, lineHeight: `${L.figureValueHeight}px` }}>{tokenCount(tokens)}</div>
      {priced && (
        <div style={{ fontSize: L.footnoteFontSize, height: L.figureLabelHeight, lineHeight: `${L.figureLabelHeight}px`, opacity: 0.45 }}>
          {cost > 0 ? `≈${money(cost, currency)}` : " "}
        </div>
      )}
    </div>
  );
}

/**
 * A month of days as bars, today's lit. Bars are sized for the whole span, not for the days on record: a
 * ledger that starts today has one day, and sized by that its one bar would take the full width.
 */
function DaysChart({ days, slots, width, height }: { days: LedgerDay[]; slots: number; width: number; height: number }) {
  const light = useLightPanel();
  const peak = Math.max(...days.map((d) => d.tokens), 1);
  const count = Math.max(days.length, slots, 1);
  const spacing = Math.max((width / count) * 0.3, 1.5);
  const w = Math.max((width - spacing * (count - 1)) / count, 1);
  return (
    <svg width={width} height={height} role="img" aria-label={t("Tokens per day")} style={{ display: "block", flex: "none" }}>
      {days.map((day, i) => {
        const isToday = i === days.length - 1;
        // A day with any work keeps a visible stub, so a quiet day reads as quiet rather than as missing.
        const h = day.tokens > 0 ? Math.min(Math.max((height * day.tokens) / peak, w), height) : Math.min(w, 2);
        return (
          <rect
            key={day.date}
            x={i * (w + spacing)}
            y={height - h}
            width={w}
            height={h}
            rx={Math.min(w, h) / 2}
            fill={ink(light, isToday ? 0.9 : day.tokens > 0 ? 0.32 : 0.12)}
          />
        );
      })}
    </svg>
  );
}

/** Re-renders on a timer so a countdown does not go stale while a card stays open. */
function useNow(ms: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(timer);
  }, [ms]);
  return now;
}

/** "38 min left", or for Codex's guaranteed floor "At least 18 min left". */
function timeLeft(lapse: PromptCacheLapse, now: number, several: boolean): string {
  const left = duration((Date.parse(lapse.expiresAt) - now) / 1000);
  if (lapse.isMinimum) return t("At least %@ left", left);
  return several ? t("Soonest in %@", left) : t("%@ left", left);
}

/**
 * The prompt cache that lapses soonest: "Prompt cache (1 hr) · 38 min left" for one conversation; for
 * several "Prompt cache · 3 chats · Soonest in 10 min" with the name of that conversation under it. The
 * soonest, not the latest: the one about to lapse is the one worth a message now. "Expired" once none is
 * alive, and gone once the last session is a day old.
 */
function PromptCacheRow({ reading, L }: { reading: PromptCacheReading; L: Layout }) {
  const now = useNow(30_000);
  const live = reading.live
    .filter((s) => Date.parse(s.lapse.expiresAt) > now)
    .sort((a, b) => Date.parse(a.lapse.expiresAt) - Date.parse(b.lapse.expiresAt));
  const lapsed =
    reading.lastLapsed ??
    reading.live.map((s) => s.lapse).sort((a, b) => Date.parse(b.expiresAt) - Date.parse(a.expiresAt))[0] ??
    null;

  const row = (label: string, value: string, dim: boolean) => (
    <div style={{ display: "flex", alignItems: "baseline", gap: 8, fontSize: L.rowFontSize, height: L.rowTextLineHeight, lineHeight: `${L.rowTextLineHeight}px`, whiteSpace: "nowrap" }}>
      <span className="ellipsis" style={{ opacity: 0.45 }}>{label}</span>
      <span style={{ flex: 1 }} />
      <span style={{ opacity: dim ? 0.45 : 0.9 }}>{value}</span>
    </div>
  );

  const urgent = live[0];
  if (urgent) {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 2 * scaleOf(L) }}>
        {row(
          live.length === 1 ? t("Prompt cache (%@)", duration(urgent.lapse.lifetime)) : t("Prompt cache · %@ chats", live.length),
          timeLeft(urgent.lapse, now, live.length > 1),
          false,
        )}
        {live.length > 1 && (
          <div className="ellipsis" style={{ fontSize: L.footnoteFontSize, height: L.footnoteHeight, lineHeight: `${L.footnoteHeight}px`, opacity: 0.45 }}>
            {sessionLabel(urgent, urgent.project)}
          </div>
        )}
      </div>
    );
  }
  if (lapsed && now - Date.parse(lapsed.expiresAt) < STALE_AFTER_MS) {
    // A guaranteed floor that has run out is not a cache known to be gone: OpenAI may still hold it.
    return row(t("Prompt cache (%@)", duration(lapsed.lifetime)), lapsed.isMinimum ? t("May have lapsed") : t("Expired"), true);
  }
  return null;
}

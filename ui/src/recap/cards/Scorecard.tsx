// The closing card: this was your month (or year). Rows of one fact each, every one with a small
// picture of its own. A fact Pulse does not have is a row (or a cell, or a chart) left out, never a
// zero: the rows that remain share the height (upstream RecapScorecardView).
import { ReactNode } from "react";
import { locale, t } from "../../shared/i18n";
import { Deck, personaTitle } from "../deck";
import * as F from "../format";
import { C, FigureText, Geo, H, Hairline, Pill, RichText, Spacer, StoryPage, T, ThatFits, V } from "../kit";
import { PulseMark } from "../marks";
import type { ScoreAgent, ScoreBar } from "../types";

type Widget =
  | { kind: "pips"; slots: ScoreBar["slot"][] }
  | { kind: "note"; text: string }
  | { kind: "hours"; hours: number[]; peak: number }
  | { kind: "meter"; rate: number };

interface CellData {
  figure: F.Figure;
  label: string;
  wordUnit?: boolean;
  widget?: Widget;
}

export function ScorecardCard({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const sections: ReactNode[] = [<Hero key="hero" deck={deck} />];
  if (recap.cost !== undefined) sections.push(<MoneyRow key="money" deck={deck} cost={recap.cost} />);
  sections.push(<Grid key="grid" deck={deck} />);
  if (deck.scoreBars.length) sections.push(<Strip key="strip" deck={deck} />);
  if (recap.deck.scoreAgents.length) sections.push(<Tools key="tools" deck={deck} />);
  if (recap.models[0] || recap.persona) sections.push(<Bottom key="bottom" deck={deck} />);
  return (
    <StoryPage page={deck.page("scorecard")} stamp={deck.scoreStamp}>
      <div style={{ paddingTop: 52 }}>
        <Headline deck={deck} />
      </div>
      {sections.map((section, index) => (
        <div key={index} style={{ display: "contents" }}>
          <Spacer />
          {section}
        </div>
      ))}
      <Spacer min={16} />
      <Notes deck={deck} />
      <Hairline strong style={{ marginTop: 14 }} />
      <div style={{ paddingTop: 26 }}>
        <Footer deck={deck} />
      </div>
    </StoryPage>
  );
}

// MARK: Headline

function Headline({ deck }: { deck: Deck }) {
  return (
    <V align="stretch" style={{ flex: "none" }}>
      <T size={84} w={900} track={-0.84} fit={0.5}>
        {t("That was")}
      </T>
      <RichText text={t("your %@.", F.mark(deck.periodName))} size={84} w={900} emphasisWeight={900} lh={1.2} style={{ letterSpacing: -0.84 }} />
    </V>
  );
}

// MARK: Rows

/** A row: the hairline above it and the room around what it holds. */
function Row({ children }: { children: ReactNode }) {
  return (
    <V align="stretch" style={{ flex: "none" }}>
      <Hairline />
      <div style={{ padding: "22px 0" }}>{children}</div>
    </V>
  );
}

// The total, huge on a lime bar, with the change on the period before.

function Hero({ deck }: { deck: Deck }) {
  return (
    <Row>
      <ThatFits>
        {[210, 180, 150, 120].map((size) => (
          <HeroRow key={size} deck={deck} size={size} />
        ))}
      </ThatFits>
    </Row>
  );
}

function HeroRow({ deck, size }: { deck: Deck; size: number }) {
  const recap = deck.recap;
  const label = recap.isInProgress && deck.lastDay ? t("Tokens · to %@", F.day(deck.lastDay)) : t("Tokens");
  const figure = F.tokens(recap.tokens);
  return (
    <H gap={16} align="flex-end">
      <div style={{ padding: `${size * 0.09}px 14px`, borderRadius: 8, background: C.lime, flex: "none" }}>
        <FigureText number={figure.number} unit={figure.unit} numberSize={size} unitSize={size * 0.45} unitGap={8} trimmed />
      </div>
      <Spacer min={12} />
      <V gap={14} align="flex-end" style={{ paddingBottom: size * 0.09, flex: "none" }}>
        {deck.change ? (
          <Pill fill={C.ink} horizontal={14} vertical={7}>
            <T size={18} w={500} color={C.lime} lines={1}>
              {`${deck.change.arrow} ${deck.change.percent} ${deck.change.versus}`}
            </T>
          </Pill>
        ) : null}
        <T size={20} color={C.secondary} fit={0.7}>
          {label}
        </T>
      </V>
    </H>
  );
}

// The money, with its line.

function MoneyRow({ deck, cost }: { deck: Deck; cost: number }) {
  return (
    <Row>
      <ThatFits>
        {[150, 120, 96].map((size) => (
          <MoneyContent key={size} deck={deck} cost={cost} size={size} />
        ))}
      </ThatFits>
    </Row>
  );
}

function MoneyContent({ deck, cost, size }: { deck: Deck; cost: number; size: number }) {
  const series = deck.costSeries;
  const label = series.length === 0 ? t("Estimated at API prices") : deck.isYear ? t("Estimated at API prices · monthly") : t("Estimated at API prices · daily");
  return (
    <H gap={16} align="flex-end">
      <FigureText number={F.money(cost, deck.recap.currency)} numberSize={size} unitSize={0} trimmed />
      <Spacer min={12} />
      <V gap={12} align="flex-end" style={{ flex: "none" }}>
        {series.length ? <MiniLine values={series} /> : null}
        <T size={20} color={C.secondary} fit={0.6} style={{ whiteSpace: "nowrap" }}>
          {label}
        </T>
      </V>
    </H>
  );
}

// MARK: The 2 x 2

function cellsOf(deck: Deck): CellData[] {
  const recap = deck.recap;
  const cells: CellData[] = [];
  const slots = deck.scoreBars.map((b) => b.slot);
  cells.push({
    figure: { number: String(recap.activeDays), unit: F.Words.daysUnit(recap.activeDays) },
    label: t("Active days"),
    wordUnit: true,
    widget: slots.length ? { kind: "pips", slots } : undefined,
  });
  cells.push({
    figure: { number: recap.sessions.toLocaleString(locale()), unit: "" },
    label: t("Sessions"),
    widget: deck.sessionsPerActiveDay !== null ? { kind: "note", text: t("%@ per active day", deck.sessionsPerActiveDay) } : undefined,
  });
  if (recap.peakHour !== undefined && recap.hours && recap.hours.length === 24) {
    cells.push({ figure: F.hour(recap.peakHour), label: t("Busiest hour"), widget: { kind: "hours", hours: recap.hours, peak: recap.peakHour } });
  }
  if (recap.cacheHitRate !== undefined) {
    // "41%" or "<1%", as the poster prints it: split at the sign.
    cells.push({ figure: F.percentFigure(recap.cacheHitRate), label: t("Cache hit"), widget: { kind: "meter", rate: recap.cacheHitRate } });
  }
  return cells;
}

function Grid({ deck }: { deck: Deck }) {
  const cells = cellsOf(deck);
  const rows = Math.ceil(cells.length / 2);
  return (
    <V align="stretch" style={{ flex: "none" }}>
      {Array.from({ length: rows }, (_, row) => (
        <H key={row} gap={40} align="flex-start" equal>
          <CellView cell={cells[row * 2]} />
          {/* An odd one out takes the whole row, not a hole beside it. */}
          {row * 2 + 1 < cells.length ? <CellView cell={cells[row * 2 + 1]} /> : null}
        </H>
      ))}
    </V>
  );
}

function CellView({ cell }: { cell: CellData }) {
  // The picture shrinks before the figure does, so the four figures stay one size wherever they can.
  const variants: { size: number; scale: number; withWidget: boolean }[] = [
    { size: 110, scale: 1, withWidget: true },
    { size: 110, scale: 0.8, withWidget: true },
    { size: 110, scale: 0.62, withWidget: true },
    { size: 92, scale: 0.62, withWidget: true },
    { size: 76, scale: 0.62, withWidget: true },
    { size: 76, scale: 0, withWidget: false },
  ];
  return (
    <ThatFits style={{ flex: "1 1 0", minWidth: 0 }}>
      {variants.map((v, i) => (
        <CellContent key={i} cell={cell} {...v} />
      ))}
    </ThatFits>
  );
}

function CellContent({ cell, size, scale, withWidget }: { cell: CellData; size: number; scale: number; withWidget: boolean }) {
  return (
    <V gap={18} align="stretch" style={{ padding: "22px 0", position: "relative" }}>
      <Hairline style={{ position: "absolute", left: 0, right: 0, top: 0 }} />
      <H gap={12} align="flex-end">
        <FigureText number={cell.figure.number} unit={cell.figure.unit} numberSize={size} unitSize={size * 0.45} unitWeight={cell.wordUnit ? 700 : 900} unitGap={6} trimmed />
        <Spacer min={12} />
        {withWidget && cell.widget ? <WidgetView widget={cell.widget} scale={scale} /> : null}
      </H>
      <T size={20} color={C.secondary} fit={0.6}>
        {cell.label}
      </T>
    </V>
  );
}

function WidgetView({ widget, scale }: { widget: Widget; scale: number }) {
  switch (widget.kind) {
    case "pips":
      return <Pips slots={widget.slots} scale={scale} />;
    case "note":
      return (
        <div style={{ maxWidth: 190 * scale + 20, paddingBottom: 4, flex: "none" }}>
          <T size={18} color={C.tertiary} lines={2} align="right">
            {widget.text}
          </T>
        </div>
      );
    case "hours":
      return <MiniHours hours={widget.hours} peak={widget.peak} scale={scale} />;
    case "meter":
      return <Meter fraction={widget.rate} width={200 * scale} />;
  }
}

// MARK: Every day

function Strip({ deck }: { deck: Deck }) {
  const bars = deck.scoreBars;
  const isYear = deck.isYear;
  const title = isYear ? t("Month by month") : t("Every day of %@", deck.periodName);
  return (
    <V gap={14} align="stretch" style={{ paddingBottom: 22, flex: "none" }}>
      <Hairline />
      <H align="baseline" style={{ paddingTop: 8 }}>
        <div style={{ minWidth: 0, flex: "0 1 auto" }}>
          <T size={22} w={700} fit={0.7}>
            {title}
          </T>
        </div>
        <Spacer min={12} />
        {deck.scoreBusiestNote ? (
          <div style={{ minWidth: 0, flex: "0 1 auto" }}>
            <T size={18} color={C.tertiary} fit={0.6}>
              {deck.scoreBusiestNote}
            </T>
          </div>
        ) : null}
      </H>
      <BarStrip bars={bars} axis={deck.scoreAxis} gap={isYear ? 12 : 5} height={116} centeredEnds={isYear} />
    </V>
  );
}

// MARK: Tools

const toneColor = (tone: ScoreAgent["tone"]) => (tone === "lime" ? C.lime : tone === "ink" ? C.ink : C.restLabel);

function Tools({ deck }: { deck: Deck }) {
  const agents = deck.recap.deck.scoreAgents;
  return (
    <V gap={14} align="stretch" style={{ paddingBottom: 22, flex: "none" }}>
      <Hairline />
      <H align="baseline" style={{ paddingTop: 8 }}>
        <T size={22} w={700} lines={1}>
          {t("Tools")}
        </T>
        <Spacer min={12} />
        <div style={{ minWidth: 0, flex: "0 1 auto" }}>
          <T size={18} color={C.tertiary} fit={0.6}>
            {t("By token share")}
          </T>
        </div>
      </H>
      <ShareBar agents={agents.map((a) => ({ share: a.share, color: toneColor(a.tone) }))} />
      <H gap={24}>
        {agents.map((agent, index) => (
          <H key={index} gap={8} align="center" style={{ flex: "0 1 auto" }}>
            <div style={{ width: 12, height: 12, borderRadius: 3, background: toneColor(agent.tone), boxShadow: "inset 0 0 0 1px rgba(27,27,30,0.2)", flex: "none" }} />
            <div style={{ minWidth: 0 }}>
              <T size={18} fit={0.6}>
                {agent.name ?? t("Other")}
              </T>
            </div>
            <T size={18} mono color={C.tertiary} style={{ whiteSpace: "nowrap" }}>
              {F.percent(agent.share)}
            </T>
          </H>
        ))}
        <Spacer />
      </H>
    </V>
  );
}

// MARK: Model and persona

function Bottom({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const model = recap.models[0];
  return (
    <V align="stretch" style={{ flex: "none" }}>
      <Hairline />
      <H gap={20} align="center" style={{ paddingTop: 22 }}>
        {model ? (
          <V gap={6} align="stretch" style={{ minWidth: 0, flex: "0 1 auto" }}>
            <T size={20} color={C.secondary} fit={0.6}>
              {t("Most used")}
            </T>
            <H gap={12} align="baseline">
              <div style={{ minWidth: 0, flex: "0 1 auto" }}>
                <T size={40} w={700} tight={-0.8} fit={0.5}>
                  {model.name}
                </T>
              </div>
              <T size={22} w={500} color={C.tertiary} style={{ whiteSpace: "nowrap" }}>
                {F.percent(model.share)}
              </T>
            </H>
          </V>
        ) : null}
        <Spacer min={12} />
        {recap.persona ? (
          <V gap={8} align="flex-end" style={{ flex: "none" }}>
            <T size={20} color={C.secondary} fit={0.6}>
              {t("Your type")}
            </T>
            <Pill fill={C.ink} horizontal={18} vertical={8}>
              <T size={22} w={500} color={C.lime} lines={1}>
                {personaTitle(recap.persona)}
              </T>
            </Pill>
          </V>
        ) : null}
      </H>
    </V>
  );
}

// MARK: Notes and footer

function Notes({ deck }: { deck: Deck }) {
  if (!deck.costIsFloor && !deck.recap.isPartial) return null;
  return (
    <V gap={4} align="stretch" style={{ flex: "none" }}>
      {deck.costIsFloor ? (
        <T size={15} color={C.tertiary} lines={2}>
          {t("Some work had no published price, so the money is a floor.")}
        </T>
      ) : null}
      {deck.recap.isPartial ? (
        <T size={15} color={C.tertiary} lines={2}>
          {t("Some tools' counts may be missing, so the total is a floor.")}
        </T>
      ) : null}
    </V>
  );
}

function Footer({ deck }: { deck: Deck }) {
  return (
    <H gap={20} align="center" style={{ flex: "none" }}>
      <PulseMark side={72} />
      <V gap={4} align="flex-start" style={{ minWidth: 0 }}>
        <T size={32} w={600}>
          Pulse
        </T>
        <T size={17} mono color={C.grey55} fit={0.7}>
          github.com/qunqin24/Pulse
        </T>
      </V>
      <Spacer min={16} />
      <div style={{ maxWidth: 260, flex: "0 1 auto" }}>
        <T size={20} color={C.tertiary} lines={2} align="right">
          {t("See you in %@", deck.nextName)}
        </T>
      </div>
    </H>
  );
}

// MARK: Small charts

/** One dot per day (or month): ink for work, grey for a quiet one that is over, hollow for one
 *  still to come. */
function Pips({ slots, scale }: { slots: ScoreBar["slot"][]; scale: number }) {
  // A month is 11 across (31 days in three rows); a year's twelve are two rows of six, bigger so
  // they read.
  const isYear = slots.length === 12;
  const columns = isYear ? 6 : 11;
  const pip = (isYear ? 14 : 9) * scale;
  const gap = 6 * scale;
  const rows: ScoreBar["slot"][][] = [];
  for (let i = 0; i < slots.length; i += columns) rows.push(slots.slice(i, i + columns));
  return (
    <V gap={gap} align="flex-start" style={{ paddingBottom: 4, flex: "none" }}>
      {rows.map((row, r) => (
        <H key={r} gap={gap}>
          {row.map((slot, c) => (
            <div
              key={c}
              style={{
                width: pip,
                height: pip,
                borderRadius: pip,
                flex: "none",
                boxSizing: "border-box",
                background: slot === "active" ? C.ink : slot === "quiet" ? "#E2E2DB" : undefined,
                border: slot === "future" ? "1.5px solid #CFCFC8" : undefined,
              }}
            />
          ))}
        </H>
      ))}
    </V>
  );
}

/** The 24 hours as small bars: the peak in ink, hours with work lime, empty ones a stub of grey. */
function MiniHours({ hours, peak, scale }: { hours: number[]; peak: number; scale: number }) {
  const maximum = Math.max(1, ...hours);
  return (
    <H gap={2 * scale} align="flex-end" style={{ height: 54, flex: "none" }}>
      {hours.map((value, hour) => (
        <div
          key={hour}
          style={{
            width: 5 * scale,
            height: Math.max(3, (value / maximum) * 54),
            borderRadius: 2,
            flex: "none",
            background: hour === peak ? C.ink : value > 0 ? C.lime : "#E2E2DB",
          }}
        />
      ))}
    </H>
  );
}

/** A thin meter: a rate as lime on a grey track, ended by an ink tick. */
function Meter({ fraction, width }: { fraction: number; width: number }) {
  const filled = Math.max(width * Math.min(Math.max(fraction, 0), 1), 4);
  return (
    <div style={{ width, height: 12, borderRadius: 6, background: "#E2E2DB", overflow: "hidden", position: "relative", marginBottom: 4, flex: "none" }}>
      <div style={{ position: "absolute", left: 0, top: 0, bottom: 0, width: filled, background: C.lime }} />
      <div style={{ position: "absolute", top: 0, bottom: 0, width: 2, left: filled - 2, background: C.ink }} />
    </div>
  );
}

/** The cost line: a stroke through one point per day (or month), a ring on each when there are
 *  few of them, and the last point filled. */
function MiniLine({ values }: { values: number[] }) {
  const width = 200;
  const height = 54;
  const maximum = Math.max(0, ...values);
  if (values.length < 2 || maximum <= 0) return <div style={{ width, height }} />;
  const inset = 5;
  const points = values.map((v, i) => [inset + (i / (values.length - 1)) * (width - inset * 2), height - 4 - (v / maximum) * (height - 10)] as const);
  const rings = points.length <= 12 ? points.map((_, i) => i) : [points.length - 1];
  return (
    <svg width={width} height={height} style={{ display: "block", flex: "none" }}>
      <path d={points.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(2)} ${y.toFixed(2)}`).join(" ")} fill="none" stroke={C.ink} strokeWidth={2.5} strokeLinecap="round" strokeLinejoin="round" />
      {rings.map((i) => (
        <circle key={i} cx={points[i][0]} cy={points[i][1]} r={4.5} fill={i === points.length - 1 ? C.ink : C.paper} stroke={C.ink} strokeWidth={2} />
      ))}
    </svg>
  );
}

/** One bar per day (or month): the busiest ink, the others lime, a quiet one only its grey slot,
 *  one still to come a dashed outline. */
function BarStrip({
  bars,
  axis,
  gap,
  height,
  centeredEnds,
}: {
  bars: ScoreBar[];
  axis: { index: number; label: string }[];
  gap: number;
  height: number;
  centeredEnds: boolean;
}) {
  const radius = gap > 8 ? 8 : 5;
  return (
    <V gap={10} align="stretch">
      <H gap={gap} align="flex-end" style={{ height }}>
        {bars.map((bar, index) =>
          bar.slot === "future" ? (
            <div key={index} style={{ flex: "1 1 0", minWidth: 0, height, borderRadius: radius, border: `1.5px dashed ${C.rule}`, boxSizing: "border-box" }} />
          ) : (
            <div key={index} style={{ flex: "1 1 0", minWidth: 0, height, borderRadius: radius, background: "#E6E6DF", position: "relative" }}>
              {bar.slot === "active" ? (
                <div
                  style={{
                    position: "absolute",
                    left: 0,
                    right: 0,
                    bottom: 0,
                    height: Math.max(radius * 2, height * bar.fraction),
                    borderRadius: radius,
                    background: bar.isBusiest ? C.ink : C.lime,
                  }}
                />
              ) : null}
            </div>
          ),
        )}
      </H>
      <Geo style={{ height: 22, width: "100%" }}>
        {(w) => {
          const slot = (w - gap * (bars.length - 1)) / Math.max(bars.length, 1);
          return (
            <>
              {axis.map((entry, item) => {
                const center = (slot + gap) * entry.index + slot / 2;
                const width = centeredEnds ? slot + gap : 60;
                const isFirst = !centeredEnds && item === 0;
                const isLast = !centeredEnds && item === axis.length - 1;
                const x = isFirst ? width / 2 : isLast ? w - width / 2 : center;
                return (
                  <div key={entry.index} style={{ position: "absolute", top: 0, left: x - width / 2, width, textAlign: isFirst ? "left" : isLast ? "right" : "center" }}>
                    <T size={16} mono color={C.tertiary} fit={0.5} align={isFirst ? "left" : isLast ? "right" : "center"}>
                      {entry.label}
                    </T>
                  </div>
                );
              })}
            </>
          );
        }}
      </Geo>
    </V>
  );
}

/** The share bar: each tool a segment, the width its share, with a sliver of a minimum so a 1%
 *  tool is still there to see. */
function ShareBar({ agents }: { agents: { share: number; color: string }[] }) {
  const weights = agents.map((a) => Math.max(a.share, 0.012));
  const total = weights.reduce((s, w) => s + w, 0);
  return (
    <Geo style={{ height: 16, width: "100%" }}>
      {(w) => {
        const gap = 3;
        const free = w - gap * Math.max(agents.length - 1, 0);
        return (
          <div style={{ position: "absolute", inset: 0, display: "flex", gap, borderRadius: 999, overflow: "hidden" }}>
            {agents.map((agent, i) => (
              <div key={i} style={{ width: (free * weights[i]) / Math.max(total, 0.0001), background: agent.color, flex: "none" }} />
            ))}
          </div>
        );
      }}
    </Geo>
  );
}

// What the period cost at API prices against what the reader pays: the cost and the multiple, a
// ruler of paid against used with the used bar cut by model, three tiles, the cache's saving, the
// models, and the days (upstream RecapPaybackView and RecapRulerScale).
import { t } from "../../shared/i18n";
import { Deck, Payback } from "../deck";
import * as F from "../format";
import { C, FigureText, Geo, H, Hairline, SectionHead, Spacer, StoryPage, T, Tile, V, toneCaption, toneDetail } from "../kit";
import { MarkTile, vendorResource } from "../marks";
import type { ModelShare } from "../types";

/** Bars for a ranked list, the same colours on the ruler and in the rows. */
const FILLS = [C.ink, "#4A4A4F", C.lime, "#9A9A93", "#C9C9C2"];
/** The slice of the ruler for what the five named models leave over. */
const REMAINDER = "#E2E2DB";

/** The payback card's ruler: it ends just past the larger figure, in round steps (labels at 1,
 *  2, 2.5 or 5 times a power of ten, with a tick halfway between each pair). */
export function rulerScale(value: number): { step: number; subdivisions: number; ticks: number; maximum: number } {
  const target = Math.max(value, 0.01) / 4;
  const magnitude = Math.pow(10, Math.floor(Math.log10(target)));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * magnitude).find((s) => s >= target) ?? 10 * magnitude;
  const subdivisions = 2;
  const ticks = Math.max(Math.ceil((value * 1.04) / step), 1) * 2;
  return { step, subdivisions, ticks, maximum: (ticks * step) / subdivisions };
}

export function PaybackCard({ deck }: { deck: Deck }) {
  const payback = deck.payback;
  if (!payback) return null;
  const recap = deck.recap;
  const money = (amount: number) => F.money(amount, recap.currency);
  return (
    <StoryPage page={deck.page("payback")}>
      <div style={{ paddingTop: 44 }}>
        <Hero deck={deck} payback={payback} money={money} />
      </div>
      <Spacer min={20} />
      <Ruler deck={deck} payback={payback} money={money} />
      <Spacer min={20} />
      <Tiles deck={deck} money={money} />
      <Spacer min={20} />
      <CacheRows deck={deck} money={money} />
      <Spacer min={20} />
      <Models deck={deck} money={money} />
      <Spacer min={20} />
      <Daily deck={deck} />
      <Footnotes deck={deck} />
    </StoryPage>
  );
}

// MARK: The figure

/** The size that keeps a figure of `text` inside `available`, from digits that are about 0.56 em
 *  wide (tracking included) and signs about half that. */
function fitted(text: string, base: number, available: number): number {
  let em = 0;
  for (const ch of text) em += /\d/.test(ch) || ch === "$" ? 0.56 : 0.3;
  return Math.min(base, available / Math.max(em, 1));
}

function Hero({ deck, payback, money }: { deck: Deck; payback: Payback; money: (n: number) => string }) {
  const cost = money(payback.used);
  const size = fitted(cost, 250, 656);
  return (
    <V align="stretch" style={{ flex: "none" }}>
      <T size={32} color={C.grey55} lines={2}>
        {t("On a plan of %@ a month, you used", money(payback.monthlyPrice))}
      </T>
      <H gap={24} align="flex-end">
        <div style={{ paddingTop: 22, flex: "none" }}>
          <FigureText number={cost} numberSize={size} unitSize={0} tracking={-0.06} trimmed />
        </div>
        <Spacer />
        <div style={{ paddingBottom: 6, flex: "none" }}>
          <V align="center" style={{ padding: "16px 24px 18px", borderRadius: 20, background: C.lime }}>
            <T size={20} w={700} color={C.limeInk}>
              {t("Payback")}
            </T>
            <T size={84} w={800} tight={-84 * 0.04} lines={1} style={{ whiteSpace: "nowrap" }}>
              {F.multiple(payback.multiple) + "×"}
            </T>
          </V>
        </div>
      </H>
      <div style={{ paddingTop: 34 }}>
        <T size={20} color={C.tertiary} fit={0.6}>
          {t("For every %@ of subscription, you used %@ of API value", F.axisMoney(1, 1, deck.recap.currency), money(payback.multiple))}
        </T>
      </div>
    </V>
  );
}

// MARK: The ruler

/** The models that have money, dearest first. */
function pricedModels(deck: Deck): ModelShare[] {
  return deck.recap.models.filter((m) => (m.cost ?? 0) > 0).sort((a, b) => (b.cost ?? 0) - (a.cost ?? 0));
}

function RulerLabel({ title }: { title: string }) {
  return (
    <div style={{ width: 110, flex: "none" }}>
      <T size={20} color={C.secondary} fit={0.6}>
        {title}
      </T>
    </div>
  );
}

function Ruler({ deck, payback, money }: { deck: Deck; payback: Payback; money: (n: number) => string }) {
  const scale = rulerScale(Math.max(payback.used, payback.paid));
  const barArea = 690;
  const named = pricedModels(deck).slice(0, 5);
  const shown = named.reduce((s, m) => s + (m.cost ?? 0), 0);
  const rest = Math.max(payback.used - shown, 0);
  const whole = Math.max(shown + rest, 0.0001);
  const usedWidth = Math.max(8, (barArea * payback.used) / scale.maximum);
  const showRest = rest > 0.005 * payback.used;
  const gaps = (named.length + (showRest ? 1 : 0) - 1) * 2;
  return (
    <V gap={16} align="stretch" style={{ flex: "none" }}>
      <H gap={18} align="center">
        <RulerLabel title={t("You paid")} />
        <H gap={12} align="center">
          <div
            style={{
              width: Math.max(8, (barArea * payback.paid) / scale.maximum),
              height: 56,
              borderRadius: 8,
              border: `2px solid ${C.ink}`,
              boxSizing: "border-box",
              flex: "none",
            }}
          />
          <T size={24} w={600} lines={1}>
            {money(payback.paid)}
          </T>
        </H>
      </H>
      <H gap={18} align="center">
        <RulerLabel title={t("You used")} />
        <H gap={12} align="center">
          <H gap={2} style={{ height: 56, borderRadius: 8, overflow: "hidden", flex: "none" }}>
            {named.map((model, index) => (
              <div key={model.name} style={{ width: Math.max(2, ((usedWidth - Math.max(gaps, 0)) * (model.cost ?? 0)) / whole), background: FILLS[index], flex: "none" }} />
            ))}
            {showRest ? <div style={{ width: Math.max(2, ((usedWidth - Math.max(gaps, 0)) * rest) / whole), background: REMAINDER, flex: "none" }} /> : null}
          </H>
          <T size={24} w={600} lines={1}>
            {money(payback.used)}
          </T>
        </H>
      </H>
      <H gap={18} align="flex-start">
        <div style={{ width: 110, height: 1, flex: "none" }} />
        <Axis scale={scale} width={barArea} currency={deck.recap.currency} />
      </H>
    </V>
  );
}

function Axis({ scale, width, currency }: { scale: ReturnType<typeof rulerScale>; width: number; currency: string }) {
  return (
    <div style={{ position: "relative", width, height: 44, flex: "none" }}>
      <div style={{ position: "absolute", left: 0, top: 0, width, height: 1.5, background: C.ink }} />
      {Array.from({ length: scale.ticks + 1 }, (_, index) => {
        const value = (index * scale.step) / scale.subdivisions;
        const labelled = index % scale.subdivisions === 0;
        const x = (width * value) / scale.maximum;
        return (
          <div
            key={index}
            style={{
              position: "absolute",
              left: x,
              top: labelled ? 30 : 4,
              transform: "translate(-50%, -50%)",
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              gap: 6,
            }}
          >
            <div style={{ width: 1.5, height: labelled ? 14 : 7, background: C.ink }} />
            {labelled ? (
              <T size={14} mono color={C.tertiary} style={{ whiteSpace: "nowrap" }}>
                {F.axisMoney(value, scale.step, currency)}
              </T>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

// MARK: Tiles

function Tiles({ deck, money }: { deck: Deck; money: (n: number) => string }) {
  const recap = deck.recap;
  const saved = deck.cacheSavings;
  const day = recap.insights.costliestDay;
  const perMillion = recap.insights.costPerMillion;
  if (saved === null && !day && perMillion === undefined) return null;
  const cost = recap.cost ?? 0;
  return (
    <H gap={14} equal style={{ height: 160, flex: "none" }}>
      {saved !== null && cost > 0 ? (
        <PaybackTile
          tone="ink"
          label={t("The cache saved you")}
          value={money(saved)}
          // "0.3x what you spent" says little; the line is for a saving that is more than the spend.
          note={saved >= cost ? t("%@× what you actually spent", F.multiple(saved / cost)) : null}
        />
      ) : null}
      {day && day.cost !== undefined ? <PaybackTile tone="white" label={t("Most expensive day")} value={money(day.cost)} note={F.dayAndWeekday(day.date)} /> : null}
      {perMillion !== undefined ? (
        <PaybackTile
          tone="white"
          label={t("Per million tokens")}
          value={money(perMillion)}
          note={recap.insights.costPerActiveDay !== undefined ? t("Average per active day: %@", money(recap.insights.costPerActiveDay)) : null}
        />
      ) : null}
    </H>
  );
}

function PaybackTile({ tone, label, value, note }: { tone: "white" | "ink"; label: string; value: string; note: string | null }) {
  return (
    <Tile tone={tone}>
      <V gap={8} align="stretch" style={{ flex: "1 1 0" }}>
        <T size={17} color={toneCaption[tone]} fit={0.6}>
          {label}
        </T>
        <T size={52} w={700} tight={-52 * 0.03} color={tone === "ink" ? C.lime : C.ink} fit={0.5}>
          {value}
        </T>
        <Spacer />
        {note ? (
          <T size={16} color={toneDetail[tone]} fit={0.6}>
            {note}
          </T>
        ) : null}
      </V>
    </Tile>
  );
}

// MARK: The cache

function CacheRows({ deck, money }: { deck: Deck; money: (n: number) => string }) {
  const recap = deck.recap;
  const saved = deck.cacheSavings;
  const cost = recap.cost ?? 0;
  if (saved === null || cost <= 0) return null;
  const without = cost + saved;
  const title = recap.cacheHitRate !== undefined ? t("Cache hit %@", F.percent(recap.cacheHitRate)) : t("The cache");
  return (
    <V gap={12} align="stretch" style={{ flex: "none" }}>
      <div style={{ paddingBottom: 2 }}>
        <SectionHead title={title} note={t("Repeated context is read straight from the cache")} />
      </div>
      <CompareRow label={t("Without the cache")} value={money(without)} fraction={1} track="#E2E2DB" fill="#E2E2DB" bold={false} />
      <CompareRow label={t("Actually spent")} value={money(cost)} fraction={cost / without} track={C.heatZero} fill={C.ink} bold />
    </V>
  );
}

function CompareRow({ label, value, fraction, track, fill, bold }: { label: string; value: string; fraction: number; track: string; fill: string; bold: boolean }) {
  return (
    <H gap={16} align="center">
      <div style={{ width: 170, flex: "none" }}>
        <T size={19} w={bold ? 700 : 400} color={bold ? C.ink : C.secondary} fit={0.6}>
          {label}
        </T>
      </div>
      <div style={{ flex: "1 1 0", minWidth: 0, height: 26, borderRadius: 6, background: track, position: "relative" }}>
        <div style={{ position: "absolute", left: 0, top: 0, bottom: 0, width: `max(8px, ${fraction * 100}%)`, borderRadius: 6, background: fill }} />
      </div>
      <div style={{ width: 120, flex: "none" }}>
        <T size={19} w={bold ? 700 : 400} mono fit={0.6} align="right">
          {value}
        </T>
      </div>
    </H>
  );
}

// MARK: Models

function Models({ deck, money }: { deck: Deck; money: (n: number) => string }) {
  const rows = pricedModels(deck).slice(0, 5);
  const top = Math.max(rows[0]?.cost ?? 1, 0.0001);
  if (!rows.length) return null;
  return (
    <V align="stretch" style={{ flex: "none" }}>
      <div style={{ paddingBottom: 10 }}>
        <SectionHead title={t("Where the money went")} note={t("By model")} />
      </div>
      {rows.map((model, index) => {
        const first = index === 0;
        return (
          <V key={model.name} align="stretch">
            <Hairline />
            <H gap={16} align="center" style={{ padding: "13px 0" }}>
              <MarkTile resource={vendorResource(model.name)} name={model.name} side={44} />
              <div style={{ width: 250, flex: "none" }}>
                <T size={24} w={first ? 600 : 400} fit={0.6}>
                  {model.name}
                </T>
              </div>
              <div style={{ flex: "1 1 0", minWidth: 0, height: 12, borderRadius: 4, background: C.heatZero, position: "relative" }}>
                <div style={{ position: "absolute", left: 0, top: 0, bottom: 0, width: `max(6px, ${((model.cost ?? 0) / top) * 100}%)`, borderRadius: 4, background: FILLS[index] }} />
              </div>
              <div style={{ width: 110, flex: "none" }}>
                <T size={24} w={first ? 600 : 400} fit={0.6} align="right">
                  {money(model.cost ?? 0)}
                </T>
              </div>
            </H>
          </V>
        );
      })}
    </V>
  );
}

// MARK: Days or months

function Daily({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const bars = recap.insights.costBars;
  if (!bars) return null;
  const top = Math.max(0, ...bars.map((b) => b ?? 0));
  if (top <= 0) return null;
  const best = bars.indexOf(top);
  return (
    <V gap={10} align="stretch" style={{ flex: "none" }}>
      <Hairline strong style={{ marginBottom: 8 }} />
      <SectionHead title={deck.isYear ? t("Spent by month") : t("Spent by day")} note={F.Words.activeDays(recap.activeDays)} />
      <H gap={4} align="flex-end" style={{ height: 92 }}>
        {bars.map((bar, index) =>
          bar !== null ? (
            <div
              key={index}
              style={{
                flex: "1 1 0",
                minWidth: 0,
                height: Math.max(4, (bar / top) * 90),
                borderRadius: 3,
                background: index === best ? C.ink : bar > 0 ? C.lime : C.restBar,
              }}
            />
          ) : (
            // A month still to come: an outline, not a zero.
            <div key={index} style={{ flex: "1 1 0", minWidth: 0, height: 12, borderRadius: 3, border: `1.5px dashed ${C.rule}`, boxSizing: "border-box" }} />
          ),
        )}
      </H>
      <AxisLabels count={bars.length} isYear={deck.isYear} />
    </V>
  );
}

/** Day numbers (1, 10, 20 and the last) or the quarter's first months, under the bar they belong
 *  to. */
function AxisLabels({ count, isYear }: { count: number; isYear: boolean }) {
  let labels: { index: number; text: string }[];
  if (isYear) {
    labels = [0, 3, 6, 9].map((i) => ({ index: i, text: F.shortMonthName(i + 1) }));
  } else {
    const list: number[] = [];
    for (const index of [0, 9, 19, count - 1].filter((i) => i < count)) {
      if (!list.includes(index) && (list.length === 0 || index - list[list.length - 1] >= 5)) list.push(index);
    }
    labels = list.map((i) => ({ index: i, text: String(i + 1) }));
  }
  return (
    <Geo style={{ height: 18, width: "100%" }}>
      {(w) => {
        const cell = w / Math.max(count, 1);
        return (
          <>
            {labels.map((label) => (
              <div key={label.index} style={{ position: "absolute", left: (label.index + 0.5) * cell - 30, top: 0, width: 60, textAlign: "center" }}>
                <T size={14} mono color={C.tertiary} lines={1} align="center">
                  {label.text}
                </T>
              </div>
            ))}
          </>
        );
      }}
    </Geo>
  );
}

// MARK: Footnotes

function Footnotes({ deck }: { deck: Deck }) {
  return (
    <V gap={4} align="stretch" style={{ paddingTop: 16, flex: "none" }}>
      <T size={15} color={C.tertiary}>
        {t("Estimated at each model's published API price. The plan price is the one you typed in Pulse.")}
      </T>
      {/* A period still running: both figures stop today. */}
      {deck.payback?.isToDate ? (
        <T size={15} color={C.tertiary}>
          {t("Figures are to date, and the plan price is prorated by the days so far.")}
        </T>
      ) : null}
      {deck.costIsFloor ? (
        <T size={15} color={C.tertiary}>
          {t("Some work had no published price, so the money is a floor.")}
        </T>
      ) : null}
    </V>
  );
}

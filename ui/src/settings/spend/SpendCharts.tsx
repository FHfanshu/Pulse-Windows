// Ported from upstream Settings/SpendCharts.swift and ChartHoverOverlay.swift. Charts are plain SVG; the
// pointer reads out the nearest column the way upstream's `ChartHoverOverlay` does.
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { t } from "../../shared/i18n";
import {
  chartDate, exactTokens, hourText, money, moneyExact, shortDate, tallyTotal, tokenCount, tokensText, unpricedText,
  type TokenCost, type TokenTally,
} from "../../shared/spend";
import { Select } from "../controls";
import { Row } from "../Group";

// MARK: - Measuring

/** The width of an element, kept current. */
export function useWidth<T extends HTMLElement>(): [React.RefObject<T>, number] {
  const ref = useRef<T>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setWidth(el.clientWidth);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}

// MARK: - Hover

export interface Sample {
  x: number;
  title: string;
  tokens: number;
}

/** The read-out card every chart shows: a caption over a token count. */
export function TooltipCard({ title, tokens }: { title: string; tokens: number }) {
  return (
    <>
      <div className="tip-title">{title}</div>
      <div className="tip-value">{tokensText(tokens)}</div>
    </>
  );
}

/**
 * A card placed beside a column or a cell: on the right of `right` when it fits, else on the left of `left`,
 * clamped inside the plot. `midY` centres it on a cell; without it the card sits at the top.
 */
export function FloatingCard({ left, right, midY, bounds, gap, children }: {
  left: number;
  right: number;
  midY?: number;
  bounds: { w: number; h: number };
  gap: number;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (el) setSize((old) => (old && old.w === el.offsetWidth && old.h === el.offsetHeight ? old : { w: el.offsetWidth, h: el.offsetHeight }));
  });
  const w = size?.w ?? 0;
  const h = size?.h ?? 0;
  const preferred = right + gap + w <= bounds.w ? right + gap : left - gap - w;
  const x = Math.min(Math.max(preferred, 0), Math.max(bounds.w - w, 0));
  const y = midY === undefined ? 0 : Math.min(Math.max(midY - h / 2, 0), Math.max(bounds.h - h, 0));
  return (
    <div ref={ref} className="tip" style={{ left: x, top: y, visibility: size ? "visible" : "hidden" }}>
      {children}
    </div>
  );
}

/**
 * A plot that fills its row, drawn at the measured width, with the whole area a hover target: the nearest
 * sample is marked and read out, including the space above a short bar and the gaps between bars.
 */
export function HoverPlot({ height: heightOf, build, label, data }: {
  /** Fixed, or worked out from the measured width (the activity plots are seven columns tall). */
  height: number | ((width: number) => number);
  build: (width: number, height: number) => { content: ReactNode; samples: Sample[] };
  label: string;
  /** What the plot draws. A different series drops the readout (upstream `onChange(of: samples)`): its index would name another datum. */
  data?: unknown;
}) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  useEffect(() => setHover(null), [data]);
  const height = typeof heightOf === "function" ? heightOf(width) : heightOf;
  const { content, samples } = width > 0 ? build(width, height) : { content: null, samples: [] as Sample[] };
  const sample = hover !== null ? samples[hover] : undefined;

  const move = (e: React.PointerEvent) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    if (!samples.length || x < 0 || x > rect.width || y < 0 || y > rect.height) return setHover(null);
    let best = 0;
    for (let i = 1; i < samples.length; i++) if (Math.abs(samples[i].x - x) < Math.abs(samples[best].x - x)) best = i;
    setHover((old) => (old === best ? old : best));
  };

  return (
    <div
      ref={ref}
      className="plot"
      role="img"
      aria-label={label}
      style={{ height }}
      onPointerMove={move}
      onPointerLeave={() => setHover(null)}
    >
      {content}
      {sample && (
        <>
          <div className="plot-guide" style={{ left: sample.x - 0.5 }} />
          <div className="plot-dot" style={{ left: sample.x - 2, top: Math.max(height - 4, 0) }} />
          <FloatingCard left={sample.x} right={sample.x} bounds={{ w: width, h: height }} gap={10}>
            <TooltipCard title={sample.title} tokens={sample.tokens} />
          </FloatingCard>
        </>
      )}
    </div>
  );
}

// MARK: - Bars

/** One agent's or one model's share of the whole. Anything at all keeps a visible stub. */
export function ShareBar({ share }: { share: number }) {
  const clamped = Math.min(Math.max(share, 0), 1);
  return (
    <span className="share" role="img" aria-label={`${Math.round(share * 100)}%`}>
      <i style={{ width: `${clamped * 100}%`, minWidth: share > 0 ? 3 : 0 }} />
    </span>
  );
}

export interface Bar {
  date: string;
  tokens: number;
}

const MAX_BAR_WIDTH = 22;

/**
 * Tokens per day. A calendar, not a money chart. A bar is never wider than 22 px: each day still gets an
 * equal slot, which keeps the spacing even and the dates honest, and the bar sits in the middle of it.
 */
export function SpendBarChart({ bars, height = 78 }: { bars: Bar[]; height?: number }) {
  const first = bars[0];
  const last = bars[bars.length - 1];
  return (
    <div>
      <HoverPlot
        height={height}
        data={bars}
        label={t("Tokens per day")}
        build={(width) => {
          const peak = Math.max(...bars.map((b) => b.tokens), 1);
          const slot = width / Math.max(bars.length, 1);
          const w = Math.max(Math.min(slot * 0.72, MAX_BAR_WIDTH), 1);
          return {
            content: (
              <svg width={width} height={height}>
                {bars.map((bar, i) => {
                  const h = bar.tokens > 0 ? Math.max(((height - 1) * bar.tokens) / peak, 3) : 2;
                  return (
                    <rect
                      key={bar.date}
                      x={slot * (i + 0.5) - w / 2}
                      y={height - h}
                      width={w}
                      height={h}
                      rx={Math.min(w, 5) / 2}
                      className={bar.tokens > 0 ? "fill-accent" : "fill-quiet"}
                    />
                  );
                })}
                <rect x={0} y={height - 1} width={width} height={1} className="fill-quiet" />
              </svg>
            ),
            samples: bars.map((bar, i) => ({ x: slot * (i + 0.5), title: chartDate(bar.date), tokens: bar.tokens })),
          };
        }}
      />
      {first && last && bars.length > 1 && (
        <div className="axis-ends" aria-hidden>
          <span>{shortDate(first.date)}</span>
          <span>{shortDate(last.date)}</span>
        </div>
      )}
    </div>
  );
}

/** Tokens by hour of the local day: the shape of today, or of a longer span, or of one model. */
export function HourProfile({ hours, height = 66 }: { hours: Record<string, number>; height?: number }) {
  return (
    <div>
      <HoverPlot
        height={height}
        data={hours}
        label={t("Tokens per hour")}
        build={(width) => {
          const spacing = 2;
          const w = Math.max((width - spacing * 23) / 24, 0);
          const values = Array.from({ length: 24 }, (_, h) => hours[String(h)] ?? 0);
          const peak = Math.max(...values, 1);
          return {
            content: (
              <svg width={width} height={height}>
                {values.map((tokens, hour) => {
                  const h = tokens > 0 ? Math.max(((height - 1) * tokens) / peak, 3) : 2;
                  return <rect key={hour} x={hour * (w + spacing)} y={height - h} width={w} height={h} rx={2} className={tokens > 0 ? "fill-accent" : "fill-quiet"} />;
                })}
                <rect x={0} y={height - 1} width={width} height={1} className="fill-quiet" />
              </svg>
            ),
            samples: values.map((tokens, hour) => ({ x: w / 2 + hour * (w + spacing), title: hourText(hour), tokens })),
          };
        }}
      />
      <div className="axis-ends" aria-hidden>
        <span>{hourText(0)}</span>
        <span>{hourText(12)}</span>
        <span>{hourText(23)}</span>
      </div>
    </div>
  );
}

// MARK: - Figures

/** An estimated amount, or the mark that says there is none. A `*` marks a partial estimate. */
export function CostText({ cost, unpriced = 0 }: { cost: number | null; unpriced?: number }) {
  if (cost === null) return <span className="dash">—</span>;
  const exact = unpriced > 0 ? `${moneyExact(cost)} · ${unpricedText(unpriced)}` : moneyExact(cost);
  return <span className="num" title={exact}>{unpriced > 0 ? `${money(cost)}*` : money(cost)}</span>;
}

/** A small label above its figure. */
export function SpendCaption({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="caption" title={title}>
      <div className="caption-label">{label}</div>
      <div className="caption-value">{value}</div>
    </div>
  );
}

/** The line that explains a `*` beside a partial amount. */
export function PartialNote({ tokens }: { tokens: number }) {
  return (
    <div className="partial-note" title={unpricedText(tokens)}>
      <span>*</span>
      <span>{t("Excludes unpriced tokens.")}</span>
    </div>
  );
}

// MARK: - Paging

/** The row counts a long table may be cut into. Ten by default: a table long enough to scroll past is a table nobody reads to the end of. */
export const PAGE_SIZES = [10, 20, 30, 50] as const;

/** The page a list is on, clamped: the span and the sort can both shorten the table under a page already on screen. */
export function pageOf(rows: number, pageSize: number, page: number) {
  const pages = Math.max(Math.ceil(rows / pageSize), 1);
  return Math.min(Math.max(page, 0), pages - 1);
}

/**
 * The foot of a paged table: the page size, where in the list the reader is, and the two arrows. Shared by the
 * day tables and the session list: the same control, and a second copy is a second place for the range
 * arithmetic to disagree.
 */
export function SpendPageFooter({ rows, pageSize, page, onPageSize, onPage }: {
  rows: number;
  pageSize: number;
  page: number;
  onPageSize: (size: number) => void;
  onPage: (page: number) => void;
}) {
  const pages = Math.max(Math.ceil(rows / pageSize), 1);
  const current = pageOf(rows, pageSize, page);
  const arrow = (dir: "left" | "right") => (
    <svg viewBox="0 0 8 12" width="8" height="12" aria-hidden>
      <path d={dir === "left" ? "M6.5 1.5 2 6l4.5 4.5" : "M1.5 1.5 6 6l-4.5 4.5"} fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
  return (
    <div className="pager">
      <Select
        label={t("Rows per page")}
        value={pageSize}
        options={PAGE_SIZES.map((size) => ({ value: size, label: t("%@ per page", size) }))}
        // A shorter page does not mean the same rows: going back to the first one is the only answer that is the same every time.
        onChange={(size) => { onPageSize(size); onPage(0); }}
      />
      <span className="pager-range secondary num">
        {t("%@–%@ of %@", current * pageSize + 1, Math.min((current + 1) * pageSize, rows), rows)}
      </span>
      <button type="button" className="btn pager-arrow" aria-label={t("Previous page")} disabled={current === 0} onClick={() => onPage(Math.max(current - 1, 0))}>{arrow("left")}</button>
      <button type="button" className="btn pager-arrow" aria-label={t("Next page")} disabled={current >= pages - 1} onClick={() => onPage(Math.min(current + 1, pages - 1))}>{arrow("right")}</button>
    </div>
  );
}

interface KindRow {
  id: "input" | "cacheRead" | "output" | "unclassified";
  label: string;
  note: string;
  tokens: number;
  cost: number | null;
  unreported?: boolean;
}

function kindRows(tally: TokenTally, unclassified: number, cost: TokenCost | null, readsUnreported: boolean): KindRow[] {
  const written = tally.cacheWrite ?? 0;
  const input = written > 0
    ? t("Recorded input, with reported cache hits counted separately. Includes %@ recorded cache writes.", tokenCount(written))
    : t("Recorded input, with reported cache hits counted separately.");
  const unknownOnly = tallyTotal(tally) === 0 && unclassified > 0;
  const missing = t("These records do not report this token category separately.");
  const hit: KindRow = (readsUnreported || unknownOnly) && (tally.cacheRead ?? 0) === 0
    ? { id: "cacheRead", label: t("Cache hit"), note: t("These records carry no cache figures."), tokens: 0, cost: null, unreported: true }
    : { id: "cacheRead", label: t("Cache hit"), note: t("Recorded cache hits. Unreported cache use is not estimated."), tokens: tally.cacheRead ?? 0, cost: cost?.cacheRead ?? null };
  const rows: KindRow[] = [
    { id: "input", label: t("Input"), note: unknownOnly ? missing : input, tokens: (tally.input ?? 0) + written, cost: cost ? cost.input + cost.cacheWrite : null, unreported: unknownOnly },
    hit,
    { id: "output", label: t("Output"), note: unknownOnly ? missing : t("Model output as recorded by each agent."), tokens: tally.output ?? 0, cost: cost?.output ?? null, unreported: unknownOnly },
  ];
  if (unclassified > 0) {
    rows.push({ id: "unclassified", label: t("Unclassified"), note: t("Reported tokens without a usable category breakdown."), tokens: unclassified, cost: null });
  }
  return rows;
}

/**
 * Recorded input, cache hits, output and any explicitly unclassified tokens. The split is the point, not
 * the total. A model's drill-down passes its own cost breakdown and gets the amount beside each kind.
 */
export function TokenKindBreakdown({ tally, unclassified = 0, cost = null, unpriced = 0, readsUnreported = false }: {
  tally: TokenTally;
  unclassified?: number;
  cost?: TokenCost | null;
  unpriced?: number;
  readsUnreported?: boolean;
}) {
  const total = Math.max(tallyTotal(tally) + unclassified, 1);
  return (
    <>
      {kindRows(tally, unclassified, cost, readsUnreported).map((row) => (
        <Row key={row.id} title={row.label} subtitle={row.note}>
          {row.unreported ? (
            <>
              <span className="value-text nowrap">{t("Not reported")}</span>
              {cost && <span className="cost-slot" />}
            </>
          ) : (
            <>
              <ShareBar share={row.tokens / total} />
              <span className="value-text nowrap num" title={exactTokens(row.tokens)}>{tokensText(row.tokens)}</span>
              {cost && <span className="cost-slot"><CostText cost={row.cost} unpriced={unpriced} /></span>}
            </>
          )}
        </Row>
      ))}
    </>
  );
}

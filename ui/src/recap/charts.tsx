// Drawing the recap cards share, kept apart from the cards that place it (upstream RecapCharts,
// and the dial, strips and rings of the redesigned cards). SVG, flat fills and strokes only.
import { CSSProperties, ReactNode } from "react";
import * as F from "./format";
import { C, Geo, H, MONO, T, V } from "./kit";
import { hourInBand, type RhythmBand } from "./rhythm";
import type { Day, Mark, MonthRow } from "./types";

// MARK: Heat

/** 0...4, the last for the busiest day only. */
export function heatStep(tokens: number, maximum: number): number {
  if (maximum <= 0) return 0;
  if (tokens >= maximum) return 4;
  return Math.min(Math.floor((tokens / maximum) * 4), 3);
}

/** The calendar colour for a day: the heaviest is ink, the rest step through four lights relative
 *  to it, and a quiet day is the empty tone. */
export function heatColor(tokens: number, maximum: number): string {
  if (tokens <= 0 || maximum <= 0) return C.heatZero;
  return C.heat[heatStep(tokens, maximum)];
}

// MARK: Month grid

/** The days of one month laid out Monday first: where the 1st falls, how many weeks it takes, and
 *  which cell holds which day. A day the report does not carry (the future of a month still
 *  running) leaves a blank. */
export function monthGrid(monthStart: string, days: Day[]): { cells: (Day | null)[]; rows: number } {
  const year = Number(monthStart.slice(0, 4));
  const month = Number(monthStart.slice(5, 7));
  const count = new Date(Date.UTC(year, month, 0)).getUTCDate();
  const lead = (new Date(Date.UTC(year, month - 1, 1)).getUTCDay() + 6) % 7;
  const byDay = new Map<number, Day>();
  for (const day of days) {
    if (Number(day.date.slice(0, 4)) === year && Number(day.date.slice(5, 7)) === month) {
      const n = F.dayOfMonth(day.date);
      if (!byDay.has(n)) byDay.set(n, day);
    }
  }
  const cells: (Day | null)[] = Array(lead).fill(null);
  for (let n = 1; n <= count; n++) cells.push(byDay.get(n) ?? null);
  while (cells.length % 7 !== 0) cells.push(null);
  return { cells, rows: cells.length / 7 };
}

// MARK: Paths

const f = (n: number) => +n.toFixed(2);

/** A clockwise (screen) arc from angle `from` to `to`, in degrees from 3 o'clock. */
function arcPath(cx: number, cy: number, r: number, from: number, to: number, move = true): string {
  const a = (from * Math.PI) / 180;
  const b = (to * Math.PI) / 180;
  const large = to - from > 180 ? 1 : 0;
  return `${move ? "M" : "L"}${f(cx + r * Math.cos(a))} ${f(cy + r * Math.sin(a))} A${f(r)} ${f(r)} 0 ${large} 1 ${f(cx + r * Math.cos(b))} ${f(cy + r * Math.sin(b))}`;
}

const rad = (degrees: number) => (degrees * Math.PI) / 180;

export const dashed = (stroke: string, width = 1.5, dash = "4 3") => ({ fill: "none", stroke, strokeWidth: width, strokeDasharray: dash });

// MARK: Sparkline

/** A line over a filled area, the area in lime and the line in ink. */
/** The unbroken stretches of a series with gaps: each run is the indices of consecutive values
 *  that have one. A lone point is a run of one (upstream `RecapLineRuns`). */
export function lineRuns(values: (number | null)[]): number[][] {
  const runs: number[][] = [];
  let current: number[] = [];
  values.forEach((v, i) => {
    if (v === null) {
      if (current.length) runs.push(current);
      current = [];
    } else current.push(i);
  });
  if (current.length) runs.push(current);
  return runs;
}

/** Null is a point with no figure (work with no price): the line breaks there rather than dipping
 *  to a zero it never had. */
export function Sparkline({ values }: { values: (number | null)[] }) {
  return (
    <Geo style={{ width: "100%", height: "100%" }}>
      {(w, h) => {
        const maximum = Math.max(0, ...values.map((v) => v ?? 0));
        if (values.length < 2 || maximum <= 0) return null;
        const at = (i: number) => [(i / (values.length - 1)) * w, h - 6 - ((values[i] ?? 0) / maximum) * (h - 12)] as const;
        return (
          <svg width={w} height={h} style={{ position: "absolute", left: 0, top: 0, display: "block" }}>
            {lineRuns(values).map((run) => {
              const points = run.map(at);
              const line = points.map(([x, y], i) => `${i ? "L" : "M"}${f(x)} ${f(y)}`).join(" ");
              const [x0] = points[0];
              const [x1] = points[points.length - 1];
              return (
                <g key={run[0]}>
                  <path d={`${line} L${f(x1)} ${f(h)} L${f(x0)} ${f(h)} Z`} fill={C.lime} />
                  <path d={line} fill="none" stroke={C.ink} strokeWidth={3} strokeLinecap="round" strokeLinejoin="round" />
                </g>
              );
            })}
          </svg>
        );
      }}
    </Geo>
  );
}

// MARK: Donut

export function Donut({ parts, thickness, size }: { parts: { share: number; color: string }[]; thickness: number; size: number }) {
  const radius = (size - thickness) / 2;
  const total = parts.reduce((s, p) => s + p.share, 0);
  // A gap between parts, as the mockup has, drawn as a round cap's worth of shortening.
  const gap = (thickness / radius) * 0.5;
  let start = -Math.PI / 2;
  const arcs: ReactNode[] = [];
  if (total > 0) {
    parts.forEach((part, i) => {
      const sweep = (part.share / total) * 2 * Math.PI;
      const length = Math.max(sweep - gap, 0.02);
      const a = start + gap / 2;
      const b = a + length;
      arcs.push(
        <path
          key={i}
          d={arcPath(size / 2, size / 2, radius, (a * 180) / Math.PI, (b * 180) / Math.PI)}
          fill="none"
          stroke={part.color}
          strokeWidth={thickness}
          strokeLinecap="round"
        />,
      );
      start += sweep;
    });
  }
  return (
    <svg width={size} height={size} style={{ display: "block", flex: "none" }}>
      {arcs}
    </svg>
  );
}

// MARK: Clock

/** 24 bars around a ring, one per hour, midnight at the top. The summary's band is lime; the rest are
 *  a muted grey that gets lighter with less work. */
export function Clock({ hours, highlight, inner, longest, barWidth, labelSize, size }: { hours: number[]; highlight: RhythmBand | null; inner: number; longest: number; barWidth: number; labelSize: number; size: number }) {
  const maximum = Math.max(0, ...hours);
  const c = size / 2;
  if (hours.length !== 24 || maximum <= 0) return <svg width={size} height={size} />;
  return (
    <svg width={size} height={size} style={{ display: "block", flex: "none" }}>
      <circle cx={c} cy={c} r={inner - 26} fill="none" stroke="rgba(245,245,241,0.12)" strokeWidth={1} />
      {hours.map((tokens, hour) => {
        const fraction = tokens / maximum;
        const length = longest * (0.12 + 0.88 * fraction);
        const highlighted = highlight !== null && hourInBand(hour, highlight);
        return (
          <rect
            key={hour}
            x={-barWidth / 2}
            y={-(inner + length)}
            width={barWidth}
            height={length}
            rx={barWidth / 2}
            transform={`translate(${c} ${c}) rotate(${hour * 15})`}
            fill={highlighted ? C.lime : C.darkRest}
            fillOpacity={highlighted ? 1 : 0.38 + 0.62 * fraction}
          />
        );
      })}
      {[0, 6, 12, 18].map((hour) => {
        const angle = rad(hour * 15 - 90);
        const radius = inner - 12 - labelSize * 0.2;
        return (
          <text
            key={hour}
            x={c + radius * Math.cos(angle)}
            y={c + radius * Math.sin(angle)}
            textAnchor="middle"
            dominantBaseline="central"
            fontFamily={MONO}
            fontSize={labelSize}
            fill="rgba(245,245,241,0.45)"
          >
            {hour}
          </text>
        );
      })}
    </svg>
  );
}

// MARK: Dial

/** The 24 hours as a clock face: one wedge per hour, as long as the hour's work, midnight at the
 *  top. The peak is ink, an hour with work lime, an hour with none grey; a hand points to the
 *  peak. */
export function Dial({ hours, peak, size = 460 }: { hours: number[]; peak: number; size?: number }) {
  const k = size / 460;
  const inner = 118 * k;
  const c = size / 2;
  const top = Math.max(0, ...hours);
  const at = (degrees: number, radius: number) => [c + radius * Math.cos(rad(degrees)), c + radius * Math.sin(rad(degrees))] as const;
  if (hours.length !== 24 || top <= 0) return <svg width={size} height={size} />;
  const handAngle = (peak + 0.5) * 15 - 90;
  const [hx, hy] = at(handAngle, inner - 40 * k);
  return (
    <svg width={size} height={size} style={{ display: "block", flex: "none" }}>
      <circle cx={c} cy={c} r={204 * k} fill={C.white} stroke={C.hairline} strokeWidth={1.5} />
      {/* Ninety-six ticks, a longer one at every hour. */}
      {Array.from({ length: 96 }, (_, index) => {
        const long = index % 4 === 0;
        const angle = index * 3.75 - 90;
        const [x1, y1] = at(angle, 196 * k);
        const [x2, y2] = at(angle, (long ? 203 : 199) * k);
        return <line key={index} x1={x1} y1={y1} x2={x2} y2={y2} stroke={long ? C.ink : "#C9C9C2"} strokeWidth={long ? 2 : 1} />;
      })}
      {/* One wedge per hour, a hair narrower than its 15 degrees. */}
      {hours.map((value, hour) => {
        const start = hour * 15 - 90 + 1.2;
        const end = (hour + 1) * 15 - 90 - 1.2;
        const reach = value > 0 ? Math.max(inner + 4 * k, inner + ((182 * k - inner) * value) / top) : inner + 4 * k;
        const [x1, y1] = at(end, inner);
        const d = `${arcPath(c, c, reach, start, end)} L${f(x1)} ${f(y1)} A${f(inner)} ${f(inner)} 0 0 0 ${f(at(start, inner)[0])} ${f(at(start, inner)[1])} Z`;
        return <path key={hour} d={d} fill={hour === peak ? C.ink : value > 0 ? C.lime : C.restBar} />;
      })}
      {/* The hand, to the middle of the peak hour. */}
      <line x1={c} y1={c} x2={hx} y2={hy} stroke={C.ink} strokeWidth={7 * k} strokeLinecap="round" />
      <circle cx={c} cy={c} r={16 * k} fill={C.ink} />
      <circle cx={c} cy={c} r={6 * k} fill={C.lime} />
      {/* 00, 06, 12, 18 inside the wedges, on the angles of the hours. */}
      {[0, 6, 12, 18].map((hour) => {
        const [x, y] = at(hour * 15 - 90, inner - 24 * k);
        return (
          <text key={hour} x={x} y={y} textAnchor="middle" dominantBaseline="central" fontFamily={MONO} fontSize={20 * k} fill={C.tertiary}>
            {F.pad2(hour)}
          </text>
        );
      })}
    </svg>
  );
}

// MARK: Segment ring

/** A ring of equal segments, the first `filled` of them in ink. Segments are butt-ended arcs with
 *  a small gap, drawn from the top. */
export function SegmentRing({ filled, segments, size, lineWidth = 9 }: { filled: number; segments: number; size: number; lineWidth?: number }) {
  if (segments <= 0) return <svg width={size} height={size} />;
  const radius = size / 2 - lineWidth / 2 - 2;
  const step = 360 / segments;
  return (
    <svg width={size} height={size} style={{ display: "block", flex: "none" }}>
      {Array.from({ length: segments }, (_, i) => (
        <path
          key={i}
          d={arcPath(size / 2, size / 2, radius, i * step - 90 + 2, (i + 1) * step - 90 - 2)}
          fill="none"
          stroke={i < filled ? C.ink : "#E2E2DB"}
          strokeWidth={lineWidth}
        />
      ))}
    </svg>
  );
}

// MARK: Streak chain

/** A run of days drawn as a chain of small squares, the run's own in lime with an ink edge. */
export function StreakChain({ days, runStart, runEnd, first, last }: { days: string[]; runStart: number; runEnd: number; first: string; last: string }) {
  return (
    <V gap={8} align="flex-start">
      <H gap={5}>
        {days.map((day, index) => {
          const inRun = index >= runStart && index < runEnd;
          return (
            <div
              key={day}
              style={{
                width: 22,
                height: 22,
                flex: "none",
                borderRadius: 6,
                background: inRun ? C.lime : C.heatZero,
                border: inRun ? `2px solid ${C.ink}` : undefined,
              }}
            />
          );
        })}
      </H>
      <H gap={8} align="center" style={{ alignSelf: "stretch" }}>
        <T size={14} mono color={C.tertiary} lines={1}>
          {F.shortDay(first)}
        </T>
        <div style={{ flex: "1 1 0", height: 1, background: C.rule }} />
        <T size={14} mono color={C.tertiary} lines={1}>
          {F.shortDay(last)}
        </T>
      </H>
    </V>
  );
}

// MARK: Agent strip

/** The days (or months) of the period as a strip of cells: ink where the agent worked, grey where
 *  another did, a pale fill for a quiet weekend and an outline for a quiet weekday. A fixed width,
 *  so a 28-day month and a 31-day one line up. */
export function AgentStrip({ marks, width = 390, height = 22 }: { marks: Mark[]; width?: number; height?: number }) {
  const count = Math.max(marks.length, 1);
  const gap = count > 12 ? 3 : 4;
  const cell = (width - gap * (count - 1)) / count;
  const radius = count > 12 ? 3 : 5;
  const fill = (mark: Mark) => (mark === "used" ? C.ink : mark === "other" ? "#E2E2DB" : mark === "quietWeekend" ? "#EDEDE7" : "transparent");
  return (
    <svg width={width} height={height} style={{ display: "block", flex: "none" }}>
      {marks.map((mark, index) => {
        const x = index * (cell + gap);
        if (mark === "quiet") return <rect key={index} x={x + 0.5} y={0.5} width={cell - 1} height={height - 1} rx={radius} fill="none" stroke="#DCDCD5" strokeWidth={1} />;
        if (mark === "toCome") return <rect key={index} x={x + 0.75} y={0.75} width={cell - 1.5} height={height - 1.5} rx={radius} {...dashed(C.rule)} />;
        return <rect key={index} x={x} y={0} width={cell} height={height} rx={radius} fill={fill(mark)} />;
      })}
    </svg>
  );
}

// MARK: Bars

/** A bar in a ranked list: a pale track with the fill over it. */
export function RankBar({ fraction, color, height, style }: { fraction: number; color: string; height: number; style?: CSSProperties }) {
  return (
    <div style={{ flex: "1 1 0", minWidth: 0, height, borderRadius: height / 2, background: C.track, position: "relative", ...style }}>
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          bottom: 0,
          width: `max(${height}px, ${Math.min(Math.max(fraction, 0), 1) * 100}%)`,
          borderRadius: height / 2,
          background: color,
        }}
      />
    </div>
  );
}

/** Twelve vertical bars, January to December, the busiest in lime. For the year poster, where it
 *  stands in for the month calendar. */
export function MonthBars({ months, labelSize, labelHeight }: { months: MonthRow[]; labelSize: number; labelHeight: number }) {
  const maximum = Math.max(1, ...months.map((m) => m.tokens));
  let busiest: number | undefined;
  let best = -1;
  for (const m of months) {
    if (m.tokens > best) {
      best = m.tokens;
      busiest = m.month;
    }
  }
  return (
    <Geo style={{ flex: "1 1 0", width: "100%" }}>
      {(_w, h) => {
        const barArea = Math.max(h - labelHeight - 8, 10);
        return (
          <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "flex-end", gap: 7 }}>
            {months.map((month) => {
              const lead = month.month === busiest;
              return (
                <div key={month.month} style={{ flex: "1 1 0", minWidth: 0, display: "flex", flexDirection: "column", alignItems: "stretch", justifyContent: "flex-end", gap: 8 }}>
                  <div
                    style={{
                      height: Math.max(6, (barArea * month.tokens) / maximum),
                      borderRadius: 6,
                      background: lead ? C.lime : "#E0E0DA",
                      border: lead ? `2px solid ${C.ink}` : undefined,
                      boxSizing: "border-box",
                    }}
                  />
                  <div style={{ height: labelHeight, display: "flex", alignItems: "center", justifyContent: "center" }}>
                    <T size={labelSize} w={lead ? 600 : 400} color={lead ? C.ink : C.secondary} fit={0.5} align="center" style={{ width: "100%" }}>
                      {F.shortMonthName(month.month)}
                    </T>
                  </div>
                </div>
              );
            })}
          </div>
        );
      }}
    </Geo>
  );
}

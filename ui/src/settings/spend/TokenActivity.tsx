// Ported from upstream Settings/TokenActivityView.swift and Usage/TokenActivity.swift (the drawing helpers):
// the last twelve months as a grid with a square per day, a bar per week, or the running total. One
// horizontal scale for all three, so the month labels are the same for each and switching views moves
// nothing sideways.
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import {
  chartDate, exactTokens, intlLocale, tokenCount,
  type ActivityDay, type ActivityWeek, type TokenActivity,
} from "../../shared/spend";
import { Segmented } from "../controls";
import { FloatingCard, HoverPlot, TooltipCard, useWidth, type Sample } from "./SpendCharts";

export type ActivityView = "daily" | "weekly" | "cumulative";

const LABEL_HEIGHT = 13;
/** How far a month label may run past the plot's right edge: inside the card's own padding. */
const LABEL_OVERHANG = 12;
const MONTH_GAP_COLUMNS = 3;

// MARK: - Derived data

/** 0 for a quiet day, 1-4 for a day with work, by quartile of the busy days. */
const step = (activity: TokenActivity, tokens: number) =>
  tokens <= 0 ? 0 : 1 + activity.cuts.filter((cut) => tokens > cut).length;

const drawn = (week: ActivityWeek) => week.days.filter((d): d is ActivityDay => d !== null);
const weekTokens = (week: ActivityWeek) => drawn(week).reduce((sum, d) => sum + d.tokens, 0);

interface Mark {
  column: number;
  date: Date;
  position: number;
}

/**
 * A label at each month's first drawn day through to the running month, placed where that day sits in week
 * columns. When two are closer than three columns the earlier one goes, so the newest month is always named.
 */
function monthMarks(activity: TokenActivity): Mark[] {
  const kept: Mark[] = [];
  let previous: number | null = null;
  activity.weeks.forEach((week, column) => {
    week.days.forEach((day, row) => {
      if (day === null && !week.unrecorded[row]) return;
      const date = new Date(week.start);
      date.setDate(date.getDate() + row);
      const month = date.getMonth();
      if (previous !== month) {
        const position = column + row / 7;
        const last = kept[kept.length - 1];
        if (last && position - last.position < MONTH_GAP_COLUMNS) kept.pop();
        kept.push({ column, date, position });
      }
      previous = month;
    });
  });
  return kept;
}

let measureContext: CanvasRenderingContext2D | null | undefined;
function textWidth(text: string): number {
  if (measureContext === undefined) {
    try { measureContext = document.createElement("canvas").getContext("2d"); } catch { measureContext = null; }
    if (measureContext) measureContext.font = `10px "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif`;
  }
  return measureContext ? measureContext.measureText(text).width : text.length * 6;
}

const monthName = (date: Date) => new Intl.DateTimeFormat(intlLocale(), { month: "short" }).format(date);

function placeMonths(marks: Mark[], width: number, columns: number) {
  const pitch = width / columns;
  const placed: { mark: Mark; x: number; width: number; label: string }[] = [];
  for (const mark of [...marks].reverse()) {
    const label = monthName(mark.date);
    const w = textWidth(label);
    const x = Math.max(Math.min(mark.position * pitch, width + LABEL_OVERHANG - w), 0);
    const next = placed[placed.length - 1];
    if (next && x + w + 4 > next.x) continue;
    placed.push({ mark, x, width: w, label });
  }
  return placed.reverse();
}

/** "Oct 6 – 12, 2025", or the one day where a clipped week holds only one. */
function weekTitle(first: ActivityDay, last: ActivityDay): string {
  const a = new Date(first.date);
  const b = new Date(last.date);
  if (a.getTime() >= b.getTime()) return chartDate(first.date);
  return new Intl.DateTimeFormat(intlLocale(), { year: "numeric", month: "short", day: "numeric" }).formatRange(a, b);
}

// MARK: - Section

export function TokenActivitySection({ activity, view, onView }: {
  activity: TokenActivity;
  view: ActivityView;
  onView: (v: ActivityView) => void;
}) {
  const columns = Math.max(activity.weeks.length, 1);
  const [ref, width] = useWidth<HTMLDivElement>();
  const marks = monthMarks(activity);

  return (
    <section>
      <div className="activity-head">
        <h3 className="group-title">{t("Token activity")}</h3>
        <Segmented
          label={t("Token activity")}
          value={view}
          onChange={onView}
          options={[
            { value: "daily", label: t("Daily") },
            { value: "weekly", label: t("Weekly") },
            { value: "cumulative", label: t("Cumulative") },
          ]}
        />
      </div>
      <div className="group-card spend-card">
        <div className="activity-line">
          <span className="num secondary">{t("%@ tokens in the last 12 months", tokenCount(activity.total))}</span>
          {view === "daily" && <Legend />}
        </div>
        <div title={t("%@ over the last 12 months, %@ active days", exactTokens(activity.total), activity.activeDays)}>
          {view === "daily" && <DailyGrid activity={activity} />}
          {view === "weekly" && <WeeklyBars activity={activity} />}
          {view === "cumulative" && <CumulativeLine activity={activity} />}
        </div>
        <div ref={ref} className="month-labels" style={{ height: LABEL_HEIGHT }} aria-hidden>
          {width > 0 &&
            placeMonths(marks, width, columns).map((p) => (
              <span key={p.mark.column} style={{ left: p.x }}>{p.label}</span>
            ))}
        </div>
        {activity.hasPartialCounts && <div className="secondary small">{t("Counts may be incomplete.")}</div>}
      </div>
    </section>
  );
}

function Legend() {
  return (
    <span className="legend" aria-hidden>
      <span>{t("Less")}</span>
      {[0, 1, 2, 3, 4].map((s) => <i key={s} className={`cell step-${s}`} />)}
      <span>{t("More")}</span>
    </span>
  );
}

// MARK: - Daily

function DailyGrid({ activity }: { activity: TokenActivity }) {
  const columns = Math.max(activity.weeks.length, 1);
  const [ref, width] = useWidth<HTMLDivElement>();
  const [hovered, setHovered] = useState<{ column: number; row: number } | null>(null);
  const pitch = width / columns;
  const height = pitch * 7;
  const gap = Math.min(Math.max(pitch * 0.2, 1), 3);
  const radius = Math.max((pitch - gap) * 0.24, 1);
  useEffect(() => setHovered(null), [activity]);
  const hoveredDay = hovered ? activity.weeks[hovered.column]?.days[hovered.row] ?? null : null;

  const move = (e: React.PointerEvent) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    let next: { column: number; row: number } | null = null;
    if (pitch > 0 && x >= 0 && y >= 0) {
      const column = Math.floor(x / pitch);
      const row = Math.floor(y / pitch);
      if (row < 7 && activity.weeks[column]?.days[row]) next = { column, row };
    }
    setHovered((old) => (old?.column === next?.column && old?.row === next?.row ? old : next));
  };

  return (
    <div ref={ref} className="plot" style={{ height }} onPointerMove={move} onPointerLeave={() => setHovered(null)} role="img" aria-label={t("Token activity")}>
      {width > 0 && (
        <svg width={width} height={height}>
          {activity.weeks.map((week, column) =>
            week.days.map((day, row) => {
              const common = { x: column * pitch, y: row * pitch, width: pitch - gap, height: pitch - gap, rx: radius };
              if (!day) return week.unrecorded[row] ? <rect key={`${column}.${row}`} {...common} className="cell-unrecorded" /> : null;
              return <rect key={`${column}.${row}`} {...common} className={`cell step-${step(activity, day.tokens)}`} />;
            }),
          )}
          {hovered && (
            <rect
              x={hovered.column * pitch - 0.5}
              y={hovered.row * pitch - 0.5}
              width={pitch - gap + 1}
              height={pitch - gap + 1}
              rx={radius}
              className="cell-ring"
            />
          )}
        </svg>
      )}
      {hovered && hoveredDay && (
        <FloatingCard
          left={hovered.column * pitch}
          right={hovered.column * pitch + pitch - gap}
          midY={hovered.row * pitch + (pitch - gap) / 2}
          bounds={{ w: width, h: height }}
          gap={8}
        >
          <TooltipCard title={chartDate(hoveredDay.date)} tokens={hoveredDay.tokens} />
        </FloatingCard>
      )}
    </div>
  );
}

// MARK: - Weekly

function WeeklyBars({ activity }: { activity: TokenActivity }) {
  const columns = Math.max(activity.weeks.length, 1);
  return (
    <HoverPlot
      height={(w) => (w / columns) * 7}
      data={activity}
      label={t("Token activity")}
      build={(width, height) => {
        const pitch = width / columns;
        const peak = Math.max(...activity.weeks.map(weekTokens), 1);
        const w = Math.max(pitch * 0.72, 1);
        const samples: Sample[] = [];
        const bars = activity.weeks.map((week, column) => {
          const days = drawn(week);
          if (!days.length) return null;
          const tokens = weekTokens(week);
          samples.push({ x: (column + 0.5) * pitch, title: weekTitle(days[0], days[days.length - 1]), tokens });
          const h = tokens > 0 ? Math.max(((height - 1) * tokens) / peak, 3) : 2;
          return <rect key={column} x={(column + 0.5) * pitch - w / 2} y={height - h} width={w} height={h} rx={Math.min(w, 5) / 2} className={tokens > 0 ? "fill-accent" : "fill-quiet"} />;
        });
        return {
          content: (
            <svg width={width} height={height}>
              {bars}
              <rect x={0} y={height - 1} width={width} height={1} className="fill-quiet" />
            </svg>
          ),
          samples,
        };
      }}
    />
  );
}

// MARK: - Cumulative

function CumulativeLine({ activity }: { activity: TokenActivity }) {
  const columns = Math.max(activity.weeks.length, 1);
  const list: { date: string; running: number; position: number }[] = [];
  // The running total, one point per drawn day.
  let running = 0;

  activity.weeks.forEach((week, column) =>
    week.days.forEach((day, row) => {
      if (!day) return;
      running += day.tokens;
      list.push({ date: day.date, running, position: column + (row + 0.5) / 7 });
    }),
  );
  return (
    <HoverPlot
      height={(w) => (w / columns) * 7}
      data={activity}
      label={t("Token activity")}
      build={(width, height) => {
        const pitch = width / columns;
        const total = Math.max(list[list.length - 1]?.running ?? 1, 1);
        const y = (value: number) => height - 1 - ((height - 3) * value) / total;
        let content = null;
        if (list.length) {
          const line = `M ${list[0].position * pitch} ${height - 1} ` + list.map((p) => `L ${p.position * pitch} ${y(p.running)}`).join(" ");
          const area = `${line} L ${list[list.length - 1].position * pitch} ${height - 1} Z`;
          content = (
            <svg width={width} height={height}>
              <path d={area} className="line-area" />
              <path d={line} className="line-stroke" />
              <rect x={0} y={height - 1} width={width} height={1} className="fill-quiet" />
            </svg>
          );
        }
        return {
          content,
          samples: list.map((p) => ({ x: p.position * pitch, title: t("Total through %@", chartDate(p.date)), tokens: p.running })),
        };
      }}
    />
  );
}


// Ported from upstream Settings/TokenSpendView.swift (`daily`, `header`, `cell`, `DayColumn`) and
// ModelSpendDetailView.swift (`daily`, `cell`, `tokenCell`): every day with work on it, the split and the
// money beside it, sorted by clicking a column heading and cut into pages. The two tables differ only in what
// a zero or a missing figure shows, so one component draws both from rows already reduced to the cells.
import { useState } from "react";
import { t } from "../../shared/i18n";
import {
  accountsFor, exactTokens, shortDate, tallyFresh, tokenCount,
  type ModelDay, type SummaryDay,
} from "../../shared/spend";
import { Group } from "../Group";
import { CostText, PAGE_SIZES, pageOf, SpendPageFooter } from "./SpendCharts";

export type DayColumn = "date" | "fresh" | "cacheRead" | "output" | "unclassified" | "total" | "cost";

const ALL_COLUMNS: DayColumn[] = ["date", "fresh", "cacheRead", "output", "unclassified", "total", "cost"];

/** Short, because columns of Chinese headings in a settings pane are a table that wraps. */
const columnTitle = (c: DayColumn) =>
  ({ date: t("Date"), fresh: t("Input"), cacheRead: t("Cached"), output: t("Output"), unclassified: t("Unclassified"), total: t("Total"), cost: t("Cost") })[c];

/** One day reduced to what the cells show. Null is a figure the day does not have: it sorts last either way. */
export interface DayRow {
  date: string;
  tokens: number;
  fresh: number | null;
  cacheRead: number | null;
  output: number | null;
  unclassified: number | null;
  /** The money, or null where nothing that day could be priced. */
  cost: number | null;
  unpriced: number;
}

/** A day of the combined or one agent's page (upstream `SummaryDay`). */
export function summaryDayRow(day: SummaryDay, cacheUnreported: boolean): DayRow {
  // Known kinds and the explicit remainder must reconcile; unknown-only work has no measured input/output zeroes to show.
  const complete = !day.hasInvalidCategories && accountsFor(day.tally, day.tokens, day.unclassifiedTokens);
  const classified = complete && !(day.tokens > 0 && day.unclassifiedTokens === day.tokens) ? day.tally : null;
  const cacheRead = classified ? (classified.cacheRead ?? 0) : null;
  return {
    date: day.date,
    tokens: day.tokens,
    fresh: classified ? tallyFresh(classified) : null,
    // No cache column in any store behind it: a zero hit is unrecorded, blank in the cell as in the sort.
    cacheRead: cacheUnreported && cacheRead === 0 ? null : cacheRead,
    output: classified ? (classified.output ?? 0) : null,
    unclassified: complete ? day.unclassifiedTokens : null,
    // An all-unpriced day is not a free day: the same unavailable mark a model's day would have, not $0.00.
    cost: day.tokens > 0 && day.unpricedTokens === day.tokens ? null : day.cost,
    unpriced: day.unpricedTokens,
  };
}

/** A day of one model's page (upstream `ModelSpendSummary.Day`). */
export function modelDayRow(day: ModelDay, cacheUnreported: boolean): DayRow {
  const classified = day.tally && !(day.tokens > 0 && day.unclassifiedTokens === day.tokens) ? day.tally : null;
  const cacheRead = classified ? (classified.cacheRead ?? 0) : null;
  const cost = day.costBreakdown ? day.costBreakdown.input + day.costBreakdown.cacheWrite + day.costBreakdown.cacheRead + day.costBreakdown.output : null;
  return {
    date: day.date,
    tokens: day.tokens,
    fresh: classified ? tallyFresh(classified) : null,
    // A store with no cache column has no hits to show, not zero.
    cacheRead: cacheUnreported && cacheRead === 0 ? null : cacheRead,
    output: classified ? (classified.output ?? 0) : null,
    unclassified: day.tally ? day.unclassifiedTokens : null,
    cost,
    unpriced: day.unpricedTokens,
  };
}

const value = (row: DayRow, column: DayColumn): number | null =>
  column === "date" ? Date.parse(row.date) : column === "total" ? row.tokens : row[column];

/**
 * The rows in the order a column asks for. A missing category or price is not a zero: a missing cell sorts
 * last whichever way the column is turned, a known zero sorts where zero belongs, and equal values keep an
 * order broken by date (upstream `SpendSummary.sorted` / `ModelSpendSummary.sorted`).
 */
export function sortDays(rows: DayRow[], column: DayColumn, ascending: boolean): DayRow[] {
  const byDate = (a: DayRow, b: DayRow) => (ascending ? 1 : -1) * (Date.parse(a.date) - Date.parse(b.date));
  return [...rows].sort((lhs, rhs) => {
    const left = value(lhs, column);
    const right = value(rhs, column);
    if (left === null && right === null) return byDate(lhs, rhs);
    if (left === null) return 1;
    if (right === null) return -1;
    if (left === right) return byDate(lhs, rhs);
    return (left < right) === ascending ? -1 : 1;
  });
}

function Chevron({ up }: { up: boolean }) {
  return (
    <svg viewBox="0 0 8 5" width="7" height="5" aria-hidden>
      <path d={up ? "M1 4 4 1l3 3" : "M1 1l3 3 3-3"} fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export function DayTable<D extends { tokens: number }>({ days, cacheUnreported, unclassifiedTokens, makeRow, zeroAsDash }: {
  /** Every day of the span, the quiet ones too; they are dropped here, where nobody reads past them. */
  days: D[];
  cacheUnreported: boolean;
  /** The span's unclassified tokens: the column is drawn only where there are some (or it is the one sorted by). */
  unclassifiedTokens: number;
  makeRow: (day: D, cacheUnreported: boolean) => DayRow;
  /** The combined page shows a zero as a dash; a model's shows "0". */
  zeroAsDash: boolean;
}) {
  const [sort, setSort] = useState<DayColumn>("date");
  const [ascending, setAscending] = useState(false);
  const [pageSize, setPageSize] = useState<number>(PAGE_SIZES[0]);
  const [page, setPage] = useState(0);

  const columns = ALL_COLUMNS.filter((c) => c !== "unclassified" || unclassifiedTokens > 0 || sort === "unclassified");
  const rows = sortDays(days.filter((d) => d.tokens > 0).map((d) => makeRow(d, cacheUnreported)), sort, ascending);
  const current = pageOf(rows.length, pageSize, page);
  const shown = rows.slice(current * pageSize, (current + 1) * pageSize);

  const choose = (column: DayColumn) => {
    setPage(0);
    if (sort === column) {
      setAscending((a) => !a);
    } else {
      setSort(column);
      // A new column starts at the end people look at first: the most recent day, or the largest figure.
      setAscending(false);
    }
  };

  const tokenCell = (tokens: number | null) =>
    tokens === null ? (
      <span className="dash">—</span>
    ) : tokens > 0 ? (
      <span title={exactTokens(tokens)}>{tokenCount(tokens)}</span>
    ) : zeroAsDash ? (
      <span className="dash">—</span>
    ) : (
      <span className="dim" title={exactTokens(0)}>0</span>
    );

  const cell = (row: DayRow, column: DayColumn) => {
    switch (column) {
      case "date": return <span>{shortDate(row.date)}</span>;
      case "fresh": return tokenCell(row.fresh);
      case "cacheRead": return tokenCell(row.cacheRead);
      case "output": return tokenCell(row.output);
      case "unclassified": return tokenCell(row.unclassified);
      case "total": return tokenCell(row.tokens);
      case "cost": return <CostText cost={row.cost} unpriced={row.unpriced} />;
    }
  };

  return (
    <Group title={t("Day by day")}>
      <div className="day-table" style={{ gridTemplateColumns: `repeat(${columns.length}, minmax(max-content, 1fr))` }}>
        {columns.map((column) => (
          <button
            key={column}
            type="button"
            className="day-head"
            data-sorted={sort === column}
            data-align={column === "date" ? "left" : "right"}
            onClick={() => choose(column)}
          >
            <span>{columnTitle(column)}</span>
            {sort === column && <Chevron up={ascending} />}
          </button>
        ))}
        {shown.map((row) =>
          columns.map((column) => (
            <div key={`${row.date}|${column}`} className="day-cell" data-align={column === "date" ? "left" : "right"}>
              {cell(row, column)}
            </div>
          )),
        )}
      </div>
      {rows.length > PAGE_SIZES[0] && (
        <SpendPageFooter rows={rows.length} pageSize={pageSize} page={current} onPageSize={setPageSize} onPage={setPage} />
      )}
    </Group>
  );
}

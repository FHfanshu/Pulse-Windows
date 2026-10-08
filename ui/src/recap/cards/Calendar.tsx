// The month's calendar card, the year's calendar and months cards, and the pieces they share: the
// hero total, the three tiles, and the split between weekdays and the weekend (upstream
// RecapCalendarView and RecapYearViews).
import { t } from "../../shared/i18n";
import { SegmentRing, StreakChain, heatColor, monthGrid } from "../charts";
import type { Deck } from "../deck";
import * as F from "../format";
import { C, FigureText, Geo, H, Hairline, Pill, SectionHead, Spacer, StoryPage, T, Tile, V, toneCaption, toneDetail } from "../kit";
import type { Day } from "../types";

// MARK: Hero

/** The period's total, huge, on a lime bar, with how it compares to the period before and what it
 *  cost beside it. */
export function CalendarHero({ deck, numberSize = 240, unitSize = 116 }: { deck: Deck; numberSize?: number; unitSize?: number }) {
  const recap = deck.recap;
  const figure = F.tokens(recap.tokens);
  // "TOKENS · ≈ $622"; without a cost, "TOKENS".
  const tokensLine = recap.cost === undefined ? "TOKENS" : "TOKENS · ≈ " + F.money(recap.cost, recap.currency);
  return (
    <V gap={34} align="stretch" style={{ flex: "none" }}>
      <T size={32} color={C.grey55} lines={2}>
        {deck.workedThroughLine}
      </T>
      <H gap={24} align="flex-end">
        <div style={{ position: "relative", flex: "none" }}>
          <div style={{ position: "absolute", left: -8, right: -14, bottom: numberSize * 0.0175, height: numberSize * 0.38, borderRadius: 6, background: C.lime }} />
          <FigureText number={figure.number} unit={figure.unit} numberSize={numberSize} unitSize={unitSize} unitGap={14} trimmed style={{ position: "relative" }} />
        </div>
        <Spacer />
        <V gap={12} align="flex-end" style={{ paddingBottom: 14, flex: "0 1 auto", minWidth: 0 }}>
          {deck.change ? (
            <Pill fill={C.ink} horizontal={12} vertical={6}>
              <T size={18} color={C.lime} lines={1}>
                {`${deck.change.arrow} ${deck.change.percent} ${deck.change.versus}`}
              </T>
            </Pill>
          ) : null}
          <div style={{ maxWidth: 360 }}>
            <T size={20} mono color={C.grey55} track={0.8} fit={0.6}>
              {tokensLine}
            </T>
          </div>
        </V>
      </H>
    </V>
  );
}

// MARK: Month calendar

/** The month, every day a cell, with the total above it and what the month held below: active
 *  days, the longest streak, the busiest day, the weeks, and the weekdays against the weekend. */
export function CalendarCard({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const grid = monthGrid(recap.start, recap.days);
  const maximum = Math.max(0, ...recap.days.map((d) => d.tokens));
  const busiest = recap.busiestDay?.date;
  const cellHeight = grid.rows > 5 ? 74 : 86;
  return (
    <StoryPage page={deck.page("calendar")}>
      <div style={{ paddingTop: 44 }}>
        <CalendarHero deck={deck} />
      </div>
      <Spacer min={24} />
      <V gap={12} align="stretch" style={{ flex: "none" }}>
        <SectionHead title={F.monthYear(recap.start)} note={t("Darker days used more · grey is the weekend")} />
        <V gap={8} align="stretch">
          <H gap={8} equal>
            {F.weekdayHeadings().map((heading, i) => (
              <div key={i} style={{ minWidth: 0 }}>
                <T size={16} color={C.tertiary} fit={0.6} align="center">
                  {heading}
                </T>
              </div>
            ))}
          </H>
          {Array.from({ length: grid.rows }, (_, row) => (
            <H key={row} gap={8} equal>
              {Array.from({ length: 7 }, (_, column) => (
                <Cell key={column} day={grid.cells[row * 7 + column]} column={column} maximum={maximum} busiest={busiest} height={cellHeight} />
              ))}
            </H>
          ))}
        </V>
      </V>
      <Spacer min={24} />
      <CalendarTiles deck={deck} />
      <Spacer min={24} />
      <Weeks deck={deck} />
      <Spacer min={24} />
      <WorkSplitView deck={deck} />
    </StoryPage>
  );
}

function Cell({ day, column, maximum, busiest, height }: { day: Day | null; column: number; maximum: number; busiest?: string; height: number }) {
  const box = { flex: "1 1 0", minWidth: 0, height } as const;
  if (!day) return <div style={box} />;
  const quiet = day.tokens === 0;
  const weekend = column >= 5;
  // Only the one day the report names, not every tie.
  const isBusiest = !quiet && day.date === busiest;
  const foreground = isBusiest ? C.lime : quiet ? C.faint : C.ink;
  return (
    <div
      style={{
        ...box,
        borderRadius: 12,
        boxSizing: "border-box",
        padding: "10px 8px 10px 12px",
        display: "flex",
        flexDirection: "column",
        background: isBusiest ? C.ink : quiet ? (weekend ? C.heatZero : "transparent") : heatColor(day.tokens, maximum),
        border: quiet && !weekend ? "1.5px dashed #D6D6CF" : undefined,
      }}
    >
      <T size={20} w={600} color={foreground} lines={1}>
        {String(F.dayOfMonth(day.date))}
      </T>
      <Spacer />
      {!quiet ? (
        <T size={13} mono color={foreground} fit={0.6} style={{ opacity: isBusiest ? 1 : 0.75 }}>
          {F.figureText(F.tokens(day.tokens))}
        </T>
      ) : null}
    </div>
  );
}

function Weeks({ deck }: { deck: Deck }) {
  const insights = deck.recap.insights;
  const weeks = insights.weeks;
  const busiest = insights.busiestWeek;
  const top = Math.max(1, ...weeks.map((w) => w.tokens));
  if (weeks.length <= 1) return null;
  return (
    <V gap={12} align="stretch" style={{ flex: "none" }}>
      <SectionHead
        title={t("By week")}
        note={busiest !== undefined ? t("Busiest week: %@", F.dayRange(weeks[busiest].first, weeks[busiest].last)) : null}
      />
      {weeks.map((week, index) => {
        const best = index === busiest;
        return (
          <H key={week.first} gap={16} align="center">
            <div style={{ width: 170, flex: "none" }}>
              <T size={17} w={best ? 700 : 400} mono color={best ? C.ink : C.secondary} fit={0.6}>
                {F.dayRange(week.first, week.last)}
              </T>
            </div>
            <div style={{ flex: "1 1 0", minWidth: 0, height: 22, borderRadius: 6, background: C.heatZero, position: "relative" }}>
              {week.tokens > 0 ? (
                <div
                  style={{
                    position: "absolute",
                    left: 0,
                    top: 0,
                    bottom: 0,
                    width: `max(8px, ${(week.tokens / top) * 100}%)`,
                    borderRadius: 6,
                    background: best ? C.ink : C.lime,
                  }}
                />
              ) : null}
            </div>
            <div style={{ width: 110, flex: "none" }}>
              <T size={17} w={best ? 700 : 400} mono color={best ? C.ink : C.secondary} fit={0.6} align="right">
                {week.tokens > 0 ? F.figureText(F.tokens(week.tokens)) : "—"}
              </T>
            </div>
          </H>
        );
      })}
    </V>
  );
}

// MARK: Tiles

/** Three tiles: the days with work against the days the period had (a ring), the longest streak
 *  (a chain of days), and the busiest day (on ink). */
export function CalendarTiles({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const chain = recap.insights.streakChain;
  const busiest = recap.busiestDay;
  return (
    <H gap={14} equal style={{ height: 190, flex: "none" }}>
      <ActiveTile deck={deck} />
      {chain ? (
        <Tile>
          <V gap={10} align="stretch" style={{ flex: "1 1 0" }}>
            <T size={17} color={toneCaption.white} fit={0.6}>
              {t("Longest streak")}
            </T>
            <FigureText
              number={String(recap.longestStreak)}
              unit={F.Words.daysUnit(recap.longestStreak)}
              numberSize={48}
              unitSize={24}
              unitWeight={700}
              tracking={-0.03}
              unitGap={6}
            />
            <Spacer />
            <StreakChain days={chain.days} runStart={chain.runStart} runEnd={chain.runEnd} first={chain.first} last={chain.last} />
          </V>
        </Tile>
      ) : null}
      {busiest && busiest.tokens > 0 ? <BusiestTile deck={deck} day={busiest} /> : null}
    </H>
  );
}

function ActiveTile({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  // A month's own days are the ring's segments; a year's are too many to draw, so its ring is a
  // fixed 36 in proportion.
  const segments = deck.isYear ? 36 : Math.max(recap.elapsedDays, 1);
  const share = recap.activeDays / Math.max(recap.elapsedDays, 1);
  const filled = deck.isYear ? (recap.activeDays > 0 ? Math.max(Math.round(share * 36), 1) : 0) : recap.activeDays;
  return (
    <Tile>
      <H gap={14} align="center" style={{ flex: "1 1 0" }}>
        <SegmentRing filled={filled} segments={segments} size={84} />
        <V gap={4} align="flex-start" style={{ minWidth: 0, flex: "1 1 0" }}>
          <T size={17} color={toneCaption.white} fit={0.6}>
            {t("Active days")}
          </T>
          <H gap={4} align="last baseline" style={{ flex: "none" }}>
            <T size={48} w={700} tight={-48 * 0.03} lines={1}>
              {String(recap.activeDays)}
            </T>
            <T size={22} color={C.tertiary} lines={1}>
              {`/ ${recap.elapsedDays}`}
            </T>
          </H>
        </V>
      </H>
    </Tile>
  );
}

function BusiestTile({ deck, day }: { deck: Deck; day: Day }) {
  const figure = F.tokens(day.tokens);
  let detail = F.dayAndWeekday(day.date);
  if (day.cost !== undefined) detail += " · ≈ " + F.money(day.cost, deck.recap.currency);
  return (
    <Tile tone="ink">
      <V gap={10} align="stretch" style={{ flex: "1 1 0" }}>
        <T size={17} color={toneCaption.ink} fit={0.6}>
          {t("Busiest day")}
        </T>
        <FigureText number={figure.number} unit={figure.unit} numberSize={48} unitSize={24} unitWeight={700} color={C.lime} tracking={-0.03} unitGap={6} />
        <Spacer />
        <T size={16} color={toneDetail.ink} fit={0.6}>
          {detail}
        </T>
      </V>
    </Tile>
  );
}

// MARK: Weekdays and the weekend

/** How the work divided between Monday to Friday and the weekend: one bar, and a line each for
 *  the days there were and the days used. */
export function WorkSplitView({ deck }: { deck: Deck }) {
  const insights = deck.recap.insights;
  const split = insights.workSplit;
  if (!split) return null;
  const total = split.weekdayTokens + split.weekendTokens;
  const weekdayShare = total > 0 ? split.weekdayTokens / total : 0;
  const weekendShare = total > 0 ? split.weekendTokens / total : 0;
  return (
    <V gap={12} align="stretch" style={{ flex: "none" }}>
      <Hairline strong style={{ marginBottom: 10 }} />
      <SectionHead
        title={t("Weekdays and weekends")}
        note={insights.tokensPerActiveDay !== undefined ? t("Average per active day: %@", F.figureText(F.tokens(insights.tokensPerActiveDay))) : null}
      />
      <Geo style={{ height: 44, width: "100%" }}>
        {(w) => {
          const gap = 4;
          const both = split.weekdayTokens > 0 && split.weekendTokens > 0;
          const room = w - (both ? gap : 0);
          return (
            <div style={{ position: "absolute", inset: 0, display: "flex", gap }}>
              {split.weekdayTokens > 0 ? (
                <Segment
                  word={t("Weekdays %@", F.percent(weekdayShare))}
                  percent={F.percent(weekdayShare)}
                  width={room * weekdayShare}
                  fill={C.ink}
                  text={C.paper}
                  trailing={false}
                />
              ) : null}
              {split.weekendTokens > 0 ? (
                <Segment
                  word={t("Weekends %@", F.percent(weekendShare))}
                  percent={F.percent(weekendShare)}
                  width={room * weekendShare}
                  fill={C.lime}
                  text={C.ink}
                  trailing
                />
              ) : null}
            </div>
          );
        }}
      </Geo>
      <H align="baseline">
        {split.weekdayDays > 0 ? (
          <div style={{ minWidth: 0, flex: "0 1 auto" }}>
            <T size={16} color={C.tertiary} fit={0.6}>
              {t("%@ weekdays, used on %@ of them", split.weekdayDays, split.weekdayActive)}
            </T>
          </div>
        ) : null}
        <Spacer min={12} />
        {split.weekendDays > 0 ? (
          <div style={{ minWidth: 0, flex: "0 1 auto" }}>
            <T size={16} color={C.tertiary} fit={0.6}>
              {t("%@ weekend days, used on %@ of them", split.weekendDays, split.weekendActive)}
            </T>
          </div>
        ) : null}
      </H>
    </V>
  );
}

/** A slice of the bar: the whole label where it fits, the percent alone where it does not, and
 *  nothing where even that would not. */
function Segment({ word, percent, width, fill, text, trailing }: { word: string; percent: string; width: number; fill: string; text: string; trailing: boolean }) {
  return (
    <div
      style={{
        width: Math.max(width, 6),
        flex: "none",
        borderRadius: 12,
        background: fill,
        display: "flex",
        alignItems: "center",
        justifyContent: trailing ? "flex-end" : "flex-start",
        overflow: "hidden",
      }}
    >
      {width >= 180 ? (
        <T size={19} color={text} lines={1} style={{ padding: "0 16px" }}>
          {word}
        </T>
      ) : width >= 64 ? (
        <T size={19} color={text} lines={1} style={{ padding: "0 14px" }}>
          {percent}
        </T>
      ) : null}
    </div>
  );
}

// MARK: Year calendar

/** The year, one small calendar per month. */
export function YearCalendarCard({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const maximum = Math.max(0, ...recap.days.map((d) => d.tokens));
  const year = Number(recap.period.slice(0, 4));
  const starts = Array.from({ length: 12 }, (_, i) => `${year}-${F.pad2(i + 1)}-01`);
  return (
    <StoryPage page={deck.page("yearCalendar")}>
      <div style={{ paddingTop: 40 }}>
        <CalendarHero deck={deck} numberSize={190} unitSize={92} />
      </div>
      <Spacer min={20} />
      <SectionHead title={F.yearName(year)} note={t("Darker days used more")} />
      <div style={{ paddingTop: 20, display: "grid", gridTemplateColumns: "repeat(3, 1fr)", columnGap: 30, rowGap: 18, alignItems: "start", flex: "none" }}>
        {starts.map((first) => (
          <MonthBlock key={first} first={first} days={recap.days} maximum={maximum} />
        ))}
      </div>
      <Spacer min={20} />
      <CalendarTiles deck={deck} />
    </StoryPage>
  );
}

function MonthBlock({ first, days, maximum }: { first: string; days: Day[]; maximum: number }) {
  const grid = monthGrid(first, days);
  const cell = 33;
  const gap = 5;
  return (
    <V gap={10} align="flex-start">
      <div style={{ maxWidth: "100%" }}>
        <T size={22} w={600} fit={0.6}>
          {F.monthName(F.monthOf(first))}
        </T>
      </div>
      <V gap={gap}>
        {Array.from({ length: 6 }, (_, row) => (
          <H key={row} gap={gap}>
            {Array.from({ length: 7 }, (_, column) => {
              const day = grid.cells[row * 7 + column];
              return day ? (
                <div key={column} style={{ width: cell, height: cell, flex: "none", borderRadius: 8, background: heatColor(day.tokens, maximum) }} />
              ) : (
                <div key={column} style={{ width: cell, height: cell, flex: "none" }} />
              );
            })}
          </H>
        ))}
      </V>
    </V>
  );
}

// MARK: Months

/** A year, month by month: the busiest named large, and the twelve as rows. */
export function MonthsCard({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const months = recap.months;
  const maximum = Math.max(1, ...months.map((m) => m.tokens));
  // The earliest of the heaviest months.
  const busiest = months.reduce<(typeof months)[number] | undefined>((best, m) => (best === undefined || m.tokens > best.tokens ? m : best), undefined);
  return (
    <StoryPage page={deck.page("months")}>
      <div style={{ paddingTop: 52 }}>
        <T size={34} color={C.grey55}>
          {t("Your busiest month")}
        </T>
      </div>
      {busiest ? (
        <div style={{ paddingTop: 30 }}>
          <T size={230} w={700} tight={-230 * 0.05} fit={0.35} style={{ lineHeight: 1 }}>
            {F.monthName(busiest.month)}
          </T>
        </div>
      ) : null}
      <div style={{ height: 2, background: C.ink, marginTop: 34, flex: "none" }} />
      <V align="stretch" style={{ paddingTop: 14, flex: "none" }}>
        {months.map((month, index) => (
          <MonthRow key={month.month} month={month} maximum={maximum} isBusiest={month.month === busiest?.month} toCome={recap.insights.monthsToCome[index] === true} />
        ))}
      </V>
      <Spacer min={16} />
      <WorkSplitView deck={deck} />
    </StoryPage>
  );
}

/** One month as a row; a month still to come in a running year is a dashed outline with no
 *  figure, not a quiet month's sliver of a bar. */
function MonthRow({ month, maximum, isBusiest, toCome }: { month: { month: number; tokens: number }; maximum: number; isBusiest: boolean; toCome: boolean }) {
  const fraction = Math.max(month.tokens / maximum, 0.02);
  return (
    <H align="center" style={{ height: 96, flex: "none" }}>
      <div style={{ width: 104, flex: "none" }}>
        <T size={22} w={isBusiest ? 600 : 400} color={isBusiest ? C.ink : C.secondary} fit={0.6}>
          {F.shortMonthName(month.month)}
        </T>
      </div>
      <Geo style={{ flex: "1 1 0", height: "100%" }}>
        {(w) => (
          <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", gap: 14 }}>
            {toCome ? (
              <div style={{ width: 30, height: 30, flex: "none", borderRadius: 4, border: `1.5px dashed ${C.rule}`, boxSizing: "border-box" }} />
            ) : (
              <>
                <div style={{ width: Math.max(6, (w - 150) * fraction), height: isBusiest ? 42 : 30, flex: "none", borderRadius: 4, background: isBusiest ? C.ink : C.restBar }} />
                <T size={18} w={isBusiest ? 600 : 400} mono color={isBusiest ? C.ink : C.secondary} lines={1}>
                  {F.figureText(F.tokens(month.tokens))}
                </T>
              </>
            )}
          </div>
        )}
      </Geo>
    </H>
  );
}

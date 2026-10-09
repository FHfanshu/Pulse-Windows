// The one-page summary: the whole period on a single card (upstream RecapPosterView).
//
// A row is drawn only when its facts exist, and the rows that remain share the height between
// them, so a recap with no price, no cost or no hour shape is a shorter poster, not one with
// holes in it.
import { Fragment, ReactNode } from "react";
import { t } from "../../shared/i18n";
import { Clock, Donut, MonthBars, RankBar, Sparkline, heatColor, monthGrid } from "../charts";
import { Deck, Payback, personaTitle } from "../deck";
import * as F from "../format";
import { Box, C, CARD_WIDTH, FigureText, H, Page, Pill, RichText, Spacer, T, V, grow } from "../kit";
import { Flame, PersonaGlyph, PulseMark } from "../marks";
import type { Day, Report } from "../types";

export function PosterCard({ deck }: { deck: Deck }) {
  const rows = buildRows(deck);
  return (
    <Page padding={[64, 64, 56, 64]}>
      <Header deck={deck} />
      <div style={{ paddingTop: 34 }}>
        <Hero deck={deck} />
      </div>
      {rows.map((row, index) => (
        <Fragment key={index}>
          <Spacer min={20} />
          {row}
        </Fragment>
      ))}
      <Spacer min={20} />
      <Footer deck={deck} />
    </Page>
  );
}

function buildRows(deck: Deck): ReactNode[] {
  const recap = deck.recap;
  const rows: ReactNode[] = [];
  if (recap.cost !== undefined) rows.push(<MoneyRow deck={deck} />);
  rows.push(<ShapeRow deck={deck} />);
  if (recap.models.length) rows.push(<ModelsCard deck={deck} />);
  const tiles: ReactNode[] = [];
  if (recap.agents.length) tiles.push(<AgentsTile deck={deck} />);
  if (recap.cacheHitRate !== undefined) tiles.push(<CacheTile deck={deck} rate={recap.cacheHitRate} />);
  if (deck.streak) tiles.push(<StreakTile deck={deck} />);
  if (tiles.length) rows.push(<H gap={20} equal style={{ height: 236, flex: "none" }}>{tiles}</H>);
  const low: ReactNode[] = [];
  if (recap.busiestDay) low.push(<BusiestCard deck={deck} day={recap.busiestDay} />);
  if (recap.projects.length) low.push(<ProjectsCard deck={deck} />);
  if (low.length) rows.push(<H gap={20} equal style={{ height: 206, flex: "none" }}>{low}</H>);
  return rows;
}

// MARK: Header and hero

function Header({ deck }: { deck: Deck }) {
  const persona = deck.recap.persona;
  return (
    <H align="center" style={{ flex: "none" }}>
      <PulseMark side={46} />
      <div style={{ paddingLeft: 14 }}>
        <T size={26} w={600} lines={1}>
          Pulse
        </T>
      </div>
      <div style={{ paddingLeft: 20 }}>
        <T size={16} mono color={C.secondary} track={1.3} lines={1}>
          {deck.periodLabel}
        </T>
      </div>
      <Spacer min={12} />
      {persona ? (
        <Pill fill={C.ink} horizontal={18} vertical={10}>
          <H gap={8} align="center">
            <PersonaGlyph persona={persona} side={16} />
            <T size={17} w={500} color={C.paper} lines={1}>
              {personaTitle(persona)}
            </T>
          </H>
        </Pill>
      ) : null}
    </H>
  );
}

function Hero({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const figure = F.tokens(recap.tokens);
  return (
    <V gap={30} align="stretch" style={{ flex: "none" }}>
      <T size={22} color={C.secondary} lines={2}>
        {deck.workedThroughLine}
      </T>
      <H gap={30} align="flex-end">
        <div style={{ position: "relative", flex: "none" }}>
          <div style={{ position: "absolute", left: -8, right: -14, bottom: 4, height: 86, borderRadius: 6, background: C.lime }} />
          <FigureText number={figure.number} unit={figure.unit} numberSize={230} unitSize={110} unitGap={14} trimmed style={{ position: "relative" }} />
        </div>
        <V gap={10} align="flex-start" style={{ paddingBottom: 14, flex: "none" }}>
          <T size={18} mono color={C.secondary} track={1.4}>
            TOKENS
          </T>
          {deck.change ? (
            <Pill fill={C.ink} horizontal={12} vertical={6}>
              <span style={{ fontSize: 17, lineHeight: 1.2 }}>
                <span style={{ color: C.lime, fontWeight: 600 }}>{`${deck.change.arrow} ${deck.change.percent}`}</span>
                <span style={{ color: C.paper }}>{` ${deck.change.versus}`}</span>
              </span>
            </Pill>
          ) : null}
        </V>
        <Spacer />
      </H>
      <div style={{ paddingTop: 30 }}>
        <SummaryLine deck={deck} />
      </div>
    </V>
  );
}

function SummaryLine({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const parts = [F.plain(deck.activeDaysLine), F.Words.sessions(recap.sessions)];
  if (deck.tokensPerDay && deck.tokensPerDay > 0) {
    parts.push(t("%@ a day on average", F.figureText(F.tokens(deck.tokensPerDay))));
  }
  return (
    <T size={20} color={C.bodyInk} lines={2}>
      {parts.join("  ·  ")}
    </T>
  );
}

// MARK: Money and payback

function MoneyRow({ deck }: { deck: Deck }) {
  // 1.25 : 1 between the two, as drawn.
  const free = CARD_WIDTH - 128 - 20;
  const payback = deck.payback;
  return (
    <H gap={20} style={{ height: 200, flex: "none" }}>
      {payback ? (
        <>
          <MoneyCard deck={deck} style={{ flex: "none", width: (free * 1.25) / 2.25 }} />
          <PaybackCard deck={deck} payback={payback} style={{ flex: "none", width: free / 2.25 }} />
        </>
      ) : (
        <MoneyCard deck={deck} />
      )}
    </H>
  );
}

function MoneyCard({ deck, style }: { deck: Deck; style?: React.CSSProperties }) {
  const recap = deck.recap;
  return (
    <Box padding={[26, 28, 26, 28]} style={style}>
      <T size={17} color={C.secondary} fit={0.7}>
        {t("Estimated at API prices")}
      </T>
      <Spacer />
      <T size={72} w={600} tight={-72 * 0.03} fit={0.6}>
        {F.money(recap.cost ?? 0, recap.currency)}
      </T>
      <Spacer />
      <div style={{ height: 46, margin: "0 -4px -6px", flex: "none" }}>
        <Sparkline values={deck.costSeries} />
      </div>
    </Box>
  );
}

function PaybackCard({ deck, payback, style }: { deck: Deck; payback: Payback; style?: React.CSSProperties }) {
  const currency = deck.recap.currency;
  return (
    <Box fill={C.lime} padding={[26, 28, 26, 28]} style={style}>
      <T size={17} color={C.limeInk} lines={1}>
        {t("Subscription payback")}
      </T>
      <Spacer />
      <FigureText number={F.multiple(payback.multiple)} unit="×" numberSize={84} unitSize={44} unitWeight={600} tracking={-0.04} unitGap={6} trimmed />
      <Spacer />
      <V gap={7} align="stretch">
        <div style={{ height: 8, borderRadius: 4, background: C.ink }} />
        <div style={{ height: 8, position: "relative" }}>
          <div
            style={{
              position: "absolute",
              left: 0,
              top: 0,
              bottom: 0,
              width: `${Math.min(1, payback.paid / payback.used) * 100}%`,
              borderRadius: 4,
              background: "rgba(27,27,30,0.28)",
            }}
          />
        </div>
        <T size={14} color={C.limeInk} fit={0.7}>
          {t("Paid %@ · used %@", F.money(payback.paid, currency), F.money(payback.used, currency))}
        </T>
      </V>
    </Box>
  );
}

// MARK: Calendar (or months) and rhythm

function CardTitle({ title, note }: { title: string; note?: string | null }) {
  return (
    <H align="baseline" gap={8}>
      <div style={{ minWidth: 0, flex: "0 1 auto" }}>
        <T size={20} w={600} fit={0.7}>
          {title}
        </T>
      </div>
      <Spacer min={8} />
      {note ? (
        <div style={{ minWidth: 0, flex: "0 1 auto" }}>
          <T size={14} color={C.secondary} fit={0.6}>
            {note}
          </T>
        </div>
      ) : null}
    </H>
  );
}

function ShapeRow({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const hasHours = recap.hours !== undefined && recap.peakHour !== undefined;
  // Alone in the row, the calendar takes the width and grows to fill it.
  const wide = !deck.isYear && !hasHours;
  return (
    <H gap={20} equal style={{ height: wide ? 520 : 330, flex: "none" }}>
      {deck.isYear ? <MonthsCard deck={deck} /> : <CalendarCard deck={deck} wide={wide} />}
      {hasHours ? <RhythmCard deck={deck} /> : null}
    </H>
  );
}

function CalendarCard({ deck, wide }: { deck: Deck; wide: boolean }) {
  const recap = deck.recap;
  const grid = monthGrid(recap.start, recap.days);
  const maximum = Math.max(0, ...recap.days.map((d) => d.tokens));
  const busiest = recap.busiestDay?.date;
  const cell = wide ? 54 : grid.rows > 5 ? 32 : 36;
  const gap = wide ? 12 : grid.rows > 5 ? 7 : 8;
  return (
    <Box padding={[24, 26, 24, 26]}>
      <V gap={12} align="stretch">
        <CardTitle title={t("Every day")} note={t("Darker days used more")} />
        <V gap={gap} align="center">
          <H gap={gap}>
            {F.weekdayHeadings().map((heading, i) => (
              <div key={i} style={{ width: cell, flex: "none" }}>
                <T size={13} color={C.secondary} fit={0.6} align="center">
                  {heading}
                </T>
              </div>
            ))}
          </H>
          {Array.from({ length: grid.rows }, (_, row) => (
            <H key={row} gap={gap}>
              {Array.from({ length: 7 }, (_, column) => (
                <DayCell key={column} day={grid.cells[row * 7 + column]} maximum={maximum} busiest={busiest} side={cell} />
              ))}
            </H>
          ))}
        </V>
      </V>
    </Box>
  );
}

function DayCell({ day, maximum, busiest, side }: { day: Day | null; maximum: number; busiest?: string; side: number }) {
  if (!day) return <div style={{ width: side, height: side, flex: "none" }} />;
  // Only the one day the report names, not every tie.
  const isBusiest = day.tokens > 0 && day.date === busiest;
  return (
    <div style={{ position: "relative", width: side, height: side, flex: "none", borderRadius: 9, background: heatColor(day.tokens, maximum) }}>
      {isBusiest ? <div style={{ position: "absolute", inset: -4, borderRadius: 11, border: `2px solid ${C.ink}` }} /> : null}
    </div>
  );
}

function MonthsCard({ deck }: { deck: Deck }) {
  return (
    <Box padding={[24, 26, 24, 26]}>
      <V gap={14} align="stretch" style={{ flex: "1 1 0" }}>
        <CardTitle title={t("Month by month")} />
        <MonthBars months={deck.recap.months} labelSize={14} labelHeight={22} />
      </V>
    </Box>
  );
}

function RhythmCard({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  return (
    <Box dark padding={[24, 26, 24, 26]}>
      <V gap={4} align="stretch" style={{ flex: "1 1 0" }}>
        <H align="baseline">
          <div style={{ minWidth: 0, flex: "0 1 auto" }}>
            <T size={20} w={600} color={C.paper} fit={0.7}>
              {t("Rhythm of the day")}
            </T>
          </div>
          <Spacer min={8} />
          {recap.peakHour !== undefined ? (
            <T size={14} color="rgba(245,245,241,0.6)" lines={1}>
              {t("Peak %@", F.hourLabel(recap.peakHour))}
            </T>
          ) : null}
        </H>
        <div style={{ flex: "1 1 0", minHeight: 0, display: "flex", alignItems: "center", justifyContent: "center" }}>
          <div style={{ position: "relative", width: 236, height: 236 }}>
            <Clock hours={recap.hours ?? []} inner={54} longest={62} barWidth={7} labelSize={11} size={236} />
            {recap.persona ? (
              <div style={{ position: "absolute", left: 118 - 15, top: 118 - 15 }}>
                <PersonaGlyph persona={recap.persona} side={30} />
              </div>
            ) : null}
          </div>
        </div>
        {rhythmLine(recap) ? (
          <T size={16} color="rgba(245,245,241,0.72)" align="center" fit={0.8}>
            {rhythmLine(recap)}
          </T>
        ) : null}
      </V>
    </Box>
  );
}

// MARK: Models

function ModelsCard({ deck }: { deck: Deck }) {
  const models = deck.recap.models.slice(0, 5);
  const top = Math.max(models[0]?.share ?? 0, 0.0001);
  return (
    <Box padding={[26, 28, 26, 28]} style={{ flex: "none" }}>
      <V gap={16} align="stretch">
        <CardTitle title={t("Model ranking")} note={t("By token share")} />
        {models.map((model, index) => (
          <H key={model.name} gap={14} align="center">
            <div style={{ width: 28, flex: "none" }}>
              <T size={15} mono color={C.faint}>{`0${index + 1}`}</T>
            </div>
            <div style={{ width: 210, flex: "none" }}>
              <T size={19} w={500} fit={0.65}>
                {model.name}
              </T>
            </div>
            <RankBar fraction={model.share / top} color={C.ranks[Math.min(index, 4)]} height={14} />
            <div style={{ width: 58, flex: "none", textAlign: "right" }}>
              <T size={16} mono align="right">
                {F.percent(model.share)}
              </T>
            </div>
          </H>
        ))}
      </V>
    </Box>
  );
}

// MARK: Tiles: agents, cache, streak

const AGENT_COLORS = [C.lime, C.ink, "#B9B9B2"];

function AgentsTile({ deck }: { deck: Deck }) {
  const agents = deck.recap.agents;
  const top = agents.slice(0, agents.length > 3 ? 2 : agents.length);
  const parts = top.map((agent, i) => ({ share: agent.share, color: AGENT_COLORS[i] }));
  const legend = top.map((agent, i) => ({ name: agent.name, share: agent.share, color: AGENT_COLORS[i] }));
  if (agents.length > 3) {
    const other = agents.slice(2).reduce((s, a) => s + a.share, 0);
    parts.push({ share: other, color: "#B9B9B2" });
    legend.push({ name: t("Other"), share: other, color: "#B9B9B2" });
  }
  return (
    <Box padding={[22, 22, 22, 22]}>
      <V gap={10} align="stretch">
        <T size={18} w={600} lines={1}>
          {t("By agent")}
        </T>
        <div style={{ display: "flex", justifyContent: "center" }}>
          <div style={{ position: "relative", width: 96, height: 96 }}>
            <Donut parts={parts} thickness={13} size={96} />
            <div style={{ position: "absolute", left: 18, top: 0, bottom: 0, width: 60, display: "flex", alignItems: "center" }}>
              <T size={13} mono color={C.secondary} fit={0.6} align="center" style={{ width: "100%" }}>
                {F.Words.agents(agents.length)}
              </T>
            </div>
          </div>
        </div>
        <V gap={5} align="stretch">
          {legend.map((item, i) => (
            <H key={i} gap={10} align="center">
              <div style={{ width: 10, height: 10, flex: "none", borderRadius: 5, background: item.color, boxShadow: "inset 0 0 0 1px rgba(27,27,30,0.15)" }} />
              <div style={{ minWidth: 0, flex: "0 1 auto" }}>
                <T size={14} fit={0.7}>
                  {item.name}
                </T>
              </div>
              <Spacer min={4} />
              <T size={14} mono color={C.secondary}>
                {F.percent(item.share)}
              </T>
            </H>
          ))}
        </V>
      </V>
    </Box>
  );
}

function CacheTile({ deck, rate }: { deck: Deck; rate: number }) {
  const saved = deck.cacheSavings;
  return (
    <Box dark padding={[22, 22, 22, 22]}>
      <T size={18} w={600} color={C.paper} lines={1}>
        {t("Cache hit")}
      </T>
      <Spacer />
      <T size={76} w={600} color={C.lime} tight={-76 * 0.04} fit={0.6}>
        {F.percent(rate)}
      </T>
      <Spacer />
      {saved !== null ? (
        <RichText
          text={t("Repeated context is read from the cache, saving about %@", F.mark(F.money(saved, deck.recap.currency)))}
          size={15}
          color="rgba(245,245,241,0.72)"
          emphasisWeight={700}
          highlights={false}
        />
      ) : null}
    </Box>
  );
}

function StreakTile({ deck }: { deck: Deck }) {
  const streak = deck.streak ?? { days: 0, isCurrent: false };
  const title = streak.isCurrent ? t("Current streak") : t("Longest streak");
  return (
    <Box padding={[22, 22, 22, 22]}>
      <T size={18} w={600} fit={0.7}>
        {title}
      </T>
      <Spacer />
      <H gap={4} align="last baseline" style={{ flex: "none" }}>
        <T size={76} w={600} tight={-76 * 0.04} lines={1}>
          {String(streak.days)}
        </T>
        <T size={28} lines={1}>
          {F.Words.daysUnit(streak.days)}
        </T>
        <div style={{ paddingLeft: 8, alignSelf: "flex-end" }}>
          <Flame width={30} height={36} />
        </div>
      </H>
      <Spacer />
      {/* Only a running period has a streak that is still going. */}
      {streak.isCurrent ? (
        <T size={15} color={C.secondary} lines={2}>
          {t("Still going · longest %@", F.Words.days(deck.recap.longestStreak))}
        </T>
      ) : null}
    </Box>
  );
}

// MARK: Busiest day and projects

/** Seven days with the busiest one among them, for the little bars: the week it falls in,
 *  trimmed to the days the recap has. */
function busiestWindow(deck: Deck, day: Day): Day[] {
  const days = deck.recap.days;
  const index = days.findIndex((d) => d.date === day.date);
  if (index < 0) return [day];
  const start = Math.max(0, Math.min(index - 5, days.length - 7));
  return days.slice(start, Math.min(days.length, start + 7));
}

function BusiestCard({ deck, day }: { deck: Deck; day: Day }) {
  const figure = F.tokens(day.tokens);
  const neighbours = busiestWindow(deck, day);
  const maximum = Math.max(1, ...neighbours.map((d) => d.tokens));
  return (
    <Box padding={[24, 26, 24, 26]}>
      <V gap={8} align="stretch" style={{ flex: "1 1 0" }}>
        <H align="baseline">
          <div style={{ minWidth: 0, flex: "0 1 auto" }}>
            <T size={18} w={600} fit={0.7}>
              {t("Busiest day")}
            </T>
          </div>
          <Spacer min={8} />
          <div style={{ minWidth: 0, flex: "0 1 auto" }}>
            <T size={14} color={C.secondary} fit={0.7}>
              {F.dayWithWeekday(day.date)}
            </T>
          </div>
        </H>
        <H gap={10} align="last baseline">
          <FigureText number={figure.number} unit={figure.unit} numberSize={46} unitSize={24} numberWeight={600} unitWeight={400} tracking={-0.03} unitGap={3} />
          {day.cost !== undefined ? (
            <T size={20} color={C.secondary} lines={1}>
              {"≈ " + F.money(day.cost, deck.recap.currency)}
            </T>
          ) : null}
        </H>
        <H gap={7} align="flex-end" style={{ flex: "1 1 0", minHeight: 0 }}>
          {neighbours.map((item) => (
            <div
              key={item.date}
              style={{
                ...grow,
                height: Math.max(8, (item.tokens / maximum) * 72),
                flex: "1 1 0",
                borderRadius: 6,
                background: item.date === day.date ? C.lime : "#E9E9E3",
              }}
            />
          ))}
        </H>
      </V>
    </Box>
  );
}

function ProjectsCard({ deck }: { deck: Deck }) {
  const projects = deck.recap.projects.slice(0, 3);
  return (
    <Box padding={[24, 26, 24, 26]}>
      <V gap={12} align="stretch">
        <T size={18} w={600} lines={1}>
          {t("Top projects")}
        </T>
        {projects.map((project, index) => (
          <H key={index} gap={12} align="baseline">
            <T size={14} mono color={C.faint}>{`${index + 1}`}</T>
            <div style={{ minWidth: 0, flex: "0 1 auto" }}>
              <T size={18} w={500} fit={0.7}>
                {deck.projectName(index, project.name)}
              </T>
            </div>
            <Spacer min={4} />
            <div style={{ minWidth: 0, flex: "0 1 auto" }}>
              <T size={15} color={C.secondary} fit={0.7}>
                {F.Words.sessions(project.sessions)}
              </T>
            </div>
            <div style={{ width: 48, flex: "none" }}>
              <T size={15} mono align="right">
                {F.percent(project.share)}
              </T>
            </div>
          </H>
        ))}
      </V>
    </Box>
  );
}

// MARK: Footer

function Footer({ deck }: { deck: Deck }) {
  return (
    <H align="flex-end" style={{ flex: "none" }}>
      <V gap={2} align="flex-start">
        {deck.provenance.map((line) => (
          <T key={line} size={14} color={C.tertiary} lines={2}>
            {line}
          </T>
        ))}
      </V>
      <Spacer min={20} />
      <T size={14} mono color={C.tertiary} track={1.2} lines={1}>
        PULSE · RECAP
      </T>
    </H>
  );
}

/** The sentence under the clock, in the same terms as the persona on its corner. Upstream always
 *  quoted the 21:00-05:00 share, which reads as nonsense under "Day shift": the reader takes the
 *  line for the busiest stretch. A night owl keeps it (that share is what made them one); early
 *  birds and day shifts get their own band's share; all-day gets the busiest four hours running. */
function rhythmLine(recap: Report): string | null {
  const hours = recap.hours;
  if (!hours || hours.length !== 24) {
    return recap.lateShare !== undefined ? t("%@ of it came between 9 PM and 5 AM.", F.percent(recap.lateShare)) : null;
  }
  const total = hours.reduce((a, b) => a + b, 0);
  if (total <= 0) return null;
  const share = (from: number, length: number) => {
    let sum = 0;
    for (let i = 0; i < length; i++) sum += hours[(from + i) % 24];
    return sum / total;
  };
  const between = (from: number, length: number) =>
    t("%1$@ of it came between %2$@ and %3$@.", F.percent(share(from, length)), F.hourLabel(from), F.hourLabel((from + length) % 24));
  switch (recap.persona) {
    case "nightOwl":
      return t("%@ of it came between 9 PM and 5 AM.", F.percent(share(21, 8)));
    case "earlyBird":
      return between(5, 5);
    case "dayShift":
      return between(10, 8);
    default: {
      let best = 0;
      for (let h = 1; h < 24; h++) if (share(h, 4) > share(best, 4)) best = h;
      return between(best, 4);
    }
  }
}

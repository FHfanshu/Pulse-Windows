// The hour you work hardest, a 24-hour dial, when the day starts and ends, the day in four parts,
// every hour as a bar, and the days of the week (upstream RecapTimetableView).
import { t } from "../../shared/i18n";
import { Dial } from "../charts";
import { Deck, personaTitle } from "../deck";
import * as F from "../format";
import { C, FigureText, H, Hairline, Pill, SectionHead, Spacer, StoryPage, T, Tile, V } from "../kit";
import { DayGlyph, DayKind } from "../marks";
import { bandEnd, rhythmBand } from "../rhythm";

export function TimetableCard({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const hours = recap.hours ?? [];
  const peak = recap.peakHour ?? 0;
  const figure = F.hour(peak);
  return (
    <StoryPage page={deck.page("timetable")}>
      <H align="flex-end" style={{ paddingTop: 44, flex: "none" }}>
        <FigureText number={figure.number} unit={figure.unit} numberSize={230} unitSize={100} unitGap={10} trimmed />
        <Spacer min={12} />
        {recap.persona ? (
          <div style={{ paddingBottom: 12 }}>
            <Pill fill={C.ink} horizontal={16} vertical={8}>
              <T size={20} color={C.lime} lines={1}>
                {personaTitle(recap.persona)}
              </T>
            </Pill>
          </div>
        ) : null}
      </H>
      <div style={{ paddingTop: 26 }}>
        <T size={42} w={800} fit={0.6}>
          {t("is your busiest hour of the day.")}
        </T>
      </div>
      <Spacer min={20} />
      <H gap={34} align="center" style={{ flex: "none" }}>
        <Dial hours={hours} peak={peak} size={460} />
        <Stats deck={deck} />
      </H>
      <Spacer min={20} />
      <Quarters deck={deck} />
      <Spacer min={20} />
      <HourBars hours={hours} peak={peak} />
      <Spacer min={20} />
      <Weekdays deck={deck} />
    </StoryPage>
  );
}

// MARK: Start, end and the late hours

function Stats({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const first = recap.insights.firstHour;
  const band = rhythmBand(recap);
  return (
    <V align="stretch" style={{ flex: "1 1 0", minWidth: 0 }}>
      {first !== undefined ? <Stat label={t("Earliest start")} value={F.hourLabel(first)} note={t("The earliest hour after 5 AM with any work")} /> : null}
      {recap.latestMinute !== undefined ? (
        <Stat
          label={t("Finishes latest")}
          value={F.clockTime(recap.latestMinute)}
          note={recap.lateNights > 0 ? F.Words.nightsPastMidnight(recap.lateNights) : null}
        />
      ) : null}
      {band ? <Stat label={t("%@ to %@", F.hourLabel(band.from), F.hourLabel(bandEnd(band)))} value={F.percent(band.share)} note={t("of your usage fell in these hours")} /> : null}
    </V>
  );
}

function Stat({ label, value, note }: { label: string; value: string; note: string | null }) {
  return (
    <V gap={6} align="stretch" style={{ padding: "18px 0", position: "relative" }}>
      <Hairline style={{ position: "absolute", left: 0, right: 0, top: 0 }} />
      <T size={18} color={C.secondary} fit={0.6}>
        {label}
      </T>
      <T size={52} w={700} tight={-52 * 0.03} fit={0.5}>
        {value}
      </T>
      {note ? (
        <T size={16} color={C.tertiary} lines={2}>
          {note}
        </T>
      ) : null}
    </V>
  );
}

// MARK: The day in four parts

function Quarters({ deck }: { deck: Deck }) {
  const insights = deck.recap.insights;
  const quarters = insights.quarters;
  const lead = insights.leadingQuarter;
  if (!quarters || lead === undefined) return null;
  const names = [t("Small hours"), t("Morning"), t("Afternoon"), t("Evening")];
  const kinds: DayKind[] = ["night", "sunrise", "day", "sunset"];
  return (
    <V gap={14} align="stretch" style={{ flex: "none" }}>
      <SectionHead title={t("Four parts of the day")} note={t("Busiest: %@", names[lead])} />
      <H gap={14} equal style={{ height: 194 }}>
        {quarters.map((quarter, index) => (
          <QuarterTile key={quarter.from} from={quarter.from} share={quarter.share} name={names[index]} kind={kinds[index]} lead={index === lead} />
        ))}
      </H>
    </V>
  );
}

function QuarterTile({ from, share, name, kind, lead }: { from: number; share: number; name: string; kind: DayKind; lead: boolean }) {
  const figure = F.percentFigure(share);
  return (
    <Tile tone={lead ? "ink" : "white"} padding={[20, 20, 18, 20]}>
      <H align="center">
        <DayGlyph kind={kind} side={30} color={lead ? C.lime : C.ink} />
        <Spacer />
        <T size={15} mono color={lead ? "rgba(245,245,241,0.55)" : C.tertiary}>
          {`${F.pad2(from)}–${F.pad2(from + 6)}`}
        </T>
      </H>
      <Spacer />
      <div style={{ paddingBottom: 8 }}>
        <T size={20} color={lead ? "rgba(245,245,241,0.75)" : C.ink} fit={0.6}>
          {name}
        </T>
      </div>
      <div style={{ paddingBottom: 12 }}>
        <FigureText
          number={figure.number}
          unit={figure.unit}
          numberSize={54}
          unitSize={26}
          numberWeight={700}
          unitWeight={700}
          color={lead ? C.lime : C.ink}
          tracking={-0.03}
          unitGap={2}
        />
      </div>
      <div style={{ height: 8, borderRadius: 4, background: lead ? "#34343A" : C.heatZero, position: "relative", flex: "none" }}>
        <div
          style={{ position: "absolute", left: 0, top: 0, bottom: 0, width: `max(4px, ${share * 100}%)`, borderRadius: 4, background: lead ? C.lime : C.ink }}
        />
      </div>
    </Tile>
  );
}

// MARK: Every hour

function HourBars({ hours, peak }: { hours: number[]; peak: number }) {
  const top = Math.max(1, ...hours);
  const figure = F.tokens(hours[peak] ?? 0);
  return (
    <V gap={10} align="stretch" style={{ flex: "none" }}>
      <SectionHead title={t("24 hours")} note={t("Peak %@", F.hourLabel(peak)) + " · " + F.figureText(figure)} />
      <H gap={5} align="flex-end" style={{ height: 124 }}>
        {hours.slice(0, 24).map((value, hour) => (
          <div
            key={hour}
            style={{
              flex: "1 1 0",
              minWidth: 0,
              height: Math.max(4, (value / top) * 120),
              borderRadius: 4,
              background: hour === peak ? C.ink : value > 0 ? C.lime : C.restBar,
            }}
          />
        ))}
      </H>
      <H gap={5}>
        {Array.from({ length: 24 }, (_, hour) => (
          <div key={hour} style={{ flex: "1 1 0", minWidth: 0 }}>
            <T size={14} w={hour === peak ? 600 : 400} mono color={hour === peak ? C.ink : C.faint} fit={0.6} align="center">
              {hour % 3 === 0 || hour === peak ? F.pad2(hour) : " "}
            </T>
          </div>
        ))}
      </H>
    </V>
  );
}

// MARK: The week

function Weekdays({ deck }: { deck: Deck }) {
  const insights = deck.recap.insights;
  const tokens = insights.weekdayTokens;
  const top = Math.max(0, ...tokens.map((v) => v ?? 0));
  const best = insights.busiestWeekday;
  if (top <= 0 || best === undefined) return null;
  const names = F.weekdayNames();
  return (
    <V gap={14} align="stretch" style={{ flex: "none" }}>
      <Hairline strong style={{ marginBottom: 8 }} />
      <SectionHead title={t("Seven days of the week")} note={t("Busiest: %@", names[best])} />
      <H gap={14} equal>
        {names.map((name, index) => (
          <Column key={index} name={name} tokens={tokens[index]} top={top} isBest={index === best} />
        ))}
      </H>
    </V>
  );
}

function Column({ name, tokens, top, isBest }: { name: string; tokens: number | null; top: number; isBest: boolean }) {
  const used = (tokens ?? 0) > 0;
  return (
    <V gap={10} align="stretch" style={{ flex: "1 1 0", minWidth: 0 }}>
      <T size={15} mono color={isBest ? C.ink : C.tertiary} fit={0.6} align="center">
        {used ? F.figureText(F.tokens(tokens ?? 0)) : "—"}
      </T>
      <div style={{ height: 120, display: "flex", alignItems: "flex-end" }}>
        {used ? (
          <div style={{ width: "100%", height: Math.max(6, ((tokens ?? 0) / top) * 120), borderRadius: 8, background: isBest ? C.ink : C.lime }} />
        ) : (
          <div style={{ width: "100%", height: 24, borderRadius: 8, border: "1.5px dashed #CFCFC8", boxSizing: "border-box" }} />
        )}
      </div>
      <T size={20} w={isBest ? 700 : 400} color={isBest ? C.ink : C.secondary} fit={0.6} align="center">
        {name}
      </T>
    </V>
  );
}

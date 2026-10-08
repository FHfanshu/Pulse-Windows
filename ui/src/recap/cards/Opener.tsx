// The month number or the year, a line, a lineup of the agents that did the work as tiles, and a
// roster with each agent's days drawn as a strip (upstream RecapOpenerView).
import { t } from "../../shared/i18n";
import { AgentStrip } from "../charts";
import type { Deck } from "../deck";
import * as F from "../format";
import { C, FigureText, H, Hairline, SectionHead, Spacer, StoryPage, T, Tile, V, toneCaption } from "../kit";
import { Glyph, MarkTile } from "../marks";
import type { AgentShare } from "../types";

export function OpenerCard({ deck }: { deck: Deck }) {
  return (
    <StoryPage page={deck.page("opener")}>
      <div style={{ paddingTop: 36 }}>
        <Big deck={deck} />
      </div>
      <div style={{ paddingTop: 34 }}>
        <Headline deck={deck} />
      </div>
      <Spacer min={24} />
      <Lineup deck={deck} />
      <Spacer min={24} />
      <Roster deck={deck} />
      <Spacer min={24} />
      <Footer />
    </StoryPage>
  );
}

// MARK: The number and the line

function Big({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const year = Number(recap.period.slice(0, 4));
  const lime = <div style={{ width: 60, height: 60, borderRadius: 8, background: C.lime, flex: "none" }} />;
  if (deck.isYear) {
    return (
      <H gap={24} align="flex-end" style={{ flex: "none" }}>
        <FigureText number={String(year)} numberSize={300} unitSize={0} tracking={-0.07} trimmed />
        <div style={{ paddingBottom: 4 }}>{lime}</div>
        <Spacer />
      </H>
    );
  }
  const month = Number(recap.period.slice(5, 7));
  return (
    <H gap={24} align="flex-end" style={{ flex: "none" }}>
      <FigureText number={F.pad2(month)} numberSize={340} unitSize={0} tracking={-0.07} trimmed />
      <V gap={10} align="flex-start" style={{ paddingBottom: 4 }}>
        {lime}
        <T size={20} mono color={C.grey55}>
          {String(year)}
        </T>
      </V>
      <Spacer />
    </H>
  );
}

function Headline({ deck }: { deck: Deck }) {
  const count = deck.recap.agents.length;
  // The count is the lineup below it, so the line says how many rather than a negative that read
  // oddly in Chinese. One tool gets its own sentence: no plural to agree.
  const second = count === 1 ? t("One AI tool coded with you.") : t("%@ AI tools coded with you.", count);
  return (
    <V align="stretch" style={{ flex: "none" }}>
      <T size={66} w={900} track={-0.66} fit={0.5}>
        {t("In %@,", deck.periodName)}
      </T>
      <div style={{ paddingTop: 6 }}>
        <T size={66} w={900} track={-0.66} fit={0.5}>
          {second}
        </T>
      </div>
    </V>
  );
}

// MARK: The lineup

type Cell = { kind: "agent"; rank: number; agent: AgentShare } | { kind: "summary" };

function Lineup({ deck }: { deck: Deck }) {
  const agents = deck.recap.agents.slice(0, 4);
  const cells: Cell[] = agents.map((agent, index): Cell => ({ kind: "agent", rank: index + 1, agent })).slice(1);
  cells.push({ kind: "summary" });
  // Up to two cells each take a row; three or four sit in pairs, and an odd one out spans the row.
  const rows: Cell[][] = [];
  if (cells.length <= 2) cells.forEach((cell) => rows.push([cell]));
  else for (let i = 0; i < cells.length; i += 2) rows.push(cells.slice(i, i + 2));
  return (
    <H gap={14} style={{ flex: "none" }}>
      {agents[0] ? (
        <div style={{ width: 430, height: 430, flex: "none", display: "flex" }}>
          <LeadTile agent={agents[0]} />
        </div>
      ) : null}
      <V gap={14} style={{ height: 430, flex: "1 1 0" }}>
        {rows.map((row, index) => (
          <H key={index} gap={14} equal style={{ flex: "1 1 0", minHeight: 0 }}>
            {row.map((cell, i) => (
              <CellTile key={i} cell={cell} deck={deck} />
            ))}
          </H>
        ))}
      </V>
    </H>
  );
}

function LeadTile({ agent }: { agent: AgentShare }) {
  const share = F.percentFigure(agent.share);
  return (
    <Tile tone="ink" radius={44} padding={36}>
      <H align="flex-start">
        <Glyph resource={agent.icon} name={agent.name} side={120} color={C.lime} />
        <Spacer />
        <T size={18} mono color="rgba(245,245,241,0.55)">
          01
        </T>
      </H>
      <Spacer />
      <V gap={8} align="stretch">
        <T size={26} color="rgba(245,245,241,0.7)" fit={0.6}>
          {agent.name}
        </T>
        <FigureText number={share.number} unit={share.unit} numberSize={110} unitSize={50} color={C.lime} unitGap={4} />
        <T size={20} color="rgba(245,245,241,0.6)" fit={0.6}>
          {t("Used %@ · top tool", F.Words.days(agent.activeDays))}
        </T>
      </V>
    </Tile>
  );
}

function CellTile({ cell, deck }: { cell: Cell; deck: Deck }) {
  if (cell.kind === "agent") {
    const { agent, rank } = cell;
    return (
      <Tile radius={28} padding={22}>
        <H align="flex-start">
          <Glyph resource={agent.icon} name={agent.name} side={56} color={C.ink} />
          <Spacer />
          <T size={15} mono color={C.faint}>
            {F.pad2(rank)}
          </T>
        </H>
        <Spacer />
        <T size={19} color={C.secondary} fit={0.6}>
          {agent.name}
        </T>
        <T size={44} w={700} tight={-44 * 0.03} lines={1}>
          {F.percent(agent.share)}
        </T>
      </Tile>
    );
  }
  const recap = deck.recap;
  return (
    <Tile tone="lime" radius={28} padding={22}>
      <V gap={4} align="stretch" style={{ flex: "1 1 0" }}>
        <T size={19} color={toneCaption.lime}>
          {t("In all")}
        </T>
        <Spacer />
        <FigureText number={String(recap.agents.length)} unit={F.Words.toolsUnit(recap.agents.length)} numberSize={58} unitSize={24} numberWeight={800} unitWeight={700} unitGap={6} />
        <T size={18} color={toneCaption.lime} fit={0.6}>
          {F.Words.activeDays(recap.activeDays)}
        </T>
      </V>
    </Tile>
  );
}

// MARK: The roster

function Roster({ deck }: { deck: Deck }) {
  const recap = deck.recap;
  const agents = recap.agents.slice(0, 5);
  const note = deck.isYear ? t("Which months · days used · token share") : t("Which days · days used · token share");
  return (
    <V align="stretch" style={{ flex: "none" }}>
      <div style={{ paddingBottom: 12 }}>
        <SectionHead title={t("Worked beside you")} note={note} />
      </div>
      {agents.map((agent, index) => {
        const first = index === 0;
        return (
          <V key={agent.agent} align="stretch">
            <Hairline />
            <H gap={20} align="center" style={{ padding: "20px 0" }}>
              <MarkTile resource={agent.icon} name={agent.name} side={64} lead={first} />
              <V gap={10} align="flex-start" style={{ flex: "0 1 auto", minWidth: 0 }}>
                <div style={{ maxWidth: 400 }}>
                  <T size={first ? 34 : 30} w={first ? 700 : 500} tight={-0.6} fit={0.5}>
                    {agent.name}
                  </T>
                </div>
                {/* A strip needs the dates; an agent without them has none to draw, and is left without one. */}
                {agent.activeDates.length ? <AgentStrip marks={recap.insights.agentMarks[recap.agents.indexOf(agent)] ?? []} /> : null}
              </V>
              <Spacer />
              <div style={{ width: 110, flex: "none" }}>
                <T size={20} color={C.grey55} fit={0.7} align="right">
                  {F.Words.days(agent.activeDays)}
                </T>
              </div>
              <div style={{ width: 110, flex: "none", background: first ? C.lime : undefined, padding: first ? "2px 8px" : 0, boxSizing: "border-box", textAlign: "right" }}>
                <T size={20} mono color={first ? C.ink : C.tertiary} align="right">
                  {F.percent(agent.share)}
                </T>
              </div>
            </H>
          </V>
        );
      })}
    </V>
  );
}

function Footer() {
  return (
    <V align="stretch" style={{ flex: "none" }}>
      <Hairline strong />
      <H align="center" style={{ paddingTop: 22 }}>
        <div style={{ flex: "0 1 auto", minWidth: 0 }}>
          <T size={22} color={C.grey55} lines={2}>
            {t("Turn the page to see what you made together")}
          </T>
        </div>
        <Spacer min={16} />
        <svg width={32} height={32} viewBox="0 0 24 24" fill="none" stroke={C.ink} strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" style={{ flex: "none" }}>
          <path d="M4 12h16M13.5 5.5 20 12l-6.5 6.5" />
        </svg>
      </H>
    </V>
  );
}

// Ported from upstream Settings/TokenSpendView.swift: what every coding agent on this PC has cost, added
// up. The per-provider card answers "how heavily am I using this"; this answers "where did the work go".
import { useEffect, useRef, useState } from "react";
import { ProviderIcon } from "../../panel/Icon";
import { t } from "../../shared/i18n";
import {
  agentName, exactTokens, hourText, intlLocale, money, moneyExact, reportsNoCache, shortDate, tokenCount, tokensText,
  type SpendAgent, type SpendLists, type SpendOverview, type SpendSummary,
} from "../../shared/spend";
import { Button, Select } from "../controls";
import { Group, Row } from "../Group";
import { DayTable, summaryDayRow } from "./DayTable";
import { ModelSpendDetail } from "./ModelDetail";
import { ProjectsGroup, SessionsGroup } from "./ProjectsSessions";
import {
  CostText, HourProfile, ShareBar, SpendBarChart, SpendCaption, TokenKindBreakdown,
} from "./SpendCharts";
import { TokenActivitySection, type ActivityView } from "./TokenActivity";

export type SpendSpan = "today" | "week" | "month" | "quarter" | "all";

/** Nothing for "all": it counts everything the transcripts go back to. */
export const spanDays: Record<SpendSpan, number | null> = { today: 1, week: 7, month: 30, quarter: 90, all: null };

const spanTitle = (span: SpendSpan) =>
  ({ today: t("Today"), week: t("Last %@ days", 7), month: t("Last %@ days", 30), quarter: t("Last %@ days", 90), all: t("All time") })[span];

/** Enough to see where the work goes without turning the pane into a table. */
const MODEL_LIMIT = 8;

export interface SpendViewProps {
  overview: SpendOverview | null;
  /** The figures on screen are for another request than the one now asked. */
  pending: boolean;
  loading: boolean;
  span: SpendSpan;
  onSpan: (s: SpendSpan) => void;
  agent: SpendAgent | null;
  onAgent: (a: SpendAgent | null) => void;
  model: string | null;
  onModel: (m: string | null) => void;
  activityView: ActivityView;
  onActivityView: (v: ActivityView) => void;
  onRescan: () => void;
}

export function TokenSpendView(p: SpendViewProps) {
  const { overview, span, agent, model } = p;
  const summary = overview?.summary ?? null;
  const focused = agent ? overview?.agent ?? null : null;
  const modelsAnchor = useRef<HTMLDivElement>(null);

  // Entering a model starts at the top of the page; coming back lands on the model list.
  const previousModel = useRef<string | null>(null);
  useEffect(() => {
    if (model && !previousModel.current) document.querySelector(".content")?.scrollTo({ top: 0 });
    if (!model && previousModel.current) modelsAnchor.current?.scrollIntoView({ block: "start" });
    previousModel.current = model;
  }, [model]);

  let body;
  if (p.pending && !overview) {
    body = <Empty loading />;
  } else if (p.pending) {
    body = <div className="spinner-row"><span className="spinner" role="progressbar" /></div>;
  } else if (model) {
    body = overview?.model && overview.model.tokens > 0 ? <ModelSpendDetail model={overview.model} /> : <NothingIn title={model} />;
  } else if (agent) {
    body = !focused || isEmpty(focused) ? <NothingIn title={agentName[agent]} /> : (
      <Overview summary={focused} p={p} focus={agent} anchor={modelsAnchor} activity={overview!.activity} lists={overview!.lists} />
    );
  } else if (!summary || isEmpty(summary)) {
    body = <Empty loading={p.loading} />;
  } else {
    body = <Overview summary={summary} p={p} focus={null} anchor={modelsAnchor} activity={overview!.activity} lists={overview!.lists} />;
  }

  return (
    <>
      {model ? (
        <BackButton label={agent ? agentName[agent] : t("All models")} name={model} onBack={() => p.onModel(null)} />
      ) : agent ? (
        <BackButton label={t("All agents")} name={agentName[agent]} icon={<ProviderIcon provider={agent} size={13} />} onBack={() => p.onAgent(null)} />
      ) : null}

      <Group title={t("Span")}>
        <Row title={t("Counting")} subtitle={t("Read from local records and exports, priced at models.dev's published rates.")}>
          <Select
            label={t("Counting")}
            value={span}
            options={(Object.keys(spanDays) as SpendSpan[]).map((s) => ({ value: s, label: spanTitle(s) }))}
            onChange={p.onSpan}
          />
          <Button disabled={p.loading} onClick={p.onRescan}>{t("Rescan")}</Button>
        </Row>
      </Group>

      {/* A partial read is stated, not hidden: a source can hold history this PC cannot decode (a compressed
          transcript) while other records read fine; without this line the readable subset would look like the whole. */}
      {overview?.hasReadLimitations && (
        <div className="footnote">{t("Some compressed records could not be read.")}</div>
      )}

      {body}

      {/* Only for the combined page: a source with no records is a fact about the whole page; under one agent
          or one model it would read as a per-agent absence the same list already shows. */}
      {!model && !agent && !p.pending && overview && overview.noRecords.length > 0 && (
        <Group title={t("No usage data read")}>
          <Row title={overview.noRecords.map((a) => agentName[a]).join(" · ")} />
        </Group>
      )}
    </>
  );
}

const isEmpty = (s: SpendSummary) => s.tokens === 0 && s.cost === 0;

function Chevron({ dir }: { dir: "left" | "right" }) {
  return (
    <svg className="chevron" viewBox="0 0 8 12" width="8" height="12" aria-hidden>
      <path d={dir === "left" ? "M6.5 1.5 2 6l4.5 4.5" : "M1.5 1.5 6 6l-4.5 4.5"} fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** The way back, and what is being looked at: this pane has no navigation stack of its own. */
function BackButton({ label, name, icon, onBack }: { label: string; name: string; icon?: React.ReactNode; onBack: () => void }) {
  return (
    <button type="button" className="back" onClick={onBack}>
      <Chevron dir="left" />
      <span>{label}</span>
      <span className="secondary">·</span>
      {icon}
      <strong>{name}</strong>
    </button>
  );
}

function NothingIn({ title }: { title: string }) {
  return (
    <Group title={title}>
      <Row title={t("Nothing in this span")} subtitle={t("Try a longer span, or rescan.")} />
    </Group>
  );
}

function Empty({ loading }: { loading: boolean }) {
  return (
    <Group title={t("Total")}>
      <Row
        title={loading ? t("Reading…") : t("Nothing yet")}
        subtitle={loading ? t("Going through local records and exports.") : t("No local records or exports have been found yet.")}
      />
    </Group>
  );
}

// MARK: - The combined page and one agent's

/** Whether the quarter-hour buckets account for every token in the span: otherwise the profile is a partial day. */
const hoursComplete = (s: SpendSummary) => Object.values(s.hours).reduce((a, b) => a + b, 0) === s.tokens;

function peakHour(s: SpendSummary): number | null {
  let best: number | null = null;
  for (let h = 0; h < 24; h++) {
    const v = s.hours[String(h)] ?? 0;
    if (v > 0 && (best === null || v > (s.hours[String(best)] ?? 0))) best = h;
  }
  return best;
}

function Overview({ summary, p, focus, anchor, activity, lists }: {
  summary: SpendSummary;
  p: SpendViewProps;
  focus: SpendAgent | null;
  anchor: React.RefObject<HTMLDivElement>;
  activity: SpendOverview["activity"];
  lists: SpendLists | null;
}) {
  const today = p.span === "today";
  const complete = hoursComplete(summary);
  const noPrice = summary.tokens > 0 && summary.unpricedTokens === summary.tokens;
  const partialEstimate = !noPrice && (summary.unpricedTokens > 0 || summary.hasPartialCounts);
  const activeDays = summary.days.filter((d) => d.tokens > 0).length;
  const busiest = summary.days.reduce<SpendSummary["days"][number] | null>((best, d) => (!best || d.tokens > best.tokens ? d : best), null);
  const peak = peakHour(summary);
  const cacheUnreported = reportsNoCache(summary.tally);
  const projects = lists?.projects ?? [];
  const sessions = lists?.sessions ?? [];

  return (
    <>
      <Group title={focus === null ? t("Total") : t("This agent")}>
        <div className="spend-body">
          <div className="figure-line">
            {noPrice ? (
              <span className="figure dim">{t("Estimate unavailable")}</span>
            ) : (
              <>
                <span className="figure" title={moneyExact(summary.cost)}>{money(summary.cost)}</span>
                {partialEstimate && <span className="secondary small">{t("Partial estimate")}</span>}
              </>
            )}
            <span className="secondary num" title={exactTokens(summary.tokens)}>{tokensText(summary.tokens)}</span>
          </div>

          {today ? (
            complete ? <HourProfile hours={summary.hours} height={78} /> : <div className="secondary small">{t("Hourly detail unavailable.")}</div>
          ) : (
            summary.days.length > 1 && <SpendBarChart bars={summary.days.map((d) => ({ date: d.date, tokens: d.tokens }))} />
          )}

          <div className="captions">
            {today ? (
              complete && (
                <>
                  <SpendCaption label={t("Active hours")} value={t("%@ of %@", Object.values(summary.hours).filter((v) => v > 0).length, 24)} />
                  {peak !== null && (
                    <SpendCaption label={t("Busiest hour")} value={`${hourText(peak)} · ${tokenCount(summary.hours[String(peak)] ?? 0)}`} />
                  )}
                </>
              )
            ) : (
              <>
                <SpendCaption label={t("Active days")} value={t("%@ of %@", activeDays, Math.max(summary.days.length, 1))} />
                {busiest && busiest.tokens > 0 && (
                  <SpendCaption label={t("Busiest")} value={`${shortDate(busiest.date)} · ${tokenCount(busiest.tokens)}`} title={exactTokens(busiest.tokens)} />
                )}
              </>
            )}
            {focus === null && summary.agents.length > 1 && <SpendCaption label={t("Agents")} value={String(summary.agents.length)} />}
          </div>
        </div>
      </Group>

      {activity.weeks.length > 0 && <TokenActivitySection activity={activity} view={p.activityView} onView={p.onActivityView} />}

      <Group title={t("By kind")}>
        {summary.hasTokenBreakdown ? (
          <TokenKindBreakdown tally={summary.tally} unclassified={summary.unclassifiedTokens} readsUnreported={cacheUnreported} />
        ) : (
          <Row title={t("Token breakdown unavailable.")} />
        )}
      </Group>

      <Group title={t("Pattern")}>
        <div className="spend-body">
          <div className="captions">
            <SpendCaption label={t("Current streak")} value={t("%@ days", summary.currentStreak)} />
            <SpendCaption label={t("Longest streak")} value={t("%@ days", summary.longestStreak)} />
            {complete && peak !== null && <SpendCaption label={t("Peak hour")} value={hourText(peak)} />}
            {summary.models[0] && <SpendCaption label={t("Most tokens")} value={summary.models[0].name} />}
          </div>
        </div>
      </Group>

      {!today && (
        <Group title={t("By hour")}>
          {complete ? <div className="spend-body"><HourProfile hours={summary.hours} /></div> : <Row title={t("Hourly detail unavailable.")} />}
        </Group>
      )}

      {!today && (
        <DayTable
          days={summary.days}
          cacheUnreported={cacheUnreported}
          unclassifiedTokens={summary.unclassifiedTokens}
          makeRow={summaryDayRow}
          zeroAsDash
        />
      )}

      {summary.months.length > 1 && (
        <Group title={t("By month")}>
          {[...summary.months].reverse().map((month) => (
            <Row
              key={month.date}
              title={new Intl.DateTimeFormat(intlLocale(), { year: "numeric", month: "long" }).format(new Date(month.date))}
              subtitle={tokensText(month.tokens)}
            >
              <CostText cost={month.tokens > 0 && month.unpricedTokens === month.tokens ? null : month.cost} unpriced={month.unpricedTokens} />
            </Row>
          ))}
        </Group>
      )}

      {focus === null && <Agents summary={summary} onAgent={p.onAgent} />}

      {summary.models.length > 0 && (
        <div ref={anchor}>
          <Models summary={summary} onModel={p.onModel} />
        </div>
      )}

      {projects.length > 0 && <ProjectsGroup projects={projects} total={summary.tokens} />}
      {sessions.length > 0 && <SessionsGroup sessions={sessions} />}

      <Footnote summary={summary} lists={lists} />
    </>
  );
}

/** One row an agent, and the whole row opens it: a target the width of a chevron is one most people miss. */
function Agents({ summary, onAgent }: { summary: SpendSummary; onAgent: (a: SpendAgent) => void }) {
  return (
    <Group title={t("By agent")}>
      {summary.agents.map((agent) => (
        <button key={agent.agent} type="button" className="drill" onClick={() => onAgent(agent.agent)}>
          <Row title={agentName[agent.agent]} subtitle={tokensText(agent.tokens)} icon={<ProviderIcon provider={agent.agent} size={18} />}>
            <ShareBar share={summary.tokens > 0 ? agent.tokens / summary.tokens : 0} />
            <span className="cost-slot">
              <CostText cost={agent.tokens > 0 && agent.unpricedTokens === agent.tokens ? null : agent.cost} unpriced={agent.unpricedTokens} />
            </span>
            <Chevron dir="right" />
          </Row>
        </button>
      ))}
    </Group>
  );
}

function Models({ summary, onModel }: { summary: SpendSummary; onModel: (m: string) => void }) {
  const [showAll, setShowAll] = useState(false);
  const shown = showAll ? summary.models : summary.models.slice(0, MODEL_LIMIT);
  return (
    <Group title={t("By model")}>
      {shown.map((model) => (
        <button key={model.name} type="button" className="drill" title={t("Show this model's details")} onClick={() => onModel(model.name)}>
          <Row title={model.name} subtitle={model.agents.map((a) => agentName[a]).join(" · ")}>
            <ShareBar share={model.share} />
            <span className="value-text nowrap num tokens-slot" title={exactTokens(model.tokens)}>{tokensText(model.tokens)}</span>
            <Chevron dir="right" />
          </Row>
        </button>
      ))}
      {summary.models.length > MODEL_LIMIT && (
        <button type="button" className="drill" onClick={() => setShowAll((v) => !v)}>
          <Row title={showAll ? t("Show fewer") : t("Show all models")}>
            <svg className="chevron" viewBox="0 0 12 8" width="12" height="8" aria-hidden>
              <path d={showAll ? "M1.5 6.5 6 2l4.5 4.5" : "M1.5 1.5 6 6l4.5-4.5"} fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </Row>
        </button>
      )}
    </Group>
  );
}

/** What the figures are not: the money is a translation of records at published rates, not a bill. */
function Footnote({ summary, lists }: { summary: SpendSummary; lists: SpendLists | null }) {
  const partial = (unpriced: number, tokens: number) => unpriced > 0 && unpriced < tokens;
  // Whether any row shows an amount with a `*` beside it: a partly priced agent, day, month, project or
  // session. A row that priced nothing shows an em dash instead, and a row that priced everything has no `*`.
  const hasPartialAmounts =
    summary.agents.some((a) => partial(a.unpricedTokens, a.tokens)) ||
    summary.days.some((d) => partial(d.unpricedTokens, d.tokens)) ||
    summary.months.some((m) => partial(m.unpricedTokens, m.tokens)) ||
    (lists?.projects.some((x) => partial(x.unpricedTokens, x.tokens)) ?? false) ||
    (lists?.sessions.some((x) => partial(x.unpricedTokens, x.tokens)) ?? false);
  return (
    <div className="footnote">
      <div>{t("Usage records, estimated at models.dev API rates—not an actual bill.")}</div>
      {summary.hasPartialCounts && <div>{t("Counts may be incomplete.")}</div>}
      {hasPartialAmounts && <div>{t("Excludes unpriced tokens.")}</div>}
      {summary.hasAggregateTiming && <div>{t("Some records use session report dates.")}</div>}
      {summary.unpricedModels.length > 0 && (
        <div title={summary.unpricedModels.join(", ")}>{t("%@ models without public pricing: tokens only.", summary.unpricedModels.length)}</div>
      )}
    </div>
  );
}


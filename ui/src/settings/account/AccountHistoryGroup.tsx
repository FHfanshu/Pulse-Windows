// Ported from upstream Settings/AccountHistoryGroup.swift, AccountHistoryModel.swift and
// Panel/AccountUsageCard.swift: what has actually been spent over time, as opposed to how much of the
// current limit is left. Mounted by the account pane for a first account; reads when it opens and again
// whenever the account's reading moves (local transcripts only, as upstream keys it).
import { useEffect, useRef, useState } from "react";
import { providerNames } from "../../panel/Icon";
import { locale, t } from "../../shared/i18n";
import { accountId } from "../../shared/model";
import type { AppSettings } from "../../shared/settings";
import { useUsage } from "../../shared/settings";
import { chartDate, money, tokensText } from "../../shared/spend";
import { Group, Row } from "../Group";
import { HoverPlot, type Sample } from "../spend/SpendCharts";
import { accountHistory, type AccountHistory, type HistoryFigure, type HistoryReadKind } from "./api";
import "./account.css";

/** Providers whose history this build reads (`pulse_core::history::provides_history`), for the spinner before the first answer. */
const readsHistory = new Set(["claudeCode", "codex"]);
/** Roughly a month, which is the span most of the figures cover. */
const SPAN = 31;

export interface AccountComponentProps {
  id: string;
  provider: string;
  settings: AppSettings;
}

export function AccountHistoryGroup({ id, provider, settings }: AccountComponentProps) {
  const primary = id === provider;
  const enabled = settings.enabledAccounts.includes(id);
  const { usages } = useUsage();
  // The reading's time too: the value estimate counts spend up to when the percentage was read, and a
  // ledger read before that is short.
  const observedAt = usages.find((u) => accountId(u.account) === id)?.observedAt ?? "";

  const [history, setHistory] = useState<{ key: string; value: AccountHistory } | null>(null);
  const [loading, setLoading] = useState(false);
  const request = useRef(0);
  const key = `${id}|${enabled}|${observedAt}`;

  useEffect(() => {
    if (!primary) return;
    const mine = ++request.current;
    setLoading(true);
    accountHistory(id)
      .then((value) => {
        if (request.current === mine) setHistory({ key, value });
      })
      .catch(() => {})
      .finally(() => {
        if (request.current === mine) setLoading(false);
      });
    return () => {
      request.current++;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, primary]);

  if (!primary) return null;
  // Nothing to say about a provider that keeps no history, before or after the first answer.
  const shown = history?.value.provider === provider ? history.value : null;
  if (shown?.read === "unsupported" || (!shown && !readsHistory.has(provider))) return null;

  const name = providerNames[provider] ?? provider;
  if (shown?.hasDays) return <AccountUsageCard provider={provider} history={shown} loading={loading} />;

  const local = readsHistory.has(provider);
  return (
    <Group title={t("Usage history")}>
      <Row
        title={loading ? (local ? t("Reading logs") : t("Asking %@", name)) : t("No history yet")}
        subtitle={loading ? undefined : emptyReason(name, shown?.read ?? null, local)}
      >
        {loading && <span className="spinner" role="progressbar" aria-label={t("Reading…")} />}
      </Row>
    </Group>
  );
}

/**
 * Both halves of the empty state have to name the **right** source: a history read from the provider's own
 * statistics has nothing to do with this PC. The two original sentences are claims about the account, and
 * neither is one Pulse can make until a read has actually answered; one that has not happened yet says nothing.
 */
function emptyReason(name: string, read: HistoryReadKind | null, local: boolean): string | undefined {
  switch (read) {
    case null:
    case "unsupported":
      return undefined;
    case "failed":
      return t("%@ didn't answer, so there is nothing to chart yet. Try again in a moment.", name);
    case "notConfigured":
      return t("Add a key above and Pulse can read this account's history.");
    case "notAsked":
      return t("This account is switched off, so Pulse hasn't asked for its history.");
    case "answered":
      return local
        ? t("Nothing has been logged on this PC yet, so there is no history to add up.")
        : t("This account hasn't used anything yet, so there is nothing to chart.");
  }
}

// MARK: - The card

function AccountUsageCard({ provider, history, loading }: { provider: string; history: AccountHistory; loading: boolean }) {
  const name = providerNames[provider] ?? provider;
  const withMoney = history.origin === "localTranscripts" || history.origin === "providerLogs";
  const rows = history.models;
  const showsCache = rows.some((r) => r.cacheRate !== null);
  const showsSpeed = rows.some((r) => r.speed !== null);
  const showsWait = rows.some((r) => r.firstToken !== null);
  const columns = 2 + (showsCache ? 1 : 0) + (showsSpeed ? 1 : 0) + (showsWait ? 1 : 0);

  return (
    <div className="history-stack">
      <section className="history-section">
        <div className="history-head">
          <h3 className="group-title">{t("Usage history")}</h3>
          {loading && (
            <>
              <span className="spinner" role="progressbar" aria-label={t("Reading…")} />
              <span className="secondary small">{t("Reading…")}</span>
            </>
          )}
        </div>
        <div className="group-card">
          <div className="history-figures">
            <Figure label={t("Today")} figure={history.today} withMoney={withMoney} currency={history.currency} />
            <Figure label={t("Last 31 days")} figure={history.last31Days} withMoney={withMoney} currency={history.currency} />
            <Figure label={t("Busiest day")} figure={history.busiestDay} withMoney={withMoney} currency={history.currency} />
            {/* "All time" is only true of a ledger that goes back: a provider's statistics are asked for a fixed window. */}
            {history.origin === "localTranscripts" ? (
              <Figure label={t("All time")} figure={history.allTime} withMoney={withMoney} currency={history.currency} />
            ) : (
              <Figure label={t("Last 7 days")} figure={history.last7Days} withMoney={withMoney} currency={history.currency} />
            )}
          </div>
          {history.days.length > 1 && (
            <div className="history-chart">
              <DailyTokensChart days={history.days} slots={SPAN} />
            </div>
          )}
        </div>
        {history.hasPartialCounts && <div className="footnote">{t("Counts may be incomplete.")}</div>}
        <div className="footnote">
          <div>{footnote(history, name)}</div>
          {history.unpricedModels.length > 0 && history.origin === "localTranscripts" && (
            <div>{t("No published price for %@, so those tokens are counted but not costed.", history.unpricedModels.join(", "))}</div>
          )}
        </div>
      </section>

      {rows.length > 0 && (
        // A card of its own: the table is about models, not days, and under the chart it read as more of the same history.
        <section className="history-section">
          <h3 className="group-title">{t("Models")}</h3>
          <div className="group-card history-table-wrap">
            <table className="history-table">
              <thead>
                <tr>
                  <th />
                  <th>{t("Share")}</th>
                  {showsCache && <th>{t("Cache hit")}</th>}
                  {showsSpeed && <th>{t("Tokens/s")}</th>}
                  {showsWait && <th>{t("First token")}</th>}
                </tr>
                <tr className="rule"><td colSpan={columns}><div /></td></tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr key={row.name}>
                    <td title={row.name}>{row.name}</td>
                    <Cell text={row.share === null ? null : percent(row.share)} />
                    {showsCache && <Cell text={row.cacheRate === null ? null : percent(row.cacheRate)} />}
                    {showsSpeed && <Cell text={row.speed === null ? null : speed(row.speed)} />}
                    {showsWait && <Cell text={row.firstToken === null ? null : seconds(row.firstToken)} />}
                  </tr>
                ))}
                {/* The account's own rate, which is not the average of the lines above: each model weighs what it read. */}
                {showsCache && rows.length > 1 && history.overallCacheRate !== null && (
                  <>
                    <tr className="rule"><td colSpan={columns}><div /></td></tr>
                    <tr className="total">
                      <td>{t("All models")}</td>
                      <td />
                      <Cell text={percent(history.overallCacheRate)} />
                      {showsSpeed && <td />}
                      {showsWait && <td />}
                    </tr>
                  </>
                )}
              </tbody>
            </table>
          </div>
          <div className="footnote">{tableNote(history)}</div>
        </section>
      )}
    </div>
  );
}

/**
 * The money is the big figure where there is money, and the tokens are where there isn't. **Not a zero**: a
 * provider's own statistics give one token total per model, which no price list can turn into a cost.
 */
function Figure({ label, figure, withMoney, currency }: { label: string; figure: HistoryFigure; withMoney: boolean; currency: string | null }) {
  return (
    <div className="history-figure">
      <div className="history-figure-label">{label}</div>
      {withMoney ? (
        <>
          {/* None of it priced: a dash, as the Token spend pane draws it. */}
          <div className="history-figure-main">{figure.cost === null ? "—" : money(figure.cost, currency)}</div>
          <div className="history-figure-sub">{tokensText(figure.tokens)}</div>
        </>
      ) : (
        <div className="history-figure-main">{tokensText(figure.tokens)}</div>
      )}
    </div>
  );
}

function Cell({ text }: { text: string | null }) {
  return <td className={text === null ? "none" : undefined}>{text ?? "–"}</td>;
}

const percent = (rate: number) => `${Math.round(rate * 100)}%`;
const speed = (tokensPerSecond: number) => Math.round(tokensPerSecond).toString();
const seconds = (value: number) => t("%@ s", value.toFixed(1));

/**
 * Where the money comes from, said plainly. It would be easy to read these as a bill; they are not one, and the
 * card shouldn't let anyone believe otherwise.
 */
function footnote(history: AccountHistory, name: string): string {
  if (history.origin === "providerStatistics") {
    // A different provenance needs different words: this one is the account's, not this PC's, and it carries no money.
    return t("Reported by %@ for the whole account, so it covers every machine you use it on. It counts tokens only — the figures behind it cannot be turned into a cost.", name);
  }
  if (history.origin === "providerLogs" && history.provider === "deepSeek") {
    return t("From %@'s console for the whole account — every key and every machine — with what was charged, day by day. Pulse reads the last 30 days.", name);
  }
  if (history.origin === "providerLogs") {
    return t("From %@'s request log for the whole account — every machine and every app that uses it — with what each request was charged. The log keeps 30 days.", name);
  }
  return t("Counted from this PC's %@ logs and priced at the published API rates from models.dev. Your plan is a subscription, so this is what the same work would cost through the API — not what you were charged.", name);
}

/** What the table's columns measure and over what span, said once under the card. */
function tableNote(history: AccountHistory): string {
  if (!history.timesReplies) return t("Share and cache hit cover the last 31 days.");
  const sentences = [
    t("Share and cache hit cover the last 31 days; tokens per second only the last 24 hours, from sending a request to the end of its reply."),
  ];
  if (history.speedsEmpty) sentences.push(t("Too few replies in the last 24 hours to time."));
  else if (history.provider === "codex") sentences.push(t("First token is Codex's own measure of how long a turn waited for it."));
  else sentences.push(t("Claude Code doesn't record when the first token arrived, so its wait isn't shown."));
  // Chinese and Japanese run sentences together; the rest put a space.
  const l = locale();
  return sentences.join(l === "zh-Hans" || l === "zh-Hant" || l === "ja" ? "" : " ");
}

// MARK: - Daily tokens chart

/**
 * Daily totals as bars, oldest on the left. **Sized for the whole span, not for the days on record**: a history
 * two days old, divided across the width, drew two blocks each half the card wide; it now draws two bars at the
 * left that the following days join.
 */
function DailyTokensChart({ days, slots }: { days: { date: string; tokens: number }[]; slots: number }) {
  const height = 58;
  return (
    <HoverPlot
      height={height}
      label={t("Tokens per day")}
      build={(width) => {
        const peak = Math.max(...days.map((d) => d.tokens), 1);
        const count = Math.max(days.length, slots, 1);
        const spacing = Math.max((width / count) * 0.22, 2);
        const w = Math.max((width - spacing * (count - 1)) / count, 1);
        const samples: Sample[] = days.map((day, i) => ({ x: w / 2 + i * (w + spacing), title: chartDate(day.date), tokens: day.tokens }));
        return {
          content: (
            <svg width={width} height={height}>
              {days.map((day, i) => {
                // A day with any work at all keeps a visible stub, so a quiet day reads as quiet rather than as missing.
                const h = day.tokens > 0 ? Math.max((height * day.tokens) / peak, 4) : 2;
                return (
                  <rect
                    key={day.date}
                    x={i * (w + spacing)}
                    y={height - h}
                    width={w}
                    height={h}
                    rx={Math.min(w, 4) / 2}
                    className={day.tokens > 0 ? "fill-accent" : "fill-quiet"}
                  />
                );
              })}
            </svg>
          ),
          samples,
        };
      }}
    />
  );
}

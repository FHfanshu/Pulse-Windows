// Ported from upstream Settings/ModelSpendDetailView.swift: one model's usage, drawn in place of the
// pane's lists so the span picker above it keeps working. The money is an API estimate for this model's
// own raw ids, and only where a price exists; a category the summary could not fill in says so.
import { ProviderIcon } from "../../panel/Icon";
import { t } from "../../shared/i18n";
import {
  agentName, exactTokens, money, moneyExact, reportsNoCache, shortDate, tokenCount, tokensText,
  type ModelSpendSummary,
} from "../../shared/spend";
import { Group, Row } from "../Group";
import {
  CostText, HourProfile, PartialNote, ShareBar, SpendBarChart, SpendCaption, TokenKindBreakdown,
} from "./SpendCharts";

const costTotal = (c: { input: number; cacheWrite: number; cacheRead: number; output: number }) =>
  c.input + c.cacheWrite + c.cacheRead + c.output;

export function ModelSpendDetail({ model }: { model: ModelSpendSummary }) {
  const cost = model.costBreakdown ? costTotal(model.costBreakdown) : null;
  const activeDays = model.days.filter((d) => d.tokens > 0).length;
  const busiest = model.days.reduce<(typeof model.days)[number] | null>((best, d) => (!best || d.tokens > best.tokens ? d : best), null);
  const partial = model.unpricedTokens > 0 || model.hasPartialCounts;
  const hours = !model.hasAggregateTiming && model.hours && Object.values(model.hours).some((v) => v > 0) ? model.hours : null;
  const readsUnreported = !!model.tally && reportsNoCache(model.tally);

  return (
    <>
      <Group title={t("Total")}>
        <div className="spend-body">
          <div className="figure-line">
            {cost !== null ? (
              <>
                <span className="figure" title={moneyExact(cost)}>{money(cost)}</span>
                <span className="secondary small">{partial ? t("Partial estimate") : t("API estimate")}</span>
              </>
            ) : (
              <span className="figure dim">{t("Estimate unavailable")}</span>
            )}
          </div>
          <div className="figure-line">
            <span className="secondary num" title={exactTokens(model.tokens)}>{tokensText(model.tokens)}</span>
            {cost !== null && model.unpricedTokens > 0 && (
              <span className="secondary small num" title={exactTokens(model.unpricedTokens)}>
                {t("%@ tokens unpriced", tokenCount(model.unpricedTokens))}
              </span>
            )}
          </div>
          {model.days.length > 1 && <SpendBarChart bars={model.days.map((d) => ({ date: d.date, tokens: d.tokens }))} />}
          <div className="captions">
            <SpendCaption label={t("Active days")} value={t("%@ of %@", activeDays, Math.max(model.days.length, 1))} />
            {busiest && busiest.tokens > 0 && (
              <SpendCaption label={t("Busiest")} value={`${shortDate(busiest.date)} · ${tokenCount(busiest.tokens)}`} title={exactTokens(busiest.tokens)} />
            )}
          </div>
        </div>
      </Group>

      <Group title={t("By kind")}>
        {model.tally ? (
          <TokenKindBreakdown
            tally={model.tally}
            unclassified={model.unclassifiedTokens}
            cost={model.costBreakdown}
            unpriced={model.unpricedTokens}
            readsUnreported={readsUnreported}
          />
        ) : (
          <Row title={t("Token breakdown unavailable.")} />
        )}
        {cost !== null && model.unpricedTokens > 0 && <PartialNote tokens={model.unpricedTokens} />}
      </Group>

      <Group title={t("By hour")}>
        {hours ? (
          <div className="spend-body"><HourProfile hours={hours} /></div>
        ) : (
          <Row title={t("Hourly detail unavailable.")} />
        )}
      </Group>

      {model.agents.length > 0 && (
        <Group title={t("By agent")}>
          {model.agents.map((row) => (
            <Row
              key={row.agent}
              title={agentName[row.agent]}
              subtitle={tokensText(row.tokens)}
              icon={<ProviderIcon provider={row.agent} size={18} />}
            >
              <ShareBar share={model.tokens > 0 ? row.tokens / model.tokens : 0} />
              <span className="cost-slot"><CostText cost={row.cost} unpriced={row.unpricedTokens} /></span>
            </Row>
          ))}
        </Group>
      )}

      <div className="footnote">
        <div>{t("Usage records, estimated at models.dev API rates—not an actual bill.")}</div>
        {model.hasPartialCounts && <div>{t("Counts may be incomplete.")}</div>}
        {model.hasAggregateTiming && <div>{t("Some records use session report dates.")}</div>}
      </div>
    </>
  );
}

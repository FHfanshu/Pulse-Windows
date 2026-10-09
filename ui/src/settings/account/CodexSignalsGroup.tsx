// Ported from upstream Settings/CodexSignalsGroup.swift.
//
// Signs in this PC's Codex sessions that a model gave less than it was asked for: reasoning cut off at the
// 518·n − 2 lattice, and settings that changed under a session without the user changing them
// (pulse_core::codex_signals). Shown for Codex's first account while it is on the panel (upstream:
// `provider == .codex, account.isPrimary, settings.isEnabled(account)`), after the history, because the signs
// belong to whatever Codex ran here, not to an account.
//
// **Signs, and said to be.** Nothing here can see which model the server ran — Codex never writes that down —
// so the copy says "signs", the footnote says what is and is not known, and a model with too few long replies
// says so rather than reading as healthy.
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { intlLocale, tokenCount } from "../../shared/spend";
import { Select } from "../controls";
import { Group, Row } from "../Group";
import type { AccountComponentProps } from "./AccountHistoryGroup";
import { codexSignals, type CodexChange, type CodexSignals, type TruncationRow } from "./consoles";
import "./account.css";
import "./consoles.css";

type Span = "month" | "quarter" | "all";

const days: Record<Span, number | null> = { month: 30, quarter: 90, all: null };

/** The most recent changes listed; the rest are counted. */
const listed = 8;

export function CodexSignalsGroup({ id, provider, settings }: AccountComponentProps) {
  const [span, setSpan] = useState<Span>("month");
  const [read, setRead] = useState<{ span: Span; signals: CodexSignals } | null>(null);
  const shown = id === provider && provider === "codex" && settings.enabledAccounts.includes(id);

  useEffect(() => {
    if (!shown) return;
    let live = true;
    codexSignals(days[span])
      .then((signals) => live && signals && setRead({ span, signals }))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [span, shown]);

  if (!shown) return null;

  const spans = [
    { value: "month" as Span, label: t("Last 30 days") },
    { value: "quarter" as Span, label: t("Last 90 days") },
    { value: "all" as Span, label: t("All sessions") },
  ];
  const signals = read?.span === span ? read.signals : null;

  return (
    <div className="signals-stack">
      <Group title={t("Signs of a weaker model")}>
        <Row title={t("Period")}>
          <Select label={t("Period")} value={span} options={spans} onChange={setSpan} />
        </Row>
      </Group>

      {signals ? (
        <>
          <Truncation signals={signals} />
          <Changes signals={signals} />
        </>
      ) : (
        <Group title={t("Reasoning cut off")}>
          <Row title={t("Reading local records…")}>
            <span className="spinner" role="progressbar" aria-label={t("Reading local records…")} />
          </Row>
        </Group>
      )}

      {/* Four short lines rather than one paragraph: each answers one question a reader has about the groups above. */}
      <div className="footnote">
        <div>{t("Read from this Mac's Codex records; nothing is sent.")}</div>
        <div>{t("Reasoning cut off: the model's reasoning stopped at exactly 516, 1,034, 1,552… tokens, as if cut off. Only about 0.2% of replies would stop there by chance.")}</div>
        <div>{t("Settings quietly lowered: a turn ran on a worse model, lower reasoning or a smaller context than you chose, without you changing anything.")}</div>
        <div>{t("These are signs, not proof: Codex doesn't record which model the server actually used.")}</div>
      </div>
    </div>
  );
}

// MARK: - Reasoning cut off

function Truncation({ signals }: { signals: CodexSignals }) {
  return (
    <Group title={t("Reasoning cut off")}>
      {signals.truncation.length === 0 ? (
        <Row title={t("No Codex replies with reasoning in this period")} />
      ) : (
        signals.truncation.map((model) => (
          <Row key={model.model} title={model.model} subtitle={detail(model)}>
            <Verdict model={model} />
          </Row>
        ))
      )}
    </Group>
  );
}

/** "126 of 279 long replies had their reasoning cut off · 2,827 replies in all". */
function detail(model: TruncationRow): string {
  const replies = t("%@ replies in all", count(model.responses));
  if (model.reachedLattice <= 0) return replies;
  const stopped = t("%@ of %@ long replies had their reasoning cut off", count(model.onLattice), count(model.reachedLattice));
  return `${stopped} · ${replies}`;
}

function Verdict({ model }: { model: TruncationRow }) {
  if (!model.isMeasurable) return <span className="value-text">{t("Too few to tell")}</span>;
  return (
    <span className="verdict">
      <span className={`verdict-share${model.isSuspicious ? " suspicious" : ""}`}>{percent(model.share ?? 0)}</span>
      <span className={`verdict-word${model.isSuspicious ? " suspicious" : ""}`}>
        {model.isSuspicious ? t("Possibly weakened") : t("Normal")}
      </span>
    </span>
  );
}

// MARK: - Settings quietly lowered

function Changes({ signals }: { signals: CodexSignals }) {
  const { changes } = signals;
  return (
    <Group title={t("Settings quietly lowered")}>
      {changes.length === 0 ? (
        <Row title={t("None found")} subtitle={checked(signals, true)} />
      ) : (
        <>
          {changes.slice(0, listed).map((change) => (
            <Row key={`${change.session}|${change.order}`} title={changeTitle(change)} subtitle={time(change.date)} />
          ))}
          {changes.length > listed && (
            <Row title={t("%@ more", count(changes.length - listed))} subtitle={checked(signals, false)} />
          )}
        </>
      )}
    </Group>
  );
}

function changeTitle(change: CodexChange): string {
  switch (change.kind) {
    case "model":
      return t("Model: %@ swapped for %@", change.asked, change.ran);
    case "effort":
      return t("Reasoning: %@ lowered to %@", change.asked, change.ran);
    case "contextWindow":
      return t("Context: %@ cut to %@", tokenCount(change.was), tokenCount(change.now));
  }
}

/**
 * How many sessions were looked at — and, when nothing was found, that nothing was changed in them — and how many
 * could not be: Codex before 0.144 writes down none of the user's own changes, and Codex's own helpers run on
 * models it picks.
 */
function checked(signals: CodexSignals, clean: boolean): string {
  const judged = clean
    ? t("Checked %@ sessions: the model, reasoning and context you chose were never changed.", count(signals.judgedSessions))
    : t("Checked %@ sessions.", count(signals.judgedSessions));
  const older = signals.sessions - signals.judgedSessions;
  return older > 0 ? `${judged} ${t("%@ other sessions can't be checked: older than Codex 0.144, or run by Codex itself.", count(older))}` : judged;
}

// MARK: - Formatting

const count = (value: number) => new Intl.NumberFormat(intlLocale()).format(value);

const percent = (value: number) =>
  new Intl.NumberFormat(intlLocale(), { style: "percent", minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(value);

const time = (iso: string) =>
  new Intl.DateTimeFormat(intlLocale(), { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }).format(new Date(iso));

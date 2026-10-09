// Ported from upstream Settings/WindowStarterGroup.swift and App/AppSettings+WindowStarter.swift.
//
// Sends "hi" after each reset so the next window starts then (pulse_core::window_starter). Shown for the first
// account of Claude Code and Codex while that account is on the panel (upstream: `account.isPrimary`,
// `WindowPrimer.providers.contains(provider)`, `settings.isEnabled(account)`).
//
// Switching it on goes through a confirmation that says plainly this is not the provider's feature and may cost
// the account; the reader decides with that in front of them, not in a subtitle.
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { updateSettings } from "../../shared/settings";
import { intlLocale } from "../../shared/spend";
import { Button, Select, Switch } from "../controls";
import { Group, Row } from "../Group";
import type { AccountComponentProps } from "./AccountHistoryGroup";
import "./account.css";
import "./consoles.css";

/** The providers the starter acts for (`pulse_core::window_starter::PROVIDERS`). */
const providers = new Set(["claudeCode", "codex"]);

const hours = Array.from({ length: 24 }, (_, hour) => ({ value: hour, label: `${String(hour).padStart(2, "0")}:00` }));

/** "Oct 9, 3:45 PM": the time of an attempt, in the reader's language (upstream template "MMMdjmm"). */
function when(seconds: number): string {
  return new Intl.DateTimeFormat(intlLocale(), { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }).format(
    new Date(seconds * 1000),
  );
}

/** The last attempt: its time and, when it did not go through, why (`WindowStarter.Outcome`). */
function status(time: number | undefined, outcome: string | undefined): string {
  if (time === undefined || !outcome) return t("Not started yet");
  const at = when(time);
  switch (outcome) {
    case "sent":
      return at;
    case "toolMissing":
      return t("%@ · the command-line tool was not found", at);
    case "failed":
      return t("%@ · it did not go through; check the tool is signed in", at);
    case "timedOut":
      return t("%@ · it did not answer in time", at);
    default:
      return at;
  }
}

export function WindowStarterGroup({ id, provider, settings }: AccountComponentProps) {
  const [confirming, setConfirming] = useState(false);
  // Nothing may stay armed after the account changes under the dialog.
  useEffect(() => setConfirming(false), [id]);

  if (id !== provider || !providers.has(provider) || !settings.enabledAccounts.includes(id)) return null;

  const on = settings.primedProviders.includes(provider);
  const setOn = (value: boolean) =>
    void updateSettings({
      primedProviders: value
        ? [...settings.primedProviders.filter((p) => p !== provider), provider]
        : settings.primedProviders.filter((p) => p !== provider),
    });
  const setHours = (patch: Partial<{ start: number; end: number }>) =>
    void updateSettings({ primerHours: { ...settings.primerHours, ...patch } });

  return (
    <>
      <Group title={t("Start windows automatically")}>
        <Row
          title={t("Start a new window after each reset")}
          subtitle={
            provider === "claudeCode"
              ? t("When the 5-hour limit resets, sends “hi” to Haiku through Claude Code, so the next window starts counting then rather than at your next message. Nothing is saved.")
              : t("When the 5-hour or weekly limit resets, sends “hi” to the cheapest model through Codex, so the next window starts counting then rather than at your next message. Nothing is saved.")
          }
        >
          <Switch
            label={t("Start a new window after each reset")}
            checked={on}
            onChange={(value) => (value ? setConfirming(true) : setOn(false))}
          />
        </Row>
        {on && (
          <>
            <Row
              title={t("Only between")}
              subtitle={t("A reset outside these hours is started when they begin. Shared by Claude Code and Codex.")}
            >
              <span className="hours">
                <Select label={t("Only between")} value={settings.primerHours.start} options={hours} onChange={(start) => setHours({ start })} />
                <span aria-hidden="true">–</span>
                <Select label={t("Only between")} value={settings.primerHours.end} options={hours} onChange={(end) => setHours({ end })} />
              </span>
            </Row>
            <Row title={t("Last started")}>
              <span className="value-text last-started">{status(settings.primerRunTimes[provider], settings.primerRunOutcomes[provider])}</span>
            </Row>
          </>
        )}
      </Group>
      {confirming && (
        <RiskDialog
          onCancel={() => setConfirming(false)}
          onConfirm={() => {
            setOn(true);
            setConfirming(false);
          }}
        />
      )}
    </>
  );
}

/** The confirmation (a destructive-styled alert upstream): turns it on only from "I understand the risk, turn it on". */
function RiskDialog({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  useEffect(() => {
    const key = (e: KeyboardEvent) => e.key === "Escape" && onCancel();
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [onCancel]);
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal" role="alertdialog" aria-modal="true" aria-labelledby="risk-title" onClick={(e) => e.stopPropagation()}>
        <h2 id="risk-title">{t("Use at your own risk")}</h2>
        <p>
          {t(
            "This is not a feature of Anthropic or OpenAI. Starting usage windows automatically may be treated as getting around usage limits, and could get your account restricted or suspended. Pulse only sends one short message through the tool you are already signed in to, and is not responsible for anything that happens to your account as a result. Turn it on only if you accept that.",
          )}
        </p>
        <div className="modal-buttons">
          <Button onClick={onCancel}>{t("Cancel")}</Button>
          <button type="button" className="btn destructive" onClick={onConfirm}>
            {t("I understand the risk, turn it on")}
          </button>
        </div>
      </div>
    </div>
  );
}

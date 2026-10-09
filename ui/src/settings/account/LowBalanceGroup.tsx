// Ported from upstream Settings/AccountConnectionRows.swift (`LowBalanceRow`) and the account pane's
// "Notifications" group: a prepaid balance has no percentage to warn at, so it gets a line of its own, the
// money, not a fraction of an allowance nobody reports. Per account, because the providers that report a
// balance do not price in the same currency (¥20 and $20 are not the same line).
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { updateSettings } from "../../shared/settings";
import { intlLocale } from "../../shared/spend";
import { Button } from "../controls";
import { Group, Row } from "../Group";
import { reportsSpendableBalance } from "./api";
import type { AccountComponentProps } from "./AccountHistoryGroup";
import "./account.css";

/**
 * Read the typed figure the way the reader writes numbers, so a comma decimal separator is read rather than
 * silently clearing the setting, and the currency symbol somebody types out of habit is ignored. Anything
 * that is not a positive figure is no figure: a warning below zero can never fire, and one at zero fires only
 * once the account is already empty, which is the moment it is too late to be told.
 */
export function parseMoney(typed: string): number | null {
  const kept = typed.replace(/[^\d.,-]/g, "");
  if (!kept) return null;
  const parts = new Intl.NumberFormat(intlLocale()).formatToParts(1234.5);
  const decimalSeparator = parts.find((p) => p.type === "decimal")?.value ?? ".";
  const lastComma = kept.lastIndexOf(",");
  const lastDot = kept.lastIndexOf(".");
  let normal: string;
  if (lastComma >= 0 && lastDot >= 0) {
    // Both: the later one is the decimal point, the other groups thousands.
    const decimal = lastComma > lastDot ? "," : ".";
    const group = decimal === "," ? "." : ",";
    normal = kept.split(group).join("").replace(decimal, ".");
  } else if (lastComma >= 0) {
    // Only commas: a decimal separator in a language that writes one (or one comma with one or two digits
    // after it), else grouping.
    const after = kept.length - lastComma - 1;
    const single = kept.indexOf(",") === lastComma;
    normal = single && (decimalSeparator === "," || after < 3) ? kept.replace(",", ".") : kept.split(",").join("");
  } else {
    normal = kept;
  }
  const value = Number(normal);
  return Number.isFinite(value) && value > 0 ? value : null;
}

/** The stored figure back in the field, without a trailing `.0` or grouping. */
export function moneyText(amount: number | undefined): string {
  if (amount === undefined) return "";
  return new Intl.NumberFormat(intlLocale(), { maximumFractionDigits: 2, useGrouping: false }).format(amount);
}

export function LowBalanceGroup({ id, provider, settings }: AccountComponentProps) {
  const [reports, setReports] = useState(false);
  useEffect(() => {
    let live = true;
    reportsSpendableBalance(provider).then((yes) => live && setReports(yes)).catch(() => {});
    return () => {
      live = false;
    };
  }, [provider]);

  const stored = settings.lowBalanceAlerts[id];
  const [draft, setDraft] = useState(moneyText(stored));
  useEffect(() => setDraft(moneyText(stored)), [stored, id]);

  // Only a first account hands over a balance that can be compared against a figure.
  if (id !== provider || !reports) return null;

  const save = () => {
    const amount = parseMoney(draft);
    const next = { ...settings.lowBalanceAlerts };
    if (amount === null) delete next[id];
    else next[id] = amount;
    // The change is judged straight away on the Rust side: a balance already under the line when the figure
    // is entered is announced once, rather than waiting for a pass.
    void updateSettings({ lowBalanceAlerts: next });
    setDraft(moneyText(next[id]));
  };

  return (
    <Group title={t("Notifications")}>
      <Row title={t("Warn below")} subtitle={t("Notify once when the balance falls under this. Blank for never.")}>
        <input
          className="text-field"
          style={{ width: 120 }}
          type="text"
          inputMode="decimal"
          aria-label={t("Warn below")}
          value={draft}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") save(); }}
        />
        <Button onClick={save}>{t("Save")}</Button>
      </Row>
    </Group>
  );
}

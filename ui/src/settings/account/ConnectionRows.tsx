// Ported from upstream Settings/AccountConnectionRows.swift and AccountEntryFields.swift: the single rows the
// Connection and Notifications cards draw. Each commits on Save (or Return), so a half-entered value is never
// read as a setting.
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import type { AppSettings } from "../../shared/settings";
import { Button, Segmented } from "../controls";
import { Row } from "../Group";
import { isUsableAddress } from "./address";
import { moneyText, parseMoney, setBalanceBasis, setBalanceBudget, setLowBalanceAlert, setServerAddress } from "./store";

/** A text box with its own draft: `onSave` is called with the draft, and the box shows whatever was stored. */
function useDraft(stored: string) {
  const [draft, setDraft] = useState(stored);
  useEffect(() => setDraft(stored), [stored]);
  return [draft, setDraft] as const;
}

/**
 * Where a self-hosted gateway lives. The only addresses in the app a reader types, so the only ones that can be
 * wrong: checked on Save rather than on every keystroke (`https://s` is somebody halfway through a word), and a
 * refusal says what the rule is rather than just colouring the box.
 */
export function ServerAddressRow({ id, settings }: { id: string; settings: AppSettings }) {
  const stored = (settings.serverAddresses[id] ?? "").trim();
  const [draft, setDraft] = useDraft(stored);
  const [invalid, setInvalid] = useState(false);

  const save = () => {
    const typed = draft.trim();
    // Blank clears it, which puts the pane back to asking for an address.
    if (typed && !isUsableAddress(typed)) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    setDraft(typed);
    void setServerAddress(settings, id, typed);
  };

  return (
    <Row
      title={t("Server address")}
      subtitle={
        invalid
          ? t("That address can't be used. It needs https://, unless the server is on your own network.")
          : t("Your own deployment's address, such as https://gateway.example.com. Pulse asks it for usage and sends nothing else.")
      }
      invalid={invalid}
      wrap
    >
      <input
        id="account-address"
        className="text-field"
        aria-label={t("Server address")}
        value={draft}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => { if (e.key === "Enter") save(); }}
      />
      <Button disabled={draft === stored} onClick={save}>{t("Save")}</Button>
    </Row>
  );
}

const basisSubtitle: Record<string, string> = {
  sinceTopUp: "How much of the balance Pulse last saw you top up to is gone.",
  balanceOnly: "The money left, with no ring. There is no allowance to measure against.",
  budget: "How much of the figure you set is gone.",
};

/**
 * An API account reports money and no allowance, so the ring has no denominator until one is chosen. Three modes
 * because there are exactly three places one can come from. Not "Ring shows": that is the Panel card's row.
 */
export function BalanceBasisRow({ id, settings }: { id: string; settings: AppSettings }) {
  const basis = settings.balanceBases[id] ?? "sinceTopUp";
  return (
    <Row title={t("Ring measures")} subtitle={t(basisSubtitle[basis] ?? basisSubtitle.sinceTopUp)} wrap>
      <Segmented
        label={t("Ring measures")}
        value={basis}
        options={[
          { value: "sinceTopUp", label: t("Since top-up") },
          { value: "balanceOnly", label: t("Balance only") },
          { value: "budget", label: t("My budget") },
        ]}
        onChange={(v) => void setBalanceBasis(settings, id, v)}
      />
    </Row>
  );
}

/** A money field with a Save button (Full tank, Warn below). Blank clears the setting. */
function MoneyRow({ title, subtitle, stored, onSave, disabled }: {
  title: string;
  subtitle: string;
  stored: number | null | undefined;
  onSave: (amount: number | null) => void;
  disabled?: boolean;
}) {
  const [draft, setDraft] = useDraft(moneyText(stored));
  const save = () => {
    const amount = parseMoney(draft);
    onSave(amount);
    setDraft(moneyText(amount));
  };
  return (
    <Row title={title} subtitle={subtitle} disabled={disabled}>
      <input
        className="text-field narrow"
        aria-label={title}
        inputMode="decimal"
        value={draft}
        disabled={disabled}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => { if (e.key === "Enter") save(); }}
      />
      <Button disabled={disabled} onClick={save}>{t("Save")}</Button>
    </Row>
  );
}

export function BalanceBudgetRow({ id, settings }: { id: string; settings: AppSettings }) {
  return (
    <MoneyRow
      title={t("Full tank")}
      subtitle={t("What you call a full balance. The ring measures against it.")}
      stored={settings.balanceBudgets[id]}
      onSave={(amount) => void setBalanceBudget(settings, id, amount)}
    />
  );
}

/** A prepaid balance has no percentage to warn at, so it gets a line of its own: the money, not a fraction. */
export function LowBalanceRow({ id, settings }: { id: string; settings: AppSettings }) {
  return (
    <MoneyRow
      title={t("Warn below")}
      subtitle={t("Notify once when the balance falls under this. Blank for never.")}
      stored={settings.lowBalanceAlerts[id]}
      onSave={(amount) => void setLowBalanceAlert(settings, id, amount)}
    />
  );
}

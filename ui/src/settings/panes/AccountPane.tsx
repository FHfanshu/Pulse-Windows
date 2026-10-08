import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { relativeTime, resetText, unavailableMessage, windowName } from "../../shared/copy";
import { t } from "../../shared/i18n";
import { accountId, percentText } from "../../shared/model";
import { updateSettings, useUsage, type AppSettings } from "../../shared/settings";
import { Button, Switch } from "../controls";
import { Group, Row } from "../Group";
import { apiProviders } from "../panes";
import { ToggleRow } from "./ToggleRow";

/** One account: whether it is on the panel, its key (API providers), and what it currently reads. */
export function AccountPane({ id, settings }: { id: string; settings: AppSettings }) {
  const provider = settings.extraAccounts.find((a) => a.id === id)?.provider ?? id;
  const enabled = settings.enabledAccounts.includes(id);

  const setEnabled = (on: boolean) =>
    updateSettings({
      enabledAccounts: on ? [...settings.enabledAccounts, id] : settings.enabledAccounts.filter((a) => a !== id),
    });

  return (
    <div className="pane-stack">
      <Group title={t("Panel")}>
        <ToggleRow
          title="Show in panel"
          checked={enabled}
          // The last one standing stays on: an empty rail has nothing to hover or drag.
          disabled={enabled && settings.enabledAccounts.length === 1}
          onChange={setEnabled}
        />
      </Group>
      {apiProviders.has(provider) && (
        <Group title={t("Connection")}>
          <KeyRow id={id} provider={provider} />
        </Group>
      )}
      {provider === "claudeCode" && enabled && (
        <Group title={t("Connection")}>
          <StatusLineRow />
        </Group>
      )}
      <CurrentUsage id={id} settings={settings} enabled={enabled} />
    </div>
  );
}

/** Claude Code's status line as a backup route for its figures; connected and disconnected from here. */
function StatusLineRow() {
  const [connected, setConnected] = useState(false);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    invoke<boolean>("status_line_installed").then(setConnected).catch(() => {});
  }, []);

  const change = async (want: boolean) => {
    setBusy(true);
    setFailed(false);
    try {
      const now = await invoke<boolean>("set_status_line", { connected: want });
      setConnected(now);
      setFailed(now !== want);
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Row
      title={t("Claude Code status line")}
      subtitle={failed ? t("Couldn't connect the status line") : t("A backup for when the saved login expires. Your own status line keeps working.")}
      invalid={failed}
    >
      <Switch label={t("Connect status line")} checked={connected} disabled={busy} onChange={(on) => void change(on)} />
    </Row>
  );
}

function keySubtitle(provider: string): string {
  return provider === "deepSeek"
    ? t("From platform.deepseek.com. Stored encrypted on this Mac.")
    : t("Stored encrypted on this Mac.");
}

/** Password field with Save and Remove; only whether a key exists is ever read back. */
function KeyRow({ id, provider }: { id: string; provider: string }) {
  const [saved, setSaved] = useState(false);
  const [draft, setDraft] = useState("");
  const [revealed, setRevealed] = useState(false);

  useEffect(() => {
    setDraft("");
    setRevealed(false);
    invoke<boolean>("has_secret", { id }).then(setSaved);
  }, [id]);

  const save = async () => {
    if (!draft.trim()) return;
    await invoke("set_secret", { id, value: draft.trim() });
    setDraft("");
    setSaved(await invoke<boolean>("has_secret", { id }));
  };
  const remove = async () => {
    await invoke("set_secret", { id, value: null });
    setSaved(false);
  };

  return (
    <Row title={t("API key")} subtitle={saved ? `${t("Saved")}. ${keySubtitle(provider)}` : keySubtitle(provider)}>
      <input
        className="text-field"
        type={revealed ? "text" : "password"}
        aria-label={t("API key")}
        value={draft}
        placeholder={saved ? "••••••••••••" : ""}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => { if (e.key === "Enter") void save(); }}
      />
      <Button onClick={() => setRevealed((r) => !r)}>{revealed ? t("Hide") : t("Show")}</Button>
      <Button disabled={!draft.trim()} onClick={() => void save()}>{t("Save")}</Button>
      <Button disabled={!saved} onClick={() => void remove()}>{t("Remove")}</Button>
    </Row>
  );
}

/** Re-renders now and then so "1 minute ago" keeps counting. */
function useTick(ms: number) {
  const [, setTick] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), ms);
    return () => clearInterval(timer);
  }, [ms]);
}

function CurrentUsage({ id, settings, enabled }: { id: string; settings: AppSettings; enabled: boolean }) {
  const { usages, refreshing } = useUsage();
  useTick(15_000);
  const usage = usages.find((u) => accountId(u.account) === id);

  if (!enabled) {
    return (
      <Group title={t("Current usage")}>
        <Row title={t("Not shown")} subtitle={t("Enable a service in its settings to start monitoring.")} />
      </Group>
    );
  }

  const reason = usage?.state.kind === "unavailable" ? unavailableMessage(usage.state.reason) : null;
  return (
    <Group title={t("Current usage")}>
      <Row title={t("Last read")}>
        <span className="value-text">{usage?.observedAt ? relativeTime(usage.observedAt) : t("Not yet")}</span>
        <Button disabled={refreshing.length > 0} onClick={() => void invoke("refresh", { account: id })}>{t("Refresh")}</Button>
      </Row>
      {!usage || usage.windows.length === 0 ? (
        <Row title={t("No reading")} subtitle={reason} />
      ) : (
        usage.windows.map((w) => (
          <Row key={w.id} title={windowName(w)} subtitle={resetText(w)}>
            <span className="value-strong">{percentText(w, settings.showsRemaining)}</span>
          </Row>
        ))
      )}
      {usage?.plan && (
        <Row title={t("Plan")}><span className="value-text">{usage.plan}</span></Row>
      )}
      {usage?.creditBalance && (
        <Row title={t("Credit balance")}><span className="value-text">{usage.creditBalance}</span></Row>
      )}
    </Group>
  );
}

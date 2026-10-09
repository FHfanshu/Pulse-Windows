// Ported from upstream Settings/ClaudeCodeStatusLineRow.swift. Registering Pulse as Claude Code's status line is the
// backup route for its figures: the stored login expires after a few hours and nothing here renews it, so the
// status line covers the gap until Claude Code is next used. Kept visible and reversible.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { Button } from "../controls";
import { Row } from "../Group";

export function StatusLineRow() {
  const [connected, setConnected] = useState(false);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    invoke<boolean>("status_line_installed").then(setConnected).catch(() => {});
  }, []);

  const change = async () => {
    const want = !connected;
    setBusy(true);
    setFailed(false);
    try {
      const now = await invoke<boolean>("set_status_line", { connected: want });
      setConnected(now);
      setFailed(now !== want);
      void invoke("refresh", { account: null });
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Row
      title={t("Claude Code status line")}
      subtitle={failed ? t("Couldn't connect the status line. Open setup help.") : t("A backup for when the saved login expires. Your own status line keeps working.")}
      invalid={failed}
    >
      <Button disabled={busy} onClick={() => void change()}>{connected ? t("Disconnect") : t("Connect")}</Button>
    </Row>
  );
}

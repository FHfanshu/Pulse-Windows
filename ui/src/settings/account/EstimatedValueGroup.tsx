// Ported from upstream Settings/EstimatedValueGroup.swift: what each limit is worth in money. The only inferred
// figure in the app, so it gets its own group and says plainly where it came from. The estimate itself is
// `pulse_core::spend::budget` (`estimated_value` in src-tauri/src/account_ipc.rs).
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { windowName } from "../../shared/copy";
import { t } from "../../shared/i18n";
import type { ProviderUsage } from "../../shared/model";
import { approximate, type WindowEstimate } from "../../shared/spend";
import { Group, Row } from "../Group";
import { pc } from "./meta";

export { approximate };

export function EstimatedValueGroup({ id, usage, readsTokenSpend }: {
  id: string;
  usage: ProviderUsage | undefined;
  readsTokenSpend: boolean;
}) {
  const [estimates, setEstimates] = useState<WindowEstimate[]>([]);
  const observed = usage?.observedAt ?? null;

  useEffect(() => {
    if (!readsTokenSpend) {
      setEstimates([]);
      return;
    }
    let live = true;
    invoke<WindowEstimate[]>("estimated_value", { account: id, card: false })
      .then((list) => { if (live) setEstimates(list); })
      .catch(() => { if (live) setEstimates([]); });
    return () => { live = false; };
  }, [id, readsTokenSpend, observed]);

  const rows = (usage?.windows ?? []).flatMap((w) => {
    const e = estimates.find((x) => x.window === w.id);
    return e ? [{ window: w, estimate: e }] : [];
  });
  if (rows.length === 0) return null;

  return (
    <div className="pane-stack tight">
      <Group title={t("Estimated value")}>
        {rows.map(({ window, estimate }) =>
          estimate.elsewhere ? (
            // A window seen spent off this PC keeps its row, saying why there is no figure.
            <Row key={window.id} title={windowName(window)} subtitle={pc("Also used somewhere this Mac's logs can't see")}>
              <span className="value-text">{t("Not estimated")}</span>
            </Row>
          ) : (
            <Row key={window.id} title={windowName(window)} subtitle={t("%@ used so far", approximate(estimate.spent))}>
              {/* Just what the whole window is worth: the remainder is only the other two numbers subtracted. */}
              <span className="value-strong">{approximate(estimate.full)}</span>
            </Row>
          ),
        )}
      </Group>
      <p className="footnote">
        {pc("An estimate, not a reported figure: what this Mac spent since each window opened, divided by the percentage the provider says is used. When the percentage rises while this Mac spends nothing, the account is being used elsewhere — another computer, or the website — and that window isn't estimated until it resets. If both are in use at once, the figure reads low. Windows with too little use to extrapolate from are left out.")}
      </p>
    </div>
  );
}

// Ported from upstream Settings/EstimatedValueGroup.swift: what each limit is worth in money. The only inferred
// figure in the app, so it gets its own group and says plainly where it came from, rather than sitting beside
// the reported percentages as though it were one of them.
import { useEffect, useState } from "react";
import { windowName } from "../../shared/copy";
import { t } from "../../shared/i18n";
import type { ProviderUsage } from "../../shared/model";
import { approximate, estimatedValues, type WindowValue } from "../../shared/spend";
import { Group, Row } from "../Group";

export function EstimatedValueGroup({ account, usage }: { account: string; usage: ProviderUsage | undefined }) {
  const [values, setValues] = useState<WindowValue[]>([]);
  // The estimate is worked from the reading's percentage and the moment it was read, so a new reading asks again.
  const reading = `${usage?.observedAt ?? ""}|${usage?.windows.map((w) => `${w.id}:${w.usedFraction}`).join(",") ?? ""}`;
  useEffect(() => {
    let live = true;
    estimatedValues(account, false)
      .then((v) => live && setValues(v))
      .catch(() => live && setValues([]));
    return () => { live = false; };
  }, [account, reading]);

  const entries = (usage?.windows ?? []).flatMap((window) => {
    const value = values.find((v) => v.windowId === window.id);
    return value ? [{ window, value }] : [];
  });
  if (entries.length === 0) return null;

  return (
    <div className="pane-sub">
      <Group title={t("Estimated value")}>
        {entries.map(({ window, value }) =>
          value.full !== null && value.spent !== null ? (
            <Row key={window.id} title={windowName(window)} subtitle={t("%@ used so far", approximate(value.spent))}>
              {/* Just what the whole window is worth: the remainder is only the other two numbers subtracted,
                  and the percentage it comes from is already on screen, in "Current usage" above. */}
              <span className="value-figure">{approximate(value.full)}</span>
            </Row>
          ) : (
            // A window seen spent off this PC keeps its row, saying why there is no figure.
            <Row key={window.id} title={windowName(window)} subtitle={t("Also used somewhere this Mac's logs can't see")}>
              <span className="value-text">{t("Not estimated")}</span>
            </Row>
          ),
        )}
      </Group>
      <div className="footnote">
        {t("An estimate, not a reported figure: what this Mac spent since each window opened, divided by the percentage the provider says is used. When the percentage rises while this Mac spends nothing, the account is being used elsewhere — another computer, or the website — and that window isn't estimated until it resets. If both are in use at once, the figure reads low. Windows with too little use to extrapolate from are left out.")}
      </div>
    </div>
  );
}

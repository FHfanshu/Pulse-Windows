import { t } from "../../shared/i18n";
import { updateSettings, type AppSettings } from "../../shared/settings";
import { Segmented, Select } from "../controls";
import { Group, Row } from "../Group";
import { ToggleRow } from "./ToggleRow";

const thresholds = [60, 70, 75, 80, 85, 90].map((v) => ({ value: v, label: `${v}%` }));

/** What each ring and the card say, and when they turn red. */
export function RingsPane({ settings }: { settings: AppSettings }) {
  const off = !settings.isPanelVisible;
  const noFigures = !settings.sideRailShowsPercentages && !settings.topRailShowsPercentages;
  return (
    <div className="pane-stack">
      <Group title={t("Figures")}>
        <ToggleRow
          title="Percentages at the side"
          subtitle="The figure under each ring, docked left or right."
          checked={settings.sideRailShowsPercentages}
          disabled={off}
          onChange={(sideRailShowsPercentages) => updateSettings({ sideRailShowsPercentages })}
        />
        <ToggleRow
          title="Percentages across"
          subtitle="When the panel lies across: docked to the top or bottom, or free."
          checked={settings.topRailShowsPercentages}
          disabled={off}
          onChange={(topRailShowsPercentages) => updateSettings({ topRailShowsPercentages })}
        />
        <ToggleRow
          title="Figures beside the rings"
          subtitle="Only when the panel lies free across. A thinner, longer panel."
          checked={settings.freeAcrossFiguresBeside}
          disabled={off || !settings.topRailShowsPercentages}
          onChange={(freeAcrossFiguresBeside) => updateSettings({ freeAcrossFiguresBeside })}
        />
        <ToggleRow
          title="Figure above the ring"
          subtitle="Swaps the two, wherever the panel is."
          checked={settings.labelAboveRing}
          disabled={off || noFigures}
          onChange={(labelAboveRing) => updateSettings({ labelAboveRing })}
        />
        <ToggleRow
          title="Show what's left"
          subtitle="Counts down instead of up, figure and ring together."
          checked={settings.showsRemaining}
          disabled={off}
          onChange={(showsRemaining) => updateSettings({ showsRemaining })}
        />
        <ToggleRow
          title="Forecast"
          subtitle="Whether each limit lasts its window, on the card."
          checked={settings.showsForecast}
          disabled={off}
          onChange={(showsForecast) => updateSettings({ showsForecast })}
        />
      </Group>

      <Group title={t("Rings")}>
        <ToggleRow
          title="Second limit inside the ring"
          subtitle="A thinner ring for the next-fullest limit, where a provider has one."
          checked={settings.showsSecondRing}
          disabled={off}
          onChange={(showsSecondRing) => updateSettings({ showsSecondRing })}
        />
        <ToggleRow
          title="Time until reset"
          subtitle="A second arc outside each ring, showing progress through the current window."
          checked={settings.showsWindowClock}
          disabled={off}
          onChange={(showsWindowClock) => updateSettings({ showsWindowClock })}
        />
        <Row
          title={t("Time ring direction")}
          subtitle={t("Choose whether the outer arc fills with elapsed time or empties with time remaining.")}
          disabled={off || !settings.showsWindowClock}
        >
          <Segmented
            label={t("Time ring direction")}
            disabled={off || !settings.showsWindowClock}
            value={settings.windowClockDirection}
            options={[
              { value: "elapsed", label: t("Elapsed") },
              { value: "remaining", label: t("Remaining") },
            ]}
            onChange={(windowClockDirection) => updateSettings({ windowClockDirection })}
          />
        </Row>
        <Row
          title={t("Turn red at")}
          subtitle={t("Where a ring stops being amber. A spent limit is red whatever this says.")}
          disabled={off}
        >
          <Select
            label={t("Turn red at")}
            disabled={off}
            value={settings.warningThreshold}
            options={thresholds}
            onChange={(warningThreshold) => updateSettings({ warningThreshold })}
          />
        </Row>
        <ToggleRow
          title="Alert colour when docked"
          subtitle="Off keeps the collapsed rail neutral even when a limit needs attention."
          checked={settings.dockShowsAlertColor}
          disabled={off}
          onChange={(dockShowsAlertColor) => updateSettings({ dockShowsAlertColor })}
        />
      </Group>
    </div>
  );
}

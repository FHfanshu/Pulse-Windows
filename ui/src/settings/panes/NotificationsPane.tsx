import { t } from "../../shared/i18n";
import { updateSettings, type AppSettings } from "../../shared/settings";
import { Select } from "../controls";
import { Group, Row } from "../Group";
import { ToggleRow } from "./ToggleRow";

/** Upstream `AlertThreshold`: off, or the share of a limit at which to speak up. 0 stands for off. */
const steps = [0, 75, 80, 90, 95].map((v) => ({ value: v, label: v === 0 ? t("Off") : `${v}%` }));

/**
 * What Pulse may say unprompted. Every switch starts off; the rules that decide
 * when each one speaks live in `pulse_core::alerts`.
 * ("When a service is down" and "When a recap is ready" are not ported yet.)
 */
export function NotificationsPane({ settings }: { settings: AppSettings }) {
  const threshold = settings.alertThreshold ?? 0;
  return (
    <div className="pane-stack">
      <Group>
        <Row title={t("Warn at")} subtitle={t("Notify when a limit passes this, and again when it is spent.")}>
          <Select
            label={t("Warn at")}
            value={threshold}
            options={steps}
            onChange={(v) => updateSettings({ alertThreshold: v === 0 ? null : v })}
          />
        </Row>
        <ToggleRow
          title="When a limit comes back"
          subtitle="Only for one you were warned about."
          checked={settings.alertsOnReset}
          // A reset is only announced for a window that was mentioned on the way up.
          disabled={threshold === 0}
          onChange={(alertsOnReset) => updateSettings({ alertsOnReset })}
        />
        <ToggleRow
          title="When a reading stops arriving"
          subtitle="After several failed checks in a row, once per outage."
          checked={settings.alertsOnFailure}
          onChange={(alertsOnFailure) => updateSettings({ alertsOnFailure })}
        />
      </Group>
    </div>
  );
}

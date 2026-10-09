import { t } from "../../shared/i18n";
import { updateSettings, type AppSettings } from "../../shared/settings";
import { Select } from "../controls";
import { Group, Row } from "../Group";
import { ToggleRow } from "./ToggleRow";

/** Upstream `AlertThreshold`: off, or the share of a limit at which to speak up. 0 stands for off. */
const steps = [0, 75, 80, 90, 95].map((v) => ({ value: v, label: v === 0 ? t("Off") : `${v}%` }));

/**
 * What Pulse may say unprompted (upstream `NotificationsPane`). Every switch starts off; the rules that
 * decide when each one speaks live in `pulse_core::alerts` (limits), `pulse_core::outage` (a service is
 * down) and `pulse_core::recap::periods::notice` (a recap is ready).
 *
 * The low-balance line is per account and lives in the account pane (`LowBalanceGroup`), as upstream's does.
 * Windows has no notification permission to ask for, so upstream's "Notifications need the bundled app" and
 * "Turned off for Pulse in System Settings" subtitles do not apply: a toast the user has muted in Windows
 * Settings is simply not shown, and the sound is Windows' own per-app switch.
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
          // Nothing to fire about: a reset is only announced for a window that was mentioned on the way up.
          disabled={threshold === 0}
          onChange={(alertsOnReset) => updateSettings({ alertsOnReset })}
        />
        <ToggleRow
          title="When a reading stops arriving"
          subtitle="After several failed checks in a row, once per outage."
          checked={settings.alertsOnFailure}
          onChange={(alertsOnFailure) => updateSettings({ alertsOnFailure })}
        />
        <ToggleRow
          title="When a service is down"
          subtitle="Codex, Claude Code and DeepSeek, from their own status pages — only the ones you have switched on."
          checked={settings.alertsOnOutage}
          // The status pages, not the readings: an outage already under way is said now (the app checks
          // them itself when this changes), and switching off forgets what was said.
          onChange={(alertsOnOutage) => updateSettings({ alertsOnOutage })}
        />
        <ToggleRow
          title="When a recap is ready"
          subtitle="Last month's recap, at the start of the month. Needs Token spend reading."
          checked={settings.alertsOnRecap}
          // The month is only known from records Pulse has read, but a switch that is on can always be
          // turned off, whatever Token spend reading says.
          disabled={!settings.readsTokenSpend && !settings.alertsOnRecap}
          onChange={(alertsOnRecap) => updateSettings({ alertsOnRecap })}
        />
      </Group>
    </div>
  );
}

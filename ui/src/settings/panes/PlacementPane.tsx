import { t } from "../../shared/i18n";
import { updateSettings, type AppSettings, type ProviderInfo } from "../../shared/settings";
import { Button, Segmented } from "../controls";
import { Group, Row } from "../Group";
import { ProviderIcon } from "../../panel/Icon";
import { allAccounts, enabledInOrder } from "../panes";
import { ToggleRow } from "./ToggleRow";

const positions = ["Left", "Top", "Bottom", "Free across", "Free upright", "Right"];

/** Whether the rail is shown, where it sits, how it tucks away, and the order of the rings. */
export function PlacementPane({ settings, providers }: { settings: AppSettings; providers: ProviderInfo[] }) {
  const off = !settings.isPanelVisible;
  const accounts = allAccounts(settings, providers);
  const shown = enabledInOrder(settings)
    .map((id) => accounts.find((a) => a.id === id))
    .filter((a) => a !== undefined);
  return (
    <div className="pane-stack">
      <Group>
        <ToggleRow
          title="Show floating panel"
          subtitle="The usage rail at the edge of the screen."
          checked={settings.isPanelVisible}
          onChange={(isPanelVisible) => updateSettings({ isPanelVisible })}
        />
        <ToggleRow
          title="Hide in full screen"
          subtitle="Keep the floating panel out of full-screen apps."
          checked={settings.hidesInFullScreen}
          disabled={off}
          onChange={(hidesInFullScreen) => updateSettings({ hidesInFullScreen })}
        />
        <ToggleRow
          title="Hide until pointed at"
          subtitle="Against a screen edge, the rail shrinks to a sliver until you point at it."
          checked={settings.autoCollapse}
          disabled={off}
          onChange={(autoCollapse) => updateSettings({ autoCollapse })}
        />
        {/* TODO(opus): placement is owned elsewhere; enable once the dock state is exposed over IPC. */}
        <Row title={t("Position")} subtitle={t("Drag it anywhere; near an edge it snaps on.")} disabled>
          <Segmented label={t("Position")} disabled value="Right" options={positions.map((p) => ({ value: p, label: t(p) }))} onChange={() => {}} />
        </Row>
        <ToggleRow
          title="Follow the active display"
          subtitle="With more than one display, the rail moves to the one the pointer is on."
          checked={settings.followsActiveDisplay}
          disabled={off}
          onChange={(followsActiveDisplay) => updateSettings({ followsActiveDisplay })}
        />
      </Group>

      {/* TODO(opus): the Order list drives rail placement, which is owned elsewhere; drawn disabled until then. */}
      <Group title={t("Order")}>
        {shown.map((a) => (
          <Row key={a.id} title={a.label} icon={<ProviderIcon provider={a.provider} size={18} />} disabled>
            <Button label={t("Move %@ up", a.label)} disabled onClick={() => {}}>↑</Button>
            <Button label={t("Move %@ down", a.label)} disabled onClick={() => {}}>↓</Button>
          </Row>
        ))}
        <Row title={t("Reset order")} subtitle={t("Back to the order Pulse ships with.")} disabled>
          <Button disabled onClick={() => {}}>{t("Reset")}</Button>
        </Row>
      </Group>
    </div>
  );
}

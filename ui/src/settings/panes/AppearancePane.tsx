import { t } from "../../shared/i18n";
import { updateSettings, type AppSettings } from "../../shared/settings";
import { Segmented, Slider } from "../controls";
import { Group, Row } from "../Group";
import { ToggleRow } from "./ToggleRow";

/** How the rail and card look: size, spacing, the ends, the surface. */
export function AppearancePane({ settings }: { settings: AppSettings }) {
  const off = !settings.isPanelVisible;
  return (
    <div className="pane-stack">
      <Group>
        <Row title={t("Size")} subtitle={t("Size of the rail on screen.")} disabled={off}>
          <Segmented
            label={t("Size")}
            disabled={off}
            value={settings.panelSize}
            options={[
              { value: "small", label: t("Small") },
              { value: "standard", label: t("Standard") },
              { value: "large", label: t("Large") },
            ]}
            onChange={(panelSize) => updateSettings({ panelSize })}
          />
        </Row>
        <Row title={t("Spacing")} subtitle={t("How much air there is between the rings.")} disabled={off}>
          <Segmented
            label={t("Spacing")}
            disabled={off}
            value={settings.railSpacing}
            options={[
              { value: "compact", label: t("Tight") },
              { value: "standard", label: t("Standard") },
              { value: "roomy", label: t("Loose") },
            ]}
            onChange={(railSpacing) => updateSettings({ railSpacing })}
          />
        </Row>
        <ToggleRow
          title="Round ends"
          subtitle="The rail's ends and the card's tail follow the ring's own curve."
          checked={settings.usesRoundEnds}
          disabled={off}
          onChange={(usesRoundEnds) => updateSettings({ usesRoundEnds })}
        />
        {/* Always shown, and greyed out under glass rather than hidden: glass is always drawn dark,
            and a row that vanishes is one nobody finds again. */}
        <Row title={t("Panel colour")} subtitle={t("Light suits a bright screen. Not available with Liquid Glass.")} disabled={off || settings.usesGlass}>
          <Segmented
            label={t("Panel colour")}
            disabled={off || settings.usesGlass}
            value={settings.usesLightPanel ? "light" : "black"}
            options={[
              { value: "black", label: t("Black") },
              { value: "light", label: t("Light") },
            ]}
            onChange={(choice) => updateSettings({ usesLightPanel: choice === "light" })}
          />
        </Row>
        <ToggleRow
          title="Liquid Glass"
          subtitle="Clear glass that shows what is behind the panel, instead of a solid surface."
          checked={settings.usesGlass}
          disabled={off}
          onChange={(usesGlass) => updateSettings({ usesGlass })}
        />
        {settings.usesGlass && (
          <ToggleRow
            title="Glitter"
            subtitle="Specks of glitter that slowly twinkle."
            checked={settings.glassGlitter}
            disabled={off}
            onChange={(glassGlitter) => updateSettings({ glassGlitter })}
          />
        )}
        {settings.usesGlass && (
          <Row title={t("Transparency")} subtitle={t("Clearer to the right. Darker reads better over bright pages.")} disabled={off}>
            <Slider
              label={t("Transparency")}
              disabled={off}
              value={settings.glassTransparency}
              onChange={(glassTransparency) => updateSettings({ glassTransparency })}
            />
          </Row>
        )}
        <ToggleRow
          title="Ring activity animation"
          subtitle="The turning mark for a working CLI or a reading being fetched. Off leaves the ring still."
          checked={settings.animatesRingActivity}
          disabled={off}
          onChange={(animatesRingActivity) => updateSettings({ animatesRingActivity })}
        />
      </Group>
    </div>
  );
}

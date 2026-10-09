import { setLanguage } from "../../shared/i18n";
import { t } from "../../shared/i18n";
import { updateSettings, type AppLanguage, type AppSettings } from "../../shared/settings";
import { Select } from "../controls";
import { Group, Row } from "../Group";
import { ToggleRow } from "./ToggleRow";

/** Pulse as an app rather than as a panel: launch, tray icon, language. */
export function GeneralPane({ settings }: { settings: AppSettings }) {
  // Languages are listed in their own language; only "System" is translated.
  const languages: { value: AppLanguage; label: string }[] = [
    { value: "system", label: t("System") },
    { value: "en", label: "English" },
    { value: "zh-Hans", label: "简体中文" },
    { value: "zh-Hant", label: "繁體中文" },
    { value: "ja", label: "日本語" },
    { value: "ko", label: "한국어" },
  ];

  return (
    <div className="pane-stack">
      <Group title={t("Application")}>
        <ToggleRow
          title="Open at login"
          subtitle="Start Pulse automatically when you log in."
          checked={settings.launchAtLogin}
          onChange={(launchAtLogin) => updateSettings({ launchAtLogin })}
        />
        <ToggleRow
          title="Hide menu bar icon"
          subtitle="Remove Pulse from the menu bar; use the panel menu or shortcut to open settings."
          checked={settings.hidesTrayIcon}
          onChange={(hidesTrayIcon) => updateSettings({ hidesTrayIcon })}
        />
        <ToggleRow
          title="Usage panel in the menu"
          subtitle="Opens the menu bar menu on an overview of every account, with a tab for each one's limits, plan and spend."
          checked={settings.showsMenuDashboard}
          disabled={settings.hidesTrayIcon}
          onChange={(showsMenuDashboard) => updateSettings({ showsMenuDashboard })}
        />
      </Group>

      <Group title={t("Language")}>
        <Row title={t("Interface language")} subtitle={t("Takes effect right away.")}>
          <Select
            label={t("Interface language")}
            value={settings.language}
            options={languages}
            onChange={(language) => {
              setLanguage(language);
              updateSettings({ language });
            }}
          />
        </Row>
      </Group>
    </div>
  );
}

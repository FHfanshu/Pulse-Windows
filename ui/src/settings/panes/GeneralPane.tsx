// Ported from upstream Settings/GeneralPane.swift.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { setLanguage } from "../../shared/i18n";
import { t } from "../../shared/i18n";
import { updateSettings, type AppLanguage, type AppSettings } from "../../shared/settings";
import { providerNames } from "../../panel/Icon";
import { Segmented, Select } from "../controls";
import { Group, Row } from "../Group";
import { ShortcutField } from "../ShortcutField";
import { ToggleRow } from "./ToggleRow";

type ShortcutStatus = { openSettings: boolean | null; togglePanel: boolean | null };

/** Pulse as an app rather than as a panel: launch, tray icon, shortcuts, language. */
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

  // Whether each stored shortcut was accepted by the system; re-read whenever they change.
  const [status, setStatus] = useState<ShortcutStatus>({ openSettings: null, togglePanel: null });
  useEffect(() => {
    invoke<ShortcutStatus>("shortcut_status").then(setStatus).catch(() => {});
  }, [settings.openSettingsShortcut, settings.togglePanelShortcut]);

  const shortcutSubtitle = (accepted: boolean | null, available: string) =>
    accepted === false ? t("Another app is already using this combination.") : t(available);

  // An account switched off falls back to the fullest ring, so only shown accounts are offered.
  const label = (id: string) =>
    settings.extraAccounts.find((a) => a.id === id)?.name || providerNames[id] || id;
  const trayAccount = settings.trayAccount && settings.enabledAccounts.includes(settings.trayAccount) ? settings.trayAccount : "";
  const trayAccounts = [
    { value: "", label: t("Fullest ring") },
    ...settings.enabledAccounts.map((id) => ({ value: id, label: label(id) })),
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
          onChange={(hidesTrayIcon) => {
            // Hiding the last way in brings the panel back first (upstream `menuBarIconMustRemainVisible`).
            const shortcutWorks = status.openSettings === true || status.togglePanel === true;
            if (hidesTrayIcon && !settings.isPanelVisible && !shortcutWorks) {
              if (settings.enabledAccounts.length === 0) return;
              updateSettings({ hidesTrayIcon, isPanelVisible: true });
            } else {
              updateSettings({ hidesTrayIcon });
            }
          }}
        />
        <ToggleRow
          title="Show usage in the menu bar"
          subtitle="A ring's mark and figure beside the icon, red past the warning line."
          checked={settings.showsUsageInTray}
          disabled={settings.hidesTrayIcon}
          onChange={(showsUsageInTray) => updateSettings({ showsUsageInTray })}
        />
        {settings.showsUsageInTray && !settings.hidesTrayIcon && (
          <>
            <Row title={t("Menu bar shows")} subtitle={t("An account switched off falls back to the fullest ring.")}>
              <Select
                label={t("Menu bar shows")}
                value={trayAccount}
                options={trayAccounts}
                onChange={(id) => updateSettings({ trayAccount: id || null })}
              />
            </Row>
            <Row title={t("Menu bar style")}>
              <Segmented
                label={t("Menu bar style")}
                value={settings.trayStyle}
                options={[
                  { value: "figure", label: t("Figure") },
                  { value: "ring", label: t("Ring") },
                  { value: "split", label: t("Split") },
                ]}
                onChange={(trayStyle) => updateSettings({ trayStyle })}
              />
            </Row>
          </>
        )}
      </Group>

      <Group title={t("Shortcuts")}>
        <Row
          title={t("Open settings")}
          subtitle={shortcutSubtitle(status.openSettings, "Reaches this window with the menu bar icon out of sight.")}
        >
          <ShortcutField
            shortcut={settings.openSettingsShortcut}
            onChange={(openSettingsShortcut) => updateSettings({ openSettingsShortcut })}
          />
        </Row>
        <Row
          title={t("Show or hide the panel")}
          subtitle={shortcutSubtitle(status.togglePanel, "Draws the usage rail, or takes it away.")}
        >
          <ShortcutField
            shortcut={settings.togglePanelShortcut}
            onChange={(togglePanelShortcut) => updateSettings({ togglePanelShortcut })}
          />
        </Row>
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

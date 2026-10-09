// Ported from upstream Settings/AboutPane.swift.
//
// Windows differences: no Sparkle window, so the pane carries the whole update: clicking "Update to X"
// downloads the installer, runs it in its passive mode (a progress bar, no pages) and Pulse starts again
// by itself. The source code row points at this port, and the credits add the app it is based on; the
// animated bot (upstream's "Morph Bot" credit) is not part of this port.
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { updateSettings, type AppSettings } from "../../shared/settings";
import { checkForUpdate, installUpdate, useUpdate, type UpdateState } from "../../shared/update";
import { Button } from "../controls";
import { Group, Row } from "../Group";
import { ToggleRow } from "./ToggleRow";

const open = (url: string) => void invoke("open_external", { url });

export function AboutPane({ settings }: { settings: AppSettings }) {
  const update = useUpdate();
  const [version, setVersion] = useState("");
  useEffect(() => { getVersion().then(setVersion).catch(() => {}); }, []);
  // Without this the row says "up to date" on nothing but the last answer, however old. Quiet: the
  // subtitle is where the result goes.
  useEffect(() => { void checkForUpdate(); }, []);

  const shown = version || update?.current || "";
  const installing = update?.stage === "downloading" || update?.stage === "installing";

  return (
    <div className="pane-stack">
      <Group>
        <Row title={t("Version")} subtitle={update ? updateSubtitle(update, shown) : undefined}>
          {update?.canCheck ? (
            <Button
              disabled={update.checking || installing}
              onClick={() => void (update.newer ? installUpdate() : checkForUpdate())}
            >
              {installing ? t("Updating…") : update.newer ? t("Update to %@", update.newer) : t("Check now")}
            </Button>
          ) : (
            <span className="value-text">{shown}</span>
          )}
        </Row>

        {update?.canCheck && (
          <ToggleRow
            title="Check automatically"
            subtitle="Every two hours. Updates are offered, never installed on their own."
            checked={settings.checksUpdatesAutomatically}
            onChange={(checksUpdatesAutomatically) => updateSettings({ checksUpdatesAutomatically })}
          />
        )}

        <Row
          title={t("Usage data")}
          subtitle={t("Read from each provider's own account. Pulse shows the figures they report; where it has to infer one, the figure itself says so.")}
        />

        {/* The address itself as the subtitle, not a sentence about it: somebody reading this pane wants to
            know where the source is, and half of them will want to type it rather than click. */}
        <Row title={t("Source code")} subtitle="github.com/FHfanshu/Pulse-Windows">
          <Button onClick={() => open("https://github.com/FHfanshu/Pulse-Windows")}>{t("Open")}</Button>
        </Row>
      </Group>

      <Group title={t("Credits")}>
        <Row title="qunqin24/Pulse" subtitle={t("This Windows port is based on Pulse by qunqin24 (Apache-2.0).")}>
          <Button onClick={() => open("https://github.com/qunqin24/Pulse")}>{t("Open")}</Button>
        </Row>
        <Row title="Vinz (@hivinz_)" subtitle={t("Panel design inspired by Vinz's work shared on X.")}>
          <Button onClick={() => open("https://x.com/hivinz_/status/2092996055248126353")}>{t("Open")}</Button>
        </Row>
        <Row title="Lobe Icons" subtitle={t("Provider marks from github.com/lobehub/lobe-icons.")} />
      </Group>
    </div>
  );
}

/** The version, and what is known about a newer one. All the states are distinguishable on purpose:
 *  "no update" and "couldn't ask" look identical otherwise, and a check that silently failed is worse
 *  than one that says so. */
function updateSubtitle(update: UpdateState, version: string): string {
  if (update.stage === "downloading") {
    const fraction = update.total ? Math.min(1, update.downloaded / update.total) : null;
    return fraction === null
      ? t("Downloading the update…")
      : t("Downloading the update… %@", `${Math.floor(fraction * 100)}%`);
  }
  if (update.stage === "installing") return t("Installing the update. Pulse restarts by itself.");
  if (update.stage === "failed") return t("Couldn't install the update. Try again.");
  if (update.newer) return t("%@ installed · %@ available", version, update.newer);
  if (update.checking) return t("Checking…");
  if (update.failed) return t("Couldn't reach the update feed.");
  if (!update.canCheck) return t("Built from source — no update check.");
  return t("%@ · up to date", version);
}

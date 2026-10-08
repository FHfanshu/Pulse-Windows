import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useMemo, useState } from "react";
import { ProviderIcon } from "../panel/Icon";
import { setLanguage, t } from "../shared/i18n";
import { updateSettings, useSettings, type ProviderInfo } from "../shared/settings";
import { apiProviders } from "../settings/panes";
import { accessDescriptions } from "./access";

const closeWindow = () => void getCurrentWindow().close();

/** Detected first, then by name (upstream `ProviderSetupWindowController`). */
function arranged(list: ProviderInfo[], detected: Set<string>): ProviderInfo[] {
  return [...list].sort((a, b) => {
    const left = detected.has(a.id);
    const right = detected.has(b.id);
    if (left !== right) return left ? -1 : 1;
    return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
  });
}

/** The provider chooser: nothing is monitored until at least one service is ticked and Done is pressed. */
export function Chooser() {
  const settings = useSettings();
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [detected, setDetected] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<Set<string>>(new Set());

  useEffect(() => {
    invoke<ProviderInfo[]>("list_providers").then(setProviders);
    invoke<string[]>("detect_providers").then((ids) => setDetected(new Set(ids)));
  }, []);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") closeWindow(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const language = settings?.language;
  if (settings) setLanguage(settings.language);
  useEffect(() => { document.title = t("Choose services to monitor"); }, [language]);

  const groups = useMemo(() => {
    const subscriptions = providers.filter((p) => !apiProviders.has(p.id));
    const api = providers.filter((p) => apiProviders.has(p.id));
    return [
      { title: t("Subscriptions"), items: arranged(subscriptions, detected) },
      { title: t("API and pay-as-you-go"), items: arranged(api, detected) },
    ].filter((g) => g.items.length > 0);
    // `language` is a dependency because the group titles are looked up here.
  }, [providers, detected, language]);

  if (!settings) return null;
  const anyDetected = providers.some((p) => detected.has(p.id));
  const initial = settings.enabledAccounts.length === 0;

  const toggle = (id: string, on: boolean) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (on) next.add(id); else next.delete(id);
      return next;
    });

  const finish = async () => {
    if (initial && selected.size === 0) return;
    const chosen = providers.map((p) => p.id).filter((id) => selected.has(id));
    await updateSettings({
      enabledAccounts: [...settings.enabledAccounts, ...chosen.filter((id) => !settings.enabledAccounts.includes(id))],
      offeredProviders: providers.map((p) => p.id),
    });
    closeWindow();
  };

  return (
    <div className="chooser">
      <header className="chooser-head">
        <h1>{initial ? t("Choose services to monitor") : t("New services detected")}</h1>
        <p>
          {initial
            ? t("Choose at least one service. Pulse reads its login and checks usage only after you finish. You can change this later in Settings.")
            : t("This version supports more services found on your Mac. Choose any you want to add; your existing choices stay in place.")}
        </p>
      </header>

      <div className="chooser-list">
        {groups.map((group, index) => (
          <section key={group.title}>
            <h2 className={index === 0 ? "first" : undefined}>{group.title}</h2>
            {group.items.map((p) => (
              <label key={p.id} className="chooser-row">
                <input
                  type="checkbox"
                  checked={selected.has(p.id)}
                  onChange={(e) => toggle(p.id, e.target.checked)}
                />
                <span className="mark"><ProviderIcon provider={p.id} size={20} /></span>
                <span className="body">
                  <span className="line">
                    <span className="name">{p.name}</span>
                    {detected.has(p.id) && <span className="hint">{t("Detected on this PC")}</span>}
                  </span>
                  {accessDescriptions[p.id] && <span className="access">{t(accessDescriptions[p.id])}</span>}
                </span>
              </label>
            ))}
          </section>
        ))}
      </div>

      <footer className="chooser-foot">
        <button
          type="button"
          className="btn"
          disabled={!anyDetected}
          onClick={() => setSelected((prev) => new Set([...prev, ...providers.map((p) => p.id).filter((id) => detected.has(id))]))}
        >
          {t("Select detected services")}
        </button>
        <span className="spacer" />
        <button type="button" className="btn" onClick={closeWindow}>{t("Not now")}</button>
        <button type="button" className="btn primary" disabled={initial && selected.size === 0} onClick={() => void finish()}>
          {t("Done")}
        </button>
      </footer>
    </div>
  );
}

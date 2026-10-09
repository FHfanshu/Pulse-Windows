import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import { setLanguage } from "../shared/i18n";
import { useSettings, type AppSettings, type ProviderInfo } from "../shared/settings";
import { accountOf, buildSections, type PaneId } from "./panes";
import { PaneGlyph, Sidebar } from "./Sidebar";
import { AboutPane } from "./panes/AboutPane";
import { AccountPane } from "./panes/AccountPane";
import { AppearancePane } from "./panes/AppearancePane";
import { GeneralPane } from "./panes/GeneralPane";
import { NetworkPane } from "./panes/NetworkPane";
import { NotificationsPane } from "./panes/NotificationsPane";
import { PlacementPane } from "./panes/PlacementPane";
import { RingsPane } from "./panes/RingsPane";
import { TokenSpendPane } from "./panes/TokenSpendPane";

/** The settings window: sidebar on the left, the selected pane under its heading on the right. */
export function Shell() {
  const settings = useSettings();
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [selected, setSelected] = useState<PaneId>("appearance");
  const [query, setQuery] = useState("");
  const content = useRef<HTMLDivElement>(null);

  useEffect(() => { invoke<ProviderInfo[]>("list_providers").then(setProviders); }, []);
  useEffect(() => { content.current?.scrollTo({ top: 0 }); }, [selected]);

  // The language is applied while rendering so every `t()` below already reads the new table.
  if (settings) setLanguage(settings.language);
  const language = settings?.language;

  const sections = useMemo(
    () => (settings ? buildSections(settings, providers) : []),
    // `language` is a dependency because titles are looked up when the sections are built.
    [settings, providers, language],
  );

  if (!settings) return null;
  const item = sections.flatMap((s) => s.items).find((i) => i.id === selected);
  const account = accountOf(selected);

  return (
    <div className="shell">
      <Sidebar sections={sections} selected={selected} query={query} onQuery={setQuery} onSelect={setSelected} />
      <main className="content" ref={content}>
        <div className="pane">
          <h2 className="pane-heading">
            {item && account && <span className="glyph"><PaneGlyph icon={item.icon} size={26} /></span>}
            {item?.title}
          </h2>
          {account ? (
            <AccountPane key={account} id={account} settings={settings} onNavigate={setSelected} />
          ) : (
            fixedPane(selected, settings, providers)
          )}
        </div>
      </main>
    </div>
  );
}

function fixedPane(id: PaneId, settings: AppSettings, providers: ProviderInfo[]) {
  switch (id) {
    case "appearance": return <AppearancePane settings={settings} />;
    case "rings": return <RingsPane settings={settings} />;
    case "placement": return <PlacementPane settings={settings} providers={providers} />;
    case "spend": return <TokenSpendPane settings={settings} />;
    case "general": return <GeneralPane settings={settings} />;
    case "notifications": return <NotificationsPane settings={settings} />;
    case "network": return <NetworkPane settings={settings} />;
    case "about": return <AboutPane />;
    default: return null;
  }
}

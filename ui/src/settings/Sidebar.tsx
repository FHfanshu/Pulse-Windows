import { ProviderIcon } from "../panel/Icon";
import { t } from "../shared/i18n";
import { Glyph } from "./Glyph";
import { matches, type PaneIcon, type PaneId, type SidebarSection } from "./panes";

export function PaneGlyph({ icon, size = 18 }: { icon: PaneIcon; size?: number }) {
  return "glyph" in icon ? <Glyph name={icon.glyph} /> : <ProviderIcon provider={icon.provider} size={size} />;
}

interface Props {
  sections: SidebarSection[];
  selected: PaneId;
  query: string;
  onQuery: (q: string) => void;
  onSelect: (id: PaneId) => void;
}

/** Search field and the source list. A search hides non-matching panes but never clears the selection. */
export function Sidebar({ sections, selected, query, onQuery, onSelect }: Props) {
  const visible = sections
    .map((s) => ({ ...s, items: s.items.filter((i) => matches(i, query)) }))
    .filter((s) => s.items.length > 0);

  return (
    <nav className="sidebar" aria-label="Settings">
      <label className="search">
        <Glyph name="search" />
        <input type="search" value={query} placeholder={t("Search")} aria-label={t("Search")} onChange={(e) => onQuery(e.target.value)} />
      </label>
      <div className="source-list">
        {visible.length === 0 && <div className="no-matches">{t("No matches")}</div>}
        {visible.map((section, index) => (
          <div key={section.title ?? `section-${index}`}>
            {section.title && <div className="source-heading">{section.title}</div>}
            {section.items.map((item) => (
              <button
                key={item.id}
                type="button"
                className={`source-row${item.id === selected ? " selected" : ""}`}
                aria-current={item.id === selected ? "page" : undefined}
                onClick={() => onSelect(item.id)}
              >
                <span className="glyph"><PaneGlyph icon={item.icon} /></span>
                <span className="name">{item.title}</span>
              </button>
            ))}
          </div>
        ))}
      </div>
    </nav>
  );
}

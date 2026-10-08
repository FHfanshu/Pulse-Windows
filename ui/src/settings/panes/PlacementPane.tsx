import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState, type PointerEvent } from "react";
import { t } from "../../shared/i18n";
import { updateSettings, type AppSettings, type ProviderInfo } from "../../shared/settings";
import { Button, Segmented } from "../controls";
import { Group, Row } from "../Group";
import { ProviderIcon } from "../../panel/Icon";
import { allAccounts, enabledInOrder } from "../panes";
import { ToggleRow } from "./ToggleRow";

type Edge = "left" | "right" | "top" | "bottom";
type Dock = { type: "edge"; value: Edge } | { type: "floating"; value: boolean };
interface Placement { dock: Dock }

/** Segments in the order upstream lists them; `label` is the upstream key. */
const positions = [
  { value: "left", label: "Left", dock: { type: "edge", value: "left" } },
  { value: "top", label: "Top", dock: { type: "edge", value: "top" } },
  { value: "bottom", label: "Bottom", dock: { type: "edge", value: "bottom" } },
  { value: "across", label: "Free across", dock: { type: "floating", value: false } },
  { value: "upright", label: "Free upright", dock: { type: "floating", value: true } },
  { value: "right", label: "Right", dock: { type: "edge", value: "right" } },
] as const;

const keyOf = (dock: Dock) =>
  positions.find((p) => p.dock.type === dock.type && p.dock.value === dock.value)?.value ?? "right";

/** The rail's dock, kept current from `placement-changed` (also fired when the rail is dragged). */
function useDock(): Dock | null {
  const [dock, setDock] = useState<Dock | null>(null);
  useEffect(() => {
    let live = true;
    invoke<Placement>("get_placement").then((p) => { if (live) setDock(p.dock); });
    const un = listen<Placement>("placement-changed", (e) => setDock(e.payload.dock));
    return () => { live = false; void un.then((f) => f()); };
  }, []);
  return dock;
}

/** Whether the rail is shown, where it sits, how it tucks away, and the order of the rings. */
export function PlacementPane({ settings, providers }: { settings: AppSettings; providers: ProviderInfo[] }) {
  const off = !settings.isPanelVisible;
  const dock = useDock();
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
        <Row title={t("Position")} subtitle={t("Drag it anywhere; near an edge it snaps on.")} wrap>
          <Segmented
            label={t("Position")}
            disabled={!dock}
            value={dock ? keyOf(dock) : "right"}
            options={positions.map((p) => ({ value: p.value, label: t(p.label) }))}
            onChange={(value) => {
              const picked = positions.find((p) => p.value === value);
              if (picked) void invoke("set_dock", { dock: picked.dock });
            }}
          />
        </Row>
        <ToggleRow
          title="Follow the active display"
          subtitle="With more than one display, the rail moves to the one the pointer is on."
          checked={settings.followsActiveDisplay}
          disabled={off}
          onChange={(followsActiveDisplay) => updateSettings({ followsActiveDisplay })}
        />
      </Group>
      <OrderGroup settings={settings} providers={providers} />
    </div>
  );
}

const Chevron = ({ up }: { up: boolean }) => (
  <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    <path d={up ? "M2.5 7.5 6 4l3.5 3.5" : "M2.5 4.5 6 8l3.5-3.5"} />
  </svg>
);

/** Upstream `AppSettings.move(_:onto:)`: the dragged row takes the target's place. */
function moveOnto(order: string[], dragged: string, target: string): string[] {
  const at = order.indexOf(target);
  if (at < 0 || !order.includes(dragged)) return order;
  const rest = order.filter((id) => id !== dragged);
  rest.splice(at, 0, dragged);
  return rest;
}

/** The accounts the rail draws, in rail order. Drag a row, or use the arrows. */
function OrderGroup({ settings, providers }: { settings: AppSettings; providers: ProviderInfo[] }) {
  const accounts = allAccounts(settings, providers);
  const byId = new Map(accounts.map((a) => [a.id, a]));
  const order = enabledInOrder(settings).filter((id) => byId.has(id));

  const [drag, setDrag] = useState<{ id: string; over: string } | null>(null);
  const press = useRef<{ id: string; y: number; moved: boolean } | null>(null);
  const rows = useRef(new Map<string, HTMLElement>());

  // What the rail shows is saved in front; switched-off accounts keep their place behind it.
  const save = (next: string[]) =>
    updateSettings({ providerOrder: [...next, ...settings.providerOrder.filter((id) => !next.includes(id))] });

  const step = (id: string, by: number) => {
    const from = order.indexOf(id);
    const to = from + by;
    if (from < 0 || to < 0 || to >= order.length) return;
    const next = [...order];
    [next[from], next[to]] = [next[to], next[from]];
    void save(next);
  };

  /** The row at this height; above the first or below the last counts as that row. */
  const rowAt = (y: number): string | null => {
    let found: string | null = null;
    for (const id of order) {
      const box = rows.current.get(id)?.getBoundingClientRect();
      if (!box) continue;
      if (y < box.top) return found ?? id;
      found = id;
      if (y <= box.bottom) return id;
    }
    return found;
  };

  // Pointer events rather than HTML drag and drop: the webview's file-drop handling swallows the latter.
  const down = (e: PointerEvent<HTMLElement>, id: string) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    press.current = { id, y: e.clientY, moved: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const move = (e: PointerEvent<HTMLElement>) => {
    const p = press.current;
    if (!p || (!p.moved && Math.abs(e.clientY - p.y) < 4)) return;
    p.moved = true;
    const over = rowAt(e.clientY);
    if (over) setDrag((d) => (d?.id === p.id && d.over === over ? d : { id: p.id, over }));
  };
  const up = () => {
    const p = press.current;
    press.current = null;
    if (p?.moved && drag && drag.over !== p.id) void save(moveOnto(order, p.id, drag.over));
    setDrag(null);
  };
  const cancel = () => { press.current = null; setDrag(null); };

  // What the rail does with no saved order: enabled accounts by id.
  const shipped = [...settings.enabledAccounts].sort();
  const custom = order.some((id, i) => id !== shipped[i]);

  return (
    <Group title={t("Order")}>
      {order.map((id, index) => {
        const a = byId.get(id)!;
        return (
          <div
            key={id}
            className="order-row"
            ref={(el) => { if (el) rows.current.set(id, el); else rows.current.delete(id); }}
            data-over={drag && drag.over === id && drag.id !== id ? "true" : undefined}
            data-dragging={drag?.id === id ? "true" : undefined}
            onPointerDown={(e) => down(e, id)}
            onPointerMove={move}
            onPointerUp={up}
            onPointerCancel={cancel}
          >
            <Row title={a.label} icon={<ProviderIcon provider={a.provider} size={18} />}>
              <Button label={t("Move %@ up", a.label)} disabled={index === 0} onClick={() => step(id, -1)}>
                <span className="icon-glyph"><Chevron up /></span>
              </Button>
              <Button label={t("Move %@ down", a.label)} disabled={index === order.length - 1} onClick={() => step(id, 1)}>
                <span className="icon-glyph"><Chevron up={false} /></span>
              </Button>
            </Row>
          </div>
        );
      })}
      <Row title={t("Reset order")} subtitle={t("Back to the order Pulse ships with.")}>
        <Button disabled={!custom} onClick={() => void updateSettings({ providerOrder: [] })}>{t("Reset")}</Button>
      </Row>
    </Group>
  );
}

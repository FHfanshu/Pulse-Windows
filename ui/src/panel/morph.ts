// Windows difference: upstream draws the rail in one SwiftUI view whose shape animates with
// `.animation(.spring(response: 0.32, dampingFraction: 0.86), value: isExpanded)` (BerthShape's
// `openness` is an `animatableData`), so docking, undocking and turning the rail are never a cut.
// Pulse's rail is placed by Rust and reported as a layout, so the same morph is done here: when the
// layout changes the rail's shape (dock <-> float, a turn between axes), one spring runs from the
// outline that was on screen to the new one and everything drawn follows it: size, position within
// the window, how docked the outline is, and where each ring sits.
import { animate } from "motion/react";
import { useEffect, useReducer, useRef } from "react";
import { axisOf, type Edge } from "./layout";
import { spring } from "./tint";

export type Vec = [number, number];
interface Rect { x: number; y: number; w: number; h: number }

export interface RailTarget {
  frame: Rect;
  rail: Rect;
  edge: Edge;
  docked: boolean;
  /** Rust grew the window to hold the old shape too, so there is room to animate in. */
  morph?: boolean;
}

/** What to draw this frame. `x`/`y` are window-local; `items` are ring-item centres from the rail's centre. */
export interface Shown {
  x: number;
  y: number;
  w: number;
  h: number;
  /** 1 = docked and flared into the screen edge, 0 = a floating capsule. */
  d: number;
  /** The edge the outline flares into (only matters while `d` > 0). */
  edge: Edge;
  items: Vec[];
  morphing: boolean;
}

interface Run {
  /** The outline that was showing, as an offset from where the new one is (all in screen DIPs). */
  dx: number;
  dy: number;
  dw: number;
  dh: number;
  d0: number;
  edge0: Edge;
  items0: Vec[];
}

interface Snapshot extends Omit<Shown, "morphing"> {
  /** Window origin on screen when this was drawn. */
  origin: Vec;
}

/**
 * Dockedness and facing edge at progress `p` of a morph from an outline that was `from.d` docked on
 * `from.edge`. Undocking drains the flare on the edge it was on; docking grows it on the new edge; going
 * from one docked edge to another on a different axis drains it, then grows it on the new edge.
 */
export function morphShape(from: { edge: Edge; d: number }, to: { edge: Edge; docked: boolean }, p: number): { edge: Edge; d: number } {
  const clamp = (v: number) => Math.min(Math.max(v, 0), 1);
  if (!to.docked) return { edge: from.edge, d: clamp(from.d * (1 - p)) };
  if (from.d <= 0 || from.edge === to.edge) return { edge: to.edge, d: clamp(from.d + (1 - from.d) * p) };
  return p < 0.5 ? { edge: from.edge, d: clamp(from.d * (1 - 2 * p)) } : { edge: to.edge, d: clamp(2 * p - 1) };
}

const shapeKey = (t: RailTarget) => `${axisOf(t.edge)}|${t.docked}`;

/**
 * The rail as it is drawn now. `size` and `items` are the target layout's; a change of shape starts a
 * spring (response 0.32, damping 0.86 like upstream) from whatever was on screen, and `onSettled` is
 * called when it comes to rest so Rust can give the window back its own size.
 */
export function useRailMorph(target: RailTarget | null, size: { w: number; h: number }, items: Vec[], onSettled: () => void): Shown | null {
  const run = useRef<Run | null>(null);
  const progress = useRef(1);
  const key = useRef<string | null>(null);
  const shown = useRef<Snapshot | null>(null);
  const generation = useRef(0);
  const settled = useRef(onSettled);
  settled.current = onSettled;
  const [, redraw] = useReducer((n: number) => n + 1, 0);

  const next = target ? shapeKey(target) : null;
  if (target && next && key.current !== null && key.current !== next) {
    // The shape changed: start from the outline last drawn (kept on screen, whatever the window did).
    const prev = shown.current;
    run.current = null;
    progress.current = 1;
    if (target.morph && prev) {
      run.current = {
        dx: prev.x + prev.origin[0] - (target.rail.x + target.frame.x),
        dy: prev.y + prev.origin[1] - (target.rail.y + target.frame.y),
        dw: prev.w - size.w,
        dh: prev.h - size.h,
        d0: prev.d,
        edge0: prev.edge,
        items0: prev.items,
      };
      progress.current = 0;
      generation.current++;
    }
  }
  if (next) key.current = next;

  // After the block above: it is what starts a run, and nothing else re-renders to start the spring.
  const started = generation.current;
  useEffect(() => {
    if (!started) return;
    const controls = animate(0, 1, {
      ...spring(0.32, 0.86),
      // Tight, because the value scales offsets of hundreds of DIPs: no visible last jump.
      restDelta: 0.0005,
      restSpeed: 0.005,
      onUpdate: (v) => {
        progress.current = v;
        redraw();
      },
      onComplete: () => {
        progress.current = 1;
        run.current = null;
        redraw();
        settled.current();
      },
    });
    return () => controls.stop();
  }, [started]);

  if (!target) return null;

  const r = run.current;
  const p = progress.current;
  const rest = r ? 1 - p : 0;
  const now = r
    ? morphShape({ edge: r.edge0, d: r.d0 }, target, p)
    : { edge: target.edge, d: target.docked ? 1 : 0 };
  const snapshot: Snapshot = {
    x: target.rail.x + (r ? r.dx * rest : 0),
    y: target.rail.y + (r ? r.dy * rest : 0),
    w: size.w + (r ? r.dw * rest : 0),
    h: size.h + (r ? r.dh * rest : 0),
    d: now.d,
    edge: now.edge,
    items: items.map((item, i): Vec => {
      const old = r?.items0[i];
      return old ? [item[0] + (old[0] - item[0]) * rest, item[1] + (old[1] - item[1]) * rest] : item;
    }),
    origin: [target.frame.x, target.frame.y],
  };
  shown.current = snapshot;
  return { ...snapshot, morphing: !!r };
}

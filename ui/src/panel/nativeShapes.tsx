// Windows difference: upstream's Liquid Glass is a SwiftUI material that takes the shape it is drawn
// with. Here the blur is a native Composition layer under the WebView (src-tauri/src/backdrop.rs),
// clipped to the outline of each shape, so the UI tells Rust where the shapes are: the rail's berth
// path and the open card's bubble path, with where each sits in the window and how visible it is.
// The rail is sent as it renders (it morphs by re-rendering every frame, morph.ts); the card is
// moved by motion's spring outside React, so its box is read back from the DOM every animation frame
// until it has been still for a while. Only changed outlines are sent.
import { invoke } from "@tauri-apps/api/core";
import { useLayoutEffect } from "react";

interface Placed {
  d: string;
  /** Window DIPs of the outline's own origin, and its scale (the card is scaled by motion). */
  x: number;
  y: number;
  s: number;
  /** Opacity (the card fades in and out). */
  o: number;
}

let rail: { d: string; x: number; y: number } | null = null;
let card: { d: string; svg: SVGSVGElement; width: number } | null = null;
let active = false;
let request = 0;
let stillFor = 0;
let sent = "";
let seq = 0;

/** Frames the card may stand still before the loop rests (until the next change wakes it). */
const REST_AFTER = 30;
const round = (v: number) => Math.round(v * 100) / 100;

function cardOutline(): Placed | null {
  if (!card || !card.svg.isConnected) return null;
  const box = card.svg.getBoundingClientRect();
  if (box.width <= 0 || card.width <= 0) return null;
  const host = card.svg.closest<HTMLElement>(".card-host");
  const o = host ? parseFloat(getComputedStyle(host).opacity) : 1;
  return { d: card.d, x: round(box.left), y: round(box.top), s: Math.round((box.width / card.width) * 10000) / 10000, o: round(Number.isFinite(o) ? o : 1) };
}

/** Sends the outlines if they differ from the last ones sent. Returns whether they did. */
function flush(): boolean {
  if (!active) return false;
  const shapes = { rail: rail ? { ...rail, x: round(rail.x), y: round(rail.y), s: 1, o: 1 } : null, card: cardOutline() };
  const key = JSON.stringify(shapes);
  if (key === sent) return false;
  sent = key;
  void invoke("set_backdrop_shapes", { shapes: { seq: ++seq, ...shapes } }).catch(() => {});
  return true;
}

function frame() {
  request = 0;
  if (!active) return;
  stillFor = flush() ? 0 : stillFor + 1;
  if (stillFor <= REST_AFTER) request = requestAnimationFrame(frame);
}

/** Something that may move a shape happened: send now, and keep watching the card. */
export function wakeNativeShapes() {
  if (!active) return;
  stillFor = 0;
  flush();
  if (!request) request = requestAnimationFrame(frame);
}

/** Whether Rust is drawing the native acrylic (so the outlines are wanted). */
export function setNativeActive(on: boolean) {
  if (on === active) return;
  active = on;
  sent = ""; // Rust may be holding nothing: say everything again
  if (on) wakeNativeShapes();
}

/** Registers the rail's outline (window coordinates of its box) for as long as it is mounted. */
export function RailOutline({ d, x, y }: { d: string; x: number; y: number }) {
  useLayoutEffect(() => {
    rail = { d, x, y };
    wakeNativeShapes();
  });
  useLayoutEffect(() => () => {
    rail = null;
    wakeNativeShapes();
  }, []);
  return null;
}

/** Registers the open card's outline: `svg` is the element the outline is drawn in, at `width` DIPs. */
export function useCardOutline(svg: React.RefObject<SVGSVGElement>, d: string, width: number) {
  useLayoutEffect(() => {
    if (!svg.current) return;
    card = { d, svg: svg.current, width };
    wakeNativeShapes();
  });
  useLayoutEffect(() => () => {
    card = null;
    wakeNativeShapes();
  }, []);
}

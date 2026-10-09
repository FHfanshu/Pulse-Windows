// Ported from upstream Panel/UsageDockView.swift (DockBerthShape) and
// Panel/UsageBubbleShape.swift. Produces SVG path strings in y-down space,
// which matches SwiftUI's own coordinate space for Shape paths.
import type { Edge } from "./layout";

type Pt = [number, number];
type Matrix = [number, number, number, number, number, number]; // a b c d tx ty

class PathBuilder {
  private parts: string[] = [];
  constructor(private m: Matrix = [1, 0, 0, 1, 0, 0]) {}
  private p([x, y]: Pt): string {
    const [a, b, c, d, tx, ty] = this.m;
    return `${+(a * x + c * y + tx).toFixed(3)} ${+(b * x + d * y + ty).toFixed(3)}`;
  }
  move(p: Pt) { this.parts.push(`M${this.p(p)}`); return this; }
  line(p: Pt) { this.parts.push(`L${this.p(p)}`); return this; }
  curve(to: Pt, c1: Pt, c2: Pt) { this.parts.push(`C${this.p(c1)} ${this.p(c2)} ${this.p(to)}`); return this; }
  close() { this.parts.push("Z"); return this; }
  toString() { return this.parts.join(""); }
}

const SQUIRCLE_EXPONENT = 4;
const CORNER_SAMPLES = 48;

/** Corner as a superellipse, sampled — upstream `appendCorner`. */
function corner(path: PathBuilder, center: Pt, r: number, from: Pt, to: Pt) {
  if (r <= 0) return;
  for (let step = 1; step <= CORNER_SAMPLES; step++) {
    const t = (step / CORNER_SAMPLES) * (Math.PI / 2);
    const along = Math.pow(Math.cos(t), 2 / SQUIRCLE_EXPONENT);
    const across = Math.pow(Math.sin(t), 2 / SQUIRCLE_EXPONENT);
    path.line([center[0] + r * (from[0] * along + to[0] * across), center[1] + r * (from[1] * along + to[1] * across)]);
  }
}

/** Circular arc between two tangents, like CoreGraphics `addArc(tangent1End:tangent2End:radius:)`. */
function tangentArc(path: PathBuilder, current: Pt, t1: Pt, t2: Pt, r: number) {
  const v1: Pt = [current[0] - t1[0], current[1] - t1[1]];
  const v2: Pt = [t2[0] - t1[0], t2[1] - t1[1]];
  const l1 = Math.hypot(...v1), l2 = Math.hypot(...v2);
  if (l1 === 0 || l2 === 0 || r <= 0) { path.line(t1); return; }
  const u1: Pt = [v1[0] / l1, v1[1] / l1], u2: Pt = [v2[0] / l2, v2[1] / l2];
  const angle = Math.acos(Math.max(-1, Math.min(1, u1[0] * u2[0] + u1[1] * u2[1])));
  const dist = r / Math.tan(angle / 2);
  const start: Pt = [t1[0] + u1[0] * dist, t1[1] + u1[1] * dist];
  const end: Pt = [t1[0] + u2[0] * dist, t1[1] + u2[1] * dist];
  path.line(start);
  // Approximate the arc with a cubic (exact for 90°, which is the only case used).
  const k = (4 / 3) * Math.tan((Math.PI - angle) / 4);
  path.curve(end, [start[0] - u1[0] * dist * k, start[1] - u1[1] * dist * k], [end[0] - u2[0] * dist * k, end[1] - u2[1] * dist * k]);
}

export interface BerthMetrics {
  flareHeight: number;
  flareWidth: number;
  cornerRadius: number;
  collapsedWidth: number;
  usesRoundEnds: boolean;
}

/** The rail's surface: flares into the docked edge; a capsule when floating. */
export function berthPath(w: number, h: number, edge: Edge, docked: boolean, openness: number, m: BerthMetrics): string {
  if (!docked) {
    const r = Math.min(w, h) / 2;
    return new PathBuilder()
      .move([r, 0]).line([w - r, 0]).curve([w, r], [w - r + r * 0.5523, 0], [w, r - r * 0.5523])
      .line([w, h - r]).curve([w - r, h], [w, h - r + r * 0.5523], [w - r + r * 0.5523, h])
      .line([r, h]).curve([0, h - r], [r - r * 0.5523, h], [0, h - r + r * 0.5523])
      .line([0, r]).curve([r, 0], [0, r - r * 0.5523], [r - r * 0.5523, 0])
      .close().toString();
  }
  let matrix: Matrix = [1, 0, 0, 1, 0, 0];
  let cw = w, ch = h;
  switch (edge) {
    case "right": break;
    case "left": matrix = [-1, 0, 0, 1, w, 0]; break;
    case "bottom": cw = h; ch = w; matrix = [0, 1, -1, 0, w, 0]; break;
    case "top": cw = h; ch = w; matrix = [0, -1, 1, 0, 0, h]; break;
  }
  return facingRight(cw, ch, openness, m, matrix);
}

function facingRight(w: number, h: number, openness: number, m: BerthMetrics, matrix: Matrix): string {
  const flareHeight = m.flareHeight * openness;
  const flareWidth = m.flareWidth * openness;
  const cornerRadius = m.collapsedWidth + (m.cornerRadius - m.collapsedWidth) * openness;
  const f = Math.min(flareHeight, h / 2);
  const r = Math.max(Math.min(cornerRadius, Math.min(w, (h - f * 2) / 2)), 0);
  const fw = Math.max(Math.min(flareWidth, w - r), 0);
  const k = 0.55;
  const p = new PathBuilder(matrix);
  p.move([r, f]).line([w - fw, f]);
  p.curve([w, 0], [w - fw * (1 - k), f], [w, f * k]);
  p.line([w, h]);
  p.curve([w - fw, h - f], [w, h - f * k], [w - fw * (1 - k), h - f]);
  p.line([r, h - f]);
  if (m.usesRoundEnds) {
    tangentArc(p, [r, h - f], [0, h - f], [0, f], r);
    p.line([0, f + r]);
    tangentArc(p, [0, f + r], [0, f], [w, f], r);
  } else {
    corner(p, [r, h - f - r], r, [0, 1], [-1, 0]);
    p.line([0, f + r]);
    corner(p, [r, f + r], r, [-1, 0], [0, -1]);
  }
  return p.close().toString();
}

export interface BubbleMetrics {
  cornerRadius: number;
  pointerWidth: number;
  pointerHeight: number;
  usesRoundEnds: boolean;
}

/**
 * Card + pointer as one outline (upstream `UsageBubbleShape`). `edge` is the
 * rail's edge: on a right rail the tail is on the card's right.
 */
export function bubblePath(w: number, h: number, edge: Edge, pointerCenter: number, m: BubbleMetrics, part: "whole" | "body" | "tail" = "whole"): string {
  if (edge === "left" || edge === "right") return sideways(w, h, edge, pointerCenter, m, part, [1, 0, 0, 1, 0, 0]);
  // Horizontal rails: draw in a rotated canonical frame, as upstream does.
  return sideways(h, w, edge, pointerCenter, m, part, [0, -1, 1, 0, 0, h]);
}

function continuousRoundedRect(p: PathBuilder, x: number, y: number, w: number, h: number, r: number, tail?: { leads: boolean; append: () => void }) {
  r = Math.min(r, w / 2, h / 2);
  const k = 0.5523;
  p.move([x + r, y]).line([x + w - r, y]).curve([x + w, y + r], [x + w - r + r * k, y], [x + w, y + r - r * k]);
  if (tail && !tail.leads) tail.append();
  p.line([x + w, y + h - r]).curve([x + w - r, y + h], [x + w, y + h - r + r * k], [x + w - r + r * k, y + h]);
  p.line([x + r, y + h]).curve([x, y + h - r], [x + r - r * k, y + h], [x, y + h - r + r * k]);
  if (tail?.leads) tail.append();
  p.line([x, y + r]).curve([x + r, y], [x, y + r - r * k], [x + r - r * k, y]).close();
}

function sideways(w: number, h: number, edge: Edge, pointerCenter: number, m: BubbleMetrics, part: string, matrix: Matrix): string {
  const tailLeads = edge === "left" || edge === "bottom";
  const body = { x: tailLeads ? m.pointerWidth : 0, w: Math.max(w - m.pointerWidth, 0) };
  const p = new PathBuilder(matrix);

  const half = m.pointerHeight / 2;
  const centre = Math.min(Math.max(pointerCenter, m.cornerRadius + half), Math.max(h - m.cornerRadius - half, m.cornerRadius + half));
  const baseX = tailLeads ? body.x : body.x + body.w;
  const tipX = tailLeads ? 0 : w;
  const reach = tipX - baseX;
  const sweep = tailLeads ? -half : half;
  const [nearAlong, nearAcross, farAlong, farAcross] = m.usesRoundEnds ? [0, 0.5, 0.55, 0.22] : [0.24, 0.44, 0.55, 0.24];

  const appendTail = () => {
    p.curve([tipX, centre], [baseX + reach * nearAlong, centre - sweep * nearAcross], [baseX + reach * farAlong, centre - sweep * farAcross]);
    p.curve([baseX, centre + sweep], [baseX + reach * farAlong, centre + sweep * farAcross], [baseX + reach * nearAlong, centre + sweep * nearAcross]);
  };
  if (part !== "tail") {
    // Windows difference: SVG strokes each subpath, including the body's edge across the tail.
    // Trace one perimeter so the upstream filled silhouette has no internal outline at the join.
    continuousRoundedRect(p, body.x, 0, body.w, h, m.cornerRadius, part === "whole" ? {
      leads: tailLeads,
      append: () => { p.line([baseX, centre - sweep]); appendTail(); },
    } : undefined);
    return p.toString();
  }

  p.move([baseX, centre - sweep]);
  appendTail();
  p.line([baseX - reach * 0.08, centre + sweep]);
  p.line([baseX - reach * 0.08, centre - sweep]);
  return p.close().toString();
}

// Checks the berth outline's morph (ui/src/panel/shapes.ts): dockedness 1 and 0 are the docked outline and the
// capsule, and every value in between is the same shape moved a little (no jump anywhere along the way).
// Run: node --experimental-transform-types scripts/check-berth.mjs   (Node 22.7+)
import { berthPath } from "../ui/src/panel/shapes.ts";

const metrics = (usesRoundEnds) =>
  usesRoundEnds
    ? { flareHeight: 32, flareWidth: 32, cornerRadius: 32, collapsedWidth: 6, usesRoundEnds }
    : { flareHeight: 24, flareWidth: 38, cornerRadius: 26, collapsedWidth: 6, usesRoundEnds };

/** The outline as points: lines as they are, cubics sampled. */
function flatten(path) {
  const points = [];
  let at = [0, 0];
  for (const [, op, args] of path.matchAll(/([MLCZ])([^MLCZ]*)/g).map((m) => [m[0], m[1], m[2]])) {
    const n = args.trim() ? args.trim().split(/[ ,]+/).map(Number) : [];
    if (op === "M" || op === "L") {
      at = [n[0], n[1]];
      points.push(at);
    } else if (op === "C") {
      const [a, b, c] = [[n[0], n[1]], [n[2], n[3]], [n[4], n[5]]];
      for (let i = 1; i <= 32; i++) {
        const t = i / 32, u = 1 - t;
        points.push([0, 1].map((k) => u ** 3 * at[k] + 3 * u * u * t * a[k] + 3 * u * t * t * b[k] + t ** 3 * c[k]));
      }
      at = c;
    }
  }
  return points;
}

/** The densest sampling of the outline's polyline, for a distance that does not depend on where the vertices fall. */
function dense(points, step = 0.25) {
  const out = [];
  for (let i = 0; i < points.length; i++) {
    const a = points[i], b = points[(i + 1) % points.length];
    const n = Math.max(1, Math.ceil(Math.hypot(b[0] - a[0], b[1] - a[1]) / step));
    for (let k = 0; k < n; k++) out.push([a[0] + ((b[0] - a[0]) * k) / n, a[1] + ((b[1] - a[1]) * k) / n]);
  }
  return out;
}

/** The largest distance from a point of one outline to the other (both ways). */
function apart(p, q, step = 1) {
  const a = dense(flatten(p), step), b = dense(flatten(q), step);
  const far = (xs, ys) => Math.max(...xs.map((x) => Math.min(...ys.map((y) => Math.hypot(x[0] - y[0], x[1] - y[1])))));
  return Math.max(far(a, b), far(b, a));
}

let failures = 0;
const check = (name, value, limit) => {
  const ok = value <= limit;
  if (!ok) failures++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}: ${value.toFixed(3)} (limit ${limit})`);
};

for (const usesRoundEnds of [false, true]) {
  const m = metrics(usesRoundEnds);
  for (const [w, h, label] of [[64, 400, "tall"], [400, 64, "wide"], [200, 220, "square-ish (mid axis turn)"]]) {
    for (const edge of ["right", "left", "top", "bottom"]) {
      const tag = `${usesRoundEnds ? "round" : "flare"} ${w}x${h} ${edge} ${label}`;
      check(`${tag}: d=0.0001 is the capsule`, apart(berthPath(w, h, edge, 0.0001, 1, m), berthPath(w, h, edge, 0, 1, m), 0.25), 0.15);
      check(`${tag}: d=0.9999 is the docked outline`, apart(berthPath(w, h, edge, 0.9999, 1, m), berthPath(w, h, edge, 1, 1, m), 0.25), 0.15);
      if (edge !== "right" && edge !== "top") continue;
      let worst = 0;
      for (let d = 0; d < 0.99; d += 0.05) worst = Math.max(worst, apart(berthPath(w, h, edge, d, 1, m), berthPath(w, h, edge, Math.min(d + 0.05, 1), 1, m)));
      check(`${tag}: biggest step over 20 steps of d`, worst, 6);
    }
  }
}
// The same command structure for every d, so a morph never changes the number of points mid-way.
const m = metrics(false);
const shape = (d) => berthPath(64, 400, "right", d, 1, m).replace(/[-\d. ]+/g, "|");
for (const d of [0.1, 0.5, 0.9]) check(`same commands at d=${d}`, shape(d) === shape(0.5) ? 0 : 1, 0);
// A boolean is still accepted.
check("true is 1", berthPath(64, 400, "right", true, 1, m) === berthPath(64, 400, "right", 1, 1, m) ? 0 : 1, 0);
check("false is 0", berthPath(64, 400, "right", false, 1, m) === berthPath(64, 400, "right", 0, 1, m) ? 0 : 1, 0);

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}

// Windows stand-in for upstream Panel/PanelSurface.swift's Liquid Glass: the window gets an acrylic
// backdrop (Rust side) and is clipped to the panel's own shapes with a window region, so the
// frosting only shows where the berth and the card are. The region follows the shapes as drawn,
// card animation included: every frame the outlines are sampled in window coordinates and sent
// only when they changed.
import { invoke } from "@tauri-apps/api/core";
import { useEffect } from "react";

/** Upstream `PanelGlass.maximumDim`: the darkest the dimming goes, at transparency 0. */
const MAXIMUM_DIM = 0.6;

export const glassDim = (transparency: number) => (1 - Math.min(Math.max(transparency, 0), 1)) * MAXIMUM_DIM;

const SAMPLES = 120;

function outline(path: SVGPathElement): [number, number][] {
  const length = path.getTotalLength();
  const ctm = path.getScreenCTM();
  if (!ctm || !(length > 0)) return [];
  const svg = path.ownerSVGElement;
  if (!svg) return [];
  const out: [number, number][] = [];
  for (let i = 0; i < SAMPLES; i++) {
    const p = path.getPointAtLength((length * i) / SAMPLES).matrixTransform(ctm);
    out.push([Math.round(p.x * 2) / 2, Math.round(p.y * 2) / 2]);
  }
  return out;
}

export function useGlassRegion(enabled: boolean) {
  useEffect(() => {
    if (!enabled) {
      invoke("set_glass_region", { polygons: [] }).catch(() => {});
      return;
    }
    let last = "";
    let frame = 0;
    const tick = () => {
      const paths = Array.from(document.querySelectorAll<SVGPathElement>("svg.berth path, svg.card-surface path"));
      const polygons = paths.map(outline).filter((p) => p.length > 2);
      const key = JSON.stringify(polygons);
      if (key !== last) {
        last = key;
        invoke("set_glass_region", { polygons }).catch(() => {});
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [enabled]);
}

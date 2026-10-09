// Windows stand-in for upstream Panel/PanelSurface.swift's Liquid Glass. Windows has no backdrop
// that follows an arbitrary shape (DWM's backdrop and the acrylic accent both ignore window regions
// and fill the whole transparent window), and reading the screen under the panel would hide it from
// screenshots and remote desktop. So src-tauri/src/backdrop.rs reads a thin strip just beside the
// window, where the window behind shows, and sends its colours from top to bottom; the glass carries
// them on under the panel as a gradient, taken towards Mica's dark base so white text stays readable
// (the transparency slider sets how far). On top: a faint sheen, glitter that slowly twinkles (a
// switch), and a faint edge so the outline still reads.
import { listen } from "@tauri-apps/api/event";
import { createContext, useContext, useEffect, useId, useRef, useSyncExternalStore } from "react";

/** The glass's edge. */
export const GLASS_EDGE = "rgba(255,255,255,0.12)";

/** The glitter switch, from Settings > Appearance; one value for every shape. */
export const GlassLook = createContext<{ glitter: boolean }>({ glitter: true });

/** Mica's dark base (WinUI's #202020), and how far the colours behind are taken towards it: at the
 *  clearest slider position and at the darkest. */
const BASE = [32, 32, 32];
const TOWARDS_CLEAREST = 0.35;
const TOWARDS_DARKEST = 0.8;
/** However bright the window behind, the glass is never lighter than this (relative luminance), so
 *  white text keeps its contrast. */
const BRIGHTEST = 0.32;
/** Before the first colours arrive: Mica's base. */
const FALLBACK = "rgb(32,32,32)";

/** Glitter: how fine the noise it is cut from, and how rare a speck is (the threshold, 0-1). */
const GLITTER_FREQUENCY = 0.42;
const GLITTER_THRESHOLD = 0.66;
/** Alpha = (noise - threshold) scaled to reach 1 at the brightest, so only the peaks show. */
const glitterSlope = 1 / (1 - GLITTER_THRESHOLD);

const luminance = ([r, g, b]: number[]) => (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;

/** One colour behind, taken towards the base by the slider, and further if it would still be too
 *  light for white text. */
function glassColour(behind: number[], transparency: number): string {
  const t = Math.min(Math.max(transparency, 0), 1);
  const wanted = TOWARDS_DARKEST - (TOWARDS_DARKEST - TOWARDS_CLEAREST) * t;
  const l = luminance(behind);
  const lb = luminance(BASE);
  const needed = l > BRIGHTEST ? (l - BRIGHTEST) / (l - lb) : 0;
  const k = Math.max(wanted, needed);
  const mixed = behind.map((c, i) => Math.round(c * (1 - k) + BASE[i] * k));
  return `rgb(${mixed.join(",")})`;
}

// The latest colours beside the window, top to bottom (null before the first). Outside React, so
// every shape shares one subscription.
let bands: number[][] | null = null;
const subscribers = new Set<() => void>();
void listen<string[]>("backdrop-tint", (e) => {
  bands = e.payload.map((c) => c.split(",").map(Number));
  subscribers.forEach((f) => f());
});
const useBands = () =>
  useSyncExternalStore(
    (f) => (subscribers.add(f), () => void subscribers.delete(f)),
    () => bands,
  );

/** The bands run the window's height; each shape's gradient is shifted back by where its own SVG
 *  sits in the window. Followed frame by frame while the shape moves, then left alone. */
function useWindowSpan(gradient: React.RefObject<SVGLinearGradientElement>) {
  useEffect(() => {
    const node = gradient.current;
    if (!node) return;
    let still = 0;
    let last = "";
    let request = 0;
    const follow = () => {
      const svg = node.ownerSVGElement;
      if (!svg) return;
      const top = svg.getBoundingClientRect().top;
      const now = `${top},${window.innerHeight}`;
      if (now !== last) {
        last = now;
        still = 0;
        node.setAttribute("y1", String(-top));
        node.setAttribute("y2", String(window.innerHeight - top));
      } else if (++still > 30) {
        return;
      }
      request = requestAnimationFrame(follow);
    };
    follow();
    return () => cancelAnimationFrame(request);
  });
}

export function FrostedGlass({ d, width, height, transparency }: { d: string; width: number; height: number; transparency: number }) {
  const id = useId().replace(/:/g, "");
  const box = { x: 0, y: 0, width, height };
  const look = useContext(GlassLook);
  const behind = useBands();
  const gradient = useRef<SVGLinearGradientElement>(null);
  useWindowSpan(gradient);
  // Glitter in user space, so the specks keep their size whatever the shape's size.
  const region = { x: 0, y: 0, width, height, filterUnits: "userSpaceOnUse" as const };
  const glitter = (seed: number) => (
    <filter id={`${id}-glitter-${seed}`} {...region}>
      <feTurbulence type="fractalNoise" baseFrequency={GLITTER_FREQUENCY} numOctaves={1} seed={seed} stitchTiles="stitch" />
      <feColorMatrix type="matrix" values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  1 0 0 0 0" />
      <feComponentTransfer>
        <feFuncA type="linear" slope={glitterSlope} intercept={-GLITTER_THRESHOLD * glitterSlope} />
      </feComponentTransfer>
      <feGaussianBlur stdDeviation={0.45} />
    </filter>
  );
  return (
    <>
      <defs>
        <clipPath id={`${id}-shape`}>
          <path d={d} />
        </clipPath>
        {glitter(11)}
        {glitter(29)}
        <linearGradient ref={gradient} id={`${id}-behind`} gradientUnits="userSpaceOnUse" x1="0" x2="0" y1="0" y2={height}>
          {(behind ?? [BASE]).map((c, i, all) => (
            <stop key={i} offset={all.length > 1 ? (i + 0.5) / all.length : 0} stopColor={behind ? glassColour(c, transparency) : FALLBACK} />
          ))}
        </linearGradient>
        <linearGradient id={`${id}-sheen`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#fff" stopOpacity={0.07} />
          <stop offset="0.3" stopColor="#fff" stopOpacity={0} />
        </linearGradient>
      </defs>
      <path d={d} fill={`url(#${id}-behind)`} />
      <g clipPath={`url(#${id}-shape)`} style={{ pointerEvents: "none" }}>
        <rect {...box} fill={`url(#${id}-sheen)`} />
        {/* Two sprinkles fading in turn: a speck lights up, dims, and another takes its place. */}
        {look.glitter && (
          <>
            <rect {...box} filter={`url(#${id}-glitter-11)`} className="glitter glitter-a" />
            <rect {...box} filter={`url(#${id}-glitter-29)`} className="glitter glitter-b" />
          </>
        )}
      </g>
      <path d={d} fill="none" stroke={GLASS_EDGE} strokeWidth={1} />
    </>
  );
}

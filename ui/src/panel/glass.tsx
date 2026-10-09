// Windows stand-in for upstream Panel/PanelSurface.swift's Liquid Glass. Upstream draws the `.clear`
// glass variant (nearly see-through) under a dim set by the transparency slider. Windows has no
// backdrop that follows an arbitrary shape (DWM's backdrop and the acrylic accent both ignore window
// regions and fill the whole transparent window), and a transparent WebView's `backdrop-filter`
// never sees the desktop. So the app reads what is behind the panel itself (src-tauri/src/backdrop.rs,
// a small live image of the screen under the window) and each shape draws it blurred, then the frost
// on top: the dim, a smoky tint, a milky haze, a fine grain, a sheen along the top, a sprinkle of
// glitter that slowly twinkles, and a faint edge so the outline still reads over a busy background.
import { listen } from "@tauri-apps/api/event";
import { useEffect, useId, useRef, useSyncExternalStore } from "react";

/** Upstream `PanelGlass.maximumDim`: the darkest the dimming goes, at transparency 0. */
const MAXIMUM_DIM = 0.6;

export const glassDim = (transparency: number) => (1 - Math.min(Math.max(transparency, 0), 1)) * MAXIMUM_DIM;

/** The glass's edge. */
export const GLASS_EDGE = "rgba(255,255,255,0.16)";

/** The frost: a smoky tint that keeps white text readable however clear the slider is, and a milky
 *  haze over it; together they are what tells frosted glass from a hole. */
const TINT = "rgba(24,24,28,0.28)";
const HAZE = 0.08;
/** The grain: white specks whose alpha follows the noise (`alpha = noise * GRAIN - GRAIN_FLOOR`).
 *  Not a blend mode: over a transparent window there is nothing to blend with. */
const GRAIN = 0.32;
const GRAIN_FLOOR = 0.1;
const GRAIN_FREQUENCY = 0.9;
/** Glitter: how fine the noise it is cut from, and how rare a speck is (the threshold, 0-1). */
const GLITTER_FREQUENCY = 0.42;
const GLITTER_THRESHOLD = 0.66;

/** Alpha = (noise - threshold) scaled to reach 1 at the brightest, so only the peaks show. */
const glitterSlope = 1 / (1 - GLITTER_THRESHOLD);

/** How far the backdrop is blurred, in DIPs. */
const BLUR = 14;

// The latest backdrop frame, a data URL of the screen under the whole window (null while glass is
// off or before the first frame). Outside React, so every shape shares one subscription.
let backdrop: string | null = null;
const subscribers = new Set<() => void>();
void listen<string | null>("backdrop", (e) => {
  backdrop = e.payload;
  subscribers.forEach((f) => f());
});
const useBackdrop = () =>
  useSyncExternalStore(
    (f) => (subscribers.add(f), () => void subscribers.delete(f)),
    () => backdrop,
  );

/** The backdrop is the whole window's; each shape draws it shifted back by where its own SVG sits
 *  in the window. Followed frame by frame while the shape moves, then left alone. */
function useWindowOrigin(image: React.RefObject<SVGImageElement>, frame: string | null) {
  useEffect(() => {
    const node = image.current;
    if (!node) return;
    let still = 0;
    let last = "";
    let request = 0;
    const follow = () => {
      const svg = node.ownerSVGElement;
      if (!svg) return;
      const r = svg.getBoundingClientRect();
      const at = `${-r.left},${-r.top},${window.innerWidth},${window.innerHeight}`;
      if (at !== last) {
        last = at;
        still = 0;
        node.setAttribute("x", String(-r.left));
        node.setAttribute("y", String(-r.top));
        node.setAttribute("width", String(window.innerWidth));
        node.setAttribute("height", String(window.innerHeight));
      } else if (++still > 30) {
        return;
      }
      request = requestAnimationFrame(follow);
    };
    follow();
    return () => cancelAnimationFrame(request);
  });
  void frame;
}

export function FrostedGlass({ d, width, height, transparency }: { d: string; width: number; height: number; transparency: number }) {
  const id = useId().replace(/:/g, "");
  const box = { x: 0, y: 0, width, height };
  const frame = useBackdrop();
  const image = useRef<SVGImageElement>(null);
  useWindowOrigin(image, frame);
  // Filters in user space, so the grain and the specks keep their size whatever the shape's size.
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
        <filter id={`${id}-blur`} x="-20%" y="-20%" width="140%" height="140%">
          <feGaussianBlur stdDeviation={BLUR} edgeMode="duplicate" />
        </filter>
        <clipPath id={`${id}-shape`}>
          <path d={d} />
        </clipPath>
        <filter id={`${id}-grain`} {...region}>
          <feTurbulence type="fractalNoise" baseFrequency={GRAIN_FREQUENCY} numOctaves={2} seed={4} stitchTiles="stitch" />
          <feColorMatrix type="matrix" values={`0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  ${GRAIN} 0 0 0 ${-GRAIN_FLOOR}`} />
          <feGaussianBlur stdDeviation={0.4} />
        </filter>
        {glitter(11)}
        {glitter(29)}
        <linearGradient id={`${id}-sheen`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#fff" stopOpacity={0.16} />
          <stop offset="0.35" stopColor="#fff" stopOpacity={0.03} />
          <stop offset="1" stopColor="#fff" stopOpacity={0} />
        </linearGradient>
      </defs>
      {frame && (
        <g clipPath={`url(#${id}-shape)`} style={{ pointerEvents: "none" }}>
          <image ref={image} href={frame} preserveAspectRatio="none" filter={`url(#${id}-blur)`} />
        </g>
      )}
      <path d={d} fill={`rgba(0,0,0,${glassDim(transparency)})`} />
      <path d={d} fill={TINT} />
      <g clipPath={`url(#${id}-shape)`} style={{ pointerEvents: "none" }}>
        <rect {...box} fill={`rgba(255,255,255,${HAZE})`} />
        <rect {...box} filter={`url(#${id}-grain)`} />
        <rect {...box} fill={`url(#${id}-sheen)`} />
        {/* Two sprinkles fading in turn: a speck lights up, dims, and another takes its place. */}
        <rect {...box} filter={`url(#${id}-glitter-11)`} className="glitter glitter-a" />
        <rect {...box} filter={`url(#${id}-glitter-29)`} className="glitter glitter-b" />
      </g>
      <path d={d} fill="none" stroke={GLASS_EDGE} strokeWidth={1} />
    </>
  );
}

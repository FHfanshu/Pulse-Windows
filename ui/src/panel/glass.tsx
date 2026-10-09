// Windows stand-in for upstream Panel/PanelSurface.swift's Liquid Glass. Upstream draws the `.clear`
// glass variant (nearly see-through) under a dim set by the transparency slider. Windows has no
// backdrop that follows an arbitrary shape (DWM's backdrop and the acrylic accent both ignore window
// regions and fill the whole transparent window), and a transparent WebView's `backdrop-filter`
// never sees the desktop. So what is behind stays sharp, and the frost is drawn on top, inside the
// shape: the dim, a milky haze, a fine grain, a sheen along the top, a sprinkle of glitter that
// slowly twinkles, and a faint edge so the outline still reads over a busy background.
import { useId } from "react";

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

export function FrostedGlass({ d, width, height, transparency }: { d: string; width: number; height: number; transparency: number }) {
  const id = useId().replace(/:/g, "");
  const box = { x: 0, y: 0, width, height };
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

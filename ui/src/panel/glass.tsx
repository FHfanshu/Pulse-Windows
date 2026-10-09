// Windows stand-in for upstream Panel/PanelSurface.swift's Liquid Glass: Mica, the material of the
// Windows 11 taskbar. Windows has no backdrop that follows an arbitrary shape (DWM's backdrop and the
// acrylic accent both ignore window regions and fill the whole transparent window), so the panel
// draws Mica itself, as Windows does: the desktop wallpaper (src-tauri/src/wallpaper.rs sends it,
// and where the window is on its display), blurred until only its colours are left, under a dark
// tint the transparency slider sets. Nothing reads the screen, so the panel shows in screenshots,
// recordings and remote desktop. On top: a faint sheen, glitter that slowly twinkles (a switch),
// and a faint edge so the outline still reads.
import { emit, listen } from "@tauri-apps/api/event";
import { createContext, useContext, useEffect, useId, useRef, useSyncExternalStore } from "react";

/** The glass's edge. */
export const GLASS_EDGE = "rgba(255,255,255,0.12)";

/** The glitter switch, from Settings > Appearance; one value for every shape. */
export const GlassLook = createContext<{ glitter: boolean }>({ glitter: true });

/** Mica's tint (WinUI's dark Mica base, #202020) at its clearest and its darkest. */
const TINT = "32,32,32";
const TINT_CLEAREST = 0.62;
const TINT_DARKEST = 0.92;
const tintAlpha = (transparency: number) =>
  TINT_DARKEST - (TINT_DARKEST - TINT_CLEAREST) * Math.min(Math.max(transparency, 0), 1);

/** The wallpaper is drawn at this fraction of the display, then blurred by `BLUR` of those pixels
 *  (an eighth of a 60-DIP blur). Mica keeps only the colours. */
const SHRINK = 8;
const BLUR = 7;

/** Glitter: how fine the noise it is cut from, and how rare a speck is (the threshold, 0-1). */
const GLITTER_FREQUENCY = 0.42;
const GLITTER_THRESHOLD = 0.66;
/** Alpha = (noise - threshold) scaled to reach 1 at the brightest, so only the peaks show. */
const glitterSlope = 1 / (1 - GLITTER_THRESHOLD);

type Fit = "fill" | "fit" | "stretch" | "center" | "tile" | "span";
interface Wallpaper {
  image: string | null;
  style: Fit;
  colour: string;
}
interface Place {
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
}

// The wallpaper and where the window sits on its display, from Rust; and the Mica made of them (a
// small blurred picture of the whole display, as a data URL). Outside React, so every shape shares
// one subscription and the blur is made once per wallpaper.
let wallpaper: Wallpaper | null = null;
let place: Place | null = null;
let mica: { url: string; place: Place } | null = null;
let made = "";
const subscribers = new Set<() => void>();
const notify = () => subscribers.forEach((f) => f());

void listen<Wallpaper>("wallpaper", (e) => {
  wallpaper = e.payload;
  made = "";
  void remake();
});
void listen<Place>("wallpaper-place", (e) => {
  place = e.payload;
  void remake();
}).then(() => emit("wallpaper-request"));

/** Where the image lands on a display of `w` x `h` DIPs, as the desktop draws it. */
function fitted(fit: Fit, img: HTMLImageElement, w: number, h: number, scale: number) {
  const iw = img.naturalWidth;
  const ih = img.naturalHeight;
  switch (fit) {
    case "stretch":
      return { x: 0, y: 0, w, h };
    case "fit": {
      const k = Math.min(w / iw, h / ih);
      return { x: (w - iw * k) / 2, y: (h - ih * k) / 2, w: iw * k, h: ih * k };
    }
    case "center":
    case "tile": {
      // Drawn at its own pixel size.
      const dw = iw / scale;
      const dh = ih / scale;
      return { x: (w - dw) / 2, y: (h - dh) / 2, w: dw, h: dh };
    }
    default: {
      const k = Math.max(w / iw, h / ih);
      return { x: (w - iw * k) / 2, y: (h - ih * k) / 2, w: iw * k, h: ih * k };
    }
  }
}

async function remake() {
  const paper = wallpaper;
  const at = place;
  if (!paper || !at) return;
  const key = `${paper.image?.length ?? 0}|${paper.style}|${paper.colour}|${at.width}|${at.height}|${at.scale}`;
  if (key === made) {
    if (mica) mica = { ...mica, place: at };
    notify();
    return;
  }
  made = key;
  const w = Math.max(1, Math.round(at.width / SHRINK));
  const h = Math.max(1, Math.round(at.height / SHRINK));
  // The display, small and sharp.
  const flat = document.createElement("canvas");
  flat.width = w;
  flat.height = h;
  const f = flat.getContext("2d");
  if (!f) return;
  f.fillStyle = paper.colour;
  f.fillRect(0, 0, w, h);
  if (paper.image) {
    try {
      const img = new Image();
      img.src = paper.image;
      await img.decode();
      if (made !== key) return;
      const r = fitted(paper.style, img, at.width, at.height, at.scale);
      f.imageSmoothingQuality = "high";
      if (paper.style === "tile") {
        for (let y = r.y % r.h - r.h; y < at.height; y += r.h)
          for (let x = r.x % r.w - r.w; x < at.width; x += r.w) f.drawImage(img, x / SHRINK, y / SHRINK, r.w / SHRINK, r.h / SHRINK);
      } else {
        f.drawImage(img, r.x / SHRINK, r.y / SHRINK, r.w / SHRINK, r.h / SHRINK);
      }
    } catch {
      // An image the WebView cannot decode leaves the desktop colour.
    }
  }
  // Blurred with a margin of its own edges stretched out, so the blur does not fade at the
  // display's edges.
  const pad = BLUR * 3;
  const soft = document.createElement("canvas");
  soft.width = w;
  soft.height = h;
  const s = soft.getContext("2d");
  if (!s) return;
  s.filter = `blur(${BLUR}px)`;
  s.drawImage(flat, -pad, -pad, w + pad * 2, h + pad * 2);
  mica = { url: soft.toDataURL("image/png"), place: at };
  notify();
}

const useMica = () =>
  useSyncExternalStore(
    (f) => (subscribers.add(f), () => void subscribers.delete(f)),
    () => mica,
  );

/** The Mica picture is the whole display's; each shape draws it shifted back by where its window
 *  is on the display and where its own SVG is in the window. Followed frame by frame while the
 *  shape moves, then left alone. */
function useDisplayOrigin(image: React.RefObject<SVGImageElement>, at: Place | null) {
  useEffect(() => {
    const node = image.current;
    if (!node || !at) return;
    let still = 0;
    let last = "";
    let request = 0;
    const follow = () => {
      const svg = node.ownerSVGElement;
      if (!svg) return;
      const r = svg.getBoundingClientRect();
      const x = -(at.x + r.left);
      const y = -(at.y + r.top);
      const now = `${x},${y},${at.width},${at.height}`;
      if (now !== last) {
        last = now;
        still = 0;
        node.setAttribute("x", String(x));
        node.setAttribute("y", String(y));
        node.setAttribute("width", String(at.width));
        node.setAttribute("height", String(at.height));
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
  const current = useMica();
  const image = useRef<SVGImageElement>(null);
  useDisplayOrigin(image, current?.place ?? null);
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
        <linearGradient id={`${id}-sheen`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#fff" stopOpacity={0.07} />
          <stop offset="0.3" stopColor="#fff" stopOpacity={0} />
        </linearGradient>
      </defs>
      {current && (
        <g clipPath={`url(#${id}-shape)`} style={{ pointerEvents: "none" }}>
          <image ref={image} href={current.url} preserveAspectRatio="none" />
        </g>
      )}
      <path d={d} fill={`rgba(${TINT},${current ? tintAlpha(transparency) : 0.9})`} />
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

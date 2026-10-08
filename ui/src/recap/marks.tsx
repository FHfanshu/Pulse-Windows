// Brand marks, the Pulse mark and the small glyphs the cards draw (upstream RecapVendorMark,
// RecapTiles and RecapStyle). Flat fills and strokes only.
import { useId } from "react";
import { C, FONT } from "./kit";

// MARK: Brand marks

const raw = import.meta.glob("../../../assets/icons/*.svg", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

const icons: Record<string, string> = {};
for (const [path, svg] of Object.entries(raw)) {
  const name = path.split("/").pop()!.replace(/\.svg$/, "");
  icons[name] = svg
    .replace(/<title>[\s\S]*?<\/title>/, "")
    .replace(/\s(width|height)="[^"]*"/g, "")
    .replace(/\sstyle="[^"]*"/, "")
    .replace(/<svg/, '<svg width="100%" height="100%"');
}

/** Lower-cased fragments of a model name, and the mark they name. The first row that matches
 *  wins, so a more specific fragment comes first. No mark is guessed. */
const vendorTable: { needles: string[]; resource: string }[] = [
  { needles: ["claude", "opus", "sonnet", "haiku"], resource: "claude" },
  { needles: ["gpt", "codex", "chatgpt", "davinci"], resource: "openai" },
  { needles: ["gemini"], resource: "gemini" },
  { needles: ["glm", "zhipu"], resource: "zai" },
  { needles: ["kimi"], resource: "kimi" },
  { needles: ["moonshot"], resource: "moonshot" },
  { needles: ["deepseek"], resource: "deepseek" },
  { needles: ["qwen", "qwq"], resource: "qwen" },
  { needles: ["mistral", "codestral", "devstral", "magistral", "ministral"], resource: "mistral" },
  { needles: ["minimax"], resource: "minimax" },
  { needles: ["grok"], resource: "grok" },
  { needles: ["doubao"], resource: "volcengine" },
  { needles: ["step-"], resource: "stepfun" },
  { needles: ["mimo"], resource: "xiaomimimo" },
  { needles: ["longcat"], resource: "longcat" },
  { needles: ["hunyuan"], resource: "tencent" },
  { needles: ["sonar"], resource: "perplexity" },
];

/** The mark (file stem in assets/icons) for a model's display name or id; undefined where the
 *  vendor is not one the app has a mark for. */
export function vendorResource(model: string): string | undefined {
  const lower = model.toLowerCase();
  for (const row of vendorTable) {
    if (row.needles.some((n) => lower.includes(n))) return row.resource;
  }
  // OpenAI's o-series: "o3", "o4-mini".
  if (/^o\d/.test(lower)) return "openai";
  return undefined;
}

/** The letter a monogram tile shows for a name. */
export function monogram(name: string): string {
  const first = Array.from(name).find((c) => /[\p{L}\p{N}]/u.test(c));
  return first ? first.toUpperCase() : "•";
}

/** A bundled brand mark in one flat colour (a template, tinted). */
export function BrandMark({ resource, side, color }: { resource: string; side: number; color: string }) {
  const svg = icons[resource];
  if (!svg) return null;
  return <span aria-hidden style={{ display: "inline-flex", width: side, height: side, color, flex: "none" }} dangerouslySetInnerHTML={{ __html: svg }} />;
}

/** A brand mark, or, for a name with no mark in the app, the name's first letter in the same
 *  square and the same colour. */
export function Glyph({ resource, name, side, color }: { resource?: string; name: string; side: number; color: string }) {
  if (resource && icons[resource]) return <BrandMark resource={resource} side={side} color={color} />;
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        width: side,
        height: side,
        flex: "none",
        fontFamily: FONT,
        fontWeight: 600,
        fontSize: side * 0.8,
        lineHeight: 1,
        color,
        whiteSpace: "nowrap",
      }}
    >
      {monogram(name)}
    </span>
  );
}

/** The square a brand mark sits on: ink with a lime mark for the lead, white with a hairline
 *  otherwise. */
export function MarkTile({ resource, name, side, lead = false }: { resource?: string; name: string; side: number; lead?: boolean }) {
  return (
    <div
      style={{
        width: side,
        height: side,
        flex: "none",
        borderRadius: side / 4,
        background: lead ? C.ink : C.white,
        border: lead ? undefined : `1px solid ${C.hairline}`,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      <Glyph resource={resource} name={name} side={side * 0.53} color={lead ? C.lime : C.ink} />
    </div>
  );
}

// MARK: The Pulse mark

/** The Pulse mark on its ink tile: the stem in paper, the bowl in lime, drawn from the app
 *  icon's path (292...772 x 184...840 of its 1024 space). */
export function PulseMark({ side }: { side: number }) {
  const w = (side * 28) / 46;
  const h = (side * 34) / 46;
  const scale = Math.min(w / 480, h / 656);
  const ox = (side - w) / 2 + (w - 480 * scale) / 2;
  const oy = (side - h) / 2 + (h - 656 * scale) / 2;
  const x = (v: number) => ((v - 292) * scale + ox).toFixed(3);
  const y = (v: number) => ((v - 184) * scale + oy).toFixed(3);
  const stroke = 100 * scale;
  return (
    <svg width={side} height={side} viewBox={`0 0 ${side} ${side}`} style={{ flex: "none", display: "block" }}>
      <rect width={side} height={side} rx={(side * 12) / 46} fill={C.ink} />
      <path d={`M${x(352)} ${y(780)} L${x(352)} ${y(244)}`} stroke={C.paper} strokeWidth={stroke} strokeLinecap="round" strokeLinejoin="round" fill="none" />
      <path
        d={`M${x(352)} ${y(244)} L${x(562)} ${y(244)} A${150 * scale} ${150 * scale} 0 0 1 ${x(562)} ${y(544)} L${x(530)} ${y(544)}`}
        stroke={C.lime}
        strokeWidth={stroke}
        strokeLinecap="round"
        strokeLinejoin="round"
        fill="none"
      />
    </svg>
  );
}

// MARK: Glyphs

/** The persona's glyph: a moon for the night owl, a sun for everyone else. */
export function PersonaGlyph({ persona, side, color = C.lime }: { persona: string; side: number; color?: string }) {
  const id = useId();
  if (persona === "nightOwl") {
    return (
      <svg width={side} height={side} viewBox="0 0 1 1" style={{ flex: "none", display: "block" }}>
        <defs>
          <mask id={`moon${id}`}>
            <circle cx={0.5} cy={0.5} r={0.5} fill="#fff" />
            <circle cx={0.79} cy={0.19} r={0.45} fill="#000" />
          </mask>
        </defs>
        <circle cx={0.5} cy={0.5} r={0.5} fill={color} mask={`url(#moon${id})`} />
      </svg>
    );
  }
  const rays = Array.from({ length: 8 }, (_, i) => {
    const angle = (i * Math.PI) / 4;
    return { x1: 0.5 + 0.32 * Math.cos(angle), y1: 0.5 + 0.32 * Math.sin(angle), x2: 0.5 + 0.48 * Math.cos(angle), y2: 0.5 + 0.48 * Math.sin(angle) };
  });
  return (
    <svg width={side} height={side} viewBox="0 0 1 1" style={{ flex: "none", display: "block" }}>
      <circle cx={0.5} cy={0.5} r={0.2} fill={color} />
      {rays.map((r, i) => (
        <line key={i} {...r} stroke={color} strokeWidth={0.09} strokeLinecap="round" />
      ))}
    </svg>
  );
}

/** A flame, for a streak. */
export function Flame({ width = 30, height = 36, color = C.ink }: { width?: number; height?: number; color?: string }) {
  return (
    <svg width={width} height={height} viewBox="0 0 24 28" preserveAspectRatio="xMinYMin meet" style={{ flex: "none", display: "block" }}>
      <path d="M12 1 C13 6 19 8.5 19 16 A7 7 0 0 1 5 16 C5 12.4 7 10.4 8.4 9 C8.6 11.4 9.7 12.8 11 13.4 C10 9 10.5 4.5 12 1 Z" fill={color} />
    </svg>
  );
}

export type DayKind = "night" | "sunrise" | "day" | "sunset";

const polar = (cx: number, cy: number, r: number, degrees: number) => {
  const a = (degrees * Math.PI) / 180;
  return [cx + r * Math.cos(a), cy + r * Math.sin(a)] as const;
};

/** A moon, a sunrise, a sun and a sunset as line drawings on a 24-point grid, for the four
 *  parts of the day. */
export function DayGlyph({ kind, side, color }: { kind: DayKind; side: number; color: string }) {
  let body: JSX.Element;
  if (kind === "night") {
    // A crescent: a small arc about one centre, a large one about the other.
    const [ax, ay] = polar(17.676, 6.324, 8.5, 74.2);
    const [bx, by] = polar(17.676, 6.324, 8.5, 195.8);
    const [cx, cy] = polar(11.824, 12.176, 8.5, 254.2);
    const [dx, dy] = polar(11.824, 12.176, 8.5, 15.8);
    body = <path d={`M${ax} ${ay} A8.5 8.5 0 0 1 ${bx} ${by} L${cx} ${cy} A8.5 8.5 0 1 0 ${dx} ${dy} Z`} />;
  } else if (kind === "day") {
    const rays = [
      [12, 2, 12, 4.5],
      [12, 19.5, 12, 22],
      [2, 12, 4.5, 12],
      [19.5, 12, 22, 12],
      [4.9, 4.9, 6.7, 6.7],
      [17.3, 17.3, 19.1, 19.1],
      [4.9, 19.1, 6.7, 17.3],
      [17.3, 6.7, 19.1, 4.9],
    ];
    body = (
      <>
        <circle cx={12} cy={12} r={4} />
        {rays.map((r, i) => (
          <line key={i} x1={r[0]} y1={r[1]} x2={r[2]} y2={r[3]} />
        ))}
      </>
    );
  } else {
    body = (
      <>
        <line x1={4} y1={18} x2={20} y2={18} />
        <path d="M7 18 A5 5 0 0 1 17 18" />
        {kind === "sunrise" ? (
          <>
            <line x1={12} y1={5} x2={12} y2={8} />
            <line x1={5.6} y1={9.6} x2={7.4} y2={11.4} />
            <line x1={18.4} y1={9.6} x2={16.6} y2={11.4} />
          </>
        ) : (
          <>
            <line x1={12} y1={12} x2={12} y2={9} />
            <path d="M9.5 10.5 L12 13 L14.5 10.5" />
          </>
        )}
      </>
    );
  }
  return (
    <svg
      width={side}
      height={side}
      viewBox="0 0 24 24"
      fill="none"
      stroke={color}
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ flex: "none", display: "block" }}
    >
      {body}
    </svg>
  );
}

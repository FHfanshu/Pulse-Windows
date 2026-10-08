// The recap cards' palette and the small pieces every card shares (upstream RecapStyle,
// RecapTiles and RecapStoryViews). Flat colour and type only: no gradient, blur, shadow or emoji.
// Lime is a fill, never text on the paper.
//
// SwiftUI stacks become flex boxes: `H` is an HStack, `V` a VStack, `Spacer` a Spacer, and
// `grow` stands for `frame(maxWidth: .infinity)`. Everything is styled inline so a card can be
// serialised and drawn to a canvas (export.ts) without a stylesheet.
import { Children, CSSProperties, ReactNode, isValidElement, useEffect, useLayoutEffect, useRef, useState } from "react";
import { segments } from "./format";

export const C = {
  paper: "#F5F5F1",
  white: "#FFFFFF",
  hairline: "#E4E4DD",
  ink: "#1B1B1E",
  lime: "#C8F03C",
  secondary: "#6E6E68",
  tertiary: "#8A8A83",
  faint: "#A3A39C",
  rule: "#D6D6CF",
  track: "#EFEFE9",
  bodyInk: "#3C3C38",
  limeInk: "#3F4D12",
  /** The calendar's four steps from a light day to a heavy one; the fifth, the busiest, is ink. */
  heat: ["#EEF7C9", "#DDF290", "#C8F03C", "#9DC41A", "#1B1B1E"],
  heatZero: "#EDEDE7",
  /** Bars for a ranked list: lime for the first, ink for the second, then quieter greys. */
  ranks: ["#C8F03C", "#1B1B1E", "#5C5C57", "#9A9A93", "#C9C9C2"],
  restBar: "#DADAD3",
  restLabel: "#9A9A93",
  darkRest: "#55554F",
  /** The mid grey the cards use for lines of body copy. */
  grey55: "#55554F",
};

export const FONT =
  '"SF Pro Text","SF Pro Display","Segoe UI Variable Text","Segoe UI","Microsoft YaHei UI","PingFang SC","Yu Gothic UI","Malgun Gothic",system-ui,sans-serif';
export const MONO =
  '"SF Mono","Cascadia Mono","Cascadia Code",Consolas,"Microsoft YaHei UI","PingFang SC","Yu Gothic UI","Malgun Gothic",monospace';

/** Every card is 1080 x 1920 points, drawn at scale 1. */
export const CARD_WIDTH = 1080;
export const CARD_HEIGHT = 1920;

// MARK: Layout helpers

interface StackProps {
  gap?: number;
  /** align-items */
  align?: CSSProperties["alignItems"];
  justify?: CSSProperties["justifyContent"];
  /** Children of one width, as SwiftUI shares the room between views that all ask for all of it
   *  (flex-grow would add the share on top of each one's own padding and border). */
  equal?: boolean;
  style?: CSSProperties;
  children?: ReactNode;
}

const stack = (direction: "row" | "column", p: StackProps): CSSProperties => ({
  ...(p.equal
    ? { display: "grid", gridAutoFlow: "column", gridAutoColumns: "minmax(0, 1fr)" }
    : { display: "flex", flexDirection: direction }),
  gap: p.gap,
  alignItems: p.align,
  justifyContent: p.justify,
  minWidth: 0,
  ...p.style,
});

/** HStack. */
export function H(p: StackProps) {
  return <div style={stack("row", p)}>{p.children}</div>;
}

/** VStack; children stretch across unless `align` says otherwise (`.leading` is "flex-start"). */
export function V(p: StackProps) {
  return <div style={stack("column", p)}>{p.children}</div>;
}

/** A Spacer: takes what is left, never less than `min`. */
export function Spacer({ min = 0 }: { min?: number }) {
  return <div style={{ flex: "1 1 0", minWidth: min, minHeight: min }} />;
}

/** frame(maxWidth: .infinity). */
export const grow: CSSProperties = { flex: "1 1 0", minWidth: 0, minHeight: 0 };

// MARK: Text

interface TProps {
  size: number;
  /** font weight; 400 regular ... 900 black */
  w?: number;
  mono?: boolean;
  color?: string;
  /** letter-spacing in px, on every character */
  track?: number;
  /** letter-spacing in px on every character but the last: a tight number keeps its last
   *  digit's whole advance, so its ink is never cut by the frame. */
  tight?: number;
  /** lineLimit */
  lines?: number;
  /** minimumScaleFactor: the text shrinks to this fraction of its size before it is cut. */
  fit?: number;
  align?: CSSProperties["textAlign"];
  lh?: number;
  style?: CSSProperties;
  children?: ReactNode;
}

function tightText(text: string, spacing: number) {
  const chars = Array.from(text);
  if (chars.length < 2) return text;
  return (
    <>
      <span style={{ letterSpacing: spacing }}>{chars.slice(0, -1).join("")}</span>
      <span style={{ letterSpacing: 0 }}>{chars[chars.length - 1]}</span>
    </>
  );
}

function textStyle(p: TProps, size = p.size): CSSProperties {
  return {
    margin: 0,
    fontSize: size,
    fontWeight: p.w ?? 400,
    fontFamily: p.mono ? MONO : FONT,
    color: p.color,
    lineHeight: p.lh ?? 1.2,
    letterSpacing: p.track,
    textAlign: p.align,
    minWidth: 0,
    ...p.style,
  };
}

/** A line that scales down to `floor` of its size where it is too wide, and is never squeezed
 *  vertically (`recapFit`). */
function FitText({ p, content, floor }: { p: TProps; content: ReactNode; floor: number }) {
  const outer = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLSpanElement>(null);
  const [scale, setScale] = useState(1);
  useLayoutEffect(() => {
    const box = outer.current;
    const text = inner.current;
    if (!box || !text) return;
    const natural = text.offsetWidth / scale;
    const available = box.clientWidth;
    if (natural <= 0 || available <= 0) return;
    const next = natural > available + 0.5 ? Math.max(floor, available / natural) : 1;
    if (Math.abs(next - scale) > 0.004) setScale(next);
  });
  return (
    <div ref={outer} style={{ ...textStyle(p), whiteSpace: "nowrap" }}>
      <span ref={inner} style={{ display: "inline-block", fontSize: p.size * scale }}>
        {content}
      </span>
    </div>
  );
}

export function T(p: TProps) {
  const content = p.tight !== undefined && typeof p.children === "string" ? tightText(p.children, p.tight) : p.children;
  if (p.fit) return <FitText p={p} content={content} floor={p.fit} />;
  const style = textStyle(p);
  if (p.lines === 1) {
    return <div style={{ ...style, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{content}</div>;
  }
  if (p.lines) {
    return (
      <div
        style={{
          ...style,
          display: "-webkit-box",
          WebkitLineClamp: p.lines,
          WebkitBoxOrient: "vertical",
          overflow: "hidden",
        }}
      >
        {content}
      </div>
    );
  }
  return <div style={style}>{content}</div>;
}

/** A sentence with some of its words set in bold, on a lime bar where the card asks for one. */
export function RichText(p: {
  text: string;
  size: number;
  w?: number;
  color?: string;
  emphasisWeight?: number;
  highlights?: boolean;
  lh?: number;
  style?: CSSProperties;
}) {
  const highlights = p.highlights ?? true;
  return (
    <div
      style={{
        margin: 0,
        fontFamily: FONT,
        fontSize: p.size,
        fontWeight: p.w ?? 400,
        color: p.color ?? C.ink,
        lineHeight: p.lh ?? 1.25,
        ...p.style,
      }}
    >
      {segments(p.text).map((s, i) => {
        if (!s.emphasised) return <span key={i}>{s.text}</span>;
        // One bar is one piece: a word joiner between the characters keeps it whole, and a
        // no-break space either side stands in for the bar's padding.
        const unbroken = highlights
          ? " " + Array.from(s.text).join("⁠").replace(/ /g, " ") + " "
          : s.text;
        return (
          <span
            key={i}
            style={{
              fontWeight: p.emphasisWeight ?? 700,
              background: highlights ? C.lime : undefined,
              whiteSpace: highlights ? "nowrap" : undefined,
            }}
          >
            {unbroken}
          </span>
        );
      })}
    </div>
  );
}

// MARK: Figures

const supportsTrim = typeof CSS !== "undefined" && typeof CSS.supports === "function" && CSS.supports("text-box", "trim-both cap alphabetic");

/** The room above and below the digits in a system-font line, as a fraction of the size. */
export const ASCENDER_ROOM = 0.262;
export const DESCENDER_ROOM = 0.211;

/** Trims a number's line box down to its digits (the cap-height box), so two heavy numbers stack
 *  with the space the mockup gives them and not with the font's ascender and descender room. */
export function trimStyle(size: number): CSSProperties {
  if (supportsTrim) return { textBox: "trim-both cap alphabetic" } as CSSProperties;
  return { lineHeight: 1.19, marginTop: -size * ASCENDER_ROOM, marginBottom: -size * DESCENDER_ROOM };
}

/** A hero number with its unit set smaller beside it ("8.4" "亿"), both on one baseline. */
export function FigureText(p: {
  number: string;
  unit?: string;
  numberSize: number;
  unitSize: number;
  numberWeight?: number;
  unitWeight?: number;
  color?: string;
  /** letter-spacing as a fraction of the font size; heavy numerals are set tight */
  tracking?: number;
  unitGap?: number;
  trimmed?: boolean;
  style?: CSSProperties;
}) {
  const unit = p.unit ?? "";
  const tracking = (p.tracking ?? -0.05) * p.numberSize;
  return (
    <div
      style={{
        display: "inline-flex",
        alignItems: "last baseline",
        gap: unit ? (p.unitGap ?? 8) : 0,
        whiteSpace: "nowrap",
        color: p.color ?? C.ink,
        flex: "none",
        ...p.style,
      }}
    >
      <span
        style={{
          fontFamily: FONT,
          fontSize: p.numberSize,
          fontWeight: p.numberWeight ?? 700,
          lineHeight: 1.2,
          ...(p.trimmed ? trimStyle(p.numberSize) : null),
        }}
      >
        {tightText(p.number, tracking)}
      </span>
      {unit && (
        <span
          style={{
            fontFamily: FONT,
            fontSize: p.unitSize,
            fontWeight: p.unitWeight ?? 900,
            lineHeight: 1.2,
            ...(p.trimmed ? trimStyle(p.unitSize) : null),
          }}
        >
          {unit}
        </span>
      )}
    </div>
  );
}

// MARK: Shapes and containers

/** A thin rule across the card, in ink or in the paler grey. */
export function Hairline({ strong = false, style }: { strong?: boolean; style?: CSSProperties }) {
  return <div style={{ height: strong ? 2 : 1, background: strong ? C.ink : C.rule, flex: "none", ...style }} />;
}

/** A section's title, and a small note at the right. */
export function SectionHead({ title, note }: { title: string; note?: string | null }) {
  return (
    <H gap={16} align="baseline">
      <div style={{ minWidth: 0, flex: "0 1 auto" }}>
        <T size={22} w={700} fit={0.7}>
          {title}
        </T>
      </div>
      <Spacer />
      {note ? (
        <div style={{ minWidth: 0, flex: "0 1 auto" }}>
          <T size={18} color={C.tertiary} fit={0.6}>
            {note}
          </T>
        </div>
      ) : null}
    </H>
  );
}

export type TileTone = "white" | "ink" | "lime";

export const toneCaption: Record<TileTone, string> = {
  white: C.secondary,
  ink: "rgba(245,245,241,0.6)",
  lime: C.limeInk,
};
export const toneDetail: Record<TileTone, string> = {
  white: C.tertiary,
  ink: "rgba(245,245,241,0.6)",
  lime: C.limeInk,
};

/** A rounded tile: white with a hairline, ink, or lime. */
export function Tile(p: {
  tone?: TileTone;
  radius?: number;
  padding?: number | [number, number, number, number];
  style?: CSSProperties;
  children?: ReactNode;
}) {
  const tone = p.tone ?? "white";
  const pad = p.padding ?? [20, 22, 20, 22];
  const [top, right, bottom, left] = typeof pad === "number" ? [pad, pad, pad, pad] : pad;
  return (
    <div
      style={{
        ...grow,
        alignSelf: "stretch",
        display: "flex",
        flexDirection: "column",
        alignItems: "stretch",
        padding: `${top}px ${right}px ${bottom}px ${left}px`,
        borderRadius: p.radius ?? 20,
        background: tone === "white" ? C.white : tone === "ink" ? C.ink : C.lime,
        border: tone === "white" ? `1px solid ${C.hairline}` : undefined,
        ...p.style,
      }}
    >
      {p.children}
    </div>
  );
}

/** A white card with a hairline, or an ink one. */
export function Box(p: {
  dark?: boolean;
  fill?: string;
  padding?: [number, number, number, number];
  style?: CSSProperties;
  children?: ReactNode;
}) {
  const [top, right, bottom, left] = p.padding ?? [24, 26, 24, 26];
  const background = p.fill ?? (p.dark ? C.ink : C.white);
  return (
    <div
      style={{
        ...grow,
        alignSelf: "stretch",
        display: "flex",
        flexDirection: "column",
        alignItems: "stretch",
        padding: `${top}px ${right}px ${bottom}px ${left}px`,
        borderRadius: 28,
        background,
        border: `1px solid ${p.dark ? C.ink : (p.fill ?? C.hairline)}`,
        ...p.style,
      }}
    >
      {p.children}
    </div>
  );
}

/** The small rounded label: "Night owl", "Peak", "up 38% vs August". */
export function Pill(p: {
  fill?: string;
  stroke?: string;
  horizontal?: number;
  vertical?: number;
  style?: CSSProperties;
  children?: ReactNode;
}) {
  return (
    <div
      style={{
        display: "inline-flex",
        alignItems: "center",
        flex: "none",
        whiteSpace: "nowrap",
        padding: `${p.vertical ?? 8}px ${p.horizontal ?? 16}px`,
        borderRadius: 999,
        background: p.fill,
        border: p.stroke ? `1.5px solid ${p.stroke}` : undefined,
        ...p.style,
      }}
    >
      {p.children}
    </div>
  );
}

/** The frame of a numbered card: paper, the running head, the page counter. */
export function StoryPage(p: {
  page: { number: number; count: number } | null;
  /** A small outlined mono stamp centred in the running head ("NO. 2026·10"). */
  stamp?: string;
  children?: ReactNode;
}) {
  return (
    <Page padding={[80, 80, 72, 80]}>
      <div style={{ position: "relative", display: "flex", alignItems: "center", justifyContent: "space-between", flex: "none" }}>
        <T size={20} w={500} mono track={1.6}>
          PULSE RECAP
        </T>
        {p.page ? (
          <T size={20} mono color={C.tertiary} track={1.6}>
            {`${String(p.page.number).padStart(2, "0")} / ${String(p.page.count).padStart(2, "0")}`}
          </T>
        ) : null}
        {p.stamp ? (
          <div
            style={{
              position: "absolute",
              left: "50%",
              top: "50%",
              transform: "translate(-50%, -50%)",
              padding: "5px 10px",
              border: `1.5px solid ${C.ink}`,
              borderRadius: 6,
              whiteSpace: "nowrap",
            }}
          >
            <T size={16} mono track={1.3}>
              {p.stamp}
            </T>
          </div>
        ) : null}
      </div>
      {p.children}
    </Page>
  );
}

/** A card's paper: 1080 x 1920 with the padding the card asks for. */
export function Page(p: { padding: [number, number, number, number]; children?: ReactNode }) {
  const [top, right, bottom, left] = p.padding;
  return (
    <div
      style={{
        width: CARD_WIDTH,
        height: CARD_HEIGHT,
        boxSizing: "border-box",
        padding: `${top}px ${right}px ${bottom}px ${left}px`,
        background: C.paper,
        color: C.ink,
        fontFamily: FONT,
        display: "flex",
        flexDirection: "column",
        alignItems: "stretch",
        overflow: "hidden",
        textAlign: "left",
      }}
    >
      {p.children}
    </div>
  );
}

// MARK: Measuring (GeometryReader)

/** Gives its child the size it was laid out at. */
export function Geo({
  children,
  style,
}: {
  children: (width: number, height: number) => ReactNode;
  style?: CSSProperties;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<[number, number]>([0, 0]);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const w = el.clientWidth;
      const h = el.clientHeight;
      setSize((old) => (old[0] === w && old[1] === h ? old : [w, h]));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  return (
    <div ref={ref} style={{ position: "relative", minWidth: 0, minHeight: 0, ...style }}>
      {size[0] > 0 || size[1] > 0 ? children(size[0], size[1]) : null}
    </div>
  );
}

/** ViewThatFits(in: .horizontal): the first candidate that fits the width it is given. */
export function ThatFits({ children, style }: { children: ReactNode; style?: CSSProperties }) {
  const options = Children.toArray(children).filter(isValidElement);
  const outer = useRef<HTMLDivElement>(null);
  const probes = useRef<(HTMLDivElement | null)[]>([]);
  const [index, setIndex] = useState(0);
  useLayoutEffect(() => {
    const available = outer.current?.clientWidth ?? 0;
    if (available <= 0) return;
    let chosen = options.length - 1;
    for (let i = 0; i < options.length; i++) {
      const probe = probes.current[i];
      if (probe && probe.scrollWidth <= available + 0.5) {
        chosen = i;
        break;
      }
    }
    if (chosen !== index) setIndex(chosen);
  });
  useEffect(() => {
    // Fonts arriving late change the widths.
    const fonts = (document as Document & { fonts?: FontFaceSet }).fonts;
    fonts?.ready.then(() => setIndex((i) => i));
  }, []);
  return (
    <div ref={outer} style={{ position: "relative", minWidth: 0, ...style }}>
      <div style={{ position: "relative" }}>{options[index]}</div>
      {/* Every candidate at its natural width, unseen, to be measured. */}
      <div aria-hidden style={{ position: "absolute", left: 0, top: 0, visibility: "hidden", pointerEvents: "none", height: 0, overflow: "hidden" }}>
        {options.map((option, i) => (
          <div key={i} ref={(el) => (probes.current[i] = el)} style={{ width: "max-content", minWidth: 0 }}>
            {option}
          </div>
        ))}
      </div>
    </div>
  );
}

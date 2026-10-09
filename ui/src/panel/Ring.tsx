// Ported from upstream Panel/UsageRingView.swift (bot mark omitted by decision).
import { motion } from "motion/react";
import { useId } from "react";
import { ProviderIcon } from "./Icon";
import { ink, useLightPanel } from "./scheme";
import { spring, usageColor } from "./tint";

const CENTRE_GAP = 4;
const ICON_SCALE = 0.8;
const SECOND_RING_SQUEEZE = 2;
const BUSY_SWEEP = 0.22;
const REFRESH_SWEEP = 0.16;
const HALO_RADIUS = 10;
const CLOCK_GAP = 3;
const CLOCK_LINE_WIDTH = 2;

export interface RingProps {
  provider: string;
  usedFraction: number | null;
  hasReading: boolean;
  chosenTint?: string | null;
  isSpent: boolean;
  showsRemaining: boolean;
  diameter: number;
  lineWidth: number;
  scale: number;
  isBusy?: boolean;
  isRefreshing?: boolean;
  animatesActivity?: boolean;
  highlight: boolean;
  windowClockFraction?: number | null;
  secondFraction?: number | null;
  secondIsSpent?: boolean;
  secondDiameter: number;
  secondLineWidth: number;
  warningAt: number;
}

const arcSpring = spring(0.5, 0.85);

/** A stroked circle trimmed from 12 o'clock clockwise, like `Circle().trim().rotationEffect(-90°)`. */
function Arc({ r, fraction, color, width, transition, opacity = 1 }: { r: number; fraction: number; color: string; width: number; transition?: object; opacity?: number }) {
  const c = 2 * Math.PI * r;
  const visible = fraction > 0;
  return (
    <motion.circle
      cx={0}
      cy={0}
      r={r}
      fill="none"
      stroke={color}
      strokeWidth={width}
      strokeLinecap="round"
      transform="rotate(-90)"
      strokeDasharray={`${c} ${c}`}
      initial={false}
      animate={{ strokeDashoffset: c * (1 - Math.min(Math.max(fraction, 0), 1)), opacity: visible ? opacity : 0 }}
      transition={transition ?? arcSpring}
    />
  );
}

export function Ring(p: RingProps) {
  const id = useId().replace(/:/g, "");
  const light = useLightPanel();
  const d = p.diameter;
  const r = d / 2;
  const spent = p.isSpent || (p.usedFraction ?? 0) >= 1;
  const automatic = usageColor(p.usedFraction ?? 0, spent, p.warningAt, light);
  const arcColour = p.chosenTint && !spent ? p.chosenTint : automatic;

  let arcFraction = 0;
  if (p.usedFraction != null) {
    const used = Math.min(Math.max(p.usedFraction, 0), 1);
    arcFraction = spent || used >= 1 ? 1 : p.showsRemaining ? 1 - used : used;
  }

  const hasSecond = p.secondFraction != null;
  const squeeze = hasSecond ? SECOND_RING_SQUEEZE * p.scale : 0;
  const centreDiameter = Math.max(d - (p.lineWidth + CENTRE_GAP) * 2 - squeeze * 2, 0);
  const busyDiameter = hasSecond
    ? Math.max(centreDiameter + SECOND_RING_SQUEEZE * p.scale, 0)
    : Math.max(d - p.lineWidth * 1.5 - CENTRE_GAP, 0);
  const clockDiameter = d + p.lineWidth + (CLOCK_GAP + CLOCK_LINE_WIDTH / 2) * 2 * p.scale;
  const refreshing = !!p.isRefreshing && p.animatesActivity !== false;
  const pad = HALO_RADIUS * 3 + clockDiameter / 2;

  let second: JSX.Element | null = null;
  if (hasSecond) {
    const used = Math.min(Math.max(p.secondFraction!, 0), 1);
    const sSpent = !!p.secondIsSpent || used >= 1;
    const shown = p.showsRemaining && !sSpent ? 1 - used : used;
    const colour = (p.chosenTint && !sSpent ? p.chosenTint : null) ?? usageColor(used, sSpent, p.warningAt, light);
    const sr = p.secondDiameter / 2;
    second = (
      <g>
        <circle r={sr} fill="none" stroke={ink(light, 0.18)} strokeWidth={p.secondLineWidth} />
        <Arc r={sr} fraction={sSpent ? 1 : shown} color={colour} width={p.secondLineWidth} transition={{ duration: 0.35, ease: "easeOut" }} />
      </g>
    );
  }

  return (
    <div className="ring" style={{ width: d, height: d }}>
      <svg
        width={d + pad * 2}
        height={d + pad * 2}
        viewBox={`${-r - pad} ${-r - pad} ${d + pad * 2} ${d + pad * 2}`}
        style={{ position: "absolute", left: -pad, top: -pad, overflow: "visible", pointerEvents: "none" }}
      >
        <defs>
          <filter id={`halo-${id}`} x="-100%" y="-100%" width="300%" height="300%">
            <feDropShadow dx="0" dy="0" stdDeviation={HALO_RADIUS / 2} floodColor={arcColour} floodOpacity={p.highlight ? 0.42 : 0} />
          </filter>
          <mask id={`mask-${id}`} maskUnits="userSpaceOnUse" x={-r - pad} y={-r - pad} width={d + pad * 2} height={d + pad * 2}>
            <rect x={-r - pad} y={-r - pad} width={d + pad * 2} height={d + pad * 2} fill="white" />
            <circle r={centreDiameter / 2} fill="black" />
          </mask>
        </defs>

        <circle r={r} fill="none" stroke={ink(light, 0.18)} strokeWidth={p.lineWidth} />
        <motion.g animate={{ opacity: refreshing ? 0.3 : 1 }} transition={{ duration: 0.2, ease: "easeOut" }} mask={`url(#mask-${id})`} filter={`url(#halo-${id})`}>
          <Arc r={r} fraction={arcFraction} color={arcColour} width={p.lineWidth} />
        </motion.g>

        {refreshing && (
          <motion.g initial={{ rotate: 0, opacity: 0 }} animate={{ rotate: 360, opacity: 1 }} transition={{ rotate: { duration: 0.85, ease: "linear", repeat: Infinity }, opacity: { duration: 0.2 } }}>
            <Arc r={r} fraction={REFRESH_SWEEP} color={arcColour} width={p.lineWidth} transition={{ duration: 0 }} />
          </motion.g>
        )}

        {second}

        {p.isBusy && p.animatesActivity !== false && (
          <motion.g initial={{ rotate: 0 }} animate={{ rotate: 360 }} transition={{ duration: 1.0, ease: "linear", repeat: Infinity }}>
            <Arc r={busyDiameter / 2} fraction={BUSY_SWEEP} color={ink(light, 1)} width={Math.max(p.lineWidth * 0.5, 1.5)} transition={{ duration: 0 }} />
          </motion.g>
        )}

        {p.windowClockFraction != null && (
          <g>
            <circle r={clockDiameter / 2} fill="none" stroke={ink(light, 0.16)} strokeWidth={CLOCK_LINE_WIDTH * p.scale} />
            <Arc r={clockDiameter / 2} fraction={p.windowClockFraction} color={ink(light, 0.7)} width={CLOCK_LINE_WIDTH * p.scale} transition={{ duration: 0.35, ease: "easeOut" }} />
          </g>
        )}
      </svg>
      <div className="ring-icon">
        <ProviderIcon provider={p.provider} size={centreDiameter * ICON_SCALE} opacity={p.hasReading ? 1 : 0.35} />
      </div>
    </div>
  );
}

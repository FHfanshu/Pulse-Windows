// Ported from upstream Panel/PanelSurface.swift (PanelLight) and FloatingUsagePanelContent.swift
// (`panelScheme`): the light panel (#74), a light solid surface with dark content.
import { createContext, useContext, useId } from "react";
import { FrostedGlass } from "./glass";
import type { Edge } from "./layout";

/**
 * Light only for the solid surface; dark under glass, which is always drawn dark. Upstream pins
 * SwiftUI's colour scheme at the panel's root; here every piece that draws reads it from context.
 */
export const LightPanel = createContext(false);

export const useLightPanel = () => useContext(LightPanel);

/** The panel's content colour at an opacity: white on black, black on the light surface. */
export const ink = (light: boolean, alpha: number) => (light ? `rgba(0,0,0,${alpha})` : `rgba(255,255,255,${alpha})`);

/**
 * The light panel's surface. Not pure white: on a white page that is a hole with a hairline round
 * it, and the slight grey reads as a surface.
 */
export const PanelLight = {
  fill: "rgb(247,247,247)",
  edge: "rgba(0,0,0,0.12)",
  edgeWidth: 1,
} as const;

/**
 * What the panel's shapes are filled with: flat black, flat light, or the glass stand-in.
 *
 * On the light surface the hairline is drawn **inside** the outline: the window is exactly card +
 * rail wide, so a line drawn outside was cut off at the window's edge. It is left off the side
 * lying on the screen's edge (`screenEdge`), where it would be a grey line along the display's
 * own edge.
 */
export function Surface(p: {
  d: string;
  width: number;
  height: number;
  usesGlass: boolean;
  glassTransparency: number;
  light: boolean;
  screenEdge?: Edge | null;
  nativeBackdrop?: boolean;
}) {
  const clip = useId();
  if (p.usesGlass) {
    return <FrostedGlass d={p.d} width={p.width} height={p.height} transparency={p.glassTransparency} nativeBackdrop={p.nativeBackdrop} />;
  }
  if (!p.light) {
    return <path d={p.d} fill="#000" />;
  }
  const w = PanelLight.edgeWidth;
  const keep = {
    x: p.screenEdge === "left" ? w : 0,
    y: p.screenEdge === "top" ? w : 0,
    width: p.width - (p.screenEdge === "left" || p.screenEdge === "right" ? w : 0),
    height: p.height - (p.screenEdge === "top" || p.screenEdge === "bottom" ? w : 0),
  };
  return (
    <>
      <defs>
        <clipPath id={`${clip}-shape`}>
          <path d={p.d} />
        </clipPath>
        <clipPath id={`${clip}-edge`}>
          <rect {...keep} />
        </clipPath>
      </defs>
      <path d={p.d} fill={PanelLight.fill} />
      <g clipPath={`url(#${clip}-edge)`}>
        <path d={p.d} fill="none" stroke={PanelLight.edge} strokeWidth={w * 2} clipPath={`url(#${clip}-shape)`} />
      </g>
    </>
  );
}

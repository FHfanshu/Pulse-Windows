// Windows stand-in for upstream Panel/PanelSurface.swift's Liquid Glass. Upstream draws the `.clear`
// glass variant (nearly see-through) under a dim set by the transparency slider. Windows has no
// backdrop that follows an arbitrary shape (DWM's backdrop and the acrylic accent both ignore window
// regions and fill the whole transparent window), so the shapes are left unblurred: the dim alone,
// plus a faint edge so the outline still reads over a busy background.

/** Upstream `PanelGlass.maximumDim`: the darkest the dimming goes, at transparency 0. */
const MAXIMUM_DIM = 0.6;

export const glassDim = (transparency: number) => (1 - Math.min(Math.max(transparency, 0), 1)) * MAXIMUM_DIM;

/** The glass's edge. */
export const GLASS_EDGE = "rgba(255,255,255,0.14)";

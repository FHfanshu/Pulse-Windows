// Ported from upstream Sources/Pulse/Panel/PanelPlacement.swift.
//! Where the panel window sits and where the rail sits inside it.
//!
//! Coordinates are DIPs, **y down** (upstream is AppKit y-up; every formula
//! below is the y-down equivalent). `vertical_ratio` is measured from the top.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub fn is_vertical(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Dock {
    Edge(Edge),
    /// Floating, lying along this axis: `true` = vertical.
    Floating(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

/// Sizes the UI computed from its layout tokens (`layout.ts`), per axis and docking.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Geometry {
    pub vertical_docked: Shapes,
    pub vertical_free: Shapes,
    pub horizontal_docked: Shapes,
    pub horizontal_free: Shapes,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Shapes {
    pub panel: Size,
    pub rail: Size,
}

impl Geometry {
    pub fn for_dock(&self, vertical: bool, docked: bool) -> Shapes {
        match (vertical, docked) {
            (true, true) => self.vertical_docked,
            (true, false) => self.vertical_free,
            (false, true) => self.horizontal_docked,
            (false, false) => self.horizontal_free,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub dock: Dock,
    pub horizontal_ratio: f64,
    pub vertical_ratio: f64,
    /// Monitor device name, when one was chosen.
    pub display: Option<String>,
}

impl Default for Placement {
    fn default() -> Self {
        Self { dock: Dock::Edge(Edge::Right), horizontal_ratio: 1.0, vertical_ratio: 0.5, display: None }
    }
}

pub const DOCK_DISTANCE: f64 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub frame: Rect,
    /// Rail rect in window coordinates.
    pub rail: Rect,
    pub edge: Edge,
    pub docked: bool,
    /// Windows difference: this layout follows a change of the rail's shape (dock <-> float, a turn
    /// between axes), so the window was grown to hold the old shape too and the UI should morph.
    pub morph: bool,
    /// Windows difference: the display's usable area (DIPs, screen coordinates), so the UI can tell
    /// how much of the window the screen shows. The window is as tall as the tallest card and a
    /// small display can be shorter than that.
    pub visible: Rect,
}

impl Layout {
    /// Whether the rail changes shape between `self` and `next`: docks or undocks, or turns between
    /// axes. Another docked edge on the same axis, or a floating rail whose card side flips, is the
    /// same shape (mirrored or not drawn differently) and is not morphed.
    pub fn reshapes(&self, next: &Layout) -> bool {
        self.docked != next.docked || self.edge.is_vertical() != next.edge.is_vertical()
    }


}

impl Placement {
    pub fn edge(&self) -> Edge {
        match self.dock {
            Dock::Edge(e) => e,
            Dock::Floating(true) => {
                if self.horizontal_ratio < 0.5 {
                    Edge::Left
                } else {
                    Edge::Right
                }
            }
            Dock::Floating(false) => {
                if self.vertical_ratio < 0.5 {
                    Edge::Top
                } else {
                    Edge::Bottom
                }
            }
        }
    }

    pub fn is_docked(&self) -> bool {
        matches!(self.dock, Dock::Edge(_))
    }

    pub fn layout(&self, visible: Rect, geometry: &Geometry) -> Layout {
        let edge = self.edge();
        let docked = self.is_docked();
        let shapes = geometry.for_dock(edge.is_vertical(), docked);
        let (panel, rail) = (shapes.panel, shapes.rail);
        let h_ratio = self.horizontal_ratio.clamp(0.0, 1.0);
        let v_ratio = self.vertical_ratio.clamp(0.0, 1.0);

        let clamp = |v: f64, lo: f64, hi: f64| v.max(lo).min(hi.max(lo));
        let free_x = || {
            clamp(
                visible.x + h_ratio * (visible.w - rail.w).max(0.0),
                visible.x + DOCK_DISTANCE,
                visible.right() - rail.w - DOCK_DISTANCE,
            )
        };
        let rail_top_by_ratio = visible.y + v_ratio * (visible.h - rail.h).max(0.0);

        let (frame, rail_origin) = match self.dock {
            Dock::Edge(Edge::Top) | Dock::Edge(Edge::Bottom) => {
                let rail_x = clamp(
                    visible.x + h_ratio * (visible.w - rail.w).max(0.0),
                    visible.x,
                    visible.right() - rail.w,
                );
                let window_x = clamp(rail_x + rail.w / 2.0 - panel.w / 2.0, visible.x, visible.right() - panel.w);
                if edge == Edge::Top {
                    (Rect { x: window_x, y: visible.y, w: panel.w, h: panel.h }, (rail_x, visible.y))
                } else {
                    let window_y = visible.bottom() - panel.h;
                    (
                        Rect { x: window_x, y: window_y, w: panel.w, h: panel.h },
                        (rail_x, visible.bottom() - rail.h),
                    )
                }
            }
            Dock::Floating(false) => {
                let rail_x = free_x();
                let rail_y = rail_top_by_ratio;
                let window_x = clamp(rail_x + rail.w / 2.0 - panel.w / 2.0, visible.x, visible.right() - panel.w);
                // Card opens below a rail in the top half, above one in the bottom half.
                let wanted_y = if edge == Edge::Bottom { rail_y + rail.h - panel.h } else { rail_y };
                let window_y = clamp(wanted_y, visible.y, visible.bottom() - panel.h);
                (Rect { x: window_x, y: window_y, w: panel.w, h: panel.h }, (rail_x, rail_y))
            }
            _ => {
                let rail_x = match self.dock {
                    Dock::Edge(Edge::Left) => visible.x,
                    Dock::Edge(Edge::Right) => visible.right() - rail.w,
                    _ => free_x(),
                };
                let rail_y = rail_top_by_ratio;
                let window_x = if edge == Edge::Left { rail_x } else { rail_x + rail.w - panel.w };
                let window_y = clamp(rail_y + rail.h / 2.0 - panel.h / 2.0, visible.y, visible.bottom() - panel.h);
                (Rect { x: window_x, y: window_y, w: panel.w, h: panel.h }, (rail_x, rail_y))
            }
        };

        let rail_rect = Rect {
            x: (rail_origin.0 - frame.x).clamp(0.0, (frame.w - rail.w).max(0.0)),
            y: (rail_origin.1 - frame.y).clamp(0.0, (frame.h - rail.h).max(0.0)),
            w: rail.w,
            h: rail.h,
        };
        Layout { frame, rail: rail_rect, edge, docked, morph: false, visible }
    }

    /// Ratios for a rail whose top-left is at `origin` (screen DIPs).
    pub fn ratios(origin: (f64, f64), visible: Rect, rail: Size) -> (f64, f64) {
        let across = (visible.w - rail.w).max(0.0);
        let down = (visible.h - rail.h).max(0.0);
        (
            if across > 0.0 { ((origin.0 - visible.x) / across).clamp(0.0, 1.0) } else { 0.5 },
            if down > 0.0 { ((origin.1 - visible.y) / down).clamp(0.0, 1.0) } else { 0.5 },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> Geometry {
        let v = Shapes { panel: Size { w: 342.0, h: 600.0 }, rail: Size { w: 64.0, h: 400.0 } };
        let h = Shapes { panel: Size { w: 600.0, h: 400.0 }, rail: Size { w: 400.0, h: 64.0 } };
        Geometry { vertical_docked: v, vertical_free: v, horizontal_docked: h, horizontal_free: h }
    }

    #[test]
    fn right_dock_centres_rail_and_hugs_edge() {
        let visible = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };
        let layout = Placement::default().layout(visible, &geometry());
        assert_eq!(layout.frame.right(), 1920.0);
        // The UI is told how much of the screen there is, to keep a card within it.
        assert_eq!(layout.visible, visible);
        assert_eq!(layout.rail.x + layout.rail.w, layout.frame.w);
        let rail_screen_top = layout.frame.y + layout.rail.y;
        assert!((rail_screen_top - 320.0).abs() < 0.01);
    }

    fn at(placement: Placement) -> Layout {
        placement.layout(Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 }, &geometry())
    }

    #[test]
    fn reshapes_on_dock_and_axis_changes_only() {
        let docked = at(Placement::default());
        let floating = at(Placement { dock: Dock::Floating(true), horizontal_ratio: 0.5, ..Placement::default() });
        let turned = at(Placement { dock: Dock::Edge(Edge::Top), ..Placement::default() });
        let left = at(Placement { dock: Dock::Edge(Edge::Left), ..Placement::default() });
        assert!(docked.reshapes(&floating) && floating.reshapes(&docked));
        assert!(docked.reshapes(&turned) && turned.reshapes(&docked));
        assert!(!docked.reshapes(&left));
        assert!(!docked.reshapes(&docked));
    }

}

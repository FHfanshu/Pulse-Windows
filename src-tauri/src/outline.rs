//! The panel's outlines as the UI draws them: SVG path strings made by `ui/src/panel/shapes.ts`,
//! which only ever write absolute `M`, `L`, `C` and `Z`. Parsed here so the native acrylic can be
//! clipped to exactly the same shape (`win::HostBackdrop`).

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    Move([f32; 2]),
    Line([f32; 2]),
    Cubic([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

/// One outline from the UI: the path in the shape's own space, and where that space sits in the
/// window (DIPs): scaled by `s`, then moved by `(x, y)`. `o` is the shape's opacity (the card fades).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Outline {
    pub d: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "one")]
    pub s: f64,
    #[serde(default = "one")]
    pub o: f64,
}

fn one() -> f64 {
    1.0
}

/// The latest outlines of the shapes that are showing.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Shapes {
    /// Counts the UI's updates, so a late one is not applied over a newer.
    #[serde(default)]
    pub seq: u64,
    pub rail: Option<Outline>,
    pub card: Option<Outline>,
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b',' | b'\t' | b'\n' | b'\r')
}

/// `count` numbers from `at`, which moves past them.
fn numbers(d: &str, at: &mut usize, count: usize) -> Option<Vec<f32>> {
    let bytes = d.as_bytes();
    let mut v = Vec::with_capacity(count);
    while v.len() < count {
        while *at < bytes.len() && is_space(bytes[*at]) {
            *at += 1;
        }
        let start = *at;
        while *at < bytes.len() && matches!(bytes[*at], b'0'..=b'9' | b'.' | b'-' | b'+' | b'e' | b'E') {
            // A sign inside a number is only an exponent's; otherwise it starts the next number.
            if *at > start && matches!(bytes[*at], b'-' | b'+') && !matches!(bytes[*at - 1], b'e' | b'E') {
                break;
            }
            *at += 1;
        }
        let n: f32 = d[start..*at].parse().ok()?;
        if !n.is_finite() {
            return None;
        }
        v.push(n);
    }
    Some(v)
}

/// Parses `d`. `None` for anything but well-formed absolute `M`/`L`/`C`/`Z`, or non-finite numbers.
pub fn parse(d: &str) -> Option<Vec<Seg>> {
    let bytes = d.as_bytes();
    let mut at = 0;
    let mut out = Vec::new();
    let mut open = false;
    while at < bytes.len() {
        match bytes[at] {
            b if is_space(b) => at += 1,
            b'M' => {
                at += 1;
                let n = numbers(d, &mut at, 2)?;
                out.push(Seg::Move([n[0], n[1]]));
                open = true;
            }
            b'L' if open => {
                at += 1;
                let n = numbers(d, &mut at, 2)?;
                out.push(Seg::Line([n[0], n[1]]));
            }
            b'C' if open => {
                at += 1;
                let n = numbers(d, &mut at, 6)?;
                out.push(Seg::Cubic([n[0], n[1]], [n[2], n[3]], [n[4], n[5]]));
            }
            b'Z' if open => {
                at += 1;
                out.push(Seg::Close);
                open = false;
            }
            _ => return None,
        }
    }
    (!out.is_empty()).then_some(out)
}

/// `p * scale + offset`, then times `px` (the monitor's scale): the path in physical pixels.
pub fn place(segs: &[Seg], outline: &Outline, px: f64) -> Vec<Seg> {
    let k = outline.s * px;
    let (ox, oy) = (outline.x * px, outline.y * px);
    let at = |p: [f32; 2]| [(p[0] as f64 * k + ox) as f32, (p[1] as f64 * k + oy) as f32];
    segs.iter()
        .map(|s| match *s {
            Seg::Move(p) => Seg::Move(at(p)),
            Seg::Line(p) => Seg::Line(at(p)),
            Seg::Cubic(a, b, c) => Seg::Cubic(at(a), at(b), at(c)),
            Seg::Close => Seg::Close,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_shapes_ts_writes() {
        let segs = parse("M3.5 2L10 2C12 2 13 3 13 5.25L13 -1.5Z").unwrap();
        assert_eq!(
            segs,
            vec![
                Seg::Move([3.5, 2.0]),
                Seg::Line([10.0, 2.0]),
                Seg::Cubic([12.0, 2.0], [13.0, 3.0], [13.0, 5.25]),
                Seg::Line([13.0, -1.5]),
                Seg::Close,
            ]
        );
    }

    #[test]
    fn numbers_may_run_into_each_other_at_a_sign() {
        assert_eq!(parse("M1-2L-3.5-4Z").unwrap(), vec![Seg::Move([1.0, -2.0]), Seg::Line([-3.5, -4.0]), Seg::Close]);
        assert_eq!(parse("M1e-3 2E2Z").unwrap()[0], Seg::Move([0.001, 200.0]));
    }

    #[test]
    fn several_figures() {
        let segs = parse("M0 0L1 1ZM5 5L6 6Z").unwrap();
        assert_eq!(segs.iter().filter(|s| matches!(s, Seg::Move(_))).count(), 2);
        assert_eq!(segs.len(), 6);
    }

    #[test]
    fn rejects_what_it_cannot_draw() {
        assert!(parse("").is_none());
        assert!(parse("L1 1").is_none()); // no current point
        assert!(parse("M0 0Q1 1 2 2").is_none());
        assert!(parse("M0 0l1 1").is_none()); // relative
        assert!(parse("M0").is_none());
        assert!(parse("M0 0C1 1 2 2 3").is_none());
        assert!(parse("M0 0LNaN 1").is_none());
        assert!(parse("M0 0Z L1 1").is_none());
    }

    #[test]
    fn placement_scales_then_moves_then_converts_to_pixels() {
        let segs = parse("M10 20L30 40Z").unwrap();
        let outline = Outline { d: String::new(), x: 100.0, y: 50.0, s: 0.5, o: 1.0 };
        assert_eq!(place(&segs, &outline, 2.0), vec![Seg::Move([210.0, 120.0]), Seg::Line([230.0, 140.0]), Seg::Close]);
    }

    #[test]
    fn shapes_come_from_the_ui_as_json() {
        let shapes: Shapes = serde_json::from_str(r#"{"rail":{"d":"M0 0Z","x":1,"y":2},"card":null}"#).unwrap();
        assert_eq!(shapes.rail.unwrap().s, 1.0);
        assert!(shapes.card.is_none());
    }
}

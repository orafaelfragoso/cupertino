//! Procedural cursor set: simple vector cursors authored for this crate, drawn directly at the
//! target scale with `tiny-skia` (no PDF, no third-party input). All geometry is in points; each
//! cursor has its own canvas ([`Kind::canvas`], 32 × 32 unless its role sets another) and its
//! hotspot is in the same unit, from the canvas's top-left corner.
//!
//! The designs are plain: a black arrow with a white outline, linework with a white halo, a white
//! hand with a black outline, small badges (plus, arrow, menu, question mark, spinner) at the
//! lower right of the arrow, and double-headed arrows for resizing. They exist so that the tool
//! produces a complete theme when no reference input is available, not to imitate any particular
//! set. Shadows use [`Shadow::COMMON`] (approximate).

use crate::shadow::Shadow;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform,
};

pub const CANVAS: f64 = 32.0;
const SPINNER_FRAMES: usize = 12;
const SPINNER_DELAY_MS: u32 = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Default,
    Pointer,
    Text,
    VerticalText,
    Grab,
    Grabbing,
    NotAllowed,
    Crosshair,
    Copy,
    Alias,
    ContextMenu,
    Help,
    Progress,
    Cell,
    Move,
    ZoomIn,
    ZoomOut,
    ResizeN,
    ResizeS,
    ResizeE,
    ResizeW,
    ResizeNE,
    ResizeNW,
    ResizeSE,
    ResizeSW,
    ResizeEW,
    ResizeNS,
    ResizeNESW,
    ResizeNWSE,
    ColResize,
    RowResize,
}

impl Kind {
    pub const ALL: [Kind; 31] = [
        Kind::Default,
        Kind::Pointer,
        Kind::Text,
        Kind::VerticalText,
        Kind::Grab,
        Kind::Grabbing,
        Kind::NotAllowed,
        Kind::Crosshair,
        Kind::Copy,
        Kind::Alias,
        Kind::ContextMenu,
        Kind::Help,
        Kind::Progress,
        Kind::Cell,
        Kind::Move,
        Kind::ZoomIn,
        Kind::ZoomOut,
        Kind::ResizeN,
        Kind::ResizeS,
        Kind::ResizeE,
        Kind::ResizeW,
        Kind::ResizeNE,
        Kind::ResizeNW,
        Kind::ResizeSE,
        Kind::ResizeSW,
        Kind::ResizeEW,
        Kind::ResizeNS,
        Kind::ResizeNESW,
        Kind::ResizeNWSE,
        Kind::ColResize,
        Kind::RowResize,
    ];

    /// Primary freedesktop name (the name of the XCursor file).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Default => "default",
            Kind::Pointer => "pointer",
            Kind::Text => "text",
            Kind::VerticalText => "vertical-text",
            Kind::Grab => "grab",
            Kind::Grabbing => "grabbing",
            Kind::NotAllowed => "not-allowed",
            Kind::Crosshair => "crosshair",
            Kind::Copy => "copy",
            Kind::Alias => "alias",
            Kind::ContextMenu => "context-menu",
            Kind::Help => "help",
            Kind::Progress => "progress",
            Kind::Cell => "cell",
            Kind::Move => "move",
            Kind::ZoomIn => "zoom-in",
            Kind::ZoomOut => "zoom-out",
            Kind::ResizeN => "n-resize",
            Kind::ResizeS => "s-resize",
            Kind::ResizeE => "e-resize",
            Kind::ResizeW => "w-resize",
            Kind::ResizeNE => "ne-resize",
            Kind::ResizeNW => "nw-resize",
            Kind::ResizeSE => "se-resize",
            Kind::ResizeSW => "sw-resize",
            Kind::ResizeEW => "ew-resize",
            Kind::ResizeNS => "ns-resize",
            Kind::ResizeNESW => "nesw-resize",
            Kind::ResizeNWSE => "nwse-resize",
            Kind::ColResize => "col-resize",
            Kind::RowResize => "row-resize",
        }
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.name() == name)
    }

    /// Hotspot in points from the canvas's top-left corner: the arrow's tip, the I-beam's centre,
    /// the pointing finger's tip, the middle of the hands and of the resize bars.
    pub fn hotspot(self) -> (f64, f64) {
        match self {
            Kind::Default
            | Kind::Copy
            | Kind::Alias
            | Kind::ContextMenu
            | Kind::Help
            | Kind::Progress => (5.0, 5.0),
            Kind::Text => (12.0, 11.0),
            Kind::Pointer => (12.0, 8.0),
            Kind::Grab | Kind::Grabbing => (15.0, 16.0),
            Kind::ColResize => (15.0, 12.0),
            Kind::RowResize => (12.0, 14.0),
            Kind::ZoomIn | Kind::ZoomOut => (13.0, 13.0),
            _ => (16.0, 16.0),
        }
    }

    /// Size of the image in points, transparent margin included: the arrow and its badged
    /// variants 28 × 40, the I-beam 23 × 22, the column and row resize cursors 30 × 24 and
    /// 24 × 28; every other cursor (hands included) 32 × 32.
    pub fn canvas(self) -> (f64, f64) {
        match self {
            Kind::Default
            | Kind::Copy
            | Kind::Alias
            | Kind::ContextMenu
            | Kind::Help
            | Kind::Progress => (28.0, 40.0),
            Kind::Text => (23.0, 22.0),
            Kind::ColResize => (30.0, 24.0),
            Kind::RowResize => (24.0, 28.0),
            _ => (CANVAS, CANVAS),
        }
    }

    /// Where the drawing's own 32 × 32 design space sits on the canvas, in points: it moves the
    /// shape's anchor (the arrow's tip, a hand's middle, a resize bar's middle) onto the hotspot.
    fn offset(self) -> (f32, f32) {
        match self {
            Kind::Default
            | Kind::Copy
            | Kind::Alias
            | Kind::ContextMenu
            | Kind::Help
            | Kind::Progress => (1.0, 2.0),
            Kind::Pointer => (-2.0, 4.0),
            Kind::Grab | Kind::Grabbing => (-1.0, 0.0),
            Kind::ColResize => (-1.0, -4.0),
            Kind::RowResize => (-4.0, -2.0),
            _ => (0.0, 0.0),
        }
    }

    pub fn frames(self) -> usize {
        if self == Kind::Progress {
            SPINNER_FRAMES
        } else {
            1
        }
    }

    pub fn delay_ms(self) -> u32 {
        if self == Kind::Progress {
            SPINNER_DELAY_MS
        } else {
            0
        }
    }

    pub fn shadow(self) -> Option<Shadow> {
        Some(Shadow::COMMON)
    }
}

const BLACK: Color = Color::BLACK;
const WHITE: Color = Color::WHITE;

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    p.set_color(c);
    p
}

fn poly(pts: &[(f32, f32)], close: bool) -> Path {
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    if close {
        pb.close();
    }
    pb.finish().expect("valid polygon")
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish().expect("valid rounded rectangle")
}

fn circle(cx: f32, cy: f32, r: f32) -> Path {
    PathBuilder::from_circle(cx, cy, r).expect("valid circle")
}

struct Ctx<'a> {
    pm: &'a mut Pixmap,
    ts: Transform,
}

impl Ctx<'_> {
    fn fill(&mut self, p: &Path, c: Color) {
        self.pm
            .fill_path(p, &paint(c), FillRule::Winding, self.ts, None);
    }

    fn stroke(&mut self, p: &Path, c: Color, w: f32, cap: LineCap) {
        let s = Stroke {
            width: w,
            line_cap: cap,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        self.pm.stroke_path(p, &paint(c), &s, self.ts, None);
    }

    /// Shapes with one colour and a halo: halos first so overlapping shapes merge cleanly.
    fn outlined(&mut self, shapes: &[Path], fill: Color, outline: Color, outline_w: f32) {
        for s in shapes {
            self.stroke(s, outline, outline_w * 2.0, LineCap::Round);
        }
        for s in shapes {
            self.fill(s, fill);
        }
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), w: f32, c: Color, cap: LineCap) {
        self.stroke(&poly(&[a, b], false), c, w, cap);
    }

    /// Line with a white halo.
    fn halo_line(&mut self, a: (f32, f32), b: (f32, f32), w: f32) {
        self.line(a, b, w + 2.4, WHITE, LineCap::Butt);
        self.line(a, b, w, BLACK, LineCap::Butt);
    }
}

fn arrow_path() -> Path {
    poly(
        &[
            (4.0, 3.0),
            (4.0, 22.5),
            (8.8, 18.2),
            (12.4, 26.2),
            (15.9, 24.7),
            (12.4, 16.9),
            (19.0, 16.9),
        ],
        true,
    )
}

/// Double-headed (or single-headed) arrow along +x, centred at the origin, total length `len`.
fn double_arrow(len: f32, both: bool) -> Path {
    let (h, head, shaft) = (len / 2.0, 5.5f32, 1.6f32);
    let mut pts = vec![
        (h, 0.0),
        (h - head, -head),
        (h - head, -shaft),
        (-h + if both { head } else { 0.0 }, -shaft),
    ];
    if both {
        pts.extend([(-h + head, -head), (-h, 0.0), (-h + head, head)]);
    } else {
        pts.extend([(-h, -shaft), (-h, shaft)]);
    }
    pts.extend([
        (-h + if both { head } else { 0.0 }, shaft),
        (h - head, shaft),
        (h - head, head),
    ]);
    poly(&pts, true)
}

/// The badge of an arrow variant, below and right of the arrow's tail, inside its 28 × 40 canvas.
fn badge(c: &mut Ctx, kind: Kind, frame: usize) {
    let (cx, cy, r) = (19.5f32, 28.0f32, 6.0f32);
    match kind {
        Kind::Copy => {
            c.fill(&circle(cx, cy, r + 1.0), WHITE);
            c.fill(&circle(cx, cy, r), Color::from_rgba8(46, 176, 76, 255));
            c.line((cx - 3.0, cy), (cx + 3.0, cy), 1.8, WHITE, LineCap::Round);
            c.line((cx, cy - 3.0), (cx, cy + 3.0), 1.8, WHITE, LineCap::Round);
        }
        Kind::Alias => {
            c.fill(&circle(cx, cy, r + 1.0), WHITE);
            c.fill(&circle(cx, cy, r), Color::from_rgba8(70, 70, 76, 255));
            let mut pb = PathBuilder::new();
            pb.move_to(cx - 3.0, cy + 2.5);
            pb.quad_to(cx - 3.0, cy - 2.5, cx + 2.0, cy - 2.5);
            c.stroke(&pb.finish().expect("curve"), WHITE, 1.6, LineCap::Round);
            c.fill(
                &poly(
                    &[(cx + 1.0, cy - 5.0), (cx + 4.6, cy - 2.5), (cx + 1.0, cy)],
                    true,
                ),
                WHITE,
            );
        }
        Kind::ContextMenu => {
            c.fill(&rrect(cx - 6.0, cy - 5.5, 12.0, 11.0, 2.0), WHITE);
            c.fill(
                &rrect(cx - 5.0, cy - 4.5, 10.0, 9.0, 1.4),
                Color::from_rgba8(70, 70, 76, 255),
            );
            for dy in [-2.4f32, 0.0, 2.4] {
                c.line(
                    (cx - 3.2, cy + dy),
                    (cx + 3.2, cy + dy),
                    1.0,
                    WHITE,
                    LineCap::Butt,
                );
            }
        }
        Kind::Help => {
            c.fill(&circle(cx, cy, r + 1.0), WHITE);
            c.fill(&circle(cx, cy, r), Color::from_rgba8(30, 110, 235, 255));
            let mut pb = PathBuilder::new();
            pb.move_to(cx - 2.3, cy - 2.2);
            pb.cubic_to(cx - 2.3, cy - 5.4, cx + 2.6, cy - 5.4, cx + 2.6, cy - 2.0);
            pb.cubic_to(cx + 2.6, cy - 0.4, cx, cy - 0.3, cx, cy + 1.6);
            c.stroke(
                &pb.finish().expect("question mark"),
                WHITE,
                1.5,
                LineCap::Round,
            );
            c.fill(&circle(cx, cy + 3.7, 0.95), WHITE);
        }
        Kind::Progress => {
            c.fill(&circle(cx, cy, r + 1.0), WHITE);
            for i in 0..8usize {
                let ang = (i as f32 / 8.0) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let lead = (frame * 8 / SPINNER_FRAMES) % 8;
                let age = (lead + 8 - i) % 8;
                let a = 1.0 - age as f32 * 0.11;
                let (px, py) = (cx + 4.0 * ang.cos(), cy + 4.0 * ang.sin());
                c.fill(
                    &circle(px, py, 1.05),
                    Color::from_rgba(0.15, 0.15, 0.18, a.clamp(0.1, 1.0)).unwrap_or(BLACK),
                );
            }
        }
        _ => {}
    }
}

fn pointing_hand() -> Vec<Path> {
    vec![
        rrect(9.0, 14.0, 15.0, 12.0, 4.5),
        rrect(13.0, 4.0, 3.4, 14.0, 1.7),
        rrect(16.4, 12.5, 3.2, 6.0, 1.6),
        rrect(19.6, 13.5, 3.2, 5.5, 1.6),
        rrect(5.5, 15.5, 3.2, 8.0, 1.6),
    ]
}

fn hand_shapes(closed: bool) -> Vec<Path> {
    if closed {
        vec![
            rrect(8.0, 13.0, 16.0, 11.0, 4.0),
            rrect(8.0, 11.0, 4.0, 6.0, 2.0),
            rrect(12.0, 10.0, 4.0, 6.0, 2.0),
            rrect(16.0, 10.0, 4.0, 6.0, 2.0),
            rrect(20.0, 11.0, 4.0, 6.0, 2.0),
        ]
    } else {
        vec![
            rrect(9.0, 14.0, 15.0, 12.0, 4.5),
            rrect(9.5, 6.0, 3.2, 12.0, 1.6),
            rrect(13.2, 4.0, 3.2, 14.0, 1.6),
            rrect(16.9, 4.5, 3.2, 13.5, 1.6),
            rrect(20.6, 7.0, 3.2, 11.0, 1.6),
            rrect(5.5, 15.5, 3.2, 8.0, 1.6),
        ]
    }
}

fn magnifier(c: &mut Ctx, plus: bool) {
    let (cx, cy, r) = (13.0f32, 13.0f32, 7.0f32);
    c.line((18.5, 18.5), (26.0, 26.0), 5.6, WHITE, LineCap::Round);
    c.line((18.5, 18.5), (26.0, 26.0), 3.0, BLACK, LineCap::Round);
    c.fill(&circle(cx, cy, r + 1.8), WHITE);
    c.fill(
        &circle(cx, cy, r - 1.2),
        Color::from_rgba8(235, 245, 255, 235),
    );
    c.stroke(&circle(cx, cy, r), BLACK, 2.0, LineCap::Butt);
    c.line((cx - 3.2, cy), (cx + 3.2, cy), 1.6, BLACK, LineCap::Butt);
    if plus {
        c.line((cx, cy - 3.2), (cx, cy + 3.2), 1.6, BLACK, LineCap::Butt);
    }
}

fn resize_shape(kind: Kind) -> (Path, f32, bool) {
    match kind {
        Kind::ResizeEW | Kind::ResizeNS | Kind::ColResize | Kind::RowResize => {
            (double_arrow(26.0, true), 0.0, false)
        }
        Kind::ResizeNESW => (double_arrow(26.0, true), -45.0, false),
        Kind::ResizeNWSE => (double_arrow(26.0, true), 45.0, false),
        Kind::ResizeE => (double_arrow(22.0, false), 0.0, true),
        Kind::ResizeW => (double_arrow(22.0, false), 180.0, true),
        Kind::ResizeN => (double_arrow(22.0, false), -90.0, true),
        Kind::ResizeS => (double_arrow(22.0, false), 90.0, true),
        Kind::ResizeNE => (double_arrow(22.0, false), -45.0, true),
        Kind::ResizeNW => (double_arrow(22.0, false), -135.0, true),
        Kind::ResizeSE => (double_arrow(22.0, false), 45.0, true),
        _ => (double_arrow(22.0, false), 135.0, true),
    }
}

/// Draw `kind` (animation frame `frame`) at `scale` pixels per point on its canvas.
pub fn render(kind: Kind, scale: f32, frame: usize) -> Pixmap {
    let (w, h) = kind.canvas();
    let px = |points: f64| (points as f32 * scale).round().max(1.0) as u32;
    let mut pm = Pixmap::new(px(w), px(h)).expect("non-empty canvas");
    let (dx, dy) = kind.offset();
    let mut c = Ctx {
        pm: &mut pm,
        ts: Transform::from_scale(scale, scale).pre_translate(dx, dy),
    };
    match kind {
        Kind::Default => c.outlined(&[arrow_path()], BLACK, WHITE, 1.1),
        Kind::Copy | Kind::Alias | Kind::ContextMenu | Kind::Help | Kind::Progress => {
            c.outlined(&[arrow_path()], BLACK, WHITE, 1.1);
            badge(&mut c, kind, frame);
        }
        Kind::Pointer => c.outlined(&pointing_hand(), WHITE, BLACK, 0.9),
        Kind::Grab => c.outlined(&hand_shapes(false), WHITE, BLACK, 0.9),
        Kind::Grabbing => c.outlined(&hand_shapes(true), WHITE, BLACK, 0.9),
        Kind::Text => {
            // centred on the hotspot (12, 11) of the 23 × 22 canvas
            c.halo_line((12.0, 3.5), (12.0, 18.5), 1.5);
            c.halo_line((8.5, 2.75), (15.5, 2.75), 1.5);
            c.halo_line((8.5, 19.25), (15.5, 19.25), 1.5);
        }
        Kind::VerticalText => {
            c.halo_line((6.5, 16.0), (25.5, 16.0), 1.5);
            c.halo_line((5.5, 12.5), (5.5, 19.5), 1.5);
            c.halo_line((26.5, 12.5), (26.5, 19.5), 1.5);
        }
        Kind::Crosshair => {
            c.halo_line((16.0, 4.0), (16.0, 13.0), 1.4);
            c.halo_line((16.0, 19.0), (16.0, 28.0), 1.4);
            c.halo_line((4.0, 16.0), (13.0, 16.0), 1.4);
            c.halo_line((19.0, 16.0), (28.0, 16.0), 1.4);
        }
        Kind::Cell => {
            c.outlined(
                &[
                    rrect(14.2, 6.0, 3.6, 20.0, 0.6),
                    rrect(6.0, 14.2, 20.0, 3.6, 0.6),
                ],
                WHITE,
                BLACK,
                1.0,
            );
        }
        Kind::NotAllowed => {
            let red = Color::from_rgba8(224, 52, 56, 255);
            c.stroke(&circle(16.0, 16.0, 9.5), WHITE, 7.4, LineCap::Butt);
            c.line((9.3, 9.3), (22.7, 22.7), 7.4, WHITE, LineCap::Butt);
            c.stroke(&circle(16.0, 16.0, 9.5), red, 3.4, LineCap::Butt);
            c.line((9.3, 9.3), (22.7, 22.7), 3.4, red, LineCap::Butt);
        }
        Kind::Move => {
            let arm = |angle: f32| {
                let ts = Transform::from_translate(16.0, 16.0).pre_rotate(angle);
                let p = double_arrow(15.0, false)
                    .transform(Transform::from_translate(7.5, 0.0))
                    .expect("transform");
                p.transform(ts).expect("transform")
            };
            let shapes: Vec<Path> = [0.0, 90.0, 180.0, 270.0].into_iter().map(arm).collect();
            c.outlined(&shapes, BLACK, WHITE, 1.1);
        }
        Kind::ZoomIn => magnifier(&mut c, true),
        Kind::ZoomOut => magnifier(&mut c, false),
        Kind::ColResize | Kind::RowResize => {
            let (shape, _, _) = resize_shape(kind);
            let angle = if kind == Kind::RowResize { 90.0 } else { 0.0 };
            let ts = Transform::from_translate(16.0, 16.0).pre_rotate(angle);
            let bar = rrect(-1.0, -9.0, 2.0, 18.0, 0.6)
                .transform(ts)
                .expect("transform");
            let shape = shape.transform(ts).expect("transform");
            c.outlined(&[shape, bar], BLACK, WHITE, 1.1);
        }
        _ => {
            let (shape, angle, single) = resize_shape(kind);
            let ts = Transform::from_translate(16.0, 16.0).pre_rotate(angle);
            let mut shapes = vec![shape.transform(ts).expect("transform")];
            if single {
                let bar = rrect(11.0, -7.0, 2.4, 14.0, 0.8)
                    .transform(ts)
                    .expect("transform");
                shapes.push(bar);
            }
            c.outlined(&shapes, BLACK, WHITE, 1.1);
        }
    }
    pm
}

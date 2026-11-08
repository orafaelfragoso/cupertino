//! The tile held by the pointer, settling into its slot after a drop or fading after a removal, with its "Remove"
//! label and the removal puff: one window over the whole output that takes no input, so the icon can leave the bar's
//! window while the bar keeps the pointer until the release.

use cupertino_kit::core::{
    CommandRegistry, Element, Key, PaintCx, Role, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::platform::{DecorationMode, WindowSpec};

use super::bar::{artwork_rect, indicator_rect, INDICATOR, INDICATOR_DARK, INDICATOR_LIGHT};
use super::paint::{self, Root};
use super::{IconSet, Input, DRAG_APP_ID};
use crate::config::Orientation;

/// The label above a dragged icon once dropping it would remove it, its size, its pill's padding and height, and its
/// gap to the icon.
pub const REMOVE_LABEL: &str = "Remove";
const REMOVE_LABEL_PT: f64 = 14.0;
const REMOVE_LABEL_WEIGHT: u16 = 400;
const REMOVE_LABEL_PAD_X: f64 = 10.0;
const REMOVE_LABEL_HEIGHT: f64 = 24.0;
const REMOVE_LABEL_GAP: f64 = 4.0;
/// The pill's fill, light and dark.
const PILL_LIGHT: paint::Rgba = [0.96, 0.96, 0.97, 0.9];
const PILL_DARK: paint::Rgba = [0.16, 0.16, 0.18, 0.88];
/// The label's colour, light and dark.
const LABEL_LIGHT: paint::Rgba = [0.0, 0.0, 0.0, 0.85];
const LABEL_DARK: paint::Rgba = [1.0, 1.0, 1.0, 0.9];

/// Number of puffs in the removal effect, the share of the tile size they spread to, and their radius at the start
/// and at the end, as shares of the tile size.
const POOF_PUFFS: usize = 6;
const POOF_SPREAD: f64 = 0.6;
const POOF_RADIUS_FROM: f64 = 0.12;
const POOF_RADIUS_TO: f64 = 0.3;
/// The puffs' alpha at the start, falling to 0 at the end.
const POOF_ALPHA: f64 = 0.6;
/// The puffs' tint, light and dark.
const POOF_LIGHT: paint::Rgba = [0.62, 0.62, 0.66, 1.0];
const POOF_DARK: paint::Rgba = [0.85, 0.85, 0.88, 1.0];

/// The dragged tile, in output points.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub rect: Rect,
    pub icon: IconSet,
    pub opacity: f64,
    /// Opacity of the "Remove" label.
    pub label: f64,
    /// Opacity of the running indicator.
    pub indicator: f64,
}

/// The removal puff, in output points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Puff {
    pub centre: Point,
    pub size: f64,
    /// 0 to 1.
    pub progress: f64,
}

pub struct DragView {
    pub output: u32,
    /// The output's size.
    pub size: Size,
    pub orientation: Orientation,
    pub held: Option<Held>,
    pub puff: Option<Puff>,
    /// Changes every frame something here moves.
    pub revision: u64,
    /// Something here moves by itself: the window sends [`Input::Frame`] at every frame.
    pub ticking: bool,
}

pub fn key() -> WindowKey {
    WindowKey(Key::from("drag"))
}

pub fn window<M: Clone + 'static>(view: DragView, input: fn(Input) -> M) -> Window<M> {
    let DragView {
        output,
        size,
        orientation,
        held,
        puff,
        revision,
        ticking,
    } = view;
    let held_icon = held.clone();
    Window {
        key: key(),
        spec: WindowSpec {
            title: REMOVE_LABEL.to_owned(),
            app_id: format!("{DRAG_APP_ID}@{output}"),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: Some(Vec::new()),
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(key()))),
        background: WindowBackground::None,
        content: Element::new(Root {
            role: Role::Group,
            name: String::new(),
            pointer: None,
            key: None,
            roving: false,
            on_blur: None,
            alpha: 1.0,
            revision,
            under: Some(Box::new(move |cx| {
                if let Some(held) = &held_icon {
                    paint_held(cx, held, orientation);
                }
                if let Some(puff) = puff {
                    paint_puff(cx, puff);
                }
            })),
            frame: ticking.then(|| input(Input::Frame)),
            children: Vec::new(),
        }),
        popup: None,
        menus: None,
    }
}

fn paint_held(cx: &mut PaintCx<'_>, held: &Held, orientation: Orientation) {
    let env = cx.env().clone();
    let dark = paint::is_dark(&env);
    let r = held.rect;
    if held.opacity > 0.0 {
        draw_faded(cx, &held.icon, artwork_rect(r), held.opacity);
    }
    if held.indicator > 0.0 {
        let c = if dark {
            INDICATOR_DARK
        } else {
            INDICATOR_LIGHT
        };
        let dot = indicator_rect(r, orientation);
        paint::fill(
            cx,
            dot,
            INDICATOR / 2.0,
            paint::color(paint::faded(c, held.indicator)),
        );
    }
    if held.label > 0.0 {
        let width = paint::width(&env, REMOVE_LABEL, REMOVE_LABEL_PT, REMOVE_LABEL_WEIGHT)
            + 2.0 * REMOVE_LABEL_PAD_X;
        let pill = Rect::new(
            r.x() + r.width() / 2.0 - width / 2.0,
            r.y() - REMOVE_LABEL_GAP - REMOVE_LABEL_HEIGHT,
            width,
            REMOVE_LABEL_HEIGHT,
        );
        let fill = if dark { PILL_DARK } else { PILL_LIGHT };
        paint::fill(
            cx,
            pill,
            REMOVE_LABEL_HEIGHT / 2.0,
            paint::color(paint::faded(fill, held.label)),
        );
        let ink = if dark { LABEL_DARK } else { LABEL_LIGHT };
        paint::text(
            cx,
            REMOVE_LABEL,
            REMOVE_LABEL_PT,
            REMOVE_LABEL_WEIGHT,
            pill.x() + REMOVE_LABEL_PAD_X,
            pill.y() + pill.height() / 2.0,
            None,
            paint::color(paint::faded(ink, held.label)),
        );
    }
}

/// `icon` in `rect` at `opacity`: the image's own alpha scaled, as a layer of its own would be.
fn draw_faded(cx: &mut PaintCx<'_>, icon: &IconSet, rect: Rect, opacity: f64) {
    if opacity >= 1.0 {
        paint::image(cx, icon, rect);
        return;
    }
    cx.push(cupertino_kit::render::PaintOp::PushLayer {
        opacity: opacity.clamp(0.0, 1.0) as f32,
        effects: Vec::new(),
    });
    paint::image(cx, icon, rect);
    cx.push(cupertino_kit::render::PaintOp::PopLayer);
}

fn paint_puff(cx: &mut PaintCx<'_>, puff: Puff) {
    let dark = paint::is_dark(cx.env());
    let tint = if dark { POOF_DARK } else { POOF_LIGHT };
    let alpha = POOF_ALPHA * (1.0 - puff.progress);
    let reach = puff.size * POOF_SPREAD * puff.progress;
    let radius =
        puff.size * (POOF_RADIUS_FROM + (POOF_RADIUS_TO - POOF_RADIUS_FROM) * puff.progress);
    for i in 0..POOF_PUFFS {
        let angle = std::f64::consts::TAU * i as f64 / POOF_PUFFS as f64;
        let centre = Point::new(
            puff.centre.x + reach * angle.cos(),
            puff.centre.y + reach * angle.sin(),
        );
        paint::circle(cx, centre, radius, paint::color(paint::faded(tint, alpha)));
    }
}

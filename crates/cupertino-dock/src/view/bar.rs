//! The bar: one window as wide (or tall) as its edge of the output, deep enough for the magnified tiles and the
//! bounces. A node at the bar's own rectangle asks for the Dock's material under it ([`PaintCx::material`]: glass,
//! and with Reduce Transparency a body in the wallpaper's colour), or paints the window's opaque fill where the session
//! shows none; each tile is a node at the rectangle the model laid it out at,
//! drawing its icon, and each running indicator and separator is a node of its own under the tiles, so a change
//! repaints only the nodes it moved. The tiles are one roving group: the arrows move the keyboard focus along the bar,
//! Return or Space activates, the Menu key or Shift+F10 opens the tile's menu, Escape gives the keyboard back.

use cupertino_kit::core::{
    AccessibleStates, Action, Change, CommandRegistry, Element, Event, EventCx, FocusTarget, Key,
    MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, LayoutSpec, Measured};
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, WindowSpec};
use cupertino_kit::render::{FillRule, GradientStop, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::theme::{Surface, BAR_GLASS_MATERIAL, GLASS_MATERIAL};

use super::paint::{self, Root};
use super::{IconSet, Input, BAR_APP_ID, BAR_TITLE};
use crate::config::Orientation;
use crate::tiles::TileKind;

/// The surface the bar's material is: the one place to change to give the bar another material. The bar's surface is
/// bar glass with the bar's shape (clear glass with a small lift of white in both appearances, so the wallpaper stays
/// readable through it), and with Reduce Transparency a dark (or pale) body in the colour of the wallpaper under it.
pub const BAR_SURFACE: Surface = Surface::DockBar;

/// Diameter of the running indicator in points.
pub const INDICATOR: f64 = 4.0;
/// Gap between the bottom of a resting tile and the top of its running indicator, in points.
pub const INDICATOR_GAP: f64 = 2.0;
/// Corner radius of the bar as a fraction of the tile size: 19.6 pt at 64, a continuous corner that reaches 30 pt along
/// each side, so the 84 pt deep bar keeps a straight stretch at each end.
pub const BAR_RADIUS: f64 = 19.6 / 64.0;
/// Side of an icon's artwork as a fraction of its tile: a 64 pt tile shows a 52 pt icon, centred.
pub const ICON_ARTWORK: f64 = 52.0 / 64.0;
/// Length of a separator's line as a fraction of the tile size (62 pt at 64), and its width in points.
pub const SEPARATOR_LENGTH: f64 = 62.0 / 64.0;
pub const SEPARATOR_WIDTH: f64 = 1.0;
/// Corner radius of the focus ring as a fraction of the tile size.
const FOCUS_RADIUS: f64 = 0.22;

/// Running indicator: light and dark, and the alpha it keeps with Increase Contrast.
pub(crate) const INDICATOR_LIGHT: paint::Rgba = [0.0, 0.0, 0.0, 1.0];
pub(crate) const INDICATOR_DARK: paint::Rgba = [1.0, 1.0, 1.0, 0.6];
/// Separator line: light and dark; with Increase Contrast it is drawn at twice the alpha.
const SEPARATOR_LIGHT: paint::Rgba = [0.0, 0.0, 0.0, 0.9];
const SEPARATOR_DARK: paint::Rgba = [1.0, 1.0, 1.0, 0.28];
/// The share of its opacity the running dot of an application that has no windows keeps: a quarter, so the dark
/// appearance's dot goes from 0.6 to 0.15 white.
pub const DIM_INDICATOR: f64 = 0.25;

/// The bar's fine edge where the bar is not glass: a hairline just inside its outline, darker than what is around it,
/// light and dark, and its width in points (one device pixel at 2x); Increase Contrast doubles it.
const EDGE_LIGHT: paint::Rgba = [0.0, 0.0, 0.0, 0.25];
const EDGE_DARK: paint::Rgba = [0.0, 0.0, 0.0, 0.6];
const EDGE_WIDTH: f64 = 0.5;
/// The rim inside the edge where the bar is not glass (which lights its own): a light line along the bar's top and
/// bottom that fades out down its sides, at this alpha at the top and the bottom (light, dark), and gone over the
/// middle of the sides ([`RIM_FADE`] of the bar's depth from either end).
const RIM_TOP: [f32; 2] = [0.5, 0.45];
const RIM_BOTTOM: [f32; 2] = [0.6, 0.5];
const RIM_FADE: f32 = 0.3;
/// With Increase Contrast the rim shows at this alpha all the way round.
const RIM_CONTRAST: f32 = 0.6;

const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const HOME: Keysym = Keysym(0xff50);
const END: Keysym = Keysym(0xff57);
const ESCAPE: Keysym = Keysym(0xff1b);
const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const SPACE: Keysym = Keysym(0x20);
const MENU: Keysym = Keysym(0xff67);
const F10: Keysym = Keysym(0xffc7);

/// One tile as the bar shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct TileView {
    pub id: u64,
    pub kind: TileKind,
    /// Where it is drawn, bounce included, in window points.
    pub rect: Rect,
    /// The resting rectangle the running indicator hangs off.
    pub rest: Rect,
    pub opacity: f64,
    /// The running indicator's opacity, from 0 (none) to 1; it fades in as the application starts.
    pub indicator: f64,
    /// The application runs without windows: its indicator is dimmed.
    pub windowless: bool,
    /// The tile's stack is open: a down chevron stands in for its icon.
    pub chevron: bool,
    pub icon: IconSet,
    /// What assistive technology calls it, and what it says about its state.
    pub name: String,
    pub description: String,
}

/// What the bar shows.
pub struct BarView {
    /// The window's key: [`super::bar_key`].
    pub key: WindowKey,
    pub output: u32,
    /// The window's size: the output's edge by the depth the magnification needs.
    pub size: Size,
    pub orientation: Orientation,
    pub tile_size: f64,
    /// The bar's own rectangle, under which the material is; empty while hidden.
    pub bar: Rect,
    pub tiles: Vec<TileView>,
    /// Where the hovered tile's name stands over: a rectangle across the tile from the top of the bar down to the
    /// bottom of the window, so the label's popup, which does not fit below it, stands above the bar.
    pub label_anchor: Option<Rect>,
    /// The same for the open stack of a folder tile and its fan: from the middle of the tile down.
    pub fan_anchor: Option<Rect>,
    /// The same for the open context menu's tail and panel: from the top of the bar down.
    pub menu_anchor: Option<Rect>,
    /// Where the window takes the pointer.
    pub input: Vec<Rect>,
    /// The tile the keyboard focus goes to when the bar takes the keyboard (`TileId.0`).
    pub initial: Option<u64>,
    /// Something on the bar moves by itself: the window sends [`Input::Frame`] at every frame.
    pub ticking: bool,
    /// Changes when the bar's surface was given a placement or keyboard interactivity, which apply with its next commit:
    /// the window presents again for them.
    pub surface_changes: u64,
}

/// The key of the tile with `id` (`TileId.0`) in the bar's window.
pub fn tile_key(id: u64) -> Key {
    Key::from(format!("tile-{id}"))
}

/// The bar's window.
pub fn window<M: Clone + 'static>(view: BarView, input: fn(Input) -> M) -> Window<M> {
    let orientation = view.orientation;
    let radius = (view.tile_size * BAR_RADIUS).min(view.bar.height().min(view.bar.width()) / 2.0);
    let mut children = Vec::with_capacity(2 * view.tiles.len() + 1);
    if view.bar.width() > 0.0 && view.bar.height() > 0.0 {
        children.push(
            Element::new(BarMaterial {
                rect: view.bar,
                radius,
            })
            .key(Key::from("material")),
        );
    }
    for tile in &view.tiles {
        let mark = match tile.kind {
            TileKind::Separator => Some(Mark::Separator(separator_rect(tile.rest, orientation))),
            _ if tile.indicator > 0.0 => {
                Some(Mark::Indicator(indicator_rect(tile.rest, orientation)))
            }
            _ => None,
        };
        if let Some(mark) = mark {
            let alpha = match mark {
                Mark::Indicator(_) if tile.windowless => tile.indicator * DIM_INDICATOR,
                Mark::Indicator(_) => tile.indicator,
                Mark::Separator(_) => 1.0,
            };
            children.push(
                Element::new(MarkNode { mark, alpha }).key(Key::from(format!("mark-{}", tile.id))),
            );
        }
    }
    for (key, rect) in [
        (LABEL_ANCHOR, view.label_anchor),
        (FAN_ANCHOR, view.fan_anchor),
        (MENU_ANCHOR, view.menu_anchor),
    ] {
        if let Some(rect) = rect {
            children.push(Element::new(AnchorNode(rect)).key(key));
        }
    }
    let initial = view.initial;
    children.extend(view.tiles.into_iter().map(|tile| {
        let key = tile_key(tile.id);
        Element::new(TileNode {
            initial: initial == Some(tile.id),
            tile,
            tile_size: view.tile_size,
            orientation,
            input,
        })
        .key(key)
    }));
    Window {
        key: view.key.clone(),
        spec: WindowSpec {
            title: BAR_TITLE.to_owned(),
            app_id: format!("{BAR_APP_ID}@{}", view.output),
            size: view.size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: Some(view.input),
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(view.key.clone()))),
        background: WindowBackground::None,
        content: Element::new(Root {
            role: Role::Toolbar,
            name: BAR_TITLE.to_owned(),
            pointer: Some(Box::new(move |p| input(Input::Bar(p)))),
            key: Some(Box::new(move |k| {
                (k.pressed && k.keysym == ESCAPE).then(|| input(Input::LeaveKeyboard))
            })),
            roving: true,
            on_blur: None,
            alpha: 1.0,
            revision: view.surface_changes,
            under: None,
            frame: view.ticking.then(|| input(Input::Frame)),
            children,
        }),
        popup: None,
        menus: None,
    }
}

/// The bar's own rectangle: the menu material under it, or the menu's opaque fill where the session shows none
/// (Reduce Transparency, Increase Contrast, no desktop effects).
struct BarMaterial {
    rect: Rect,
    radius: f64,
}

impl<M: Clone + 'static> Widget<M> for BarMaterial {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.radius != old.radius {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let all = Rect::new(0.0, 0.0, size.width, size.height);
        let shown = cx.material(BAR_SURFACE, self.radius, 1.0);
        if !shown {
            if let Some(fill) = cx.env().theme.surface(BAR_SURFACE).fill {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: all,
                        radius: self.radius,
                    },
                    paint: Paint::Solid(fill),
                });
            }
        }
        paint_edge(cx, all, self.radius, shown && lit_by_glass(cx));
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// Whether the bar's surface is glass, which lights its own rim along the top and the bottom.
pub(crate) fn lit_by_glass(cx: &PaintCx<'_>) -> bool {
    matches!(
        cx.env().theme.surface(BAR_SURFACE).material,
        Some(GLASS_MATERIAL | BAR_GLASS_MATERIAL)
    )
}

/// The bar's fine edge over its material or fill, in the bar's rectangle `all`, and inside it the light rim; neither
/// where the glass under the bar lights its own rim (`lit`), except with Increase Contrast.
pub(crate) fn paint_edge(cx: &mut PaintCx<'_>, all: Rect, radius: f64, lit: bool) {
    let env = cx.env().clone();
    let (dark, contrast) = (paint::is_dark(&env), paint::is_high_contrast(&env));
    // the glass's own rim is the bar's edge; with Increase Contrast the edge and the rim stand out over it too
    if lit && !contrast {
        return;
    }
    let edge = if dark { EDGE_DARK } else { EDGE_LIGHT };
    let width = if contrast {
        2.0 * EDGE_WIDTH
    } else {
        EDGE_WIDTH
    };
    let inset = |r: Rect, d: f64| {
        Rect::new(
            r.x() + d,
            r.y() + d,
            r.width() - 2.0 * d,
            r.height() - 2.0 * d,
        )
    };
    cx.push(PaintOp::Stroke {
        shape: Shape::RoundedRect {
            rect: inset(all, width / 2.0),
            radius: (radius - width / 2.0).max(0.0),
        },
        width,
        paint: Paint::Solid(paint::color(edge)),
    });
    let rim = |alpha: f32| paint::color([1.0, 1.0, 1.0, alpha]);
    let mode = usize::from(dark);
    let stops = if contrast {
        vec![
            GradientStop {
                offset: 0.0,
                color: rim(RIM_CONTRAST),
            },
            GradientStop {
                offset: 1.0,
                color: rim(RIM_CONTRAST),
            },
        ]
    } else {
        vec![
            GradientStop {
                offset: 0.0,
                color: rim(RIM_TOP[mode]),
            },
            GradientStop {
                offset: RIM_FADE,
                color: rim(0.0),
            },
            GradientStop {
                offset: 1.0 - RIM_FADE,
                color: rim(0.0),
            },
            GradientStop {
                offset: 1.0,
                color: rim(RIM_BOTTOM[mode]),
            },
        ]
    };
    let w = EDGE_WIDTH;
    cx.push(PaintOp::Stroke {
        shape: Shape::RoundedRect {
            rect: inset(all, width + w / 2.0),
            radius: (radius - width - w / 2.0).max(0.0),
        },
        width: w,
        paint: Paint::Linear {
            start: Point::new(0.0, all.y()),
            end: Point::new(0.0, all.y() + all.height()),
            stops,
        },
    });
}

/// The top and bottom colours of the plate under the chevron, and the chevron's colour and stroke width as a share of
/// the plate's side.
const CHEVRON_PLATE: [[f32; 4]; 2] = [[0.62, 0.60, 0.59, 0.95], [0.40, 0.38, 0.37, 0.95]];
const CHEVRON_INK: [f32; 4] = [0.08, 0.08, 0.09, 0.85];
const CHEVRON_STROKE: f64 = 0.0625;

/// A grey plate with a down chevron on it, drawn over `rect` while the tile's stack is open.
fn paint_chevron(cx: &mut PaintCx<'_>, rect: Rect) {
    let radius = rect.width() * 0.225;
    cx.push(PaintOp::Fill {
        shape: Shape::RoundedRect { rect, radius },
        paint: Paint::Linear {
            start: Point::new(0.0, rect.y()),
            end: Point::new(0.0, rect.y() + rect.height()),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: paint::color(CHEVRON_PLATE[0]),
                },
                GradientStop {
                    offset: 1.0,
                    color: paint::color(CHEVRON_PLATE[1]),
                },
            ],
        },
    });
    let (cx_, cy) = (
        rect.x() + rect.width() / 2.0,
        rect.y() + rect.height() / 2.0,
    );
    let arm = rect.width() * 0.2;
    cx.push(PaintOp::Stroke {
        shape: Shape::Path {
            verbs: vec![
                PathVerb::MoveTo(Point::new(cx_ - arm, cy - arm * 0.45)),
                PathVerb::LineTo(Point::new(cx_, cy + arm * 0.45)),
                PathVerb::LineTo(Point::new(cx_ + arm, cy - arm * 0.45)),
            ],
            fill_rule: FillRule::NonZero,
        },
        width: rect.width() * CHEVRON_STROKE,
        paint: Paint::Solid(paint::color(CHEVRON_INK)),
    });
}

/// What the bar draws under a tile.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mark {
    /// The running indicator, at this rectangle of the window.
    Indicator(Rect),
    /// The separator's line, at this rectangle of the window.
    Separator(Rect),
}

impl Mark {
    fn rect(self) -> Rect {
        match self {
            Mark::Indicator(r) | Mark::Separator(r) => r,
        }
    }
}

/// A running indicator or a separator's line, at `alpha` of its opacity.
struct MarkNode {
    mark: Mark,
    alpha: f64,
}

impl<M: Clone + 'static> Widget<M> for MarkNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.mark.rect())
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        match (self.mark, old.mark) {
            (a, b) if a == b && self.alpha == old.alpha => Change::None,
            (a, b) if a == b => Change::Paint,
            (Mark::Indicator(_), Mark::Indicator(_)) | (Mark::Separator(_), Mark::Separator(_)) => {
                Change::Layout
            }
            _ => Change::Paint,
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.mark.rect().size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let (dark, contrast) = (paint::is_dark(&env), paint::is_high_contrast(&env));
        let size = cx.size();
        let all = Rect::new(0.0, 0.0, size.width, size.height);
        match self.mark {
            Mark::Indicator(_) => {
                // in the dark the dot lightens the glass behind it, which only the compositor can read
                if !contrast && cx.material(Surface::DockIndicator, INDICATOR / 2.0, self.alpha) {
                    return;
                }
                let c = if dark {
                    INDICATOR_DARK
                } else {
                    INDICATOR_LIGHT
                };
                let c = if contrast { [c[0], c[1], c[2], 1.0] } else { c };
                let c = paint::color(paint::faded(c, self.alpha));
                paint::fill(cx, all, INDICATOR / 2.0, c);
            }
            Mark::Separator(_) => {
                let c = if dark {
                    SEPARATOR_DARK
                } else {
                    SEPARATOR_LIGHT
                };
                let c = paint::color(if contrast {
                    paint::faded([c[0], c[1], c[2], c[3] * 2.0], 1.0)
                } else {
                    c
                });
                paint::fill(cx, all, SEPARATOR_WIDTH / 2.0, c);
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// The keys of the nodes the popups of the bar stand over.
pub const LABEL_ANCHOR: &str = "label-anchor";
pub const FAN_ANCHOR: &str = "fan-anchor";
pub const MENU_ANCHOR: &str = "menu-anchor";

/// A rectangle of the bar's window that only a popup anchors to: it paints nothing and takes no input.
struct AnchorNode(Rect);

impl<M: Clone + 'static> Widget<M> for AnchorNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.0)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.0 != old.0 {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.0.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// The separator's line across the middle of the slot `r`.
fn separator_rect(r: Rect, orientation: Orientation) -> Rect {
    if orientation.is_horizontal() {
        let length = r.height() * SEPARATOR_LENGTH;
        Rect::new(
            r.x() + (r.width() - SEPARATOR_WIDTH) / 2.0,
            r.y() + (r.height() - length) / 2.0,
            SEPARATOR_WIDTH,
            length,
        )
    } else {
        let length = r.width() * SEPARATOR_LENGTH;
        Rect::new(
            r.x() + (r.width() - length) / 2.0,
            r.y() + (r.height() - SEPARATOR_WIDTH) / 2.0,
            length,
            SEPARATOR_WIDTH,
        )
    }
}

/// The running indicator on the screen-edge side of a tile resting at `r`.
pub fn indicator_rect(r: Rect, orientation: Orientation) -> Rect {
    let (cx, cy) = (r.x() + r.width() / 2.0, r.y() + r.height() / 2.0);
    match orientation {
        Orientation::Bottom => Rect::new(
            cx - INDICATOR / 2.0,
            r.y() + r.height() + INDICATOR_GAP,
            INDICATOR,
            INDICATOR,
        ),
        Orientation::Left => Rect::new(
            r.x() - INDICATOR_GAP - INDICATOR,
            cy - INDICATOR / 2.0,
            INDICATOR,
            INDICATOR,
        ),
        Orientation::Right => Rect::new(
            r.x() + r.width() + INDICATOR_GAP,
            cy - INDICATOR / 2.0,
            INDICATOR,
            INDICATOR,
        ),
    }
}

/// Where a tile at `r` draws its icon: the artwork square, [`ICON_ARTWORK`] of the tile, centred in it.
pub fn artwork_rect(r: Rect) -> Rect {
    let side = r.width().min(r.height()) * ICON_ARTWORK;
    Rect::new(
        r.x() + (r.width() - side) / 2.0,
        r.y() + (r.height() - side) / 2.0,
        side,
        side,
    )
}

struct TileNode<M> {
    tile: TileView,
    tile_size: f64,
    orientation: Orientation,
    /// The first tile takes the focus when the bar takes the keyboard.
    initial: bool,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for TileNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.tile.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.tile.rect != old.tile.rect {
            Change::Layout
        } else if self.tile != old.tile || self.initial != old.initial {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.tile.rect.width(), self.tile.rect.height()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if self.tile.kind == TileKind::Separator {
            return Propagation::Continue;
        }
        let id = self.tile.id;
        match event {
            Event::Focus { gained: true, .. } => {
                cx.emit((self.input)(Input::TileFocused(id)));
                return Propagation::Continue;
            }
            Event::Action(request) => match request.action {
                Action::Click => cx.emit((self.input)(Input::Activate(id))),
                Action::ShowMenu => cx.emit((self.input)(Input::ShowMenu(id))),
                Action::Focus => cx.request_focus(),
                _ => return Propagation::Continue,
            },
            Event::Key(key, _) if key.pressed => {
                let horizontal = self.orientation.is_horizontal();
                match key.keysym {
                    LEFT if horizontal => cx.focus_item(FocusTarget::Previous),
                    RIGHT if horizontal => cx.focus_item(FocusTarget::Next),
                    UP if !horizontal => cx.focus_item(FocusTarget::Previous),
                    DOWN if !horizontal => cx.focus_item(FocusTarget::Next),
                    HOME => cx.focus_item(FocusTarget::First),
                    END => cx.focus_item(FocusTarget::Last),
                    RETURN | KP_ENTER | SPACE if !key.repeat => {
                        cx.emit((self.input)(Input::Activate(id)))
                    }
                    MENU => cx.emit((self.input)(Input::ShowMenu(id))),
                    F10 if key.mods.contains(Mods::SHIFT) => {
                        cx.emit((self.input)(Input::ShowMenu(id)))
                    }
                    ESCAPE => cx.emit((self.input)(Input::LeaveKeyboard)),
                    _ => return Propagation::Continue,
                }
            }
            _ => return Propagation::Continue,
        }
        Propagation::Stop
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.tile.kind == TileKind::Separator {
            return;
        }
        let size = cx.size();
        let bounds = Rect::new(0.0, 0.0, size.width, size.height);
        if self.tile.opacity < 1.0 {
            cx.set_group(self.tile.opacity.clamp(0.0, 1.0) as f32, Vec::new());
        }
        let artwork = artwork_rect(bounds);
        if self.tile.chevron {
            paint_chevron(cx, artwork);
        } else {
            paint::image(cx, &self.tile.icon, artwork);
        }
        // the ring goes with the keyboard: a bar that gave it back keeps its focused tile but shows no ring
        if cx.focus_visible() && cx.window_active() {
            paint::focus_ring(cx, artwork, self.tile_size * ICON_ARTWORK * FOCUS_RADIUS);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.tile.kind == TileKind::Separator {
            cx.role(Role::Separator);
            return;
        }
        cx.role(Role::Button);
        cx.name(self.tile.name.clone());
        if !self.tile.description.is_empty() {
            cx.description(self.tile.description.clone());
        }
        cx.focusable();
        cx.roving_item();
        if self.initial {
            cx.initial_focus();
        }
        cx.states(AccessibleStates {
            focusable: true,
            ..AccessibleStates::default()
        });
        cx.action(Action::Click);
        cx.action(Action::ShowMenu);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BAR_EDGE_PADDING_RATIO, BAR_PADDING_RATIO};

    /// How far a continuous corner reaches along each side, per point of its radius.
    const CORNER_REACH: f64 = 1.53;

    #[test]
    fn the_bar_corners_reach_30_points_and_leave_a_straight_stretch_at_each_end() {
        let tile = 64.0;
        let depth = tile * (1.0 + BAR_PADDING_RATIO + BAR_EDGE_PADDING_RATIO);
        assert_eq!(depth, 84.0);
        let reach = tile * BAR_RADIUS * CORNER_REACH;
        assert!((reach - 30.0).abs() < 0.1, "{reach}");
        // the two corners of an end leave 24 points of it straight, so the end is no half circle
        assert!(depth - 2.0 * reach > 23.0);
    }

    #[test]
    fn the_dot_of_an_application_without_windows_keeps_a_quarter_of_its_opacity() {
        let dim = f64::from(INDICATOR_DARK[3]) * DIM_INDICATOR;
        assert!((dim - 0.15).abs() < 1e-6, "{dim}");
    }
}

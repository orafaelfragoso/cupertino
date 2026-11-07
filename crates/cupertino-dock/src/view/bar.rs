//! The bar: one window as wide (or tall) as its edge of the output, deep enough for the magnified tiles and the
//! bounces, with the menu material under the bar's own rectangle only. Each tile is a node at the rectangle the model
//! laid it out at, drawing its icon; the bar draws the running indicators and the separators under them. The tiles are
//! one roving group: the arrows move the keyboard focus along the bar, Return or Space activates, the Menu key or
//! Shift+F10 opens the tile's menu, Escape gives the keyboard back.

use cupertino_kit::core::{
    AccessibleStates, Action, Change, CommandRegistry, Element, Event, EventCx, FocusTarget, Key,
    MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::{Constraints, LayoutSpec, Measured};
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, WindowSpec};
use cupertino_kit::theme::Surface;

use super::paint::{self, Root};
use super::{IconSet, Input, BAR_APP_ID, BAR_TITLE};
use crate::config::Orientation;
use crate::tiles::TileKind;

/// Diameter of the running indicator in points.
pub const INDICATOR: f64 = 4.0;
/// Corner radius of the bar as a fraction of the tile size.
pub const BAR_RADIUS: f64 = 0.3;
/// Corner radius of the focus ring as a fraction of the tile size.
const FOCUS_RADIUS: f64 = 0.22;

/// Running indicator: light and dark, and the alpha it keeps with Increase Contrast.
const INDICATOR_LIGHT: paint::Rgba = [0.1, 0.1, 0.12, 0.8];
const INDICATOR_DARK: paint::Rgba = [1.0, 1.0, 1.0, 0.85];
/// Separator line: light and dark; with Increase Contrast it is drawn at twice the alpha.
const SEPARATOR_LIGHT: paint::Rgba = [0.0, 0.0, 0.0, 0.2];
const SEPARATOR_DARK: paint::Rgba = [1.0, 1.0, 1.0, 0.25];

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
    pub indicator: bool,
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
    /// Where the window takes the pointer.
    pub input: Vec<Rect>,
    /// Changes whenever the drawing under the tiles changes.
    pub revision: u64,
}

/// The bar's window.
pub fn window<M: Clone + 'static>(view: BarView, input: fn(Input) -> M) -> Window<M> {
    let orientation = view.orientation;
    let marks: Vec<(TileKind, Rect, bool)> = view
        .tiles
        .iter()
        .map(|t| (t.kind, t.rest, t.indicator))
        .collect();
    let first = view
        .tiles
        .iter()
        .find(|t| t.kind != TileKind::Separator)
        .map(|t| t.id);
    let children = view
        .tiles
        .into_iter()
        .map(|tile| {
            let key = Key::from(format!("tile-{}", tile.id));
            Element::new(TileNode {
                initial: first == Some(tile.id),
                tile,
                tile_size: view.tile_size,
                orientation,
                input,
            })
            .key(key)
        })
        .collect();
    let bar = view.bar;
    let radius = view.tile_size * BAR_RADIUS;
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
        background: if bar.width() > 0.0 && bar.height() > 0.0 {
            WindowBackground::SurfaceRect {
                surface: Surface::Menu,
                rect: bar,
                radius,
                alpha: 1.0,
            }
        } else {
            WindowBackground::None
        },
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
            revision: view.revision,
            under: Some(Box::new(move |cx| marks_under(cx, &marks, orientation))),
            children,
        }),
        popup: None,
        menus: None,
    }
}

/// The running indicators and the separators, under the tiles.
fn marks_under(cx: &mut PaintCx<'_>, marks: &[(TileKind, Rect, bool)], orientation: Orientation) {
    let env = cx.env().clone();
    let (dark, contrast) = (paint::is_dark(&env), paint::is_high_contrast(&env));
    let indicator = {
        let c = if dark {
            INDICATOR_DARK
        } else {
            INDICATOR_LIGHT
        };
        paint::color(if contrast { [c[0], c[1], c[2], 1.0] } else { c })
    };
    let separator = {
        let c = if dark {
            SEPARATOR_DARK
        } else {
            SEPARATOR_LIGHT
        };
        paint::color(if contrast {
            paint::faded([c[0], c[1], c[2], c[3] * 2.0], 1.0)
        } else {
            c
        })
    };
    for &(kind, r, running) in marks {
        if kind == TileKind::Separator {
            let line = if orientation.is_horizontal() {
                Rect::new(
                    r.x() + r.width() / 2.0 - 0.5,
                    r.y() + r.height() * 0.15,
                    1.0,
                    r.height() * 0.7,
                )
            } else {
                Rect::new(
                    r.x() + r.width() * 0.15,
                    r.y() + r.height() / 2.0 - 0.5,
                    r.width() * 0.7,
                    1.0,
                )
            };
            paint::fill(cx, line, 0.5, separator);
        } else if running {
            let dot = indicator_rect(r, orientation);
            paint::fill(cx, dot, INDICATOR / 2.0, indicator);
        }
    }
}

/// The running indicator on the screen-edge side of a tile resting at `r`.
pub fn indicator_rect(r: Rect, orientation: Orientation) -> Rect {
    let (cx, cy) = (r.x() + r.width() / 2.0, r.y() + r.height() / 2.0);
    match orientation {
        Orientation::Bottom => Rect::new(
            cx - INDICATOR / 2.0,
            r.y() + r.height() + 1.0,
            INDICATOR,
            INDICATOR,
        ),
        Orientation::Left => Rect::new(
            r.x() - 1.0 - INDICATOR,
            cy - INDICATOR / 2.0,
            INDICATOR,
            INDICATOR,
        ),
        Orientation::Right => Rect::new(
            r.x() + r.width() + 1.0,
            cy - INDICATOR / 2.0,
            INDICATOR,
            INDICATOR,
        ),
    }
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
        paint::image(cx, &self.tile.icon, bounds);
        if cx.is_focused() && cx.focus_visible() {
            paint::focus_ring(cx, bounds, self.tile_size * FOCUS_RADIUS);
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

//! The panel's windows as CupertinoKit draws them: the module tiles, each its own glass shape on a transparent window,
//! the open detail view grown out of its tile, and the transparent catcher under the panel that takes a press outside
//! it.
//!
//! The window paints no panel of its own: every tile asks the compositor for glass under its outline, and the tiles
//! join one glass container so the compositor can blend them where they come close. A level's track is glass of its own
//! over its tile's. With Reduce Transparency the tiles are opaque and sit on one opaque panel, in the session's
//! appearance. Otherwise the tiles look the same in both appearances: mid-toned glass with light labels and symbols;
//! with Increase Contrast the glass takes its contrast face and each tile draws a rim.

use std::cell::RefCell;
use std::rc::Rc;

use cupertino_kit::controls::{Icon, IconDirection, PressCatcher};
use cupertino_kit::core::next_resource_id;
use cupertino_kit::core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, Checked, Command,
    CommandRegistry, Element, Environment, Event, EventCx, Glass, Key, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Shortcut, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
    Overflow, SizeSpec,
};
use cupertino_kit::platform::{
    DecorationMode, Keysym, Mods, MouseButton, PointerEvent, WindowSpec,
};
use cupertino_kit::render::{
    self, FillRule, ImageResource, Paint, PaintOp, PathVerb, ResourceId, Sampling, Shape,
};
use cupertino_kit::text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign, TextStyle, Wrap,
};
use cupertino_kit::theme::{Appearance, Color, ColorRole, StateVariant};

use crate::arrangement::Arrangement;
use crate::layout::{
    self, detail as rows, editing, Outline, BAR_MARGIN, GRID_ORIGIN, GRID_WIDTH, INSET,
    LARGE_RADIUS, SHADOW_REACH, SHADOW_TOP, SURFACE, WINDOW, WINDOW_ORIGIN,
};
use crate::model::{Controls, Detail, MediaCommand, Module, Reading, Slider, TileState};
use crate::motion::{visibility, Visibility};

mod builder;
mod label;
mod shadow;
mod widgets;

use builder::*;
use label::*;
use shadow::*;
use widgets::*;

pub use shadow::{BandPictures, Bands, BandsWanted};

/// The `app_id` of the panel's window, which gives it its layer surface.
pub const APP_ID: &str = "cupertino-controlcenter";
/// The key of the panel's window, which the compositor's fades name.
pub const PANEL_KEY: &str = "panel";
/// The `app_id` of the catcher's window.
pub const CATCHER_APP_ID: &str = "cupertino-controlcenter.catcher";
/// What assistive technology calls the panel.
pub const TITLE: &str = "Control Panel";

const ESCAPE: Keysym = Keysym(0xff1b);
const SPACE: Keysym = Keysym(0x20);
const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const HOME: Keysym = Keysym(0xff50);
const END: Keysym = Keysym(0xff57);
const BACKSPACE: Keysym = Keysym(0xff08);
const DELETE: Keysym = Keysym(0xffff);

/// The glass container the tiles join, and how close two of them come before their outlines blend: less than the gap
/// between tiles, so tiles at rest stay apart.
const CONTAINER: u32 = 1;
const BLEND: f64 = 8.0;

/// The fills a tile paints itself, in the dark and the light appearance. `RT_*`: the opaque panel and tiles with Reduce
/// Transparency. `FALLBACK`: a tile the window cannot show glass under.
const RT_GROUP: Color = Color::new(0.302, 0.302, 0.302, 1.0);
const RT_TILE: Color = Color::new(0.114, 0.114, 0.114, 1.0);
const RT_GROUP_LIGHT: Color = Color::new(0.82, 0.82, 0.82, 1.0);
const RT_TILE_LIGHT: Color = Color::new(0.96, 0.96, 0.96, 1.0);
/// The rim a tile draws over its glass with Increase Contrast, half a point wide: a mid grey over the dark tiles (the
/// tiles keep the dark appearance in both), a darker one over light tiles.
const IC_RIM_COLOR: Color = Color::new(0.341, 0.341, 0.341, 1.0);
const IC_RIM_COLOR_LIGHT: Color = Color::new(0.45, 0.45, 0.45, 1.0);
const IC_RIM: f64 = 0.5;

/// The width in points of the Increase Contrast rim on a display of `scale`: [`IC_RIM`], and never less than one
/// device pixel, so the line is drawn at its full strength where a point is one pixel instead of covering half of one
/// at half its contrast.
pub(crate) fn ic_rim_width(scale: f64) -> f64 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    IC_RIM.max(1.0 / scale)
}
const FALLBACK: Color = Color::new(0.16, 0.16, 0.17, 0.92);
const FALLBACK_LIGHT: Color = Color::new(0.93, 0.93, 0.94, 0.92);
/// A switch that is on: a white disc behind its symbol, drawn in the accent colour.
const ON_FILL: Color = Color::new(1.0, 1.0, 1.0, 1.0);
/// A switch badge that is off: a veil of white in the dark appearance, of black in the light one ([`veil`]).
const OFF_BADGE: f32 = 0.1078;
const OFF_BADGE_LIGHT: f32 = 0.1016;
/// Laid over a control while it is pressed: darker on a white disc, a veil on glass.
const PRESSED_ON: Color = Color::new(0.0, 0.0, 0.0, 0.12);
const PRESSED_OFF: f32 = 0.12;
/// Under a list row the pointer is over.
const HOVER_ROW: f32 = 0.1;
/// A level: white over its track's glass, as high as the track. Without the glass the track is a veil, over it a fainter
/// one.
const TRACK_FILL: Color = Color::new(1.0, 1.0, 1.0, 1.0);
const TRACK_EMPTY: f32 = 0.2;
/// The veil over the track's glass: as faint as an off badge's.
const TRACK_ON_GLASS: f32 = 0.1;
/// A disabled control's content is drawn at this opacity.
const DISABLED_ALPHA: f32 = 0.4;
/// The media tile's artwork placeholder and the separator of a detail view, as veils.
const ARTWORK: f32 = 0.12;
const ARTWORK_RADIUS: f64 = 10.0;
const SEPARATOR: f32 = 0.16;

/// Type of the panel. Sizes come from the theme's type scale: titles in its body size (13) semibold, second lines in
/// its caption size (11), the Edit Controls label in the body size medium, a detail's heading in the caption size
/// semibold, its rows in the body size.
const TITLE_WEIGHT: u16 = 600;
const LINE_WEIGHT: u16 = 500;
const EDIT_WEIGHT: u16 = 500;
const ROW_WEIGHT: u16 = 400;

/// A capsule's badge: its diameter, its inset from the capsule's leading and top edges, and where the text starts.
const BADGE: f64 = 36.0;
const BADGE_INSET: f64 = 14.0;
const TEXT_START: f64 = 56.0;
const TEXT_END_INSET: f64 = 6.0;
/// Symbol sizes: in a badge, on a circular tile, in a list row.
const BADGE_SYMBOL: f64 = 18.0;
const CIRCLE_SYMBOL: f64 = 22.0;
const ROW_SYMBOL: f64 = 16.0;
/// The slider tiles: the title's inset, the track's centre line and the hit band around it.
const SLIDER_TITLE_INSET: f64 = 16.0;
const SLIDER_TITLE_TOP: f64 = 9.0;
const TRACK_CENTRE: f64 = 42.0;
const SLIDER_BAND: f64 = 24.0;
/// How far past each end of its track a level takes the pointer.
const LEVEL_REACH: f64 = 4.0;
/// The display tile's track and its two suns; the sound tile's track, speakers and output button.
const DISPLAY_TRACK: (f64, f64) = (36.0, 254.0);
const SOUND_TRACK: (f64, f64) = (33.0, 213.0);
const OUTPUT_BUTTON: f64 = 26.0;
const OUTPUT_CENTRE_X: f64 = 265.0;
/// A level step of the arrow keys.
const KEY_STEP: f64 = 1.0 / 16.0;
/// The media tile: artwork square, text block and the three buttons on a centre line.
const ART: f64 = 40.0;
const MEDIA_TEXT_TOP: f64 = 66.0;
const MEDIA_BUTTONS_Y: f64 = 111.0;
const MEDIA_BUTTONS_X: [f64; 3] = [30.0, 70.0, 110.0];
const MEDIA_BUTTON: f64 = 32.0;
/// Corner radius of a detail view's row highlight.
const ROW_RADIUS: f64 = 8.0;
/// While the controls are edited a tile's content is drawn at this opacity: it shows what the tile is and does not
/// act.
const EDITED_CONTENT: f64 = 0.55;
/// The marks that remove a tile and say a chip adds: a disc, white where the tiles are dark and dark where they are
/// light, with a bar or a cross of the other tone, this long and this thick.
const MARK_DISC: Color = Color::new(1.0, 1.0, 1.0, 1.0);
const MARK_DISC_LIGHT: Color = Color::new(0.2, 0.2, 0.2, 1.0);
const MARK_SIGN: Color = Color::new(0.1, 0.1, 0.1, 1.0);
const MARK_SIGN_LIGHT: Color = Color::new(1.0, 1.0, 1.0, 1.0);
const MARK_SIGN_LENGTH: f64 = 8.0;
const MARK_SIGN_WIDTH: f64 = 1.5;
/// Symbol size on a chip that adds a control.
const CHIP_SYMBOL: f64 = 18.0;

/// What the user did on the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// A module's switch or button was pressed.
    Press(Module),
    /// A module's detail view is asked for.
    Expand(Detail, Via),
    /// The open detail view is to close: Escape or a press outside it.
    Collapse(Via),
    /// A button of the media tile.
    Media(MediaCommand),
    /// A network of the Wi-Fi view, by name.
    Join(String),
    /// A device of the Bluetooth view, by identifier: it connects or disconnects.
    Connect(String),
    /// A sound output, by identifier.
    Output(u32),
    /// The settings row of a detail view.
    Settings(Detail),
    /// The Edit Controls capsule: the controls are to be edited.
    EditControls,
    /// While editing: a tile's remove mark, or Delete on the tile.
    Remove(Module, Via),
    /// While editing: a control offered to add.
    Add(Module, Via),
    /// While editing: an arrow key on a tile moves it one place later (`true`) or earlier.
    Step(Module, bool),
    /// While editing: the pointer holds a tile with its corner at this point, from the grid's corner.
    DragTo(Module, Point),
    /// While editing: the pointer let the tile go.
    Drop,
    /// While editing: Done, which keeps the edit.
    Done,
    /// While editing: Cancel or Escape, which drops it.
    Cancel,
    /// An arrow key on a control: the focus goes to the control of this key.
    Focus(String),
    /// A slider moved to a fraction, 0 to 1.
    Slide(Slider, f64),
    /// The slider's drag or key step ended at a fraction.
    Release(Slider, f64),
    /// Escape, or a press outside the panel.
    Dismiss,
    /// The server closed the window: its output went away.
    Closed,
}

/// How a control was taken. The focus follows a detail view in and out only when the keyboard or assistive
/// technology opened or closed it, so a click leaves no focus ring behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Pointer,
    Keyboard,
}

impl Input {
    /// What the input is when the keyboard or assistive technology takes the control instead of the pointer.
    fn by_key(self) -> Input {
        match self {
            Input::Expand(detail, _) => Input::Expand(detail, Via::Keyboard),
            Input::Collapse(_) => Input::Collapse(Via::Keyboard),
            Input::Remove(module, _) => Input::Remove(module, Via::Keyboard),
            Input::Add(module, _) => Input::Add(module, Via::Keyboard),
            input => input,
        }
    }
}

/// How the panel is shown besides its readings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presentation {
    /// The panel's opacity while it fades out.
    pub alpha: f64,
    /// The session is in the light appearance.
    pub light: bool,
    /// The panel takes input: false while it fades out.
    pub input: bool,
    /// A detail view opening, open or closing.
    pub detail: Option<Opened>,
    /// The controls are being edited.
    pub edit: Option<Editing>,
}

impl Presentation {
    /// The panel at rest in a dark session.
    pub const SHOWN: Presentation = Presentation {
        alpha: 1.0,
        light: false,
        input: true,
        detail: None,
        edit: None,
    };
}

/// The panel while its controls are edited: every tile carries a remove mark and can be moved, the controls not
/// shown are offered under them, and Cancel and Done take the place of Edit Controls.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Editing {
    /// The tile the pointer holds.
    pub held: Option<Held>,
}

/// A tile the pointer holds while the controls are edited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Held {
    pub module: Module,
    /// The tile's corner as the pointer holds it, from the grid's corner.
    pub origin: Point,
}

/// A detail view and how far it has opened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opened {
    pub detail: Detail,
    /// 0 (the tile in the grid) to 1 (the open view).
    pub progress: f64,
    /// The tile grows into the view; with Reduce Motion the two cross-fade in place instead.
    pub morph: bool,
}

/// The key of the control that holds the focus for `module` on the grid.
pub fn focus_key(module: Module) -> String {
    format!("module.{}", slug(module))
}

/// The key of the first control of an open detail view.
pub const DETAIL_FOCUS: &str = "detail.header";

/// The key of the Edit Controls capsule, and of Done while the controls are edited, so the focus stays on it through
/// both.
pub const EDIT_FOCUS: &str = "edit-controls";

/// The key of the control that moves `module`'s tile while the controls are edited.
pub fn edit_key(module: Module) -> String {
    format!("edit.tile.{}", slug(module))
}

fn slug(module: Module) -> &'static str {
    module.id()
}

/// The symbol of a module.
fn symbol(controls: &Controls, module: Module) -> &'static str {
    let on = controls.tile(module) == TileState::On;
    match module {
        Module::WiFi => "symbol.wifi",
        Module::Bluetooth => "symbol.bluetooth",
        Module::Microphone if on => "symbol.mic",
        Module::Microphone => "symbol.mic.slash",
        Module::DockHiding => "symbol.dock.hide",
        Module::LockScreen => "symbol.lock-screen",
        Module::Mute => "symbol.speaker.slash",
        Module::KeyboardBrightness if on => "symbol.keyboard-brightness",
        Module::KeyboardBrightness => "symbol.keyboard-brightness.off",
        Module::FocusMode => "symbol.moon.fill",
        Module::Display => "symbol.sun",
        Module::Sound => "symbol.speaker.high",
        Module::NowPlaying => "symbol.music",
    }
}

/// Whether the panel is drawn in the dark appearance.
fn dark(cx: &PaintCx<'_>) -> bool {
    cx.env().theme.preferences.appearance.is_dark()
}

/// A veil of `alpha`: white over the dark appearance's glass, black over the light one's.
fn veil(cx: &PaintCx<'_>, alpha: f32) -> Color {
    let tone = if dark(cx) { 1.0 } else { 0.0 };
    Color::new(tone, tone, tone, alpha)
}

/// `dark` in the dark appearance, `light` in the light one.
fn by_appearance(cx: &PaintCx<'_>, dark_color: Color, light: Color) -> Color {
    if dark(cx) {
        dark_color
    } else {
        light
    }
}

fn reduce_transparency(cx: &PaintCx<'_>) -> bool {
    cx.env().theme.preferences.reduce_transparency
}

fn increase_contrast(cx: &PaintCx<'_>) -> bool {
    matches!(
        cx.env().theme.preferences.appearance,
        Appearance::LightHighContrast | Appearance::DarkHighContrast
    )
}

fn finite_alpha(alpha: f64) -> f64 {
    if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// The panel's window with every module shown in reading order: [`arranged`] with the arrangement nothing stored
/// gives.
pub fn panel<M: Clone + 'static>(
    controls: &Controls,
    presentation: Presentation,
    bands: &BandPictures,
    to: fn(Input) -> M,
) -> Window<M> {
    arranged(controls, &Arrangement::default(), presentation, bands, to)
}

/// The shadow's bands the panel of `arrangement` at `presentation` lays on a display of `scale`, for a picture of them
/// to be drawn ([`BandsWanted::draw`]) and given back to [`arranged`] in `bands`: `None` while a detail view is
/// moving, as an outline that changes every frame is filled in place, its picture costing as much and not used again.
/// The panel shows no bands with Reduce Transparency or Increase Contrast, which the caller knows.
pub fn bands_wanted(
    controls: &Controls,
    arrangement: &Arrangement,
    presentation: Presentation,
    scale: f64,
) -> Option<BandsWanted> {
    let presentation = Presentation {
        detail: presentation.detail.filter(|_| presentation.edit.is_none()),
        ..presentation
    };
    let resting = presentation
        .detail
        .is_none_or(|o| o.progress <= 0.0 || o.progress >= 1.0);
    if !resting {
        return None;
    }
    let builder: Builder<'_, Input> = Builder {
        controls,
        arrangement,
        presentation,
        to: std::convert::identity,
        map: Rc::new(RefCell::new(Vec::new())),
        inert: false,
    };
    let (rect, top_reach) = outline(builder.shadow());
    BandsWanted::new(rect, top_reach, scale)
}

/// The panel's window: the tiles of `arrangement` at `presentation`. Escape collapses an open detail view, else
/// dismisses the panel; while the controls are edited it cancels the edit. The shadow's bands are laid from `bands`
/// when it holds a picture of them at the window's scale, else filled in place.
pub fn arranged<M: Clone + 'static>(
    controls: &Controls,
    arrangement: &Arrangement,
    presentation: Presentation,
    bands: &BandPictures,
    to: fn(Input) -> M,
) -> Window<M> {
    // the controls are edited on the grid: no view is open then
    let presentation = Presentation {
        detail: presentation.detail.filter(|_| presentation.edit.is_none()),
        ..presentation
    };
    let builder = Builder {
        controls,
        arrangement,
        presentation,
        to,
        map: Rc::new(RefCell::new(Vec::new())),
        inert: false,
    };
    // Escape and a press outside the open view close it; with no view open they close the panel.
    let away = |via| {
        if presentation.detail.is_some() {
            Input::Collapse(via)
        } else {
            Input::Dismiss
        }
    };
    let editing = presentation.edit.is_some();
    // while editing the controls offered lie under the window, in the margin its shadow takes
    let mut reach = Rect::from_origin_size(WINDOW_ORIGIN, WINDOW);
    if editing {
        let bottom = GRID_ORIGIN.y + builder.height() + INSET;
        reach = Rect::new(
            reach.x(),
            reach.y(),
            reach.width(),
            reach.height().max(bottom - reach.y()),
        );
    }
    Window {
        key: WindowKey(Key::from(PANEL_KEY)),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: APP_ID.to_owned(),
            size: SURFACE,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            // only the modules take input: not the shadow's margin, and nothing while the panel fades out
            input_region: Some(if presentation.input {
                vec![reach]
            } else {
                Vec::new()
            }),
        },
        shortcuts: CommandRegistry::new().with(
            Shortcut {
                mods: Mods::NONE,
                keysym: ESCAPE,
            },
            to(if editing {
                Input::Cancel
            } else {
                away(Via::Keyboard)
            }),
        ),
        on_close: Some(to(Input::Closed)),
        background: WindowBackground::None,
        content: Element::new(Board {
            alpha: presentation.alpha,
            height: builder.height(),
            shadow: builder.shadow(),
            bands: bands.all(),
            cutouts: builder.cutouts(),
            on_outside: (!editing).then(|| to(away(Via::Pointer))),
            children: builder.board(),
        }),
        popup: None,
        menus: None,
    }
}

/// The transparent window under the panel, over the whole output while the panel is shown: a left or right press
/// anywhere on it dismisses the panel and goes no further.
pub fn catcher<M: Clone + 'static>(to: fn(Input) -> M) -> Window<M> {
    Window::from(PressCatcher {
        key: WindowKey(Key::from("catcher")),
        title: TITLE.to_owned(),
        app_id: CATCHER_APP_ID.to_owned(),
        on_press: to(Input::Dismiss),
        on_close: Some(to(Input::Closed)),
    })
}

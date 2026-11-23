//! Typed wrapper of `cupertino_backdrop_v1`: a material the server draws below a surface.

use crate::app::{AppEvent, Error};
use crate::State;
use cupertino_app_protocols::client::app_effects::cupertino_backdrop_v1::{
    self, CupertinoBackdropV1, GlassStyle as WireStyle, Tone,
};
use cupertino_geometry::Rect;
use std::sync::atomic::{AtomicU64, Ordering};
use wayland_client::{Connection as WlConnection, Dispatch, Proxy, QueueHandle};

/// Names a [`Backdrop`] of this process in [`AppEvent::BackdropTone`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BackdropId(pub u64);

static NEXT_BACKDROP: AtomicU64 = AtomicU64::new(1);

/// User data of a backdrop: the id its events name.
pub(crate) struct BackdropData(BackdropId);

impl BackdropData {
    pub(crate) fn new() -> Self {
        Self(BackdropId(NEXT_BACKDROP.fetch_add(1, Ordering::Relaxed)))
    }
}

/// What the text and symbols over a glass backdrop are drawn with ([`AppEvent::BackdropTone`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BackdropTone {
    /// The appearance's: nothing was measured, or the shape is too large to follow what lies behind it.
    #[default]
    Appearance,
    /// The face behind the backdrop is dark: light text and symbols.
    Dark,
    /// The face behind the backdrop is light: dark text and symbols.
    Light,
}

/// The look of a glass backdrop ([`Backdrop::set_glass`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassStyle {
    /// The frosted glass of bars, menus, panels and controls.
    #[default]
    Regular,
    /// The clearer glass of controls over rich content, such as media.
    Clear,
    /// The regular glass of a large floating panel, toned further towards the middle, with a soft drop shadow.
    LargePanel,
    /// The glass of a floating bar of icons: a narrow blur, a thin bending edge and a thin line of light along its top and bottom.
    Bar,
    /// The glass of a dense floating surface with text on it, regular glass with a wider blur over a washed-out face.
    Dense,
    /// The glass of the tiles of a panel of modules: the clear face under a wide blur and a strong bending edge.
    Module,
    /// The glass of a notification card: the clear face over a wide blur and a blur fill, with a faint ring shadow.
    Card,
    /// The glass of a level's track: no blur, a bending edge that follows its height and a ring shadow.
    Track,
    /// The glass of a small popover over a floating bar: a wide soft blur fill, a strong bending edge and a thin line
    /// of light along its top and bottom edges.
    Popover,
    /// The glass of a wide floating panel of large icons (the application switcher): a blur of about a point, a strong
    /// bending edge and no shadow.
    Switcher,
    /// The glass of a window's sidebar: a wide soft blur fill over a light pane in the light appearance and a dark,
    /// nearly grey one in the dark.
    Sidebar,
    /// The module glass of a tile that steps back: twice the blur and a quieter face.
    SubduedModule,
    /// The glass of a small card of content over the desktop: a faint blur under a wide soft blur fill and a veiled face.
    Snippet,
    /// Clear glass veiled black from a fifth at its top to fully at its bottom.
    Veiled,
    /// The glass of a window's toolbar controls: regular glass with a face lifted a little further and a thin line of
    /// light along its top and bottom edges.
    Toolbar,
    /// The glass of a menu: a wide blur under a face that tones the backdrop into a narrow band, with a thin line of light
    /// along its top and bottom edges.
    Menu,
}

impl GlassStyle {
    /// The material a server without glass draws instead.
    pub fn fallback_material(self) -> &'static str {
        "Menu"
    }
}

/// A server-rendered material region below the content of a surface. Every setter takes
/// effect with the next commit of the surface.
/// Dropping it removes the backdrop with that commit.
pub struct Backdrop {
    proxy: CupertinoBackdropV1,
}

/// Integer rectangle of a backdrop and its corner radius, as the wire carries them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: f64,
}

impl Shape {
    /// Round `rect` to whole logical points. Errors on a negative extent or radius or a
    /// non-finite value, which the server would reject as a protocol error.
    fn new(rect: &Rect, radius: f64) -> Result<Self, Error> {
        let values = [rect.x(), rect.y(), rect.width(), rect.height(), radius];
        if !values.iter().all(|v| v.is_finite())
            || rect.width().round() < 0.0
            || rect.height().round() < 0.0
            || radius < 0.0
        {
            return Err(Error::InvalidShape);
        }
        Ok(Self {
            x: rect.x().round() as i32,
            y: rect.y().round() as i32,
            width: rect.width().round() as i32,
            height: rect.height().round() as i32,
            radius,
        })
    }
}

impl Backdrop {
    pub(crate) fn new(proxy: CupertinoBackdropV1) -> Self {
        Self { proxy }
    }

    /// The id [`AppEvent::BackdropTone`] names this backdrop with.
    pub fn id(&self) -> BackdropId {
        self.proxy
            .data::<BackdropData>()
            .map_or(BackdropId(0), |d| d.0)
    }

    /// Whether the server tells the tone of the backdrop's face ([`AppEvent::BackdropTone`], version 7). A server
    /// before it sends none, and the client keeps the appearance's colours.
    pub fn tells_tone(&self) -> bool {
        self.proxy.version() >= TONE_VERSION
    }

    /// Name of a material recipe of the design tokens, for example `"Menu"`.
    pub fn set_material(&self, name: &str) {
        self.proxy.set_material(name.to_string());
    }

    /// Region in surface-local logical points with continuous corners of `radius` points; a
    /// zero width or height hides the backdrop.
    pub fn set_shape(&self, rect: &Rect, radius: f64) -> Result<(), Error> {
        let s = Shape::new(rect, radius)?;
        // a server before it takes whole points only
        if self.proxy.version() >= FRACTIONAL_SHAPE_VERSION {
            self.proxy.set_shape_fixed(
                rect.x(),
                rect.y(),
                rect.width().max(0.0),
                rect.height().max(0.0),
                s.radius,
            );
        } else {
            self.proxy.set_shape(s.x, s.y, s.width, s.height, s.radius);
        }
        Ok(())
    }

    /// Draw the backdrop as glass of `style` instead of a material recipe. Backdrops of one surface with the same
    /// nonzero `container` join into one shape whose outlines blend over `spacing` points (0 joins them with a hard
    /// edge); container 0 keeps the backdrop on its own. A server before version 6 draws the backdrop with the
    /// material of [`GlassStyle::fallback_material`] instead, one before version 10 draws
    /// [`GlassStyle::LargePanel`] as regular glass, and one before version 15 draws [`GlassStyle::Card`] as dense glass
    /// (regular glass before version 13) and [`GlassStyle::Track`] as clear glass, and one before version 16 draws
    /// [`GlassStyle::Popover`] as regular glass, and one before version 17 draws [`GlassStyle::Switcher`] as regular
    /// glass; one before version 18 draws [`GlassStyle::SubduedModule`] as module glass, [`GlassStyle::Veiled`] as clear
    /// glass and [`GlassStyle::Sidebar`] and [`GlassStyle::Snippet`] as regular glass; one before version 19 draws
    /// [`GlassStyle::Toolbar`] as regular glass; one before version 20 draws [`GlassStyle::Menu`] as dense glass (regular
    /// glass before version 13).
    pub fn set_glass(&self, style: GlassStyle, container: u32, spacing: f64) -> Result<(), Error> {
        if !(spacing.is_finite() && spacing >= 0.0) {
            return Err(Error::InvalidGlass);
        }
        if self.proxy.version() < 6 {
            self.set_material(style.fallback_material());
            return Ok(());
        }
        let style = match style {
            GlassStyle::Regular => WireStyle::Regular,
            GlassStyle::Clear => WireStyle::Clear,
            // a server before it draws the large panel as regular glass, without its shadow
            GlassStyle::LargePanel if self.proxy.version() < LARGE_PANEL_VERSION => {
                WireStyle::Regular
            }
            GlassStyle::LargePanel => WireStyle::LargePanel,
            // a server before it draws the bar as clear glass
            GlassStyle::Bar if self.proxy.version() < BAR_VERSION => WireStyle::Clear,
            GlassStyle::Bar => WireStyle::Bar,
            // a server before it draws them as regular glass
            GlassStyle::Dense | GlassStyle::Module if self.proxy.version() < DENSE_VERSION => {
                WireStyle::Regular
            }
            GlassStyle::Dense => WireStyle::Dense,
            GlassStyle::Module => WireStyle::Module,
            // a server before it draws the card as dense glass and the track as clear glass
            GlassStyle::Card | GlassStyle::Track if self.proxy.version() < CARD_VERSION => {
                legacy_style(style, self.proxy.version())
            }
            GlassStyle::Card => WireStyle::Card,
            GlassStyle::Track => WireStyle::Track,
            GlassStyle::Popover => popover_style(self.proxy.version()),
            GlassStyle::Switcher => switcher_style(self.proxy.version()),
            GlassStyle::Sidebar
            | GlassStyle::SubduedModule
            | GlassStyle::Snippet
            | GlassStyle::Veiled => sidebar_style(style, self.proxy.version()),
            GlassStyle::Toolbar => toolbar_style(self.proxy.version()),
            GlassStyle::Menu => menu_style(self.proxy.version()),
        };
        self.proxy.set_glass(style, container, spacing);
        Ok(())
    }

    /// Opacity of the whole backdrop, 0 to 1.
    pub fn set_alpha(&self, alpha: f64) -> Result<(), Error> {
        if !(0.0..=1.0).contains(&alpha) {
            return Err(Error::InvalidAlpha);
        }
        self.proxy.set_alpha(alpha);
        Ok(())
    }

    /// Whether the server varies the backdrop's blur from its top to its bottom ([`Self::set_blur_ramp`], version 22).
    pub fn takes_blur_ramp(&self) -> bool {
        self.proxy.version() >= BLUR_RAMP_VERSION
    }

    /// Blur the backdrop at `top` times its blur along the top edge of its shape and at `bottom` times it along the
    /// bottom edge, smoothly between (a variable blur); 1 and 1 blur evenly. Errors on a weight below 0, above 1 or not
    /// finite, which the server would reject as a protocol error. A server before version 22 blurs evenly and is sent
    /// nothing.
    pub fn set_blur_ramp(&self, top: f64, bottom: f64) -> Result<(), Error> {
        let weight = |w: f64| w.is_finite() && (0.0..=1.0).contains(&w);
        if !(weight(top) && weight(bottom)) {
            return Err(Error::InvalidRamp);
        }
        if self.takes_blur_ramp() {
            self.proxy.set_blur_ramp(top, bottom);
        }
        Ok(())
    }
}

impl GlassStyle {
    /// The style's wire value at the protocol's current version, as other extensions of the same server name glass
    /// styles by it.
    pub(crate) fn wire(self) -> u32 {
        let style = match self {
            GlassStyle::Regular => WireStyle::Regular,
            GlassStyle::Clear => WireStyle::Clear,
            GlassStyle::LargePanel => WireStyle::LargePanel,
            GlassStyle::Bar => WireStyle::Bar,
            GlassStyle::Dense => WireStyle::Dense,
            GlassStyle::Module => WireStyle::Module,
            GlassStyle::Card => WireStyle::Card,
            GlassStyle::Track => WireStyle::Track,
            GlassStyle::Popover => WireStyle::Popover,
            GlassStyle::Switcher => WireStyle::Switcher,
            GlassStyle::Sidebar => WireStyle::Sidebar,
            GlassStyle::SubduedModule => WireStyle::SubduedModule,
            GlassStyle::Snippet => WireStyle::Snippet,
            GlassStyle::Veiled => WireStyle::Veiled,
            GlassStyle::Toolbar => WireStyle::Toolbar,
            GlassStyle::Menu => WireStyle::Menu,
        };
        style as u32
    }
}

/// The version of `cupertino_backdrop_v1` that has the `tone` event.
const TONE_VERSION: u32 = 7;

/// The version of `cupertino_backdrop_v1` that has the large panel glass style.
const LARGE_PANEL_VERSION: u32 = 10;

/// The version of `cupertino_backdrop_v1` that has the bar glass style.
const BAR_VERSION: u32 = 12;

/// The version of `cupertino_backdrop_v1` that has the dense and module glass styles.
const DENSE_VERSION: u32 = 13;
/// The first version that takes a shape in fractional points.
const FRACTIONAL_SHAPE_VERSION: u32 = 14;
/// The version of `cupertino_backdrop_v1` that has the card and track glass styles.
const CARD_VERSION: u32 = 15;
/// The version of `cupertino_backdrop_v1` that has the popover glass style.
const POPOVER_VERSION: u32 = 16;
/// The version of `cupertino_backdrop_v1` that has the switcher glass style.
const SWITCHER_VERSION: u32 = 17;
/// The version of `cupertino_backdrop_v1` that has the sidebar, subdued module, snippet and veiled glass styles.
const SIDEBAR_VERSION: u32 = 18;
/// The version of `cupertino_backdrop_v1` that has the toolbar glass style.
const TOOLBAR_VERSION: u32 = 19;
/// The version of `cupertino_backdrop_v1` that has the menu glass style.
const MENU_VERSION: u32 = 20;
/// The version of `cupertino_backdrop_v1` that has `set_blur_ramp`.
const BLUR_RAMP_VERSION: u32 = 22;

/// The wire style of the menu on a server of `version`: one before [`MENU_VERSION`] draws it as dense glass (regular
/// glass before [`DENSE_VERSION`]).
fn menu_style(version: u32) -> WireStyle {
    if version >= MENU_VERSION {
        WireStyle::Menu
    } else if version >= DENSE_VERSION {
        WireStyle::Dense
    } else {
        WireStyle::Regular
    }
}

/// The wire style a server of `version`, older than [`CARD_VERSION`], draws in place of the card or the track: the
/// card as dense glass (regular glass before [`DENSE_VERSION`]), the track as clear glass.
fn legacy_style(style: GlassStyle, version: u32) -> WireStyle {
    match style {
        GlassStyle::Card if version >= DENSE_VERSION => WireStyle::Dense,
        GlassStyle::Track => WireStyle::Clear,
        _ => WireStyle::Regular,
    }
}

/// The wire style of the popover on a server of `version`: one before [`POPOVER_VERSION`] draws it as regular glass.
fn popover_style(version: u32) -> WireStyle {
    if version < POPOVER_VERSION {
        WireStyle::Regular
    } else {
        WireStyle::Popover
    }
}

/// The wire style of the switcher on a server of `version`: one before [`SWITCHER_VERSION`] draws it as regular glass.
fn switcher_style(version: u32) -> WireStyle {
    if version < SWITCHER_VERSION {
        WireStyle::Regular
    } else {
        WireStyle::Switcher
    }
}

/// The wire style of the sidebar, the subdued module, the snippet or the veiled glass on a server of `version`: one before
/// [`SIDEBAR_VERSION`] draws the subdued module as module glass (regular glass before [`DENSE_VERSION`]), the veiled
/// glass as clear glass and the others as regular glass.
fn sidebar_style(style: GlassStyle, version: u32) -> WireStyle {
    match style {
        _ if version >= SIDEBAR_VERSION => match style {
            GlassStyle::SubduedModule => WireStyle::SubduedModule,
            GlassStyle::Snippet => WireStyle::Snippet,
            GlassStyle::Veiled => WireStyle::Veiled,
            _ => WireStyle::Sidebar,
        },
        GlassStyle::SubduedModule if version >= DENSE_VERSION => WireStyle::Module,
        GlassStyle::Veiled => WireStyle::Clear,
        _ => WireStyle::Regular,
    }
}

/// The wire style of the toolbar on a server of `version`: one before [`TOOLBAR_VERSION`] draws it as regular glass.
fn toolbar_style(version: u32) -> WireStyle {
    if version < TOOLBAR_VERSION {
        WireStyle::Regular
    } else {
        WireStyle::Toolbar
    }
}

impl Dispatch<CupertinoBackdropV1, BackdropData> for State {
    fn event(
        state: &mut State,
        _: &CupertinoBackdropV1,
        event: cupertino_backdrop_v1::Event,
        data: &BackdropData,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        if let cupertino_backdrop_v1::Event::Tone { tone } = event {
            let tone = match tone.into_result() {
                Ok(Tone::Appearance) => BackdropTone::Appearance,
                Ok(Tone::Dark) => BackdropTone::Dark,
                Ok(Tone::Light) => BackdropTone::Light,
                _ => return state.fail(crate::Error::Protocol("unknown backdrop tone")),
            };
            state.emit(AppEvent::BackdropTone {
                backdrop: data.0,
                tone,
            });
        }
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}

//! Typed wrapper of `cupertino_backdrop_v1`: a material the server draws below a surface.

use crate::{AppEvent, Error};
use cupertino_app_protocols::client::app_effects::cupertino_backdrop_v1::{
    self, CupertinoBackdropV1, GlassStyle as WireStyle, Tone,
};
use cupertino_geometry::Rect;
use cupertino_wayland_client::State;
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
    /// The glass of a dense floating surface with text on it, regular glass over a mid grey veil.
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
    /// [`GlassStyle::Popover`] as regular glass.
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
            // a server before it draws the popover as regular glass
            GlassStyle::Popover if self.proxy.version() < POPOVER_VERSION => WireStyle::Regular,
            GlassStyle::Popover => WireStyle::Popover,
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

/// The wire style a server of `version`, older than [`CARD_VERSION`], draws in place of the card or the track: the
/// card as dense glass (regular glass before [`DENSE_VERSION`]), the track as clear glass.
fn legacy_style(style: GlassStyle, version: u32) -> WireStyle {
    match style {
        GlassStyle::Card if version >= DENSE_VERSION => WireStyle::Dense,
        GlassStyle::Track => WireStyle::Clear,
        _ => WireStyle::Regular,
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
                _ => {
                    return state.fail(cupertino_wayland_client::Error::Protocol(
                        "unknown backdrop tone",
                    ))
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_rounds_to_whole_points() {
        let s = Shape::new(&Rect::new(10.4, 20.5, 343.6, 71.49), 12.0).unwrap();
        assert_eq!(
            (s.x, s.y, s.width, s.height, s.radius),
            (10, 21, 344, 71, 12.0)
        );
    }

    #[test]
    fn an_older_server_draws_the_card_and_the_track_with_styles_it_has() {
        assert_eq!(legacy_style(GlassStyle::Card, 14), WireStyle::Dense);
        assert_eq!(legacy_style(GlassStyle::Card, 13), WireStyle::Dense);
        assert_eq!(legacy_style(GlassStyle::Card, 12), WireStyle::Regular);
        assert_eq!(legacy_style(GlassStyle::Track, 14), WireStyle::Clear);
        assert_eq!(legacy_style(GlassStyle::Track, 6), WireStyle::Clear);
    }

    #[test]
    fn shape_rejects_what_the_server_rejects() {
        assert!(matches!(
            Shape::new(&Rect::new(0.0, 0.0, -1.0, 5.0), 1.0),
            Err(Error::InvalidShape)
        ));
        assert!(matches!(
            Shape::new(&Rect::new(0.0, 0.0, 5.0, 5.0), -0.5),
            Err(Error::InvalidShape)
        ));
        assert!(matches!(
            Shape::new(&Rect::new(f64::NAN, 0.0, 5.0, 5.0), 1.0),
            Err(Error::InvalidShape)
        ));
    }
}

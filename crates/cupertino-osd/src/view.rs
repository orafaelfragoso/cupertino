//! The overlay's window as CupertinoKit draws it: the glyph and the 16-segment meter over the HUD material, faded as
//! one layer with the material.

use cupertino_kit::controls::{Canvas, Icon, IconDirection, Stack};
use cupertino_kit::core::{
    AccessibleValue, Change, CommandRegistry, Element, Event, EventCx, Key, Live, MeasureCx,
    PaintCx, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::{Color, ColorRole, StateVariant, Surface, ThemeTokens};

use crate::layout::{self, CORNER_RADIUS, GLYPH_SIZE, GLYPH_TOP, WINDOW_SIZE};
use crate::model::{segments, Fill, Kind, Overlay, View};

/// The `app_id` of the overlay's window, which gives it its layer surface.
pub const APP_ID: &str = "cupertino-osd";

/// The colours of the meter in the session's appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterColors {
    /// The 161 x 8 container behind the segments; empty segments show only this.
    pub track: Color,
    /// The filled part of the segments.
    pub segment: Color,
}

impl MeterColors {
    /// Light: the secondary label colour as track, white segments. Dark: black at 0.25 as track (opaque with Reduce
    /// Transparency), the secondary label colour as segments. High contrast makes the light track black and the dark
    /// segments white.
    pub fn of(theme: &ThemeTokens) -> Option<Self> {
        let secondary = theme
            .colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL)?;
        let preferences = theme.preferences;
        let contrast = preferences.appearance.is_high_contrast();
        let white = Color::new(1.0, 1.0, 1.0, 1.0);
        Some(if preferences.appearance.is_dark() {
            let track = if preferences.reduce_transparency {
                1.0
            } else {
                0.25
            };
            Self {
                track: Color::new(0.0, 0.0, 0.0, track),
                segment: if contrast { white } else { secondary },
            }
        } else {
            Self {
                track: if contrast {
                    Color::new(0.0, 0.0, 0.0, 1.0)
                } else {
                    secondary
                },
                segment: white,
            }
        })
    }
}

/// What assistive technology calls the overlay.
pub fn accessible_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Volume => "Volume",
        Kind::Mute => "Volume muted",
        Kind::Brightness => "Brightness",
        Kind::KeyboardBrightness => "Keyboard brightness",
        Kind::Microphone => "Microphone",
        Kind::MicrophoneMute => "Microphone muted",
    }
}

/// The meter row: the full width of the window, the meter at its design position inside it.
#[derive(Clone, Debug, PartialEq)]
struct Meter {
    level: f64,
    label: &'static str,
}

impl<M: 'static> Widget<M> for Meter {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(layout::meter().height()),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(constraints.width.ideal(), layout::meter().height()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(colors) = MeterColors::of(&cx.env().theme) else {
            return;
        };
        let top = layout::meter().y();
        let fill = |cx: &mut PaintCx<'_>, rect: Rect, color| {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(rect.translate(0.0, -top)),
                paint: Paint::Solid(color),
            });
        };
        fill(cx, layout::meter(), colors.track);
        for (index, segment) in segments(self.level).into_iter().enumerate() {
            let rect = layout::segment(index);
            let width = match segment {
                Fill::Full => rect.width(),
                Fill::Partial(width) => f64::from(width),
                Fill::Empty => continue,
            };
            let filled = Rect::new(rect.x(), rect.y(), width, rect.height());
            fill(cx, filled, colors.segment);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ProgressIndicator);
        cx.name(self.label.to_owned());
        // the overlay never takes the focus, so its level is announced as it changes
        cx.live(Live::Polite);
        cx.value(AccessibleValue::Number {
            value: self.level,
            min: 0.0,
            max: 1.0,
            step: None,
        });
    }
}

/// The overlay's content: the glyph and the meter, faded by `alpha` as one layer.
fn content<M: 'static>(overlay: &Overlay, alpha: f64) -> Element<M> {
    let label = accessible_name(overlay.kind());
    let side = f64::from(WINDOW_SIZE);
    let meter_top = layout::meter().y() - GLYPH_TOP - GLYPH_SIZE;
    Element::new(Canvas {
        size: Some(Size::new(side, side)),
        label: label.to_owned(),
        paint: Box::new(move |cx| cx.set_group(alpha.clamp(0.0, 1.0) as f32, Vec::new())),
        children: vec![Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets {
                top: GLYPH_TOP,
                ..Insets::default()
            },
            spacing: meter_top,
            align: Align::Center,
            children: vec![
                Element::new(Icon {
                    name: overlay.icon_name().to_owned(),
                    size: GLYPH_SIZE,
                    direction: IconDirection::Fixed,
                    label: None,
                    tint: Some(ColorRole::ControlText),
                }),
                Element::new(Meter {
                    level: overlay.level(),
                    label,
                }),
            ],
        })],
    })
}

/// The overlay's one window: always open, so a show needs no new surface. While hidden it draws nothing and has no
/// material.
pub fn window<M: Clone + 'static>(view: Option<View>) -> Window<M> {
    let side = f64::from(WINDOW_SIZE);
    let (title, background, content) = match view {
        Some(View { overlay, alpha }) => (
            accessible_name(overlay.kind()),
            WindowBackground::Surface {
                surface: Surface::Hud,
                radius: CORNER_RADIUS,
                alpha,
            },
            content(&overlay, alpha),
        ),
        None => (
            "Volume and brightness",
            WindowBackground::None,
            Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::default(),
                spacing: 0.0,
                align: Align::Start,
                children: Vec::new(),
            }),
        ),
    };
    Window {
        key: WindowKey(Key::from("osd")),
        spec: WindowSpec {
            title: title.to_owned(),
            app_id: APP_ID.to_owned(),
            size: Size::new(side, side),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background,
        content,
        popup: None,
        menus: None,
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use cupertino_assets::BUILTIN_FONT;
    use cupertino_kit::core::{Application, Command, PlaceholderIcons, RuntimeOptions};
    use cupertino_kit::layout::LayoutDirection;
    use cupertino_kit::platform::{Capabilities, Timestamp, WindowEvent, WindowId, WindowStates};
    use cupertino_kit::render::{CpuConfig, CpuRenderer};
    use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
    use cupertino_kit::theme::{Appearance, ThemePreferences};
    use cupertino_theme::Theme;
    use cupertino_ui_test::{AssistiveTechnology, ManualClock, TestDriver};

    use super::*;

    fn tokens(appearance: Appearance, reduce_transparency: bool) -> ThemeTokens {
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let preferences = ThemePreferences {
            appearance,
            reduce_transparency,
            reduce_motion: false,
        };
        ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
    }

    fn secondary(theme: &ThemeTokens) -> Color {
        theme
            .colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL)
            .unwrap()
    }

    #[test]
    fn the_meter_colours_follow_the_appearance_and_reduce_transparency() {
        let black = |a| Color::new(0.0, 0.0, 0.0, a);
        let white = Color::new(1.0, 1.0, 1.0, 1.0);
        let light = tokens(Appearance::Light, false);
        let dark = tokens(Appearance::Dark, false);
        let reduced = tokens(Appearance::Dark, true);
        let cases = [
            (&light, secondary(&light), white),
            (&dark, black(0.25), secondary(&dark)),
            (&reduced, black(1.0), secondary(&reduced)),
            (
                &tokens(Appearance::LightHighContrast, false),
                black(1.0),
                white,
            ),
            (
                &tokens(Appearance::DarkHighContrast, false),
                black(0.25),
                white,
            ),
        ];
        for (theme, track, segment) in cases {
            assert_eq!(
                MeterColors::of(theme),
                Some(MeterColors { track, segment }),
                "{:?}",
                theme.preferences
            );
        }
    }

    #[test]
    fn a_shown_overlay_has_the_hud_material_at_its_alpha_and_a_hidden_one_has_none() {
        let overlay = Overlay::new(Kind::Brightness, 0.5).unwrap();
        let shown = window::<()>(Some(View {
            overlay,
            alpha: 0.25,
        }));
        assert_eq!(
            shown.background,
            WindowBackground::Surface {
                surface: Surface::Hud,
                radius: 18.0,
                alpha: 0.25
            }
        );
        assert_eq!(shown.spec.app_id, APP_ID);
        assert_eq!(shown.spec.size, Size::new(200.0, 200.0));
        assert_eq!(window::<()>(None).background, WindowBackground::None);
    }

    struct Shown;

    impl Application for Shown {
        type Model = Option<View>;
        type Message = ();

        fn init() -> (Option<View>, Vec<Command<()>>) {
            let overlay = Overlay::new(Kind::Volume, 0.5).unwrap();
            (
                Some(View {
                    overlay,
                    alpha: 1.0,
                }),
                Vec::new(),
            )
        }

        fn update(_: &mut Option<View>, _: ()) -> Vec<Command<()>> {
            Vec::new()
        }

        fn view(model: &Option<View>) -> Vec<Window<()>> {
            vec![window(*model)]
        }
    }

    fn budget() -> NonZeroU32 {
        NonZeroU32::new(64).unwrap()
    }

    fn start(theme: ThemeTokens) -> (TestDriver<Shown>, WindowId) {
        let (driver, window, _) = start_heard(theme);
        (driver, window)
    }

    /// The overlay shown with a screen reader listening.
    fn start_heard(theme: ThemeTokens) -> (TestDriver<Shown>, WindowId, AssistiveTechnology) {
        let reader = AssistiveTechnology::new();
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 100,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 20,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: Some(reader.adapter()),
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let mut driver = TestDriver::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(budget()).unwrap();
        let window = driver.runtime().windows()[0].1;
        driver
            .window_system_mut()
            .configure(window, None, WindowStates::default())
            .unwrap();
        driver
            .window_system_mut()
            .enqueue_window(window, WindowEvent::Scale(2.0))
            .unwrap();
        for _ in 0..8 {
            driver.run_until_idle(budget()).unwrap();
            let system = driver.window_system_mut();
            if !system.frame_requested(window).unwrap() {
                break;
            }
            system.frame_ready(window, Timestamp(0)).unwrap();
        }
        driver.assert_accessible("OSD");
        (driver, window, reader)
    }

    #[test]
    fn the_level_is_announced_politely_without_taking_the_focus() {
        let (_driver, window, reader) = start_heard(tokens(Appearance::Light, false));
        let said: Vec<_> = reader
            .announcements()
            .into_iter()
            .map(|a| (a.live, a.text))
            .collect();
        assert_eq!(said, [(Live::Polite, "Volume, 0.5".to_owned())]);
        assert_eq!(
            (reader.focus(window), reader.focus_moves(window)),
            (None, 0)
        );
    }

    #[test]
    fn the_meter_is_a_named_progress_indicator_with_the_level() {
        let (driver, window) = start(tokens(Appearance::Light, false));
        let snapshot = driver.runtime().snapshot(window).unwrap();
        let meters: Vec<_> = snapshot
            .accessibility
            .iter()
            .filter(|(_, n)| n.role == Role::ProgressIndicator)
            .map(|(_, n)| (n.name.clone(), n.value.clone()))
            .collect();
        assert_eq!(
            meters,
            vec![(
                Some("Volume".to_owned()),
                Some(AccessibleValue::Number {
                    value: 0.5,
                    min: 0.0,
                    max: 1.0,
                    step: None
                })
            )]
        );
    }

    #[test]
    fn full_segments_show_the_segment_colour_and_empty_ones_the_track_at_2x() {
        let theme = tokens(Appearance::LightHighContrast, false);
        let colors = MeterColors::of(&theme).unwrap();
        let (mut driver, window) = start(theme);
        let frame = driver
            .window_system_mut()
            .frames(window)
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!((frame.width, frame.height), (400, 400));
        let pixel = |x: f64, y: f64| {
            let at = ((y * 2.0) as usize * frame.width as usize * 4) + (x * 2.0) as usize * 4;
            [frame.pixels[at], frame.pixels[at + 1], frame.pixels[at + 2]]
        };
        let rgb = |c: Color| -> [u8; 3] { c.to_rgba8()[..3].try_into().unwrap() };
        // both colours are opaque in high contrast; at level 0.5 segment 7 is the last full one, and segment 8 and the
        // gap before it show the track
        assert_eq!(pixel(95.5, 175.0), rgb(colors.segment));
        assert_eq!(pixel(105.5, 175.0), rgb(colors.track));
        assert_eq!(pixel(100.5, 175.0), rgb(colors.track));
    }
}

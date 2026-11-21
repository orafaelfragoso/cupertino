//! The overlay's window as CupertinoKit draws it: the glyph and the 16-segment meter over the HUD material, faded as
//! one layer with the material, and while it fades a pacer that asks for each of the display's frames.

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

/// What the meter reads as: the overlay's name and, unless it shows a mute, the level in whole percent
/// ("Volume 50%").
fn meter_name(kind: Kind, level: f64) -> String {
    let name = accessible_name(kind);
    match kind {
        Kind::Mute | Kind::MicrophoneMute => name.to_owned(),
        _ => format!("{name} {}%", (level * 100.0).round()),
    }
}

/// The meter row: the full width of the window, the meter at its design position inside it.
#[derive(Clone, Debug, PartialEq)]
struct Meter {
    level: f64,
    /// What the meter reads as, from [`meter_name`].
    name: String,
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
        cx.name(self.name.clone());
        // the overlay never takes the focus, so its level is announced as it changes; a bridge announces a live node
        // only as it appears or its name changes, which is why the name holds the level
        cx.live(Live::Polite);
        cx.value(AccessibleValue::Number {
            value: self.level,
            min: 0.0,
            max: 1.0,
            step: None,
        });
    }
}

/// Sends its message at every frame the compositor gives the window, from its first paint: the fade advances with the
/// display's frames. It draws nothing and takes no room.
pub(crate) struct Pacer<M> {
    pub(crate) frame: M,
}

impl<M: Clone + 'static> Widget<M> for Pacer<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(0.0),
            height: SizeSpec::Fixed(0.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        // painted again with each new view, so it asks for the next frame
        Change::Paint
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::AnimationFrame(_) = event {
            cx.emit(self.frame.clone());
            cx.request_animation_frame();
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.request_animation_frame();
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The overlay's content: the glyph and the meter, faded by `alpha` as one layer, and the pacer sending `frame` when
/// there is one.
fn content<M: Clone + 'static>(overlay: &Overlay, alpha: f64, frame: Option<M>) -> Element<M> {
    let label = accessible_name(overlay.kind());
    let side = f64::from(WINDOW_SIZE);
    let meter_top = layout::meter().y() - GLYPH_TOP - GLYPH_SIZE;
    let pacer = frame.map(|frame| Element::new(Pacer { frame }));
    Element::new(Canvas {
        size: Some(Size::new(side, side)),
        label: label.to_owned(),
        paint: Box::new(move |cx| cx.set_group(alpha.clamp(0.0, 1.0) as f32, Vec::new())),
        children: std::iter::once(Element::new(Stack {
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
                    name: meter_name(overlay.kind(), overlay.level()),
                }),
            ],
        }))
        .chain(pacer)
        .collect(),
    })
}

/// The overlay's one window: always open, so a show needs no new surface. While hidden it draws nothing and has no
/// material. It takes no input. `frame` is sent at every frame of the display while it is given, for a fade.
pub fn window<M: Clone + 'static>(view: Option<View>, frame: Option<M>) -> Window<M> {
    let side = f64::from(WINDOW_SIZE);
    let (title, background, content) = match view {
        Some(View { overlay, alpha }) => (
            accessible_name(overlay.kind()),
            WindowBackground::Surface {
                surface: Surface::Hud,
                radius: CORNER_RADIUS,
                alpha,
            },
            content(&overlay, alpha, frame),
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
            // the window ignores the pointer
            input_region: Some(Vec::new()),
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background,
        content,
        popup: None,
        menus: None,
    }
}

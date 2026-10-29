//! Runs one-window applications on the headless window system, with a theme whose colours are distinct per role and
//! state so a test can tell which one a control painted.

#![allow(dead_code)]

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::{
    AccessibilityNode, Application, CommandRegistry, Element, Environment, FrameSnapshot, Key,
    Runtime, RuntimeOptions, Window, WindowKey,
};
use cupertino_ui_layout::LayoutDirection;
use cupertino_ui_platform::{
    DecorationMode, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, TouchEvent,
    WindowEvent, WindowId, WindowSpec, WindowStates,
};
use cupertino_ui_render::{CpuConfig, CpuRenderer, PaintNodeId, PaintOp};
use cupertino_ui_test::{HeadlessWindowSystem, ManualClock};
use cupertino_ui_text::{
    AttributedText, FontIndex, FontStyle, FontWeight, GlyphKey, GlyphRasterizer, ParagraphStyle,
    RasterGlyph, TextError, TextStyle, TextSystem,
};
use cupertino_ui_theme::{
    Appearance, Color, ColorRole, ColorTokens, ControlMetrics, ControlSize, FocusRing,
    InteractionVariant, Motion, StateVariant, ThemePreferences, ThemeTokens, TypeRole, TypeScale,
};
use std::num::NonZeroU32;
use std::sync::Arc;

pub const SPACE: u32 = 0x20;
pub const RETURN: u32 = 0xff0d;
pub const ESCAPE: u32 = 0xff1b;
pub const TAB: u32 = 0xff09;

const INTER: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cupertino-assets/fonts/InterVariable-subset.ttf"
));

/// Shapes labels with real metrics; the renderer below rasterizes none of their glyphs.
pub fn text() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts
        .add_face(
            "Inter",
            FontWeight::REGULAR,
            FontStyle::Normal,
            Arc::new(INTER.to_vec()),
            0,
        )
        .unwrap();
    Arc::new(TextSystem::new(fonts, "Inter"))
}

/// The one-line width of `label` in the fixture's 13 pt body type.
pub fn label_width(label: &str) -> f64 {
    text()
        .measure(
            &AttributedText::plain(label, TextStyle::new(13.0)),
            &ParagraphStyle::default(),
            None,
        )
        .unwrap()
        .width
}

struct NoGlyphs;

impl GlyphRasterizer for NoGlyphs {
    fn rasterize(&mut self, _: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        Ok(None)
    }
}

pub fn gray(level: u8) -> Color {
    let v = f32::from(level) / 255.0;
    Color::new(v, v, v, 1.0)
}

pub fn state(interaction: InteractionVariant, selected: bool) -> StateVariant {
    StateVariant {
        interaction,
        selected,
        window_active: true,
    }
}

/// Every role gets its own grey per state: `base + 1` hover, `+ 2` pressed, `+ 3` disabled, `+ 8` selected.
pub fn theme() -> ThemeTokens {
    use ColorRole::*;
    use InteractionVariant::*;
    let roles = [
        ControlFill,
        ControlBorder,
        ControlText,
        DefaultButtonFill,
        DefaultButtonText,
        DestructiveFill,
        DestructiveText,
        SwitchTrack,
        SwitchThumb,
        TextBackground,
        TextFieldBorder,
        Text,
        PlaceholderText,
        SelectedTextBackground,
        KeyboardFocusIndicator,
        Separator,
    ];
    let mut entries = Vec::new();
    for (i, role) in roles.into_iter().enumerate() {
        let base = 4 + 15 * i as u8;
        for (offset, interaction) in [(0, Normal), (1, Hover), (2, Pressed), (3, Disabled)] {
            for selected in [false, true] {
                let level = base + offset + if selected { 8 } else { 0 };
                entries.push(((role, state(interaction, selected)), gray(level)));
            }
        }
    }
    ThemeTokens {
        preferences: ThemePreferences {
            appearance: Appearance::Light,
            reduce_transparency: false,
            reduce_motion: false,
        },
        colors: ColorTokens::new(entries),
        metrics: ControlMetrics {
            size: ControlSize::Regular,
            button_height: 20.0,
            button_inset: 10.0,
            button_radius: 5.0,
            field_height: 20.0,
            field_inset: 6.0,
            field_radius: 5.0,
            switch: Size::new(38.0, 22.0),
            switch_thumb: 18.0,
            switch_thumb_inset: 2.0,
            min_target: Size::new(24.0, 24.0),
        },
        type_scale: TypeScale {
            family: "Inter".into(),
            body: TypeRole {
                size: 13.0,
                weight: 400,
            },
            caption: TypeRole {
                size: 11.0,
                weight: 400,
            },
            section: TypeRole {
                size: 12.0,
                weight: 600,
            },
            title: TypeRole {
                size: 15.0,
                weight: 600,
            },
        },
        focus: FocusRing {
            width: 2.0,
            outset: 2.0,
            radius_extra: 3.0,
        },
        motion: Motion {
            reduce: false,
            caret_phase_ms: 500,
        },
    }
}

pub fn color(role: ColorRole, interaction: InteractionVariant, selected: bool) -> Color {
    theme()
        .colors
        .get(role, state(interaction, selected))
        .expect("the fixture has every state")
}

pub fn window<M: Clone>(content: Element<M>) -> Vec<Window<M>> {
    vec![Window {
        key: WindowKey("test".into()),
        spec: WindowSpec {
            title: "Test".into(),
            app_id: "test.controls".into(),
            size: Size::new(200.0, 100.0),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::Preferred,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        content,
    }]
}

pub struct Harness<A: Application> {
    pub rt: Runtime<A, HeadlessWindowSystem>,
    pub window: WindowId,
    clock: Arc<ManualClock>,
    text: Arc<TextSystem>,
}

impl<A: Application> Harness<A> {
    pub fn new() -> Self {
        Self::with_direction(LayoutDirection::Ltr)
    }

    pub fn with_direction(direction: LayoutDirection) -> Self {
        let clock = Arc::new(ManualClock::new(Timestamp(0)));
        let text = text();
        let options = RuntimeOptions {
            theme: Arc::new(theme()),
            text: text.clone(),
            direction,
            node_limit: 1000,
            renderers: Box::new(|| {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 22,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    NoGlyphs,
                ))
            }),
            accessibility: None,
            clock: clock.clone(),
            on_window_error: None,
        };
        let mut rt = Runtime::new(HeadlessWindowSystem::default(), options);
        rt.step(turns()).unwrap();
        let window = rt.windows()[0].1;
        rt.window_system_mut()
            .configure(window, Some(Size::new(200.0, 100.0)), active())
            .unwrap();
        let mut harness = Self {
            rt,
            window,
            clock,
            text,
        };
        harness.settle();
        harness
    }

    /// Steps until the window has drawn everything it asked to draw.
    pub fn settle(&mut self) {
        for _ in 0..8 {
            self.rt.step(turns()).unwrap();
            let system = self.rt.window_system_mut();
            if !system.frame_requested(self.window).unwrap() {
                return;
            }
            system.frame_ready(self.window, Timestamp(0)).unwrap();
        }
        panic!("the window keeps asking for frames");
    }

    pub fn send(&mut self, event: WindowEvent) {
        self.rt
            .window_system_mut()
            .enqueue_window(self.window, event)
            .unwrap();
        self.settle();
    }

    pub fn pointer(&mut self, event: PointerEvent) {
        self.send(WindowEvent::Pointer(event, Timestamp(0)));
    }

    pub fn button(&mut self, position: Point, pressed: bool) {
        self.pointer(PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 1,
        });
    }

    /// Moves the pointer onto `position`, presses and releases there.
    pub fn click(&mut self, position: Point) {
        self.pointer(PointerEvent::Motion { position });
        self.button(position, true);
        self.button(position, false);
    }

    pub fn touch(&mut self, event: TouchEvent) {
        self.send(WindowEvent::Touch(event, Timestamp(0)));
    }

    pub fn key(&mut self, keysym: u32, pressed: bool) {
        self.key_event(keysym, pressed, false, Mods::NONE);
    }

    pub fn key_event(&mut self, keysym: u32, pressed: bool, repeat: bool, mods: Mods) {
        self.press_key(0, keysym, pressed, repeat, mods);
    }

    /// A key with its evdev `code`, which the editing chords match on.
    pub fn press_key(&mut self, code: u32, keysym: u32, pressed: bool, repeat: bool, mods: Mods) {
        self.send(WindowEvent::Key(
            KeyEvent {
                code,
                keysym: Keysym(keysym),
                text: char::from_u32(keysym).filter(|c| !c.is_control() && keysym < 0x100),
                pressed,
                repeat,
                mods,
            },
            Timestamp(0),
        ));
    }

    /// Types `text`, one key per character.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.send(WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(c as u32),
                    text: Some(c),
                    pressed: true,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ));
        }
    }

    /// Presses and releases a key.
    pub fn tap(&mut self, keysym: u32) {
        self.key(keysym, true);
        self.key(keysym, false);
    }

    pub fn snapshot(&self) -> FrameSnapshot {
        self.rt.snapshot(self.window).unwrap()
    }

    pub fn rect(&self, key: &'static str) -> Rect {
        let id = self.rt.find(self.window, &Key::from(key)).unwrap();
        self.snapshot()
            .nodes
            .into_iter()
            .find(|n| n.id == id)
            .unwrap()
            .rect
    }

    pub fn center(&self, key: &'static str) -> Point {
        let r = self.rect(key);
        Point::new(r.min_x() + r.width() / 2.0, r.min_y() + r.height() / 2.0)
    }

    /// The operations the node painted, in its own coordinates.
    pub fn ops(&self, key: &'static str) -> Vec<PaintOp> {
        let id = PaintNodeId::from(self.rt.find(self.window, &Key::from(key)).unwrap());
        self.snapshot()
            .display_list
            .nodes
            .into_iter()
            .find(|n| n.id == id)
            .unwrap()
            .ops
    }

    pub fn accessible(&self, key: &'static str) -> AccessibilityNode {
        let id = self.rt.find(self.window, &Key::from(key)).unwrap();
        self.snapshot()
            .accessibility
            .into_iter()
            .find(|(n, _)| *n == id)
            .unwrap()
            .1
    }

    pub fn focused(&self, key: &'static str) -> bool {
        self.snapshot().focus == self.rt.find(self.window, &Key::from(key))
    }

    pub fn model(&self) -> &A::Model {
        self.rt.model()
    }

    pub fn env(&self) -> Environment {
        Environment {
            theme: Arc::new(theme()),
            text: self.text.clone(),
            direction: LayoutDirection::Ltr,
            scale: 1.0,
        }
    }

    pub fn clock(&self) -> &ManualClock {
        &self.clock
    }
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

fn active() -> WindowStates {
    WindowStates {
        activated: true,
        ..WindowStates::default()
    }
}

pub fn point(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

//! Runs one-window applications on the headless window system, with a theme whose colours are distinct per role and
//! state so a test can tell which one a control painted.

#![allow(dead_code)]

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::{
    overlay_id, AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, Action,
    ActionRequest, Application, CommandRegistry, Element, Environment, FrameSnapshot, Icons, Key,
    NodeId, PlaceholderIcons, Runtime, RuntimeOptions, Window, WindowBackground, WindowKey,
};
use cupertino_ui_layout::LayoutDirection;
use cupertino_ui_platform::{
    Capabilities, DecorationMode, KeyEvent, Keysym, Mods, MouseButton, PlatformEvent, PointerEvent,
    Timestamp, TouchEvent, WindowEvent, WindowId, WindowSpec, WindowStates,
};
use cupertino_ui_render::{CpuConfig, CpuRenderer, PaintNodeId, PaintOp, Shape};
use cupertino_ui_test::{HeadlessWindowSystem, ManualClock};
use cupertino_ui_text::{
    AttributedText, FontIndex, FontStyle, FontWeight, GlyphKey, GlyphRasterizer, ParagraphStyle,
    RasterGlyph, TextError, TextStyle, TextSystem,
};
use cupertino_ui_theme::{
    Appearance, Color, ColorRole, ColorTokens, ControlMetrics, ControlSize, FocusRing,
    InteractionVariant, Motion, StateVariant, ThemePreferences, ThemeTokens, TypeRole, TypeScale,
};
use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::rc::Rc;
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

/// The one-line size of `label` in the fixture's 13 pt body type.
pub fn label_size(label: &str) -> Size {
    text()
        .measure(
            &AttributedText::plain(label, TextStyle::new(13.0)),
            &ParagraphStyle::default(),
            None,
        )
        .unwrap()
}

pub fn label_width(label: &str) -> f64 {
    label_size(label).width
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

fn placeholder_icons() -> Arc<dyn Icons> {
    Arc::new(PlaceholderIcons::default())
}

thread_local! {
    /// Whether the next harness's desktop has effects.
    static EFFECTS: Cell<bool> = const { Cell::new(false) };
    static LAYER: Cell<bool> = const { Cell::new(false) };
}

/// Every role gets its own grey per state, 13 apart: `base + 1` hover, `+ 2` pressed, `+ 3` disabled, `+ 8` selected.
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
        WindowBackground,
        SecondarySelectedControlFill,
        SecondaryText,
    ];
    let mut entries = Vec::new();
    for (i, role) in roles.into_iter().enumerate() {
        let base = 4 + 13 * i as u8;
        for (offset, interaction) in [(0, Normal), (1, Hover), (2, Pressed), (3, Disabled)] {
            for selected in [false, true] {
                let level = base + offset + if selected { 8 } else { 0 };
                entries.push(((role, state(interaction, selected)), gray(level)));
                // a slider's empty track takes the switch track's greys
                if role == SwitchTrack {
                    entries.push(((SliderTrack, state(interaction, selected)), gray(level)));
                }
            }
        }
    }
    // translucent, as the real thumbs and page dots are, so a test can read the fade from its alpha
    for interaction in [Normal, Hover, Pressed, Disabled] {
        for selected in [false, true] {
            entries.push((
                (ScrollerThumb, state(interaction, selected)),
                gray(128).with_alpha(0.5),
            ));
            entries.push((
                (PageDot, state(interaction, selected)),
                gray(0).with_alpha(0.2),
            ));
        }
    }
    entries.push(((ToolTipText, state(Normal, false)), gray(1)));
    for (role, level) in [(SelectedControlFill, 2), (SelectedControlText, 3)] {
        entries.push(((role, state(Normal, false)), gray(level)));
    }
    entries.push(((Link, state(Normal, false)), gray(8)));
    entries.push(((PanelTrack, state(Normal, false)), gray(9)));
    entries.push(((QuaternaryLabel, state(Normal, false)), gray(10)));
    ThemeTokens {
        preferences: ThemePreferences {
            appearance: Appearance::Light,
            reduce_transparency: false,
            reduce_motion: false,
        },
        colors: ColorTokens::new(entries),
        metrics: ControlMetrics {
            size: ControlSize::Regular,
            compact: false,
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
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Theme,
        popup: None,
        content,
        menus: None,
    }]
}

pub struct Harness<A: Application> {
    pub rt: Runtime<A, HeadlessWindowSystem>,
    pub window: WindowId,
    clock: Arc<ManualClock>,
    text: Arc<TextSystem>,
    animated: bool,
    icons: Arc<dyn Icons>,
    asked: Option<Rc<RefCell<Vec<ActionRequest>>>>,
}

/// An active screen reader that only asks for the actions a test queues.
struct Assistive(Rc<RefCell<Vec<ActionRequest>>>);

impl AccessibilityAdapter for Assistive {
    fn is_active(&self) -> bool {
        true
    }

    fn window_focus(&mut self, _: WindowId, _: bool) {}

    fn update(&mut self, _: &AccessibilityUpdate) {}

    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>) {
        out.append(&mut self.0.borrow_mut());
    }
}

impl<A: Application> Harness<A> {
    pub fn new() -> Self {
        Self::with_direction(LayoutDirection::Ltr)
    }

    pub fn with_direction(direction: LayoutDirection) -> Self {
        Self::build(direction, theme(), placeholder_icons(), false)
    }

    /// A harness with a screen reader attached, which [`ask`](Self::ask) speaks for.
    pub fn assistive() -> Self {
        Self::assistive_with(false)
    }

    /// [`assistive`](Self::assistive) for content that animates without end, as [`animated`](Self::animated) is.
    pub fn assistive_animated() -> Self {
        Self::assistive_with(true)
    }

    fn assistive_with(animated: bool) -> Self {
        let asked = Rc::new(RefCell::new(Vec::new()));
        let mut harness = Self::build_with(
            LayoutDirection::Ltr,
            theme(),
            placeholder_icons(),
            animated,
            Some(Rc::clone(&asked)),
        );
        harness.asked = Some(asked);
        harness
    }

    /// A harness for content that animates without end: each settle shows one more frame instead of waiting for the
    /// window to stop asking.
    pub fn animated() -> Self {
        Self::build(LayoutDirection::Ltr, theme(), placeholder_icons(), true)
    }

    /// A harness whose widgets draw their symbols from `icons`.
    pub fn with_icons(direction: LayoutDirection, icons: Arc<dyn Icons>) -> Self {
        Self::build(direction, theme(), icons, false)
    }

    /// [`with_theme`](Self::with_theme) on a desktop with effects, which shows the glass and materials the nodes
    /// ask for.
    pub fn with_effects(theme: ThemeTokens) -> Self {
        EFFECTS.with(|e| e.set(true));
        let harness = Self::with_theme(theme);
        EFFECTS.with(|e| e.set(false));
        harness
    }

    /// [`with_effects`](Self::with_effects) on a desktop that also shows a window's raised nodes in a layer over its
    /// content.
    pub fn with_layer(theme: ThemeTokens) -> Self {
        LAYER.with(|e| e.set(true));
        let harness = Self::with_effects(theme);
        LAYER.with(|e| e.set(false));
        harness
    }

    /// A harness whose window draws with `theme` instead of the fixture theme.
    pub fn with_theme(theme: ThemeTokens) -> Self {
        Self::build(LayoutDirection::Ltr, theme, placeholder_icons(), false)
    }

    fn build(
        direction: LayoutDirection,
        theme: ThemeTokens,
        icons: Arc<dyn Icons>,
        animated: bool,
    ) -> Self {
        Self::build_with(direction, theme, icons, animated, None)
    }

    fn build_with(
        direction: LayoutDirection,
        theme: ThemeTokens,
        icons: Arc<dyn Icons>,
        animated: bool,
        asked: Option<Rc<RefCell<Vec<ActionRequest>>>>,
    ) -> Self {
        let clock = Arc::new(ManualClock::new(Timestamp(0)));
        let text = text();
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text: text.clone(),
            icons: icons.clone(),
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
            accessibility: asked
                .map(|asked| Box::new(Assistive(asked)) as Box<dyn AccessibilityAdapter>),
            clock: clock.clone(),
            on_window_error: None,
        };
        let system = if EFFECTS.with(Cell::get) {
            HeadlessWindowSystem::new(Capabilities {
                app_effects: true,
                raised_layer: LAYER.with(Cell::get),
                ..Capabilities::default()
            })
        } else {
            HeadlessWindowSystem::default()
        };
        let mut rt = Runtime::new(system, options);
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
            animated,
            icons,
            asked: None,
        };
        harness.settle();
        harness
    }

    /// Steps until the window has drawn everything it asked to draw.
    pub fn settle(&mut self) {
        let rounds = if self.animated { 2 } else { 8 };
        for round in 0..rounds {
            if self.animated && round + 1 == rounds {
                self.rt.step(turns()).unwrap();
                return;
            }
            self.rt.step(turns()).unwrap();
            let system = self.rt.window_system_mut();
            // a popup the step closed asks for nothing more
            if !system.frame_requested(self.window).unwrap_or(false) {
                return;
            }
            system.frame_ready(self.window, Timestamp(0)).unwrap();
        }
        panic!("the window keeps asking for frames");
    }

    pub fn resize(&mut self, size: Size) {
        self.rt
            .window_system_mut()
            .configure(self.window, Some(size), active())
            .unwrap();
        self.settle();
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
        self.button_with(position, pressed, Mods::NONE);
    }

    pub fn button_with(&mut self, position: Point, pressed: bool, mods: Mods) {
        self.pointer(PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 1,
            mods,
        });
    }

    /// Moves the pointer onto `position`, presses and releases there.
    pub fn click(&mut self, position: Point) {
        self.click_with(position, Mods::NONE);
    }

    pub fn click_with(&mut self, position: Point, mods: Mods) {
        self.pointer(PointerEvent::Motion { position });
        self.button_with(position, true, mods);
        self.button_with(position, false, mods);
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

    /// Asks for `action` on the node keyed `key` as a screen reader does, and wakes the application for it.
    pub fn ask(&mut self, key: &Key, action: Action) {
        let target = self.rt.find(self.window, key).unwrap();
        self.ask_node(target, action);
    }

    /// Asks for `action` on the object `target`, as a screen reader holding it does.
    pub fn ask_node(&mut self, target: NodeId, action: Action) {
        self.asked
            .as_ref()
            .expect("an assistive harness")
            .borrow_mut()
            .push(ActionRequest {
                target,
                action,
                data: None,
            });
        self.rt
            .window_system_mut()
            .enqueue(PlatformEvent::Wake)
            .unwrap();
        self.settle();
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

    /// The operations the node painted, in its own coordinates, then the fills its overlay draws (a caret, an
    /// underline), moved into them.
    pub fn ops(&self, key: &'static str) -> Vec<PaintOp> {
        let node = self.rt.find(self.window, &Key::from(key)).unwrap();
        let (id, over) = (PaintNodeId::from(node), overlay_id(node));
        let origin = self.rect(key).origin;
        let nodes = self.snapshot().display_list.nodes;
        let mut ops = nodes.iter().find(|n| n.id == id).unwrap().ops.clone();
        if let Some(overlay) = nodes.iter().find(|n| n.id == over) {
            ops.extend(overlay.ops.iter().filter_map(|op| match op {
                PaintOp::Fill {
                    shape: Shape::Rect(rect),
                    paint,
                } => Some(PaintOp::Fill {
                    shape: Shape::Rect(rect.translate(-origin.x, -origin.y)),
                    paint: paint.clone(),
                }),
                _ => None,
            }));
        }
        ops
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
            icons: self.icons.clone(),
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

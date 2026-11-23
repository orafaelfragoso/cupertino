//! The bar and its open menus as CupertinoKit windows. A bar is one window along the top of its output with its slots
//! where the controller laid them out; each open menu level is a popup of the level before it (the first of the bar)
//! at the rectangle the controller gave it. The windows draw and describe what the controller holds and send the
//! pointer and the keys back as [`Input`], in the coordinates of the bar.

use crate::controller::{Level, MenuController};
use crate::extras::{ExtraGlyph, Passphrase};
use crate::fallback::HELP_SEARCH;
use crate::feed::{Shared, StatusIcon, SCREEN_CAPTURE_SLOT};
use crate::geom::{self, Point as BarPoint};
use crate::icons::{Pixels, Request as IconRequest};
use crate::item::{MenuItem, PanelRow, ShortcutGlyphs, Toggle};
use crate::layout::{
    panel, BarMetrics, MenuMetrics, PopupRow, Slot, SlotKey, TextMeasure, PRIVACY_DOT_PT,
    STATUS_ICON_PT, SYSTEM_MARK_PT,
};
use crate::popups;
use crate::tone::{BarTone, MenuPalette, Rgba, SlotTones};
use cupertino_kit::controls::{
    Button, ButtonRole, ButtonStyle, CheckBox, SecureField, Spacer, Stack, Text, TextField,
    TextRole,
};
use cupertino_kit::core::{
    next_resource_id, AccessibleStates, AccessibleValue, Action, Change, Checked, CommandRegistry,
    Element, Environment, Event, EventCx, IconImage, Key, MeasureCx, PaintCx, Phase, Popup,
    PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, Overflow, SizeSpec};
use cupertino_kit::platform::{
    DecorationMode, KeyEvent, MouseButton, PointerEvent, Timestamp, WindowSpec,
};
use cupertino_kit::render::{
    Effect, FillRule, ImageResource, Paint, PaintOp, PathVerb, PixelFormat, Sampling, Shape,
};
use cupertino_kit::text::{
    AttributedText, ContentPurpose, FontWeight, ParagraphLayout, ParagraphStyle, SecureBuffer,
    TextStyle, TextSystem,
};
use cupertino_kit::theme::{Appearance, Color, ColorRole, StateVariant, Surface};
use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

mod body;
mod rows;

/// The `app_id` of a bar's window is this followed by the bar's number; the roles give it its layer surface.
pub const BAR_APP_ID: &str = "cupertino-menubar.bar-";
/// What assistive technology calls a bar.
const BAR_TITLE: &str = "Menu Bar";
/// Corner radius of a popup menu in points.
pub const POPUP_RADIUS: f64 = 11.5;
/// The share of the label's opacity a row's shortcut keeps, on an enabled row and on a disabled one (whose label is
/// dimmed already).
const SHORTCUT_ALPHA: f32 = 0.28;
const DISABLED_SHORTCUT_ALPHA: f32 = 0.7;
/// Opacity factor of a row's secondary text. approximate.
const DETAIL_ALPHA: f32 = 0.65;
/// Edge of a row's icon in points.
const ROW_ICON_PT: f64 = 14.0;
/// Opacity factor of a status item's dot, drawn when it has no icon. approximate.
const DOT_ALPHA: f32 = 0.7;
/// Opacity factor of an item symbol whose radio is off or has no link. approximate (the design does not say).
const DIMMED_ALPHA: f32 = 0.4;
/// The search field's capsule: its height, its distance from the top of its row and from the menu's sides, in points.
const FIELD_HEIGHT: f64 = 24.0;
const FIELD_TOP: f64 = 5.0;
const FIELD_INSET_X: f64 = 10.5;
/// Inset of the field's caret and selection from the field's top and bottom, in points. approximate.
const FIELD_INSET_Y: f64 = 2.0;
/// The field's fill in the dark and in the light appearance: opaque.
const FIELD_FILL_DARK: Rgba = [34.0 / 255.0, 34.0 / 255.0, 34.0 / 255.0, 1.0];
const FIELD_FILL_LIGHT: Rgba = [1.0, 1.0, 1.0, 1.0];
/// Space between the search field's box and its text in points. approximate.
const FIELD_TEXT_INSET: f64 = 12.0;
/// Space between the search field's box and the chord after it in points. approximate.
const FIELD_CHORD_GAP: f64 = 8.0;
/// Width of the search field's caret in points.
const CARET_WIDTH: f64 = 2.0;
/// Opacity factor of the row highlight colour behind the field's selected text. approximate.
const FIELD_SELECTION_ALPHA: f32 = 0.35;
/// Inset of a row's highlight from the menu's sides, and its corner radius, in points.
const ROW_HIGHLIGHT_INSET: f64 = 5.5;
const ROW_HIGHLIGHT_RADIUS: f64 = 6.75;
/// How far the submenu chevron stands further in than the end of the shortcut column.
const CHEVRON_SHIFT: f64 = 2.7;
/// Half the width of the submenu chevron in points.
const CHEVRON_HALF_WIDTH: f64 = 1.8;
/// Stroke widths of the check mark and the submenu chevron in points. approximate.
const CHECK_STROKE: f64 = 1.6;
const CHEVRON_STROKE: f64 = 1.4;
/// Radius of a radio row's dot in points. approximate.
const RADIO_DOT_RADIUS: f64 = 2.4;

/// One output's bar, numbered by the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BarId(pub u64);

impl BarId {
    /// The bar whose window was opened with `app_id`.
    pub fn of(app_id: &str) -> Option<BarId> {
        app_id.strip_prefix(BAR_APP_ID)?.parse().ok().map(BarId)
    }

    pub fn window(self) -> WindowKey {
        WindowKey(Key::from(format!("bar-{}", self.0)))
    }

    fn prompt(self) -> WindowKey {
        WindowKey(Key::from(format!("prompt-{}", self.0)))
    }

    fn menu(self, level: usize) -> WindowKey {
        WindowKey(Key::from(format!("menu-{}-{level}", self.0)))
    }
}

/// What the pointer did, at a point of the bar's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BarPointer {
    Move(BarPoint),
    Press(BarPoint),
    Release(BarPoint),
    /// Assistive technology clicked the slot or the row around the point.
    Click(BarPoint),
}

/// What the user did on a bar or its menus.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Pointer {
        bar: BarId,
        pointer: BarPointer,
    },
    Key {
        bar: BarId,
        key: KeyEvent,
    },
    /// The compositor dismissed the bar's menus.
    Dismissed(BarId),
    /// The compositor closed the bar's window.
    Closed(BarId),
    /// The bar's own window got the keyboard, or lost it.
    Keyboard {
        bar: BarId,
        focused: bool,
    },
    /// Something in the join prompt of the bar.
    Prompt {
        bar: BarId,
        event: PromptEvent,
    },
}

/// What the person did in the join prompt.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptEvent {
    /// The passphrase as typed so far.
    Typed(Passphrase),
    /// The Show Password box was set or cleared.
    Show(bool),
    /// Join with the passphrase as typed, from the button or from Return in the field.
    Join(Passphrase),
    /// Cancel, or a press outside the prompt.
    Cancel,
}

/// How far a join stands.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptPhase {
    Editing,
    /// The service is joining the network.
    Joining,
    /// The attempt failed, with the line that says so.
    Failed(String),
}

/// The join prompt: the passphrase of a secured network, asked in the panel's style under the Wi-Fi item.
pub struct PromptScene<'a> {
    /// The network, by name.
    pub ssid: &'a str,
    /// The prompt's top-left corner in the bar's coordinates: where the Wi-Fi panel's is.
    pub at: BarPoint,
    /// The passphrase is shown as text.
    pub show: bool,
    pub text: &'a Passphrase,
    pub phase: PromptPhase,
}

/// The type of the bar and its menus: the regular items' and the bold application name's weights at one size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarFont {
    pub size: f32,
    pub regular: u16,
    pub bold: u16,
    /// Cap height in em: the text is centred on it.
    pub cap_height: f64,
}

impl BarFont {
    /// The type the theme gives the menu bar.
    pub fn of(design: &cupertino_kit_shell::design::MenuBar) -> Self {
        Self {
            size: design.item.size,
            regular: design.item.weight.0,
            bold: design.app_name.weight.0,
            cap_height: design.cap_height,
        }
    }

    fn layout(&self, text: &TextSystem, s: &str, bold: bool) -> Option<ParagraphLayout> {
        if s.is_empty() {
            return None;
        }
        let style = TextStyle {
            weight: FontWeight(if bold { self.bold } else { self.regular }),
            ..TextStyle::new(self.size)
        };
        let paragraph = text
            .paragraph(&AttributedText::plain(s, style), &ParagraphStyle::default())
            .ok()?;
        Some(paragraph.layout(None))
    }
}

/// Measures the bar's and the menus' text with the text system the windows draw it with.
pub struct KitMeasure {
    pub text: Arc<TextSystem>,
    pub font: BarFont,
}

impl TextMeasure for KitMeasure {
    fn width(&self, text: &str, bold: bool) -> f64 {
        self.font
            .layout(&self.text, text, bold)
            .map_or(0.0, |l| l.size().width)
    }
}

/// What a bar's windows show.
pub struct BarScene<'a> {
    pub bar: BarId,
    pub controller: &'a MenuController,
    pub shared: &'a Shared,
    pub font: BarFont,
    pub tone: BarTone,
    /// The tone of each item of the status area that follows the wallpaper under it; the others take [`Self::tone`].
    pub slot_tones: &'a SlotTones,
    /// Whether the bar shows its strip of the menu bar material: with Reduce Transparency, where the compositor draws
    /// the material opaque, and when `menubar.conf` asks for a backdrop of the bar's own ([`crate::tone::solid_bar`]
    /// of the display options the bar's tone follows).
    pub solid: bool,
    /// The bar's size: the output's width and the bar's height.
    pub size: Size,
    /// The items whose panel, shown by another process, is open on this bar's output
    /// ([`crate::extras::OpenPanels::open_on`]): drawn highlighted as an open menu's title is.
    pub open_panels: &'a [SlotKey],
    /// The icons of menu rows.
    pub icons: &'a Rc<RowIcons>,
    /// The join prompt, when it is open on this bar.
    pub prompt: Option<PromptScene<'a>>,
}

/// The bar's window and one popup per open menu level.
pub fn windows<M: Clone + 'static>(scene: &BarScene<'_>, input: fn(Input) -> M) -> Vec<Window<M>> {
    let mut windows = vec![bar(scene, input)];
    let levels = scene.controller.levels();
    let rects: Vec<geom::Rect> = levels.iter().map(|l| popups::snap(l.layout.rect)).collect();
    for (level, at) in popups::relative_to_parents(&rects).into_iter().enumerate() {
        let parent = match level {
            0 => scene.bar.window(),
            _ => scene.bar.menu(level - 1),
        };
        windows.push(rows::menu(scene, level, rects[level], at, parent, input));
    }
    if let Some(prompt) = &scene.prompt {
        windows.push(join_prompt(scene.bar, prompt, input));
    }
    windows
}

/// The prompt's size: the panel's width, and a height that holds its title, field, box, status line and buttons.
const PROMPT_HEIGHT: f64 = 176.0;
/// Space between the prompt's edge and what it holds, and between its rows.
const PROMPT_PAD: f64 = panel::TEXT_INSET;
const PROMPT_GAP: f64 = 9.0;
/// The most bytes of a passphrase the field takes (the longest WPA passphrase is 63).
const PASSPHRASE_MAX: usize = 128;

fn plain_text<M: 'static>(text: &str, role: TextRole, size: f32, secondary: bool) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(text, TextStyle::new(size)),
        role,
        wrap: true,
        secondary,
        selected: false,
    })
}

/// The join prompt as a popup of the bar, with the panel's material and corners.
fn join_prompt<M: Clone + 'static>(
    bar: BarId,
    prompt: &PromptScene<'_>,
    input: fn(Input) -> M,
) -> Window<M> {
    let event = move |e: PromptEvent| input(Input::Prompt { bar, event: e });
    let joining = prompt.phase == PromptPhase::Joining;
    let empty = prompt.text.as_str().is_empty();
    let field = if prompt.show {
        Element::new(TextField {
            label: "Password".into(),
            placeholder: "Password".into(),
            value: prompt.text.as_str().to_owned(),
            revision: 0,
            purpose: ContentPurpose::Password,
            enabled: !joining,
            on_change: Some(Box::new(move |change| {
                event(PromptEvent::Typed(Passphrase::new(&change.text)))
            })),
            on_submit: (!empty).then(|| event(PromptEvent::Join(prompt.text.clone()))),
            selection: None,
            initial: true,
        })
        .key("shown")
    } else {
        Element::new(SecureField {
            label: "Password".into(),
            placeholder: "Password".into(),
            enabled: !joining,
            on_change: Some(Box::new(move |secret| {
                event(PromptEvent::Typed(typed(secret)))
            })),
            on_submit: Some(Box::new(move |secret| {
                event(PromptEvent::Join(typed(secret)))
            })),
            invalid: matches!(prompt.phase, PromptPhase::Failed(_)),
            max_len: Some(PASSPHRASE_MAX),
            initial: true,
            ..SecureField::default()
        })
        .key("hidden")
    };
    let status = match &prompt.phase {
        PromptPhase::Editing => String::new(),
        PromptPhase::Joining => "Joining\u{2026}".to_owned(),
        PromptPhase::Failed(why) => why.clone(),
    };
    let buttons = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: 8.0,
        align: Align::Center,
        children: vec![
            Element::new(Spacer::default()),
            Element::new(Button {
                label: "Cancel".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: Some(event(PromptEvent::Cancel)),
            })
            .key("cancel"),
            Element::new(Button {
                label: "Join".into(),
                role: ButtonRole::Default,
                style: ButtonStyle::Bordered,
                enabled: !empty && !joining,
                busy: joining,
                on_press: Some(event(PromptEvent::Join(prompt.text.clone()))),
            })
            .key("join"),
        ],
    });
    let content = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets {
            top: PROMPT_PAD,
            left: PROMPT_PAD,
            bottom: PROMPT_PAD,
            right: PROMPT_PAD,
        },
        spacing: PROMPT_GAP,
        align: Align::Stretch,
        children: vec![
            plain_text(
                &format!("Join \u{201c}{}\u{201d}", prompt.ssid),
                TextRole::Section,
                13.0,
                false,
            ),
            field,
            Element::new(CheckBox {
                label: "Show Password".into(),
                checked: if prompt.show {
                    Checked::True
                } else {
                    Checked::False
                },
                enabled: !joining,
                on_toggle: Some(Box::new(move |on| event(PromptEvent::Show(on)))),
            })
            .key("show"),
            plain_text(
                &status,
                TextRole::Caption,
                11.0,
                !matches!(prompt.phase, PromptPhase::Failed(_)),
            ),
            buttons,
        ],
    });
    let size = Size::new(panel::WIDTH, PROMPT_HEIGHT);
    Window {
        key: bar.prompt(),
        spec: spec(BAR_APP_ID.to_owned(), size),
        shortcuts: CommandRegistry::new(),
        on_close: None,
        // the body is the menu glass the content lies on
        background: WindowBackground::None,
        content: Element::new(body::Body {
            radius: panel::RADIUS,
            children: vec![content],
        }),
        popup: Some(Popup {
            parent: bar.window(),
            // `At` hangs the popup off the far corner of the point's one-point square
            anchor: PopupAnchor::At(Point::new(prompt.at.x - 1.0, prompt.at.y - 1.0)),
            grab: true,
            on_dismiss: event(PromptEvent::Cancel),
        }),
        menus: None,
    }
}

fn typed(secret: &SecureBuffer) -> Passphrase {
    secret.with_secret(Passphrase::new)
}

fn spec(app_id: String, size: Size) -> WindowSpec {
    WindowSpec {
        title: BAR_TITLE.to_owned(),
        app_id,
        size,
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: None,
    }
}

fn bar<M: Clone + 'static>(scene: &BarScene<'_>, input: fn(Input) -> M) -> Window<M> {
    let controller = scene.controller;
    let highlight = controller.highlighted_slot();
    let mut places = 0;
    let children: Vec<_> = controller
        .layout()
        .slots
        .iter()
        .map(|slot| {
            let (role, name) = slot_semantics(slot, scene);
            let place = (role == Role::MenuItem).then(|| {
                places += 1;
                places - 1
            });
            Element::new(SlotNode {
                place,
                bar: scene.bar,
                slot: slot.clone(),
                look: slot_look(slot, scene),
                highlighted: highlight == Some(&slot.key) || scene.open_panels.contains(&slot.key),
                expanded: expanded(&slot.key, scene),
                role,
                name,
                metrics: controller.bar_metrics.clone(),
                font: scene.font,
                tone: scene.slot_tones.tone(&slot.key).unwrap_or(scene.tone),
                input,
            })
        })
        .collect();
    Window {
        key: scene.bar.window(),
        spec: spec(format!("{BAR_APP_ID}{}", scene.bar.0), scene.size),
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(scene.bar))),
        // the solid strip under Reduce Transparency, or the bar's own backdrop `menubar.conf` asks for: the menu bar
        // material, which the compositor draws opaque with Reduce Transparency; see-through, the bar draws nothing
        background: if scene.solid {
            WindowBackground::Surface {
                surface: Surface::MenuBar,
                radius: 0.0,
                alpha: 1.0,
            }
        } else {
            WindowBackground::None
        },
        content: Element::new(Root {
            body: None,
            bar: scene.bar,
            keyboard: true,
            origin: BarPoint::new(0.0, 0.0),
            size: None,
            role: Role::MenuBar,
            name: BAR_TITLE.to_owned(),
            items: places,
            input,
            children,
        }),
        popup: None,
        menus: None,
    }
}

/// Whether the slot's menu or panel is open, for a slot that opens one: its menu, or the panel of another process.
fn expanded(key: &SlotKey, scene: &BarScene<'_>) -> Option<bool> {
    match key {
        SlotKey::ControlPanel | SlotKey::Clock => Some(scene.open_panels.contains(key)),
        _ => has_menu(key, scene).then(|| scene.controller.open_slot() == Some(key)),
    }
}

/// Whether the slot opens a menu.
fn has_menu(key: &SlotKey, scene: &BarScene<'_>) -> bool {
    let content = scene.controller.content();
    match key {
        SlotKey::System | SlotKey::AppMenu | SlotKey::Menu(_) => true,
        SlotKey::Status(id) => content
            .status
            .iter()
            .any(|s| &s.id == id && s.menu.is_some()),
        SlotKey::ControlPanel | SlotKey::Clock => false,
    }
}

fn slot_semantics(slot: &Slot, scene: &BarScene<'_>) -> (Role, String) {
    let content = scene.controller.content();
    let item = if has_menu(&slot.key, scene) {
        Role::MenuItem
    } else {
        Role::Button
    };
    let name = match &slot.key {
        SlotKey::System => "System".to_owned(),
        SlotKey::AppMenu => content.app_name.clone(),
        SlotKey::Menu(id) => content
            .menus
            .iter()
            .find(|m| m.id == *id)
            .map(|m| m.label.clone())
            .unwrap_or_default(),
        SlotKey::Status(id) if id == SCREEN_CAPTURE_SLOT => {
            return (Role::Image, "Screen Capture".to_owned())
        }
        SlotKey::Status(id) => content
            .status
            .iter()
            .find(|s| &s.id == id)
            .and_then(|s| s.menu.as_ref())
            .map(|m| m.label.clone())
            .unwrap_or_default(),
        SlotKey::ControlPanel => "Control Panel".to_owned(),
        SlotKey::Clock => content.clock.clone(),
    };
    (item, name)
}

/// What a slot of the bar draws.
#[derive(Clone, Debug, PartialEq)]
enum SlotLook {
    SystemMark,
    Text {
        text: String,
        padding: f64,
        bold: bool,
    },
    PrivacyDot,
    Image(IconImage),
    Symbol(ExtraGlyph),
    Dot,
    ControlPanel,
    Nothing,
}

impl SlotLook {
    /// Whether this is a status item's symbol in the bar's text colour: an extra's symbol, a template icon, the dot of an
    /// item without one or the control panel's glyph. Text, the system mark, the privacy dot and coloured icons are not.
    fn is_status_glyph(&self) -> bool {
        match self {
            SlotLook::Symbol(_) | SlotLook::Dot | SlotLook::ControlPanel => true,
            SlotLook::Image(icon) => is_template(&icon.image.pixels),
            SlotLook::SystemMark
            | SlotLook::Text { .. }
            | SlotLook::PrivacyDot
            | SlotLook::Nothing => false,
        }
    }
}

fn slot_look(slot: &Slot, scene: &BarScene<'_>) -> SlotLook {
    let content = scene.controller.content();
    let metrics = &scene.controller.bar_metrics;
    match &slot.key {
        SlotKey::System => SlotLook::SystemMark,
        SlotKey::AppMenu => SlotLook::Text {
            text: content.app_name.clone(),
            padding: metrics.title_padding,
            bold: true,
        },
        SlotKey::Menu(id) => match content.menus.iter().find(|m| m.id == *id) {
            Some(m) => SlotLook::Text {
                text: m.label.clone(),
                padding: metrics.title_padding,
                bold: false,
            },
            None => SlotLook::Nothing,
        },
        SlotKey::Status(id) if id == SCREEN_CAPTURE_SLOT => SlotLook::PrivacyDot,
        SlotKey::Status(id) => match scene.shared.status_icon(id) {
            Some(StatusIcon::Image(image)) => SlotLook::Image(image.clone()),
            Some(StatusIcon::Symbol(glyph)) => SlotLook::Symbol(glyph),
            Some(StatusIcon::Dot) => SlotLook::Dot,
            None => SlotLook::Nothing,
        },
        SlotKey::ControlPanel => SlotLook::ControlPanel,
        SlotKey::Clock => SlotLook::Text {
            text: content.clock.clone(),
            padding: metrics.clock_padding,
            bold: false,
        },
    }
}

/// Whether the session's appearance is dark, which picks the palette of the menus and of a bar without its own tone.
/// The colours of a popup's rows: the appearance's, strengthened with Increase Contrast.
fn popup_palette(env: &Environment) -> MenuPalette {
    let palette = MenuPalette::new(dark(env));
    if env.theme.preferences.appearance.is_high_contrast() {
        palette.high_contrast()
    } else {
        palette
    }
}

/// Whether the session's appearance has Increase Contrast, which makes the bar's text opaque.
fn high_contrast(env: &Environment) -> bool {
    env.theme.preferences.appearance.is_high_contrast()
}

fn dark(env: &Environment) -> bool {
    matches!(
        env.theme.preferences.appearance,
        Appearance::Dark | Appearance::DarkHighContrast
    )
}

fn color(c: Rgba) -> Color {
    Color::new(c[0], c[1], c[2], c[3])
}

fn faded(c: Rgba, alpha: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * alpha]
}

/// What a status slot showing `glyph` draws at `scale` device pixels per point in a bar of `palette`: the template symbol's
/// name, its side in device pixels (the bar's status icon size) and its colour (the bar's text colour, faded when the
/// item is dimmed).
pub fn status_symbol(
    glyph: ExtraGlyph,
    palette: &MenuPalette,
    scale: f64,
) -> (&'static str, u32, Rgba) {
    let alpha = if glyph.dimmed { DIMMED_ALPHA } else { 1.0 };
    let px = (STATUS_ICON_PT * scale).ceil().max(1.0) as u32;
    (glyph.symbol, px, faded(palette.text, alpha))
}

/// A status item's own icon as the bar draws it: a template (grey strokes only) in the bar's text colour, as the
/// symbols are, and a coloured icon as it is.
pub fn status_image(icon: &IconImage, palette: &MenuPalette) -> IconImage {
    RowIcon {
        template: is_template(&icon.image.pixels),
        image: icon.clone(),
    }
    .tinted(palette.text)
}

fn fill(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, c: Rgba) {
    let shape = if radius > 0.0 {
        Shape::RoundedRect { rect, radius }
    } else {
        Shape::Rect(rect)
    };
    cx.push(PaintOp::Fill {
        shape,
        paint: Paint::Solid(color(c)),
    });
}

fn circle(cx: &mut PaintCx<'_>, centre: Point, radius: f64, c: Rgba) {
    let rect = Rect::new(
        centre.x - radius,
        centre.y - radius,
        2.0 * radius,
        2.0 * radius,
    );
    fill(cx, rect, radius, c);
}

fn polyline(cx: &mut PaintCx<'_>, points: &[Point], width: f64, c: Rgba) {
    let mut verbs = Vec::with_capacity(points.len());
    for (i, &p) in points.iter().enumerate() {
        verbs.push(if i == 0 {
            PathVerb::MoveTo(p)
        } else {
            PathVerb::LineTo(p)
        });
    }
    cx.push(PaintOp::Stroke {
        shape: Shape::Path {
            verbs,
            fill_rule: FillRule::NonZero,
        },
        width,
        paint: Paint::Solid(color(c)),
    });
}

/// `text` with its left edge at `x`, centred on its cap height in the band `top..top + height`.
#[allow(clippy::too_many_arguments)]
fn text(
    cx: &mut PaintCx<'_>,
    font: &BarFont,
    s: &str,
    x: f64,
    top: f64,
    height: f64,
    bold: bool,
    c: Rgba,
) {
    let system = Arc::clone(&cx.env().text);
    let Some(layout) = font.layout(&system, s, bold) else {
        return;
    };
    let baseline = top + height / 2.0 + font.cap_height * f64::from(font.size) / 2.0;
    let first = layout.lines().first().map_or(0.0, |line| line.baseline);
    let origin = Point::new(x, baseline - first);
    for line in layout.lines() {
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                origin: origin + run.origin,
                run: run.run.clone(),
                color: color(c),
            });
        }
    }
}

fn width(cx: &PaintCx<'_>, font: &BarFont, s: &str) -> f64 {
    font.layout(&cx.env().text, s, false)
        .map_or(0.0, |l| l.size().width)
}

/// `image` centred at `centre`, scaled down to fit `side` points square.
fn image(cx: &mut PaintCx<'_>, image: &IconImage, centre: Point, side: f64) {
    let scale = cx.env().scale;
    let (w, h) = (f64::from(image.image.width), f64::from(image.image.height));
    let k = (side * scale / w.max(h)).min(1.0 + 1e-3) / scale;
    let dst = Rect::new(centre.x - w * k / 2.0, centre.y - h * k / 2.0, w * k, h * k);
    cx.image(image.id, &image.image, dst, Sampling::Linear);
}

/// `pixmap` as an image the windows draw; each call names new pixels.
pub fn icon_image(pixels: &Pixels) -> IconImage {
    IconImage {
        id: next_resource_id(),
        image: ImageResource {
            revision: 1,
            width: pixels.width,
            height: pixels.height,
            stride: pixels.width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixels.rgba.as_slice()),
        },
    }
}

/// Device pixels of a row's icon at `scale` device pixels per point.
fn row_icon_px(scale: f64) -> u32 {
    (ROW_ICON_PT * scale).ceil().max(1.0) as u32
}

/// An icon of a menu row, loaded.
#[derive(Clone, Debug)]
pub struct RowIcon {
    image: IconImage,
    /// Grey strokes only: a symbol, which takes the colour of the row's text. A coloured icon keeps its own.
    template: bool,
}

/// Whether every visible pixel of the premultiplied `pixels` is grey, within a small margin.
fn is_template(pixels: &[u8]) -> bool {
    pixels.as_chunks::<4>().0.iter().all(|px| {
        let a = u32::from(px[3]);
        a == 0 || {
            let plain = |c: u8| (u32::from(c) * 255 / a).min(255);
            let (r, g, b) = (plain(px[0]), plain(px[1]), plain(px[2]));
            r.max(g).max(b) - r.min(g).min(b) <= 24
        }
    })
}

impl RowIcon {
    fn new(pixels: &Pixels) -> Self {
        Self {
            template: is_template(&pixels.rgba),
            image: icon_image(pixels),
        }
    }

    /// The icon in `c` when it is a symbol, else as it is.
    fn tinted(&self, c: Rgba) -> IconImage {
        if !self.template {
            return self.image.clone();
        }
        type Tints = Mutex<HashMap<(u64, [u8; 4]), IconImage>>;
        static TINTS: OnceLock<Tints> = OnceLock::new();
        let key = (
            self.image.id.0,
            c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8),
        );
        let mut tints = TINTS
            .get_or_init(Tints::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        tints
            .entry(key)
            .or_insert_with(|| {
                let mut tinted = self.image.clone();
                let strength = c[3].clamp(0.0, 1.0);
                let pixels: Vec<u8> = self
                    .image
                    .image
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|px| {
                        let a = f32::from(px[3]) / 255.0 * strength;
                        let part = |v: f32| (v.clamp(0.0, 1.0) * a * 255.0).round() as u8;
                        [
                            part(c[0]),
                            part(c[1]),
                            part(c[2]),
                            (a * 255.0).round() as u8,
                        ]
                    })
                    .collect();
                tinted.id = next_resource_id();
                tinted.image.pixels = Arc::from(pixels);
                tinted
            })
            .clone()
    }
}

/// The icons of menu rows, as the component loaded them on the I/O pool ([`crate::icons::Loader`]), once per icon and
/// size. A paint that finds an icon not loaded yet draws the row without it and asks for it ([`RowIcons::take_wanted`]);
/// each icon that arrives moves [`RowIcons::revision`] on, which repaints the rows.
#[derive(Default)]
pub struct RowIcons {
    loaded: RefCell<HashMap<(u64, u32), Option<RowIcon>>>,
    /// Icons asked for and not loaded yet, the oldest first, each once.
    wanted: RefCell<Vec<IconRequest>>,
    asked: RefCell<HashSet<(u64, u32)>>,
    /// Told once whenever `wanted` gains its first request.
    notify: Option<async_channel::Sender<()>>,
    revision: Cell<u64>,
}

impl RowIcons {
    /// Icons that say on `notify` when a paint asked for one that is not loaded.
    pub fn new(notify: async_channel::Sender<()>) -> Self {
        Self {
            notify: Some(notify),
            ..Self::default()
        }
    }

    /// The icons a paint asked for since the last call, for the component to load.
    pub fn take_wanted(&self) -> Vec<IconRequest> {
        std::mem::take(&mut *self.wanted.borrow_mut())
    }

    /// The icon of a request arrived (`None`: there is none); the rows that show it are drawn again.
    pub fn loaded(&self, key: (u64, u32), pixels: Option<&Pixels>) {
        self.asked.borrow_mut().remove(&key);
        self.loaded
            .borrow_mut()
            .insert(key, pixels.map(RowIcon::new));
        self.revision.set(self.revision.get() + 1);
    }

    /// Moves on with every icon that arrives.
    pub fn revision(&self) -> u64 {
        self.revision.get()
    }

    /// The icon of `item` at `px` device pixels: what it sent inline, else its themed name; `None` without one, and
    /// while it is not loaded.
    fn row_icon(&self, item: &MenuItem, px: u32) -> Option<RowIcon> {
        if item.icon_name.is_none() && item.icon_data.is_none() {
            return None;
        }
        let mut hasher = DefaultHasher::new();
        item.icon_data.hash(&mut hasher);
        item.icon_name.hash(&mut hasher);
        let key = (hasher.finish(), px);
        if let Some(icon) = self.loaded.borrow().get(&key) {
            return icon.clone();
        }
        if self.asked.borrow_mut().insert(key) {
            let mut wanted = self.wanted.borrow_mut();
            wanted.push(IconRequest {
                key,
                data: item.icon_data.clone(),
                name: item.icon_name.clone(),
                px,
            });
            if wanted.len() == 1 {
                if let Some(notify) = &self.notify {
                    // a full channel has told already
                    let _ = notify.try_send(());
                }
            }
        }
        None
    }
}

/// The system menu's mark at `px` device pixels in `c`, drawn once per size and colour.
fn system_mark(px: u32, c: Rgba) -> IconImage {
    type Marks = Mutex<HashMap<(u32, [u8; 4]), IconImage>>;
    static MARKS: OnceLock<Marks> = OnceLock::new();
    let key = (px, c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
    let mut marks = MARKS
        .get_or_init(Marks::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    marks
        .entry(key)
        .or_insert_with(|| {
            let mark = cupertino_icons::system_mark(px, c);
            icon_image(&Pixels {
                width: mark.width(),
                height: mark.height(),
                rgba: mark.data().to_vec(),
            })
        })
        .clone()
}

/// The root of a bar's window or of one of its menus: it takes the pointer and the keys for the controller before any
/// node below it, and lays its children out where the controller put them.
struct Root<M> {
    /// A menu's body under its rows, with corners of this radius ([`body::paint`]); a bar paints none.
    body: Option<f64>,
    bar: BarId,
    /// The bar's own root: it holds the window's focus, so it hears the window get the keyboard and lose it.
    keyboard: bool,
    /// The window's top-left corner in the bar's coordinates.
    origin: BarPoint,
    /// A menu's size; a bar fills its window.
    size: Option<Size>,
    role: Role,
    name: String,
    /// How many items the bar or menu holds, which a screen reader reads with each item's place.
    items: usize,
    input: fn(Input) -> M,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Root<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let (width, height) = match self.size {
            Some(size) => (SizeSpec::Fixed(size.width), SizeSpec::Fixed(size.height)),
            None => (SizeSpec::Fill, SizeSpec::Fill),
        };
        LayoutSpec {
            width,
            height,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else if self.name != old.name
            || self.items != old.items
            || self.origin != old.origin
            || self.body != old.body
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size.unwrap_or(Size::ZERO),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let at = |p: Point| BarPoint::new(p.x + self.origin.x, p.y + self.origin.y);
        let input = match event {
            Event::Pointer(pointer, _) if cx.phase() == Phase::Capture => {
                let pointer = match *pointer {
                    PointerEvent::Enter { position } | PointerEvent::Motion { position } => {
                        BarPointer::Move(at(position))
                    }
                    PointerEvent::Button {
                        position,
                        button: MouseButton::Left,
                        pressed,
                        ..
                    } => {
                        if pressed {
                            BarPointer::Press(at(position))
                        } else {
                            BarPointer::Release(at(position))
                        }
                    }
                    // the controller keeps its selection until the pointer shows up elsewhere
                    _ => return Propagation::Stop,
                };
                Input::Pointer {
                    bar: self.bar,
                    pointer,
                }
            }
            Event::Key(key, _) => Input::Key {
                bar: self.bar,
                key: *key,
            },
            Event::WindowActive(focused) if self.keyboard => {
                cx.emit((self.input)(Input::Keyboard {
                    bar: self.bar,
                    focused: *focused,
                }));
                return Propagation::Continue;
            }
            _ => return Propagation::Continue,
        };
        cx.emit((self.input)(input));
        Propagation::Stop
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if let Some(radius) = self.body {
            body::paint(cx, radius);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(self.role);
        cx.name(self.name.clone());
        cx.item_count(self.items);
        if self.keyboard {
            cx.focusable();
            cx.initial_focus();
        }
    }
}

/// Places a leaf at `rect` of its absolute parent.
fn placed(rect: Rect) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fixed(rect.width()),
        height: SizeSpec::Fixed(rect.height()),
        position: rect.origin,
        ..LayoutSpec::new(ContainerKind::Leaf)
    }
}

fn kit_rect(r: geom::Rect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

fn centre(r: geom::Rect) -> BarPoint {
    BarPoint::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// Assistive technology's click on a slot or a row: the pointer pressed and released at its centre.
fn clicked<M>(
    event: &Event,
    cx: &mut EventCx<'_, M>,
    bar: BarId,
    at: BarPoint,
    input: fn(Input) -> M,
) -> Propagation {
    match event {
        Event::Action(request) if request.action == Action::Click => {
            cx.emit(input(Input::Pointer {
                bar,
                pointer: BarPointer::Click(at),
            }));
            Propagation::Stop
        }
        _ => Propagation::Continue,
    }
}

/// One slot of a bar: its highlight and what it draws.
struct SlotNode<M> {
    /// The slot's place among the bar's menus, if it opens one.
    place: Option<usize>,
    bar: BarId,
    slot: Slot,
    look: SlotLook,
    highlighted: bool,
    /// Whether the slot's menu is open; `None` for a slot without a menu.
    expanded: Option<bool>,
    role: Role,
    name: String,
    metrics: BarMetrics,
    font: BarFont,
    tone: BarTone,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for SlotNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(kit_rect(self.slot.rect))
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.slot.rect != old.slot.rect {
            Change::Layout
        } else if self.look != old.look
            || self.highlighted != old.highlighted
            || self.expanded != old.expanded
            || self.name != old.name
            || self.place != old.place
            || self.tone != old.tone
            || self.font != old.font
            || self.metrics != old.metrics
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.slot.rect.w, self.slot.rect.h),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        clicked(event, cx, self.bar, centre(self.slot.rect), self.input)
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let palette = self.tone.palette(dark(cx.env()), high_contrast(cx.env()));
        let size = cx.size();
        let m = &self.metrics;
        if self.highlighted {
            // a capsule: a title's reaches past the slot's padding, over the room between two titles; a status item's
            // fills its frame
            let (x, width) = m.highlight_span(&self.slot.key, size.width);
            let height = size.height - 2.0 * m.selection_inset_y;
            let rect = Rect::new(x, m.selection_inset_y, width, height);
            let radius = m.selection_corner_radius.min(height / 2.0);
            fill(cx, rect, radius, palette.bar_highlight);
        }
        // the text and symbols lie straight on the wallpaper, sharp, with no halo under them: their tone, dark or light
        // for the wallpaper under them, keeps them readable; a status symbol drawn light keeps one faint shadow below it
        let shadow = self
            .look
            .is_status_glyph()
            .then(|| crate::tone::status_shadow(self.tone, dark(cx.env())))
            .flatten();
        if let Some(c) = shadow {
            cx.push(PaintOp::PushLayer {
                opacity: 1.0,
                effects: vec![Effect::DropShadow {
                    offset: Point::new(0.0, crate::tone::STATUS_SHADOW_DROP),
                    blur: crate::tone::STATUS_SHADOW_BLUR,
                    color: color(c),
                }],
            });
        }
        self.paint_look(cx, &palette);
        if shadow.is_some() {
            cx.push(PaintOp::PopLayer);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(self.role);
        cx.name(self.name.clone());
        if let Some(place) = self.place {
            cx.item_index(place);
        }
        cx.states(AccessibleStates {
            selected: self.highlighted,
            expanded: self.expanded,
            ..AccessibleStates::default()
        });
        if self.role != Role::Image {
            cx.action(Action::Click);
        }
    }
}

impl<M> SlotNode<M> {
    /// The slot's text, symbol or mark in `palette`.
    fn paint_look(&self, cx: &mut PaintCx<'_>, palette: &MenuPalette) {
        let size = cx.size();
        let middle = Point::new(size.width / 2.0, size.height / 2.0);
        let scale = cx.env().scale;
        match &self.look {
            SlotLook::SystemMark => {
                let px = (SYSTEM_MARK_PT * scale).round().max(1.0) as u32;
                let mark = system_mark(px, palette.text);
                let side = f64::from(mark.image.width) / scale;
                // on whole device pixels, so the strokes stay sharp
                let snap = |v: f64| (v * scale).round() / scale;
                let dst = Rect::new(
                    snap(middle.x - side / 2.0),
                    snap(middle.y - side / 2.0),
                    side,
                    f64::from(mark.image.height) / scale,
                );
                cx.image(mark.id, &mark.image, dst, Sampling::Nearest);
            }
            SlotLook::Text {
                text: s,
                padding,
                bold,
            } => text(
                cx,
                &self.font,
                s,
                *padding,
                0.0,
                size.height,
                *bold,
                palette.text,
            ),
            SlotLook::PrivacyDot => {
                circle(cx, middle, PRIVACY_DOT_PT / 2.0, palette.screen_capture)
            }
            SlotLook::Image(icon) => {
                image(cx, &status_image(icon, palette), middle, STATUS_ICON_PT)
            }
            SlotLook::Symbol(glyph) => {
                let (name, px, tint) = status_symbol(*glyph, palette, scale);
                let icon = cx.env().icons.symbol(name, px, color(tint), false);
                image(cx, &icon, middle, STATUS_ICON_PT);
            }
            SlotLook::Dot => circle(
                cx,
                middle,
                STATUS_ICON_PT * 0.3,
                faded(palette.text, DOT_ALPHA),
            ),
            SlotLook::ControlPanel => {
                // two small switches
                for (dy, knob) in [(-3.2, 1.8), (3.2, -1.8)] {
                    let track = Rect::new(middle.x - 6.0, middle.y + dy - 2.2, 12.0, 4.4);
                    fill(cx, track, 2.2, faded(palette.text, 0.45));
                    let at = Point::new(middle.x + knob * 2.0, middle.y + dy);
                    circle(cx, at, 2.6, palette.text);
                }
            }
            SlotLook::Nothing => {}
        }
    }
}

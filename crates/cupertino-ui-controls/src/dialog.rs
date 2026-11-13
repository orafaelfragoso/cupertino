//! A window's modal dialogs: the content behind and the panel hanging near the window's top. A dialog is a sheet, a
//! solid panel with its controls over a scrim that dims the window; an alert is a narrow panel of dense glass over the
//! window's content (a nearly opaque grey where there is no such glass) with the application's icon, its title and
//! message and a column of capsule buttons, hanging closer to the top with no scrim.
//! A sheet shows at once, which a dialog shown as a window of its own gets motion for from the compositor; an alert
//! pops in (its panel grows past its size and settles, its content fading in a little after) and goes at once.

use crate::button::{SheetButton, CAPSULE_INSET};
use crate::glass::nested_radius;
use crate::look;
use crate::{
    Button, ButtonRole, ButtonStyle, Dialog, DialogAction, DialogKind, Modal, Stack, Text, TextRole,
};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Change, Element, Event, EventCx, Glass, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec, Track,
};
use cupertino_ui_platform::{Keysym, Timestamp};
use cupertino_ui_render::{Effect, FillRule, GradientStop, Paint, PaintOp, Sampling, Shape};
use cupertino_ui_text::{AttributedText, FontWeight, TextStyle};
use cupertino_ui_theme::{Color, ColorRole, StateVariant};
use std::cell::Cell;

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const ESCAPE: Keysym = Keysym(0xff1b);
const TAB: Keysym = Keysym(0xff09);
const LEFT_TAB: Keysym = Keysym(0xfe20);
/// The sheet: its width, corner radius and padding.
const WIDTH: f64 = 340.0;
const RADIUS: f64 = 16.0;
const PADDING: f64 = 20.0;
/// Between the title, the message, the body and the buttons.
const SPACING: f64 = 12.0;
const BUTTON_SPACING: f64 = 8.0;
/// The hairline around a panel, and the sheet's light rim just inside its edge in the dark appearance.
const EDGE: f64 = 1.0;
const RIM_DARK: f32 = 0.08;
/// The hairline's darkness around a panel in the dark and the light appearance.
const OUTLINE_DARK: f32 = 0.5;
const OUTLINE_LIGHT: f32 = 0.12;
/// The sheet's shadow.
const SHADOW_OFFSET: f64 = 8.0;
const SHADOW_BLUR: f64 = 24.0;
const SHADOW_ALPHA: f32 = 0.3;
/// The alert's body: a grey at an opacity that keeps the content behind from showing through its text, in the dark
/// and the light appearance (opaque under Reduce Transparency and Increase Contrast), and its rim one device pixel wide
/// just inside the edge: a share of white in the dark appearance, of black in the light one.
const ALERT_BODY_DARK: f32 = 0.2;
const ALERT_BODY_LIGHT: f32 = 0.99;
const ALERT_BODY_ALPHA: f32 = 0.97;
/// The faint warm tint at the top of a light alert's body: its colour, how far down it fades out, in points, where it is
/// strongest as a share of that and how strong it is at the top and there.
const ALERT_TINT: [f32; 3] = [1.0, 0.90, 0.75];
const ALERT_TINT_HEIGHT: f64 = 130.0;
const ALERT_TINT_PEAK_AT: f32 = 0.12;
const ALERT_TINT_TOP: f32 = 0.17;
const ALERT_TINT_PEAK: f32 = 0.21;
const ALERT_RIM_DARK: f32 = 0.12;
const ALERT_RIM_LIGHT: f32 = 0.06;
/// How far below the window's top a dialog hangs: under the toolbar row.
const HANG: f64 = 52.0;
/// How far below the window's top an alert hangs.
const ALERT_HANG: f64 = 24.0;
/// An alert's pop-in. The panel starts at [`POP_START`] of its size, grows to `1 + POP_OVERSHOOT` of it at
/// [`POP_PEAK`] of the way through [`POP_DURATION`] and settles back to its size by the end; it fades in over
/// [`POP_PANEL_FADE`] seconds, and its content fades in between [`POP_CONTENT_DELAY`] and the same again later.
const POP_DURATION: f64 = 0.31;
const POP_START: f64 = 0.07;
const POP_OVERSHOOT: f64 = 0.18;
const POP_PEAK: f64 = 0.5;
/// How sharply the pop-in's rise eases in (the exponent of the sine it follows) and how its settling creeps in.
const POP_RISE_EXPONENT: f64 = 1.7;
const POP_SETTLE_EXPONENT: f64 = 1.35;
const POP_PANEL_FADE: f64 = 0.1;
const POP_CONTENT_DELAY: f64 = 0.06;
const POP_CONTENT_FADE: f64 = 0.08;
/// The alert's shadow: lower and fainter above than below.
const ALERT_SHADOW_OFFSET: f64 = 9.0;
const ALERT_SHADOW_BLUR: f64 = 34.0;
const ALERT_SHADOW_ALPHA: f32 = 0.27;
/// The alert: its width, corner radius and padding; the icon and the text sit further in than the buttons.
const ALERT_WIDTH: f64 = 260.0;
const ALERT_RADIUS: f64 = 26.0;
const ALERT_PADDING: Insets = Insets {
    top: 22.0,
    left: 16.0,
    bottom: 16.0,
    right: 16.0,
};
const ALERT_TEXT_INSET: f64 = 6.0;
/// The application's icon, and the room under it, under the title and above the buttons.
const ALERT_ICON: f64 = 64.0;
const ALERT_ICON_GAP: f64 = 14.0;
const ALERT_TITLE_GAP: f64 = 10.0;
const ALERT_BUTTONS_GAP: f64 = 16.0;
/// Between two capsule buttons, side by side or stacked.
const ALERT_BUTTON_GAP: f64 = 8.0;
/// Average advance of a character of a button's label, in ems: what decides whether two buttons fit side by side.
const LABEL_CHAR_EM: f64 = 0.58;
const LABEL_PT: f64 = 13.0;

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

impl<M: Clone + 'static> From<Modal<M>> for Element<M> {
    fn from(modal: Modal<M>) -> Self {
        let mut children = vec![Element::new(Behind {
            hidden: modal.dialog.is_some(),
            children: vec![modal.content],
        })];
        if let Some(dialog) = modal.dialog {
            // an alert dims nothing, but still takes every press meant for the content
            children.push(Element::new(Scrim {
                dim: dialog.kind == DialogKind::Dialog,
            }));
            children.push(Element::new(Hang {
                top: match dialog.kind {
                    DialogKind::Dialog => HANG,
                    DialogKind::Alert => ALERT_HANG,
                },
                children: vec![match dialog.kind {
                    DialogKind::Dialog => sheet(dialog),
                    DialogKind::Alert => alert(dialog),
                }],
            }));
        }
        Element::new(Frame { children })
    }
}

/// Text at the leading edge, so it mirrors in right-to-left layouts with the sheet's controls.
fn label<M: 'static>(content: &str, role: TextRole, secondary: bool) -> Element<M> {
    styled_label(content, role, TextStyle::new(13.0), secondary)
}

/// [`label`] in `style`, whose weight the role keeps unless it is the regular one.
fn styled_label<M: 'static>(
    content: &str,
    role: TextRole,
    style: TextStyle,
    secondary: bool,
) -> Element<M> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Start,
        children: vec![Element::new(Text {
            content: AttributedText::plain(content, style),
            role,
            wrap: true,
            secondary,
            selected: false,
        })],
    })
}

/// The message of the first button with `action`.
fn answer<M: Clone>(dialog: &Dialog<M>, action: DialogAction) -> Option<M> {
    dialog
        .buttons
        .iter()
        .find(|b| b.action == action)
        .map(|b| b.on_press.clone())
}

/// The button that takes the focus when the dialog opens: the least destructive answer, so a stray Space does no
/// harm.
fn initial_button<M>(dialog: &Dialog<M>) -> Option<usize> {
    [
        DialogAction::Cancel,
        DialogAction::Default,
        DialogAction::Other,
    ]
    .into_iter()
    .find_map(|action| dialog.buttons.iter().position(|b| b.action == action))
}

fn button_role(action: DialogAction) -> ButtonRole {
    match action {
        DialogAction::Default => ButtonRole::Default,
        DialogAction::Destructive => ButtonRole::Destructive,
        DialogAction::Cancel | DialogAction::Other => ButtonRole::Normal,
    }
}

fn sheet<M: Clone + 'static>(dialog: Dialog<M>) -> Element<M> {
    let default = answer(&dialog, DialogAction::Default);
    let cancel = answer(&dialog, DialogAction::Cancel);
    // a body control takes the focus first; otherwise the least destructive answer does
    let initial = initial_button(&dialog);
    let mut children = vec![label(&dialog.title, TextRole::Section, false)];
    if !dialog.message.is_empty() {
        children.push(label(&dialog.message, TextRole::Body, true));
    }
    children.extend(dialog.body.map(|body| {
        Element::new(Body {
            children: vec![body],
        })
    }));
    let buttons = dialog
        .buttons
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            Element::new(SheetButton {
                button: Button {
                    label: b.label,
                    role: button_role(b.action),
                    style: ButtonStyle::Bordered,
                    enabled: true,
                    busy: false,
                    on_press: Some(b.on_press),
                },
                initial: initial == Some(i),
                capsule: false,
            })
        })
        .collect();
    children.push(Element::new(Buttons { children: buttons }));
    Element::new(Sheet {
        kind: dialog.kind,
        title: dialog.title,
        message: dialog.message,
        default,
        cancel,
        children,
    })
}

/// Whether an alert's buttons go side by side: at most two, each label fitting half the row with its insets, its
/// width reckoned from its character count.
pub(crate) fn side_by_side(labels: &[&str]) -> bool {
    let half = (ALERT_WIDTH - ALERT_PADDING.left - ALERT_PADDING.right - ALERT_BUTTON_GAP) / 2.0;
    labels.len() <= 2
        && labels.iter().all(|label| {
            label.chars().count() as f64 * LABEL_CHAR_EM * LABEL_PT + 2.0 * CAPSULE_INSET <= half
        })
}

/// An alert: the application's icon, the title in bold body text, the message, the body if any, then the buttons as capsules.
/// Two short answers go side by side in their given order, the default trailing; otherwise they are stacked full
/// width with the default on top, so the order is the given one reversed.
fn alert<M: Clone + 'static>(dialog: Dialog<M>) -> Element<M> {
    let default = answer(&dialog, DialogAction::Default);
    let cancel = answer(&dialog, DialogAction::Cancel);
    let initial = initial_button(&dialog);
    let labels: Vec<&str> = dialog.buttons.iter().map(|b| b.label.as_str()).collect();
    let paired = side_by_side(&labels);
    let mut text = vec![
        Element::new(AppIcon {
            name: dialog.icon.clone(),
        }),
        Element::new(Gap(ALERT_ICON_GAP)),
        styled_label(
            &dialog.title,
            TextRole::Body,
            TextStyle {
                weight: FontWeight::BOLD,
                ..TextStyle::new(13.0)
            },
            false,
        ),
    ];
    if !dialog.message.is_empty() {
        text.push(Element::new(Gap(ALERT_TITLE_GAP)));
        text.push(label(&dialog.message, TextRole::Body, false));
    }
    let mut children = vec![Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets {
            top: 0.0,
            left: ALERT_TEXT_INSET,
            bottom: 0.0,
            right: ALERT_TEXT_INSET,
        },
        spacing: 0.0,
        align: Align::Stretch,
        children: text,
    })];
    if let Some(body) = dialog.body {
        children.push(Element::new(Gap(SPACING)));
        children.push(Element::new(Body {
            children: vec![body],
        }));
    }
    children.push(Element::new(Gap(ALERT_BUTTONS_GAP)));
    let mut buttons: Vec<Element<M>> = dialog
        .buttons
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            Element::new(SheetButton {
                button: Button {
                    label: b.label,
                    role: button_role(b.action),
                    style: ButtonStyle::Pill,
                    enabled: true,
                    busy: false,
                    on_press: Some(b.on_press),
                },
                initial: initial == Some(i),
                capsule: true,
            })
        })
        .collect();
    if !paired {
        buttons.reverse();
    }
    let count = buttons.len().max(1);
    children.push(Element::new(AlertButtons {
        columns: if paired { count } else { 1 },
        rows: if paired { 1 } else { count },
        children: buttons,
    }));
    Element::new(Sheet {
        kind: DialogKind::Alert,
        title: dialog.title,
        message: dialog.message,
        default,
        cancel,
        children: vec![Element::new(Reveal { children })],
    })
}

/// Hangs the dialog `top` points below the window's top, centred across the window.
struct Hang<M> {
    top: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Hang<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: self.top,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.top != old.top {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Fixed room in a column.
struct Gap(f64);

impl<M: 'static> Widget<M> for Gap {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            height: SizeSpec::Fixed(self.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
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
            size: Size::new(0.0, self.0),
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

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The application's own icon at the alert's leading edge; the room stays when the icons have none.
struct AppIcon {
    name: Option<String>,
}

impl<M: 'static> Widget<M> for AppIcon {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(ALERT_ICON),
            height: SizeSpec::Fixed(ALERT_ICON),
            align_self: Some(Align::Start),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.name != old.name {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(ALERT_ICON, ALERT_ICON),
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
        let px = (ALERT_ICON * cx.env().scale.max(1.0)).round() as u32;
        let icons = &cx.env().icons;
        let icon = match &self.name {
            Some(name) => icons.named(name, px),
            None => icons.application(px),
        };
        if let Some(icon) = icon {
            let dst = Rect::new(0.0, 0.0, ALERT_ICON, ALERT_ICON);
            cx.image(icon.id, &icon.image, dst, Sampling::Linear);
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// An alert's capsule buttons: equal columns side by side, or one column of rows.
struct AlertButtons<M> {
    columns: usize,
    rows: usize,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for AlertButtons<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Grid {
            columns: vec![Track::Fraction(1.0); self.columns],
            rows: vec![Track::Auto; self.rows],
            gaps: Size::new(ALERT_BUTTON_GAP, ALERT_BUTTON_GAP),
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.columns, self.rows) != (old.columns, old.rows) {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The content and, while a dialog is up, the scrim and the sheet over it.
struct Frame<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Frame<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The content, hidden from assistive technology and the Tab order while a dialog is up.
struct Behind<M> {
    hidden: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Behind<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.hidden != old.hidden {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.hidden {
            cx.hidden();
        }
    }
}

/// The dialog's own controls, whose first Tab stop takes the focus when the sheet opens.
struct Body<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Body<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.initial_focus();
    }
}

/// Takes every press and movement meant for the content, and dims it when `dim`.
struct Scrim {
    dim: bool,
}

impl<M: 'static> Widget<M> for Scrim {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.dim != old.dim {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(..) | Event::Touch(..) => Propagation::Stop,
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.dim {
            return;
        }
        let size = cx.size();
        let color = cx
            .env()
            .theme
            .colors
            .get(ColorRole::DialogScrim, StateVariant::NORMAL);
        look::fill(cx, Rect::new(0.0, 0.0, size.width, size.height), 0.0, color);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The panel: its title, message, body and buttons in a column, and the keys that answer it.
struct Sheet<M> {
    kind: DialogKind,
    title: String,
    message: String,
    default: Option<M>,
    cancel: Option<M>,
    children: Vec<Element<M>>,
}

/// When a node first painted, which times an alert's pop-in.
type Born = Cell<Option<Timestamp>>;

/// Seconds since the node first painted (the first paint is 0), and a request for the next frame while the pop-in
/// lasts; `None` once it is over.
fn pop_time(born: &Born, cx: &mut PaintCx<'_>) -> Option<f64> {
    let now = cx.now();
    let start = born.get().unwrap_or_else(|| {
        born.set(Some(now));
        now
    });
    let elapsed = now.0.saturating_sub(start.0) as f64 / 1e9;
    (elapsed < POP_DURATION).then(|| {
        cx.request_animation_frame();
        elapsed
    })
}

/// The alert panel's size at `t` seconds into its pop-in, as a share of its size: it grows past it and eases back.
fn pop_scale(t: f64) -> f64 {
    let peak = POP_DURATION * POP_PEAK;
    if t >= POP_DURATION {
        1.0
    } else if t < peak {
        let u = (t / peak).clamp(0.0, 1.0);
        POP_START
            + (1.0 + POP_OVERSHOOT - POP_START)
                * (u * std::f64::consts::FRAC_PI_2)
                    .sin()
                    .powf(POP_RISE_EXPONENT)
    } else {
        let u = ((t - peak) / (POP_DURATION - peak)).clamp(0.0, 1.0);
        1.0 + POP_OVERSHOOT * (1.0 - u).powf(POP_SETTLE_EXPONENT)
    }
}

/// The shadow of a panel `rect` rounded by `radius` that is glass, drawn only outside it: from an opaque copy of its
/// shape that a clip hides, so the glass under the panel stays clear of it.
fn halo(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, shadow: Effect) {
    let reach = ALERT_SHADOW_OFFSET.abs() + 3.0 * ALERT_SHADOW_BLUR;
    let mut outside = crate::tabs::rounded(rect.inset(Insets::uniform(-reach)), 0.0);
    outside.extend(crate::tabs::rounded(rect, radius));
    cx.push(PaintOp::PushClip(Shape::Path {
        verbs: outside,
        fill_rule: FillRule::EvenOdd,
    }));
    cx.push(PaintOp::PushLayer {
        opacity: 1.0,
        effects: vec![shadow],
    });
    cx.push(PaintOp::Fill {
        shape: Shape::RoundedRect { rect, radius },
        paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)),
    });
    cx.push(PaintOp::PopLayer);
    cx.push(PaintOp::PopClip);
}

/// A fade from 0 at `delay` seconds to 1 at `delay + length`.
fn ramp(t: f64, delay: f64, length: f64) -> f32 {
    ((t - delay) / length).clamp(0.0, 1.0) as f32
}

/// An alert's content: it fades in a little after its panel has begun to grow.
struct Reveal<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Reveal<M> {
    type State = Born;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> Born {
        Cell::new(None)
    }

    fn update(&self, _: &Self, _: &mut Born) -> Change {
        Change::None
    }

    fn measure(&self, _: &Born, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut Born, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::AnimationFrame(_) = event {
            cx.request_paint();
        }
        Propagation::Continue
    }

    fn paint(&self, born: &Born, cx: &mut PaintCx<'_>) {
        if let Some(t) = pop_time(born, cx) {
            let (delay, length) = if cx.env().theme.motion.reduce {
                (0.0, POP_PANEL_FADE)
            } else {
                (POP_CONTENT_DELAY, POP_CONTENT_FADE)
            };
            cx.set_group(ramp(t, delay, length), Vec::new());
        }
    }

    fn semantics(&self, _: &Born, _: &mut SemanticsCx<'_>) {}
}

impl<M: Clone + 'static> Widget<M> for Sheet<M> {
    type State = Born;

    fn layout(&self) -> LayoutSpec {
        let (width, insets, spacing) = match self.kind {
            DialogKind::Dialog => (WIDTH, Insets::uniform(PADDING), SPACING),
            DialogKind::Alert => (ALERT_WIDTH, ALERT_PADDING, 0.0),
        };
        LayoutSpec {
            insets,
            width: SizeSpec::Fixed(width),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> Born {
        Cell::new(None)
    }

    /// An alert floats in the raised layer, so its glass lies over the window's content, which shows blurred through
    /// it.
    fn raised(&self, _: &Born) -> bool {
        self.kind == DialogKind::Alert
    }

    /// It covers nothing for the rest of the window: the content keeps its edges and bars under it.
    fn obscures(&self, _: &Born) -> bool {
        false
    }

    fn update(&self, old: &Self, _: &mut Born) -> Change {
        if self.kind != old.kind {
            Change::Layout
        } else if self.title != old.title || self.message != old.message {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Born, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    /// Return and Escape answer the sheet once the focused control has passed them on; every other key the sheet's
    /// controls leave stops here, so the window's shortcuts stay out while it is up. Tab still moves the focus.
    fn event(&self, _: &mut Born, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::AnimationFrame(_) = event {
            cx.request_paint();
            return Propagation::Continue;
        }
        let Event::Key(key, _) = event else {
            return Propagation::Continue;
        };
        if !key.pressed || cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let answer = match key.keysym {
            TAB | LEFT_TAB => return Propagation::Continue,
            RETURN | KP_ENTER if key.mods.is_empty() => &self.default,
            ESCAPE if key.mods.is_empty() => &self.cancel,
            _ => &None,
        };
        if let Some(message) = answer {
            cx.emit(message.clone());
        }
        Propagation::Stop
    }

    /// A sheet is solid with a shadow and, in the dark appearance, a light rim just inside its edge; an alert is dense
    /// glass (a nearly opaque grey while it pops in and without the glass) with a rim; both have a dark hairline just
    /// outside their edge.
    fn paint(&self, born: &Born, cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let mut rect = Rect::new(0.0, 0.0, size.width, size.height);
        let theme = cx.env().theme.clone();
        // an alert pops in: its panel grows about its centre and fades in, as a group with its shadow
        let mut grow = 1.0;
        // settled in the raised layer, an alert's body is dense glass over the window's content; while it pops in,
        // with Reduce Transparency or Increase Contrast and where there is no such glass, a grey that keeps the content
        // from showing through its text
        let mut glass = false;
        if self.kind == DialogKind::Alert {
            let shadow = Effect::DropShadow {
                offset: Point::new(0.0, ALERT_SHADOW_OFFSET),
                blur: ALERT_SHADOW_BLUR,
                color: Color::new(0.0, 0.0, 0.0, ALERT_SHADOW_ALPHA),
            };
            let opaque = theme.preferences.reduce_transparency
                || theme.preferences.appearance.is_high_contrast();
            match pop_time(born, cx) {
                Some(t) => {
                    if !theme.motion.reduce {
                        grow = pop_scale(t);
                    }
                    cx.set_group(ramp(t, 0.0, POP_PANEL_FADE), vec![shadow]);
                }
                None => {
                    glass = !opaque
                        && cx.in_raised_layer()
                        && cx.glass(Glass::dense().radius(ALERT_RADIUS));
                    if glass {
                        // the glass casts no shadow of its own: one from an opaque copy of the panel, outside it
                        halo(cx, rect, ALERT_RADIUS, shadow);
                    } else {
                        cx.set_group(1.0, vec![shadow]);
                    }
                }
            }
            let (w, h) = (size.width * grow, size.height * grow);
            rect = Rect::new((size.width - w) / 2.0, (size.height - h) / 2.0, w, h);
        }
        let dark = theme.preferences.appearance.is_dark();
        let px = 1.0 / cx.env().scale.max(1.0);
        let outline = Color::new(
            0.0,
            0.0,
            0.0,
            if dark { OUTLINE_DARK } else { OUTLINE_LIGHT },
        );
        let radius = match self.kind {
            DialogKind::Dialog => {
                cx.set_group(
                    1.0,
                    vec![Effect::DropShadow {
                        offset: Point::new(0.0, SHADOW_OFFSET),
                        blur: SHADOW_BLUR,
                        color: Color::new(0.0, 0.0, 0.0, SHADOW_ALPHA),
                    }],
                );
                let fill = theme
                    .colors
                    .get(ColorRole::WindowBackground, StateVariant::NORMAL);
                look::fill(cx, rect, RADIUS, fill);
                if dark {
                    look::stroke(
                        cx,
                        rect.inset(Insets::uniform(EDGE / 2.0)),
                        RADIUS - EDGE / 2.0,
                        EDGE,
                        Some(Color::new(1.0, 1.0, 1.0, RIM_DARK)),
                    );
                }
                RADIUS
            }
            DialogKind::Alert => {
                let opaque = theme.preferences.reduce_transparency
                    || theme.preferences.appearance.is_high_contrast();
                let (grey, rim) = if dark {
                    (ALERT_BODY_DARK, Color::new(1.0, 1.0, 1.0, ALERT_RIM_DARK))
                } else {
                    (ALERT_BODY_LIGHT, Color::new(0.0, 0.0, 0.0, ALERT_RIM_LIGHT))
                };
                let round = ALERT_RADIUS * grow;
                if !glass {
                    let alpha = if opaque { 1.0 } else { ALERT_BODY_ALPHA };
                    look::fill(cx, rect, round, Some(Color::new(grey, grey, grey, alpha)));
                }
                if !dark {
                    let [r, g, b] = ALERT_TINT;
                    let at = |alpha: f32| Color::new(r, g, b, alpha);
                    let height = ALERT_TINT_HEIGHT.min(rect.height());
                    cx.push(PaintOp::PushClip(Shape::RoundedRect {
                        rect,
                        radius: round,
                    }));
                    cx.push(PaintOp::Fill {
                        shape: Shape::Rect(Rect::new(
                            rect.min_x(),
                            rect.min_y(),
                            rect.width(),
                            height,
                        )),
                        paint: Paint::Linear {
                            start: Point::new(0.0, rect.min_y()),
                            end: Point::new(0.0, rect.min_y() + height),
                            stops: vec![
                                GradientStop {
                                    offset: 0.0,
                                    color: at(ALERT_TINT_TOP),
                                },
                                GradientStop {
                                    offset: ALERT_TINT_PEAK_AT,
                                    color: at(ALERT_TINT_PEAK),
                                },
                                GradientStop {
                                    offset: 1.0,
                                    color: at(0.0),
                                },
                            ],
                        },
                    });
                    cx.push(PaintOp::PopClip);
                }
                look::stroke(
                    cx,
                    rect.inset(Insets::uniform(px / 2.0)),
                    nested_radius(round, px / 2.0),
                    px,
                    Some(rim),
                );
                round
            }
        };
        look::stroke(
            cx,
            rect.outset(px / 2.0),
            radius + px / 2.0,
            px,
            Some(outline),
        );
    }

    fn semantics(&self, _: &Born, cx: &mut SemanticsCx<'_>) {
        cx.role(match self.kind {
            DialogKind::Dialog => Role::Dialog,
            DialogKind::Alert => Role::AlertDialog,
        });
        cx.name(self.title.clone());
        if !self.message.is_empty() {
            cx.description(self.message.clone());
        }
        cx.states(AccessibleStates {
            modal: true,
            ..AccessibleStates::default()
        });
        cx.focus_scope();
    }
}

/// The button row, at the trailing edge.
struct Buttons<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Buttons<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Horizontal,
            spacing: BUTTON_SPACING,
            align: Align::Center,
            justify: Justify::End,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_short_answers_go_side_by_side_and_anything_else_is_stacked() {
        assert!(side_by_side(&["Cancel", "Delete"]));
        assert!(side_by_side(&["OK"]));
        assert!(!side_by_side(&["Don't Save", "Cancel", "Save"]), "three");
        assert!(
            !side_by_side(&["Cancel", "Replace Everything Here"]),
            "a label too long for half the row"
        );
    }
}

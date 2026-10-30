//! The standard CupertinoKit controls. Each is a [`cupertino_ui_core::Widget`] that reads its look from the theme
//! tokens, takes values from the application's model and reports changes as messages.

mod button;
mod checkbox;
mod choice;
mod field;
mod icon;
mod look;
mod press;
mod progress;
mod slider;
mod stack;
mod text;
mod toggle;

pub use field::{FieldState, SecureState};
pub use press::Press;
pub use slider::SliderState;

use cupertino_geometry::Insets;
use cupertino_ui_core::{Checked, Element};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_text::{AttributedText, ContentPurpose, SecureBuffer};
use cupertino_ui_theme::ColorRole;

/// The space a form-style window (a form, a dialog) leaves between its edges and its content, for a [`Stack`]'s
/// padding. Windows add none themselves: sidebars, lists and canvases run to the edges.
pub const WINDOW_CONTENT_INSET: Insets = Insets {
    top: 20.0,
    left: 20.0,
    bottom: 20.0,
    right: 20.0,
};

/// Turns a control's new value into the application's message.
pub type Handler<T, M> = Box<dyn Fn(T) -> M>;

/// Turns a submitted password into the application's message; the secret is only borrowed.
pub type SecureHandler<M> = Box<dyn Fn(&SecureBuffer) -> M>;

/// The type role a text is set in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextRole {
    #[default]
    Body,
    Caption,
    Section,
    Title,
}

/// Static text; not a tab stop.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub content: AttributedText,
    pub role: TextRole,
    pub wrap: bool,
}

/// Whether an icon's artwork mirrors in right-to-left layouts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IconDirection {
    #[default]
    Fixed,
    /// Points along the reading direction (back, forward, disclosure), so it mirrors.
    Semantic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Icon {
    /// A system symbol name.
    pub name: String,
    pub size: f64,
    pub direction: IconDirection,
    /// `None` marks the icon decorative, so accessibility skips it.
    pub label: Option<String>,
    /// The colour role it is drawn in; `None` is the label colour ([`ColorRole::Text`]).
    pub tint: Option<ColorRole>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ButtonRole {
    #[default]
    Normal,
    /// Activated by Return in its window.
    Default,
    Destructive,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ButtonStyle {
    #[default]
    Bordered,
    Borderless,
    Pill,
    RoundIcon,
}

pub struct Button<M> {
    pub label: String,
    pub role: ButtonRole,
    pub style: ButtonStyle,
    pub enabled: bool,
    /// Shows progress and ignores activation until cleared.
    pub busy: bool,
    pub on_press: Option<M>,
}

/// A check box with its label, which is part of its target and names it.
pub struct CheckBox<M> {
    pub label: String,
    pub checked: Checked,
    pub enabled: bool,
    /// Asked with the new state: on from off or mixed, off from on.
    pub on_toggle: Option<Handler<bool, M>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SliderStyle {
    /// A thin track with a round knob.
    #[default]
    Regular,
    /// A wide track filled up to the knob, as a volume or brightness level.
    Level,
}

/// A value in `min..=max`, set by dragging or the arrow keys; it takes the whole width it is offered.
pub struct Slider<M> {
    /// The accessible name.
    pub label: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// The values snap to `min + n × step`; without it the arrow keys move a hundredth of the range.
    pub step: Option<f64>,
    pub style: SliderStyle,
    /// Tick marks along a regular track, evenly spaced from end to end; fewer than two draws none.
    pub ticks: u32,
    pub enabled: bool,
    pub on_change: Option<Handler<f64, M>>,
}

/// How far a task has come, or that it is going on (`value: None`); not a Tab stop.
/// Going on, it asks for a frame on every refresh while shown, so hide it when the task is idle.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// The accessible name.
    pub label: String,
    /// From 0 to 1; `None` sweeps without an end.
    pub value: Option<f64>,
}

/// One choice of several, each a radio button with its label; one Tab stop, the arrows choose.
pub struct RadioGroup<M> {
    /// The group's accessible name.
    pub label: String,
    pub options: Vec<String>,
    pub selected: Option<usize>,
    pub axis: Axis,
    pub spacing: f64,
    pub enabled: bool,
    pub on_select: Option<Handler<usize, M>>,
}

/// One choice of several as equal segments of one control; one Tab stop, the arrows choose.
pub struct SegmentedControl<M> {
    /// The control's accessible name.
    pub label: String,
    pub segments: Vec<String>,
    pub selected: Option<usize>,
    pub enabled: bool,
    pub on_select: Option<Handler<usize, M>>,
}

pub struct Toggle<M> {
    pub label: String,
    pub on: bool,
    pub enabled: bool,
    pub on_toggle: Option<Handler<bool, M>>,
}

/// A field's text after an edit, with the revision the field gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub text: String,
    pub revision: u64,
}

pub struct TextField<M> {
    /// The accessible name, separate from the placeholder.
    pub label: String,
    pub placeholder: String,
    pub value: String,
    /// The revision of `value` the model last accepted; a different one resets the composition.
    pub revision: u64,
    pub purpose: ContentPurpose,
    pub enabled: bool,
    pub on_change: Option<Handler<TextChange, M>>,
    pub on_submit: Option<M>,
}

/// A password field. Its text stays in the field's own buffer and reaches the application only on submit.
pub struct SecureField<M> {
    pub label: String,
    pub placeholder: String,
    pub enabled: bool,
    pub on_submit: Option<SecureHandler<M>>,
}

pub struct Stack<M> {
    pub axis: Axis,
    /// Space between the stack's edges and its children.
    pub padding: Insets,
    pub spacing: f64,
    pub align: Align,
    pub children: Vec<Element<M>>,
}

/// Flexible empty space in a stack.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spacer {
    pub min: f64,
}

/// A line across a stack, drawn with the separator colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Separator {
    pub axis: Axis,
}

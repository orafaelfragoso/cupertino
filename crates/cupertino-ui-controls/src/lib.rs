//! The standard CupertinoKit controls. Each is a [`cupertino_ui_core::Widget`] that reads its look from the theme
//! tokens, takes values from the application's model and reports changes as messages.

mod field;
mod look;
mod press;
mod stack;
mod toggle;

pub use field::{FieldState, SecureState};
pub use press::Press;

use cupertino_ui_core::Element;
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_text::{AttributedText, ContentPurpose, SecureBuffer};

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

use crate::button::SheetButton;
use crate::look;
use crate::{
    Button, ButtonRole, ButtonStyle, Dialog, DialogAction, DialogKind, Modal, Text, TextRole,
};
use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Change, Element, Event, EventCx, MeasureCx, PaintCx, Phase, Propagation,
    Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_platform::Keysym;
use cupertino_ui_text::{AttributedText, TextStyle};
use cupertino_ui_theme::{ColorRole, StateVariant};

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const ESCAPE: Keysym = Keysym(0xff1b);
const TAB: Keysym = Keysym(0xff09);
const LEFT_TAB: Keysym = Keysym(0xfe20);
const WIDTH: f64 = 340.0;
const RADIUS: f64 = 10.0;
const PADDING: f64 = 20.0;
/// Between the title, the message, the body and the buttons.
const SPACING: f64 = 12.0;
const BUTTON_SPACING: f64 = 8.0;
const EDGE: f64 = 1.0;

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
            children.push(Element::new(Scrim));
            children.push(sheet(dialog));
        }
        Element::new(Frame { children })
    }
}

fn label<M: 'static>(content: &str, role: TextRole, secondary: bool) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role,
        wrap: true,
        secondary,
        selected: false,
    })
}

fn sheet<M: Clone + 'static>(dialog: Dialog<M>) -> Element<M> {
    let message = |action| {
        dialog
            .buttons
            .iter()
            .find(|b| b.action == action)
            .map(|b| b.on_press.clone())
    };
    let default = message(DialogAction::Default);
    let cancel = message(DialogAction::Cancel);
    let mut children = vec![label(&dialog.title, TextRole::Section, false)];
    if !dialog.message.is_empty() {
        children.push(label(&dialog.message, TextRole::Body, true));
    }
    children.extend(dialog.body);
    // the least destructive answer takes the focus, so a stray Space does no harm
    let initial = [
        DialogAction::Cancel,
        DialogAction::Default,
        DialogAction::Other,
    ]
    .into_iter()
    .find_map(|action| dialog.buttons.iter().position(|b| b.action == action));
    let buttons = dialog
        .buttons
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let role = match b.action {
                DialogAction::Default => ButtonRole::Default,
                DialogAction::Destructive => ButtonRole::Destructive,
                DialogAction::Cancel | DialogAction::Other => ButtonRole::Normal,
            };
            Element::new(SheetButton {
                button: Button {
                    label: b.label,
                    role,
                    style: ButtonStyle::Bordered,
                    enabled: true,
                    busy: false,
                    on_press: Some(b.on_press),
                },
                initial: initial == Some(i),
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

/// Dims the content and takes every press and movement meant for it.
struct Scrim;

impl<M: 'static> Widget<M> for Scrim {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
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

impl<M: Clone + 'static> Widget<M> for Sheet<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::uniform(PADDING),
            width: SizeSpec::Fixed(WIDTH),
            align_self: Some(Align::Center),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: SPACING,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.kind != old.kind || self.title != old.title || self.message != old.message {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    /// Return and Escape answer the sheet once the focused control has passed them on; every other key the sheet's
    /// controls leave stops here, so the window's shortcuts stay out while it is up. Tab still moves the focus.
    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let theme = &cx.env().theme;
        let fill = theme
            .colors
            .get(ColorRole::WindowBackground, StateVariant::NORMAL);
        let edge = theme.colors.get(ColorRole::Separator, StateVariant::NORMAL);
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        look::fill(cx, rect, RADIUS, fill);
        look::stroke(
            cx,
            rect.inset(Insets::uniform(EDGE / 2.0)),
            RADIUS - EDGE / 2.0,
            EDGE,
            edge,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
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

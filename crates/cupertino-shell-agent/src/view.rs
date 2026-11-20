//! The session agent's panels as CupertinoKit draws them: the confirmation panels (log out, restart, shut down and the
//! power button's) and the panel that interrupts an end an application held up. Each function builds a window from the
//! panels' state; what the person does comes back as an [`Input`], which the component maps into its own messages.
//! Each window is an alert on glass that the component places on an overlay layer surface: centred when it appears,
//! moved when its background is dragged ([`Input::MovePanel`]), holding the keyboard until the person turns to a
//! window ([`Input::LetGo`]).

mod panel_parts;
mod widgets;

use cupertino_kit::controls::{Button, ButtonRole, ButtonStyle, Canvas, CheckBox, Text, TextRole};
use cupertino_kit::core::{Checked, CommandRegistry, Element, Window, WindowBackground, WindowKey};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::platform::{DecorationMode, Keysym, WindowSpec};
use cupertino_kit::text::{AttributedText, FontWeight, TextStyle, TextSystem};

use crate::interrupt::{self, Choice, Reason, Stage};
use crate::panel::{self, Action, Decision, Frames, Icon, Key, Kind, Panel};
use widgets::{AlertBody, PanelKeys, Place, ESCAPE};

/// The `app_id` of the confirmation panel's and of the interrupt panel's window.
pub const PANEL_APP_ID: &str = "cupertino-shell-agent.panel";
pub const INTERRUPT_APP_ID: &str = "cupertino-shell-agent.interrupt";

/// Text size the panels' title and lines are measured at.
const PANEL_TITLE_SIZE: f32 = 13.0;
/// Corner radius of the alert of the confirmation and interrupt panels.
const PANEL_ALERT_RADIUS: f64 = 26.0;

/// What the person did on a panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// A button or key of the confirmation panel.
    Decide(Decision),
    /// The confirmation panel's "Open my windows again" checkbox was set to this, by a click, Space or assistive
    /// technology.
    Reopen(bool),
    /// A button or key of the interrupt panel.
    Interrupt(Choice),
    /// The background of a panel was dragged: move the panel by this many points.
    MovePanel(Point),
    /// The panel's window lost the keyboard to another surface: from then on it takes the keyboard only when clicked.
    LetGo,
}

/// A panel's window of `size`, with `content`, which paints its own alert: the window itself shows nothing.
fn window<M: Clone>(
    key: WindowKey,
    app_id: &str,
    title: &str,
    size: Size,
    content: Element<M>,
    on_close: M,
) -> Window<M> {
    Window {
        key,
        spec: WindowSpec {
            title: title.to_owned(),
            app_id: app_id.to_owned(),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(on_close),
        background: WindowBackground::None,
        content,
        popup: None,
        menus: None,
    }
}

/// The style the panels' title, lines and checkbox label are measured in; a countdown also sets its digits at one width
/// each (`tnum`), so the line keeps its length and does not shift as the number changes.
fn panel_style(weight: FontWeight, tabular: bool) -> TextStyle {
    TextStyle {
        weight,
        features: if tabular {
            vec![cupertino_kit::text::FontFeature {
                tag: *b"tnum",
                value: 1,
            }]
        } else {
            Vec::new()
        },
        ..TextStyle::new(PANEL_TITLE_SIZE)
    }
}

/// How many lines `text` takes in `style`, wrapped at `width`.
fn lines_in(text_system: &TextSystem, text: &str, style: TextStyle, width: f64) -> u32 {
    let content = AttributedText::plain(text, style);
    let paragraph = cupertino_kit::text::ParagraphStyle {
        wrap: cupertino_kit::text::Wrap::Word,
        ..Default::default()
    };
    text_system.paragraph(&content, &paragraph).map_or(1, |p| {
        p.layout(Some(width as f32)).lines().len().max(1) as u32
    })
}

/// How wide `text` is on one line in `style`.
fn line_width(text_system: &TextSystem, text: &str, style: TextStyle) -> f64 {
    let content = AttributedText::plain(text, style);
    text_system
        .paragraph(&content, &Default::default())
        .map_or(0.0, |p| p.layout(None).size().width)
}

/// The confirmation panel's frames for `kind`, its checkbox label, title and info line measured with `text_system`:
/// the panel as wide as its checkbox row needs, and the info line as many lines as its longest countdown message.
pub fn panel_frames(kind: Kind, text_system: &TextSystem) -> Frames {
    let label = line_width(
        text_system,
        panel::REOPEN_LABEL,
        panel_style(FontWeight::REGULAR, false),
    );
    let width = panel::width_for(label);
    let wrap = panel::wrap_width(width);
    let title = lines_in(
        text_system,
        kind.text().title,
        panel_style(FontWeight::BOLD, false),
        wrap,
    );
    let info = (0..=panel::COUNTDOWN_SECONDS as u32)
        .filter_map(|seconds| kind.countdown_message(seconds))
        .map(|message| {
            lines_in(
                text_system,
                &message,
                panel_style(FontWeight::REGULAR, true),
                wrap,
            )
        })
        .max()
        .unwrap_or(1);
    kind.frames(width, title, info)
}

/// What the panels' keys mean: the model's keys, by keysym.
fn panel_key(keysym: Keysym, text: Option<char>) -> Option<Key> {
    match keysym {
        Keysym(0xff0d) | Keysym(0xff8d) => Some(Key::Return),
        ESCAPE => Some(Key::Escape),
        _ => text.filter(|c| !c.is_control()).map(Key::Char),
    }
}

fn panel_text<M: 'static>(
    text: &str,
    role: TextRole,
    weight: FontWeight,
    secondary: bool,
) -> Element<M> {
    styled_panel_text(text, role, panel_style(weight, false), secondary)
}

fn styled_panel_text<M: 'static>(
    text: &str,
    role: TextRole,
    style: TextStyle,
    secondary: bool,
) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(text, style),
        role,
        wrap: true,
        secondary,
        selected: false,
    })
}

/// The picture `icon` of a session panel at `rect`: our own glyph on a grey disc.
fn panel_icon<M: 'static>(rect: Rect, icon: Icon) -> Element<M> {
    Element::new(Place::new(
        rect,
        vec![Element::new(Canvas {
            size: Some(rect.size),
            label: String::new(),
            paint: Box::new(move |cx| {
                for op in panel_parts::icon_ops(icon, rect.width()) {
                    cx.push(op);
                }
            }),
            children: Vec::new(),
        })],
    ))
}

/// The confirmation panel, an alert on glass: the picture of its action, its title, the countdown line (its digits
/// of one width), the "Open my windows again" checkbox at `reopen`, which has the focus, and its buttons as capsules. Return
/// does the default action, Escape cancels, Space flips the checkbox, and the power-button panel's letters choose. A
/// press on its background that no control takes drags it ([`Input::MovePanel`]); it is no modal dialog, so the rest
/// of the session stays usable while it counts down.
pub fn panel<M: Clone + 'static>(
    key: WindowKey,
    kind: Kind,
    seconds_left: Option<u32>,
    reopen: bool,
    frames: &Frames,
    map: fn(Input) -> M,
) -> Window<M> {
    let text = kind.text();
    let size = Size::new(frames.width, frames.height);
    let mut children = vec![
        Element::new(AlertBody {
            width: size.width,
            height: Some(size.height),
            radius: PANEL_ALERT_RADIUS,
            padding: Insets::ZERO,
            children: Vec::new(),
        }),
        panel_icon(frames.icon, kind.icon()),
        Element::new(Place::text(
            frames.title,
            vec![panel_text(
                text.title,
                TextRole::Body,
                FontWeight::BOLD,
                false,
            )],
        )),
    ];
    if let (Some(info), Some(frame)) = (
        seconds_left.and_then(|s| kind.countdown_message(s)),
        frames.info,
    ) {
        children.push(Element::new(Place::text(
            frame,
            vec![styled_panel_text(
                &info,
                TextRole::Body,
                panel_style(FontWeight::REGULAR, true),
                false,
            )],
        )));
    }
    let checkbox = frames.checkbox;
    children.push(
        // against the text column's leading edge; it has the focus when the panel appears
        Element::new(Place {
            initial: true,
            ..Place::text(
                checkbox.frame,
                vec![Element::new(CheckBox {
                    label: checkbox.label.to_owned(),
                    checked: if reopen {
                        Checked::True
                    } else {
                        Checked::False
                    },
                    enabled: true,
                    on_toggle: Some(Box::new(move |on| map(Input::Reopen(on)))),
                })],
            )
        })
        .key("reopen"),
    );
    let default = Decision::Do(kind.default_action());
    for button in &frames.buttons {
        children.push(
            Element::new(Place::filled(
                button.frame,
                vec![Element::new(Button {
                    label: button.label.to_owned(),
                    role: if button.decision == default {
                        ButtonRole::Default
                    } else {
                        ButtonRole::Normal
                    },
                    style: ButtonStyle::Pill,
                    enabled: true,
                    busy: false,
                    on_press: Some(map(Input::Decide(button.decision))),
                })],
            ))
            .key(button.label),
        );
    }
    let content = Element::new(PanelKeys {
        keys: Box::new(move |keysym, text| {
            let key = panel_key(keysym, text)?;
            let panel = Panel::new(kind, 0.0, reopen);
            if panel.toggles(key) {
                return Some(map(Input::Reopen(!reopen)));
            }
            panel
                .press(key)
                .map(|decision| map(Input::Decide(decision)))
        }),
        label: text.title.to_owned(),
        on_let_go: map(Input::LetGo),
        children: vec![Element::new(panel_parts::Movable {
            on_move: Box::new(move |by| map(Input::MovePanel(by))),
            children,
        })],
    });
    window(
        key,
        PANEL_APP_ID,
        text.title,
        size,
        content,
        map(Input::Decide(Decision::Cancel)),
    )
}

/// What the interrupt panel shows: the application, why it is still there, the question asked and what it stops, and
/// the text system its words are measured with.
#[derive(Clone, Copy)]
pub struct InterruptView<'a> {
    pub app: &'a str,
    pub reason: Reason,
    pub stage: Stage,
    pub action: Action,
    pub text: &'a TextSystem,
}

/// The interrupt panel's frames, its title and info line wrapped in its text column as `text` lays them out.
pub fn interrupt_frames(shown: &InterruptView<'_>) -> interrupt::Frames {
    let InterruptView {
        app,
        reason,
        stage,
        action,
        text,
    } = *shown;
    let lines = |words: &str, weight| {
        lines_in(
            text,
            words,
            panel_style(weight, false),
            interrupt::WRAP_WIDTH,
        )
    };
    interrupt::Frames::new(
        reason,
        stage,
        lines(
            &interrupt::title(reason, stage, action, app),
            FontWeight::BOLD,
        ),
        lines(
            &interrupt::info(reason, stage, action, app),
            FontWeight::REGULAR,
        ),
    )
}

/// The interrupt panel, on the glass of the confirmation it interrupts: the picture of what it stops, why, and its
/// buttons as capsules: Try Again (the default), Cancel and, for an application that failed or hangs, Force Quit, which
/// asks again before it kills. On that second question neither button is the default and Cancel has the focus, so a
/// Return held from typing kills nothing. It moves when its background is dragged and is no modal dialog.
pub fn interrupt<M: Clone + 'static>(
    key: WindowKey,
    shown: InterruptView<'_>,
    map: fn(Input) -> M,
) -> Window<M> {
    let frames = interrupt_frames(&shown);
    let InterruptView {
        app,
        reason,
        stage,
        action,
        ..
    } = shown;
    let title = interrupt::title(reason, stage, action, app);
    let icon = match action {
        Action::Restart => Icon::Restart,
        Action::ShutDown => Icon::Power,
        Action::LogOut | Action::Sleep => Icon::LogOut,
    };
    let mut children = vec![
        Element::new(AlertBody {
            width: interrupt::WIDTH,
            height: Some(frames.height),
            radius: PANEL_ALERT_RADIUS,
            padding: Insets::ZERO,
            children: Vec::new(),
        }),
        panel_icon(frames.icon, icon),
        Element::new(Place::text(
            frames.title,
            vec![panel_text(&title, TextRole::Body, FontWeight::BOLD, false)],
        )),
        Element::new(Place::text(
            frames.info,
            vec![panel_text(
                &interrupt::info(reason, stage, action, app),
                TextRole::Caption,
                FontWeight::REGULAR,
                true,
            )],
        )),
    ];
    // the first question focuses its default button; the force quit question focuses Cancel
    let focused = frames
        .buttons
        .iter()
        .find(|b| b.default)
        .or_else(|| frames.buttons.iter().find(|b| b.choice == Choice::Back))
        .map(|b| b.choice);
    let mut question = Vec::new();
    for button in &frames.buttons {
        question.push(
            Element::new(Place {
                initial: Some(button.choice) == focused,
                ..Place::filled(
                    button.frame,
                    vec![Element::new(Button {
                        label: button.label.to_owned(),
                        role: if button.default {
                            ButtonRole::Default
                        } else {
                            ButtonRole::Normal
                        },
                        style: ButtonStyle::Pill,
                        enabled: true,
                        busy: false,
                        on_press: Some(map(Input::Interrupt(button.choice))),
                    })],
                )
            })
            .key(format!("{stage:?}-{}", button.label)),
        );
    }
    // each question its own focus scope, so the second one starts at its Cancel and the first one's focus comes back
    // after it
    children
        .push(Element::new(panel_parts::Question { children: question }).key(format!("{stage:?}")));
    let content = Element::new(PanelKeys {
        keys: Box::new(move |keysym, text| {
            panel_key(keysym, text)
                .and_then(|key| interrupt::press(stage, key))
                .map(|choice| map(Input::Interrupt(choice)))
        }),
        label: title.clone(),
        on_let_go: map(Input::LetGo),
        children: vec![Element::new(panel_parts::Movable {
            on_move: Box::new(move |by| map(Input::MovePanel(by))),
            children,
        })],
    });
    window(
        key,
        INTERRUPT_APP_ID,
        &title,
        Size::new(interrupt::WIDTH, frames.height),
        content,
        map(Input::Interrupt(match stage {
            Stage::Ask => Choice::Cancel,
            Stage::ConfirmForceQuit => Choice::Back,
        })),
    )
}

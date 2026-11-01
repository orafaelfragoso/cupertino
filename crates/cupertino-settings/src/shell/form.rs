//! A pane's sections as Kit controls: each row its label, its second line and its control; a control's change is
//! the [`Value`] its row commits, as the panes' model reads it.

use cupertino_kit::controls::{PopUpButton, SegmentedControl, Slider, SliderStyle, TextRole};
use cupertino_kit::core::Window;
use cupertino_kit::geometry::Insets;
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, TextStyle};

use super::Message;
use crate::controls::{metrics, Control, Value};
use crate::panes::{ControlId, Row, Section};
use crate::search::Hit;

/// Between a form's rows and between controls in a row, an approximate design value.
pub const ROW_GAP: f64 = 8.0;
/// Between a form's sections, an approximate design value.
pub const SECTION_GAP: f64 = 20.0;

/// A line of `content`, none when it is empty: an empty text is a node a screen reader cannot announce.
pub fn text(content: &str, role: TextRole) -> Option<Element<Message>> {
    styled(content, role, false)
}

fn styled(content: &str, role: TextRole, secondary: bool) -> Option<Element<Message>> {
    (!content.is_empty()).then(|| line(content, role, secondary))
}

fn line(content: &str, role: TextRole, secondary: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role,
        wrap: true,
        secondary,
        selected: false,
    })
}

fn column(spacing: f64, children: Vec<Element<Message>>) -> Element<Message> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing,
        align: Align::Stretch,
        children,
    })
}

fn button(label: &str, enabled: bool, on_press: Message) -> Element<Message> {
    Element::new(Button {
        label: label.into(),
        role: ButtonRole::Normal,
        style: ButtonStyle::Bordered,
        enabled,
        busy: false,
        on_press: Some(on_press),
    })
}

fn control_key(id: ControlId) -> Key {
    Key::from(format!("{id:?}-control"))
}

/// The control of `row`; a disabled row's control takes no input.
fn control(row: &Row, open: Option<ControlId>) -> Option<Element<Message>> {
    let id = row.id;
    let enabled = row.disabled.is_none();
    let commit = move |value: Value| Message::Commit(id, value);
    let control = match &row.control {
        Control::Switch { on } => Element::new(Toggle {
            label: row.label.clone(),
            on: *on,
            enabled,
            on_toggle: Some(Box::new(move |on| commit(Value::Bool(on)))),
        }),
        Control::Segmented { segments, selected } => Element::new(SegmentedControl {
            label: row.label.clone(),
            segments: segments.iter().map(|s| s.to_string()).collect(),
            selected: Some(*selected),
            enabled,
            on_select: Some(Box::new(move |i| commit(Value::Index(i)))),
        }),
        Control::Popup { choices, selected } => Element::new(PopUpButton {
            label: row.label.clone(),
            options: choices.clone(),
            selected: *selected,
            open: open == Some(id),
            enabled,
            on_open: Some(Message::Open(Some(id))),
        }),
        Control::Slider { value, stops, .. } => {
            let stops = stops.filter(|n| *n > 1);
            Element::new(Slider {
                label: row.label.clone(),
                value: *value,
                min: 0.0,
                max: 1.0,
                step: stops.map(|n| 1.0 / (n - 1) as f64),
                style: SliderStyle::Regular,
                ticks: stops.map_or(0, |n| n as u32),
                enabled,
                on_change: Some(Box::new(move |v| match stops {
                    Some(n) => commit(Value::Index(Control::nearest_stop(v, n))),
                    None => Message::Slide(id, v),
                })),
            })
            .width(metrics::SLIDER_WIDTH)
        }
        Control::Level { value } => Element::new(Slider {
            label: row.label.clone(),
            value: *value,
            min: 0.0,
            max: 1.0,
            step: None,
            style: SliderStyle::Regular,
            ticks: 0,
            enabled,
            on_change: Some(Box::new(move |v| Message::Level(id, v))),
        })
        .width(metrics::LEVEL_WIDTH),
        Control::Button { title } => button(title, enabled, commit(Value::Index(0))),
        Control::Buttons { titles } => Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: ROW_GAP,
            align: Align::Center,
            children: titles
                .iter()
                .enumerate()
                .map(|(i, title)| button(title, enabled, commit(Value::Index(i))))
                .collect(),
        }),
        Control::Text { value } => styled(value, TextRole::Body, true)?,
        Control::Preview { title, note, .. } => {
            let lines = text(title, TextRole::Body)
                .into_iter()
                .chain(
                    note.as_deref()
                        .and_then(|n| styled(n, TextRole::Body, true)),
                )
                .collect();
            column(2.0, lines)
        }
        Control::Tiles { tiles, selected } => column(
            ROW_GAP,
            tiles
                .iter()
                .enumerate()
                .map(|(i, tile)| {
                    let label = if *selected == Some(i) {
                        format!("{} ✓", tile.title)
                    } else {
                        tile.title.clone()
                    };
                    button(&label, enabled, commit(Value::Index(i)))
                })
                .collect(),
        ),
    };
    Some(control.key(control_key(id)))
}

fn row(row: &Row, open: Option<ControlId>) -> Element<Message> {
    let second = row
        .error
        .as_ref()
        .or(row.disabled.as_ref())
        .or(row.detail.as_ref());
    let label = text(&row.label, TextRole::Body)
        .into_iter()
        .chain(second.and_then(|s| styled(s, TextRole::Body, true)))
        .collect();
    let mut children = vec![column(2.0, label), Element::new(Spacer::default())];
    children.extend(control(row, open));
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: ROW_GAP,
        align: Align::Center,
        children,
    })
    .key(format!("{:?}", row.id))
}

/// The pane's sections, top to bottom.
pub fn pane(sections: &[Section], open: Option<ControlId>) -> Element<Message> {
    column(
        SECTION_GAP,
        sections
            .iter()
            .enumerate()
            .map(|(i, section)| {
                let mut children = Vec::new();
                children.extend(
                    section
                        .header
                        .as_deref()
                        .and_then(|h| text(h, TextRole::Section)),
                );
                children.extend(section.rows.iter().map(|r| row(r, open)));
                children.extend(
                    section
                        .footer
                        .as_deref()
                        .and_then(|f| styled(f, TextRole::Body, true)),
                );
                column(ROW_GAP, children).key(i as u64)
            })
            .collect(),
    )
}

/// The search's results, each a button to its pane.
pub fn results(hits: &[Hit]) -> Element<Message> {
    if hits.is_empty() {
        return line("No Results", TextRole::Body, true);
    }
    column(
        ROW_GAP,
        hits.iter()
            .enumerate()
            .map(|(i, hit)| {
                let label = match hit.subtitle() {
                    Some(sub) => format!("{} — {sub}", hit.pane.pane().title),
                    None => hit.pane.pane().title.to_string(),
                };
                button(&label, true, Message::Found(i)).key(i as u64)
            })
            .collect(),
    )
}

/// The open menu of the pop-up button `id`, over it in `parent`.
pub fn menu(
    sections: &[Section],
    id: ControlId,
    parent: WindowKey,
) -> Option<Vec<Window<Message>>> {
    let row = sections.iter().flat_map(|s| &s.rows).find(|r| r.id == id)?;
    let Control::Popup { choices, selected } = &row.control else {
        return None;
    };
    let button = PopUpButton {
        label: row.label.clone(),
        options: choices.clone(),
        selected: *selected,
        open: true,
        enabled: true,
        on_open: None,
    };
    let menu = button.menu(
        parent,
        control_key(id),
        Box::new(move |i| Message::Commit(id, Value::Index(i))),
        Message::Open(None),
    );
    Some(menu.windows(WindowKey(format!("menu-{id:?}").into())))
}

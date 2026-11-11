//! A page as Kit controls: the pane's header card, then each section as a group of rows under its title, each row its
//! label, its second line and its control, with the section's footer below; a group of rows that open the pane's
//! sub-pages; a control's change is the [`Value`] its row commits, as the panes' model reads it.

use std::collections::HashMap;

use cupertino_kit::controls::{
    FormGroup, FormHeader, FormRow, IconTile, NavigationRow, PopUpButton, SegmentedControl, Slider,
    SliderStyle, TextRole, TileColor,
};
use cupertino_kit::core::{
    Change, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget, Window,
};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, FontWeight, TextStyle};

use super::Message;
use crate::controls::{metrics, Control, Value};
use crate::pages::{Block, SubPage};
use crate::pane::{Enclosure, PaneId};
use crate::panes::{ControlId, Row, Section};
use crate::search::Hit;

/// Between the controls in a row, and between a row's label and its control.
pub const ROW_GAP: f64 = 8.0;
/// Between a page's groups, with or without a title above them.
pub const SECTION_GAP: f64 = 10.0;
/// Between a row's label and its second line.
const LABEL_LINE_GAP: f64 = 2.0;
/// A section title: the space above it, its weight, and its leading inset, the group's rows' own.
pub(crate) const SECTION_TITLE_TOP: f64 = 20.0;
const SECTION_TITLE_WEIGHT: u16 = 700;
const SECTION_TITLE_INSET: f64 = 10.0;
/// A footer below a group: the space between them and its inset at both sides.
pub(crate) const FOOTER_GAP: f64 = 6.0;
const FOOTER_INSET: f64 = 10.0;
/// The tile of a pane's header card.
const HEADER_TILE: f64 = 48.0;
/// The side of a row's tile.
const ROW_TILE: f64 = 24.0;

/// The tile colour of an enclosure.
pub fn tile_color(enclosure: Enclosure) -> TileColor {
    match enclosure {
        Enclosure::Blue => TileColor::Blue,
        Enclosure::Black => TileColor::Graphite,
        Enclosure::Gray => TileColor::Gray,
        Enclosure::Pink => TileColor::Pink,
        Enclosure::Indigo => TileColor::Indigo,
        Enclosure::Green => TileColor::Green,
        Enclosure::Red => TileColor::Red,
    }
}

/// The header card of `pane`: its tile, `title` and description.
pub fn header(pane: PaneId, title: &str, description: &str) -> Element<Message> {
    let p = pane.pane();
    Element::new(FormHeader {
        tile: IconTile {
            symbol: p.symbol.into(),
            color: tile_color(p.enclosure),
            size: HEADER_TILE,
        },
        title: title.into(),
        description: description.into(),
    })
    .key("header")
}

/// What assistive technology calls each control of `sections`: its row's label, with the section's title after it
/// where the same label stands in more than one section, so that, as each application's "Show previews", every one
/// has a name of its own. Names still alike (two applications of one name) are numbered in the order they show.
pub fn control_names(sections: &[Section]) -> Names {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for row in sections.iter().flat_map(|s| &s.rows) {
        *counts.entry(row.label.as_str()).or_default() += 1;
    }
    let mut names = Vec::new();
    for section in sections {
        for row in &section.rows {
            let name = match &section.header {
                Some(header) if counts[row.label.as_str()] > 1 => {
                    format!("{}, {header}", row.label)
                }
                _ => row.label.clone(),
            };
            names.push((row.id, name));
        }
    }
    let mut alike: HashMap<String, usize> = HashMap::new();
    for (_, name) in &names {
        *alike.entry(name.clone()).or_default() += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (_, name) in &mut names {
        if alike[name.as_str()] > 1 {
            let n = seen.entry(name.clone()).or_default();
            *n += 1;
            *name = format!("{name} ({n})");
        }
    }
    Names(names)
}

/// The accessible name of each control of a page, by its id.
pub struct Names(Vec<(ControlId, String)>);

impl Names {
    pub fn get(&self, id: ControlId) -> Option<&str> {
        self.0
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, name)| name.as_str())
    }
}

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

/// The control of `row`, named `name`; a disabled row's control takes no input.
fn control(row: &Row, name: &str, open: Option<ControlId>) -> Option<Element<Message>> {
    let id = row.id;
    let enabled = row.disabled.is_none();
    let commit = move |value: Value| Message::Commit(id, value);
    let control = match &row.control {
        Control::Switch { on } => Element::new(
            Toggle {
                label: name.to_owned(),
                on: *on,
                enabled,
                on_toggle: Some(Box::new(move |on| commit(Value::Bool(on)))),
            }
            .small(),
        ),
        Control::Segmented { segments, selected } => Element::new(SegmentedControl {
            label: name.to_owned(),
            segments: segments.iter().map(|s| s.to_string()).collect(),
            selected: Some(*selected),
            enabled,
            on_select: Some(Box::new(move |i| commit(Value::Index(i)))),
        }),
        Control::Popup { choices, selected } => Element::new(PopUpButton {
            label: name.to_owned(),
            options: choices.clone(),
            selected: *selected,
            open: open == Some(id),
            enabled,
            on_open: Some(Message::Open(Some(id))),
        }),
        Control::Slider { value, stops, .. } => {
            let stops = stops.filter(|n| *n > 1);
            Element::new(Slider {
                label: name.to_owned(),
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
                on_end: None,
            })
            .width(metrics::SLIDER_WIDTH)
        }
        Control::Level { value } => Element::new(Slider {
            label: name.to_owned(),
            value: *value,
            min: 0.0,
            max: 1.0,
            step: None,
            style: SliderStyle::Regular,
            ticks: 0,
            enabled,
            on_change: Some(Box::new(move |v| Message::Level(id, v))),
            on_end: None,
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
        Control::ButtonAndSwitch { title, on } => Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: ROW_GAP,
            align: Align::Center,
            children: vec![
                // the buttons of every row read alike; each is described by the name of its row's switch
                Element::new(Described {
                    description: name.to_owned(),
                    child: vec![button(title, enabled, commit(Value::Index(0)))],
                }),
                Element::new(
                    Toggle {
                        label: name.to_owned(),
                        on: *on,
                        enabled,
                        on_toggle: Some(Box::new(move |on| commit(Value::Bool(on)))),
                    }
                    .small(),
                ),
            ],
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

/// A row's label over its second line. When the row is too narrow for it and its control, it shrinks along the row (not
/// across it), so a long label or second line wraps instead of pushing the control past the window's edge.
struct Label<M> {
    lines: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Label<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            // its own width while the row has room, down to its longest word when it has not
            flex: Flex {
                grow: 0.0,
                shrink: 1.0,
                basis: None,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: LABEL_LINE_GAP,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.lines)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// `child` described to assistive technology by `description`, after its own: a row's button by the row's name. It is
/// no node of its own.
struct Described<M> {
    description: String,
    child: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Described<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Horizontal,
            spacing: 0.0,
            align: Align::Center,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.child)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if old.description == self.description {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.describe_content(self.description.clone());
    }
}

fn row(row: &Row, names: &Names, open: Option<ControlId>) -> Element<Message> {
    let second = row
        .error
        .as_ref()
        .or(row.disabled.as_ref())
        .or(row.detail.as_ref());
    let label = text(&row.label, TextRole::Body)
        .into_iter()
        .chain(second.and_then(|s| styled(s, TextRole::Caption, true)))
        .collect();
    let mut children = vec![
        Element::new(Label { lines: label }),
        Element::new(Spacer::default()),
    ];
    let name = names.get(row.id).unwrap_or(&row.label);
    children.extend(control(row, name, open));
    Element::from(FormRow {
        tile: None,
        content: Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: ROW_GAP,
            align: Align::Center,
            children,
        }),
    })
    .key(format!("{:?}", row.id))
}

/// A section title, bold, over its group.
fn section_title(title: &str, top: f64) -> Element<Message> {
    let style = TextStyle {
        weight: FontWeight(SECTION_TITLE_WEIGHT),
        ..TextStyle::new(13.0)
    };
    let title = Element::new(Text {
        content: AttributedText::plain(title, style),
        role: TextRole::Body,
        wrap: true,
        secondary: false,
        selected: false,
    });
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets {
            top,
            left: SECTION_TITLE_INSET,
            bottom: SECTION_GAP,
            right: SECTION_TITLE_INSET,
        },
        spacing: 0.0,
        align: Align::Stretch,
        children: vec![title],
    })
}

/// A section: its title, its rows in a group, and its footer. A title at the top of the page (`at_top`) has no space
/// above it beyond the page's own inset, as a header card or a group there has none.
fn section(
    section: &Section,
    names: &Names,
    open: Option<ControlId>,
    at_top: bool,
) -> Vec<Element<Message>> {
    let mut out = Vec::new();
    if let Some(header) = section.header.as_deref().filter(|h| !h.is_empty()) {
        let top = if at_top { 0.0 } else { SECTION_TITLE_TOP };
        out.push(section_title(header, top));
    }
    if !section.rows.is_empty() {
        out.push(Element::from(FormGroup {
            rows: section.rows.iter().map(|r| row(r, names, open)).collect(),
        }));
    }
    if let Some(footer) = section
        .footer
        .as_deref()
        .and_then(|f| styled(f, TextRole::Caption, true))
    {
        out.push(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets {
                top: FOOTER_GAP,
                left: FOOTER_INSET,
                bottom: 0.0,
                right: FOOTER_INSET,
            },
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![footer],
        }));
    }
    out
}

/// The rows that open `subs`, in one group.
fn links(subs: &[SubPage]) -> Element<Message> {
    Element::from(FormGroup {
        rows: subs
            .iter()
            .map(|&sub| {
                Element::new(NavigationRow {
                    label: sub.title().into(),
                    tile: Some(IconTile {
                        symbol: sub.symbol().into(),
                        color: tile_color(sub.enclosure()),
                        size: ROW_TILE,
                    }),
                    on_open: Some(Message::OpenPage(sub)),
                })
                .key(format!("open-{sub:?}"))
            })
            .collect(),
    })
}

/// A page: its header card, when it has one, then its blocks, top to bottom. `sections` are all the pane's, which
/// name the controls.
pub fn page(
    header: Option<Element<Message>>,
    blocks: &[Block],
    sections: &[Section],
    open: Option<ControlId>,
) -> Element<Message> {
    let names = control_names(sections);
    let at_top = header.is_none();
    let mut children: Vec<Element<Message>> = header.into_iter().collect();
    for (i, block) in blocks.iter().enumerate() {
        let parts = match block {
            Block::Section(s) => section(s, &names, open, at_top && i == 0),
            Block::Links(subs) => vec![links(subs)],
        };
        // each block is keyed by its place, so its rows keep their state while other blocks come and go
        children.push(column(0.0, parts).key(i as u64));
    }
    column(SECTION_GAP, children)
}

/// The search's results, each a row that opens its page.
pub fn results(hits: &[Hit]) -> Element<Message> {
    if hits.is_empty() {
        return line("No Results", TextRole::Body, true);
    }
    Element::from(FormGroup {
        rows: hits
            .iter()
            .enumerate()
            .map(|(i, hit)| {
                let pane = hit.pane.pane();
                let label = match hit.subtitle() {
                    Some(sub) => format!("{} — {sub}", pane.title),
                    None => pane.title.to_string(),
                };
                Element::new(NavigationRow {
                    label,
                    tile: Some(IconTile {
                        symbol: pane.symbol.into(),
                        color: tile_color(pane.enclosure),
                        size: ROW_TILE,
                    }),
                    on_open: Some(Message::Found(i)),
                })
                .key(i as u64)
            })
            .collect(),
    })
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
    let name = control_names(sections)
        .get(id)
        .map_or_else(|| row.label.clone(), str::to_owned);
    let button = PopUpButton {
        label: name,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Control;
    use crate::live::Live;
    use crate::pane::PaneId;
    use crate::panes::sections;

    fn settings() -> crate::settings::Settings {
        crate::settings::Settings {
            dock: Default::default(),
            compositor: Default::default(),
            menubar: cupertino_defaults::menubar::MenuBarSettings {
                clock: cupertino_defaults::menubar::ClockSettings::UNSET,
            },
            keyboard: Default::default(),
            power: Default::default(),
            accessibility: Default::default(),
            display_height: 1080.0,
            clipboard: Default::default(),
            screen_sharing: Default::default(),
            desktop_control: Default::default(),
            notifications: Default::default(),
        }
    }

    /// The accessible name of each application's previews pop-up, by application id.
    fn previews_names(settings: &crate::settings::Settings, live: &Live) -> Vec<(String, String)> {
        let session = crate::panes::Session {
            dark: false,
            display: Default::default(),
        };
        let apps = crate::panes::notification_apps(settings, live);
        let shown = sections(PaneId::Notifications, settings, session, live);
        let names = control_names(&shown);
        let mut out: Vec<(String, String)> = shown
            .iter()
            .flat_map(|s| &s.rows)
            .filter_map(|r| match r.id {
                ControlId::NotificationAppPreviews(i) => {
                    Some((apps[i].0.clone(), names.get(r.id).unwrap().to_owned()))
                }
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn two_apps_of_one_name_keep_their_numbers_whichever_has_a_choice() {
        let mail = |id: &str| (id.to_owned(), "Mail".to_owned());
        let live = |apps: Vec<(String, String)>| Live {
            notifying_apps: apps,
            ..Live::default()
        };
        let mut plain = settings();
        let expected = vec![
            (
                "org.a.Mail".to_owned(),
                "Show previews, Mail (1)".to_owned(),
            ),
            (
                "org.z.Mail".to_owned(),
                "Show previews, Mail (2)".to_owned(),
            ),
        ];
        let both = [mail("org.z.Mail"), mail("org.a.Mail")];
        assert_eq!(previews_names(&plain, &live(both.to_vec())), expected);
        let reversed = [mail("org.a.Mail"), mail("org.z.Mail")];
        assert_eq!(previews_names(&plain, &live(reversed.to_vec())), expected);
        // a choice for the second makes it a saved application, listed before the centre's
        plain.notifications.apps.insert(
            "org.z.Mail".into(),
            cupertino_defaults::notifications::AppOverride {
                show_on_lock_screen: Some(false),
                show_previews: None,
            },
        );
        assert_eq!(previews_names(&plain, &live(both.to_vec())), expected);
        assert_eq!(previews_names(&plain, &live(reversed.to_vec())), expected);
    }

    #[test]
    fn two_apps_of_one_name_get_popups_of_names_of_their_own() {
        let live = Live {
            notifying_apps: vec![
                ("org.example.Mail".into(), "Mail".into()),
                ("org.other.Mail".into(), "Mail".into()),
                ("org.example.Chat".into(), "Chat".into()),
            ],
            ..Live::default()
        };
        let settings = crate::settings::Settings {
            dock: Default::default(),
            compositor: Default::default(),
            menubar: cupertino_defaults::menubar::MenuBarSettings {
                clock: cupertino_defaults::menubar::ClockSettings::UNSET,
            },
            keyboard: Default::default(),
            power: Default::default(),
            accessibility: Default::default(),
            display_height: 1080.0,
            clipboard: Default::default(),
            screen_sharing: Default::default(),
            desktop_control: Default::default(),
            notifications: Default::default(),
        };
        let session = crate::panes::Session {
            dark: false,
            display: Default::default(),
        };
        let shown = sections(PaneId::Notifications, &settings, session, &live);
        let names = control_names(&shown);
        let mut popups: Vec<&str> = shown
            .iter()
            .flat_map(|s| &s.rows)
            .filter(|r| matches!(r.control, Control::Popup { .. }))
            .map(|r| names.get(r.id).unwrap())
            .collect();
        popups.sort_unstable();
        assert_eq!(
            popups,
            [
                "Show on lock screen, Chat",
                "Show on lock screen, Mail (1)",
                "Show on lock screen, Mail (2)",
                "Show previews, Chat",
                "Show previews, Lock Screen",
                "Show previews, Mail (1)",
                "Show previews, Mail (2)",
            ]
        );
    }
}

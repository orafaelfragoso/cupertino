//! A page as Kit controls: the pane's header card, then each section as a group of rows under its title, each row its
//! label, its second line and its control, with the section's footer below; a group of rows that open the pane's
//! sub-pages; a control's change is the [`Value`] its row commits, as the panes' model reads it.

use std::collections::HashMap;

use cupertino_kit::controls::{
    AppIcon, AppIconRow, FormGroup, FormRow, FullWidthSegmentedControl, IconTile, ImageSource,
    InlinePopUpButton, NavigationRow, PaneHeader, PopUpButton, ScrollView, Scroller,
    SegmentedControl, Slider, SliderStyle, TextRole, ThumbnailTile, TileColor,
};
use cupertino_kit::core::{
    Change, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget, Window,
};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::layout::{Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, FontWeight, TextStyle};

use super::{Message, UNSCROLLED};
use crate::controls::{metrics, Control, Value};
use crate::pages::{Block, SubPage};
use crate::pane::{Enclosure, PaneId};
use crate::panes::{ControlId, Row, Section};
use crate::search::Hit;

/// What a page draws besides its rows' own values: the control whose menu is open, the application icons by name, the
/// decoded thumbnails by picture, and how far each strip of thumbnails is scrolled.
pub struct View<'a> {
    pub open: Option<ControlId>,
    pub icons: &'a HashMap<String, String>,
    pub thumbnails: &'a HashMap<std::path::PathBuf, ImageSource>,
    pub tile_scrolls: &'a [(ControlId, ScrollState)],
}

/// Between a row's label and the segmented control under it.
const SEGMENTED_LABEL_GAP: f64 = 6.0;
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
/// The tile of a pane's header.
const HEADER_TILE: f64 = 26.0;
/// The side of a row's tile, and of the icon of a row that names an application or a category.
const ROW_TILE: f64 = 24.0;
const ROW_ICON: f64 = 22.0;
/// The icon before an application's section title.
const TITLE_ICON: f64 = 20.0;
/// Between a section title's icon and its text.
const TITLE_ICON_GAP: f64 = 6.0;

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

/// The header of `pane`: its tile, `title` and description, and the pane's master switch, which is `master`'s control
/// when the pane has one.
pub fn pane_header(
    pane: PaneId,
    title: &str,
    description: &str,
    master: Option<&Row>,
) -> Element<Message> {
    let p = pane.pane();
    // what keeps the master switch from working says so where the description would
    let description = master
        .and_then(|row| row.error.as_ref().or(row.disabled.as_ref()))
        .map_or(description, String::as_str);
    let switch = master.and_then(|row| {
        let Control::Switch { on } = &row.control else {
            return None;
        };
        let id = row.id;
        Some(
            Element::new(
                Toggle {
                    label: row.label.clone(),
                    on: *on,
                    enabled: row.disabled.is_none(),
                    on_toggle: Some(Box::new(move |on| Message::Commit(id, Value::Bool(on)))),
                }
                .header(),
            )
            .key(control_key(id)),
        )
    });
    Element::from(PaneHeader {
        tile: IconTile {
            symbol: p.symbol.into(),
            color: tile_color(p.enclosure),
            size: HEADER_TILE,
        },
        title: title.into(),
        description: description.into(),
        switch,
    })
    .key("header")
}

/// The pane's master switch: its first block's only row, a switch that stands on its own, taken out of `blocks`.
pub fn take_master(pane: PaneId, blocks: &mut Vec<Block>) -> Option<Row> {
    if !matches!(pane, PaneId::Wifi | PaneId::Bluetooth) {
        return None;
    }
    let Some(Block::Section(first)) = blocks.first() else {
        return None;
    };
    let single = first.header.is_none()
        && first.footer.is_none()
        && matches!(first.rows.as_slice(), [row] if matches!(row.control, Control::Switch { .. }));
    if !single {
        return None;
    }
    match blocks.remove(0) {
        Block::Section(mut section) => section.rows.pop(),
        Block::Links(_) => None,
    }
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
fn control(row: &Row, name: &str, view: &View<'_>) -> Option<Element<Message>> {
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
        Control::Segmented { segments, selected } => {
            Element::new(FullWidthSegmentedControl(SegmentedControl {
                label: name.to_owned(),
                segments: segments.iter().map(|s| s.to_string()).collect(),
                selected: Some(*selected),
                enabled,
                on_select: Some(Box::new(move |i| commit(Value::Index(i)))),
            }))
        }
        Control::Popup { choices, selected } => Element::new(InlinePopUpButton(PopUpButton {
            label: name.to_owned(),
            options: choices.clone(),
            selected: *selected,
            open: view.open == Some(id),
            enabled,
            on_open: Some(Message::Open(Some(id))),
        })),
        Control::Slider { .. } => slider_column(row, name, metrics::SLIDER_WIDTH, false),
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
        Control::Tiles { tiles, selected } => tile_strip(id, tiles, *selected, enabled, view),
    };
    Some(control.key(control_key(id)))
}

/// The width of each slider of a pair sharing one row.
const PAIRED_SLIDER_WIDTH: f64 = 200.0;
/// Between a slider and the end labels under it.
const SLIDER_LABEL_GAP: f64 = 2.0;

/// The slider of `row` `width` wide with its two end labels under it, and over it the row's label when `titled`.
fn slider_column(row: &Row, name: &str, width: f64, titled: bool) -> Element<Message> {
    let Control::Slider {
        value,
        stops,
        labels,
        ..
    } = &row.control
    else {
        return column(0.0, Vec::new());
    };
    let id = row.id;
    let enabled = row.disabled.is_none();
    let stops = stops.filter(|n| *n > 1);
    let slider = Element::new(Slider {
        label: name.to_owned(),
        value: *value,
        min: 0.0,
        max: 1.0,
        step: stops.map(|n| 1.0 / (n - 1) as f64),
        style: SliderStyle::Regular,
        ticks: stops.map_or(0, |n| n as u32),
        enabled,
        on_change: Some(Box::new(move |v| match stops {
            Some(n) => Message::Commit(id, Value::Index(Control::nearest_stop(v, n))),
            None => Message::Slide(id, v),
        })),
        on_end: None,
    })
    .key(control_key(id));
    let caption = |content: &str| line(content, TextRole::Caption, false);
    let ends = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Center,
        children: vec![
            caption(labels.0),
            Element::new(Spacer::default()),
            caption(labels.1),
        ],
    });
    let mut parts = Vec::new();
    if titled {
        parts.extend(text(&row.label, TextRole::Body));
    }
    parts.push(slider);
    parts.push(ends);
    column(SLIDER_LABEL_GAP, parts)
        .width(width)
        .key(format!("{:?}-slider", row.id))
}

/// Two sliders side by side in one row, each under its label.
fn slider_pair(first: &Row, second: &Row, names: &Names) -> Element<Message> {
    let name = |row: &Row| names.get(row.id).unwrap_or(&row.label).to_owned();
    Element::from(FormRow {
        tile: None,
        content: Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: ROW_GAP,
            align: Align::Start,
            children: vec![
                slider_column(first, &name(first), PAIRED_SLIDER_WIDTH, true),
                Element::new(Spacer::default()),
                slider_column(second, &name(second), PAIRED_SLIDER_WIDTH, true),
            ],
        }),
    })
    .key(format!("{:?}-{:?}", first.id, second.id))
}

/// Whether `row` is a slider that can share its row: it has no second line.
fn pairs(row: &Row) -> bool {
    matches!(row.control, Control::Slider { .. })
        && row.error.is_none()
        && row.disabled.is_none()
        && row.detail.is_none()
}

/// The rows of a section as elements, two sliders in a row of their own side by side.
fn rows(rows: &[Row], names: &Names, view: &View<'_>) -> Vec<Element<Message>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if i + 1 < rows.len() && pairs(&rows[i]) && pairs(&rows[i + 1]) {
            out.push(slider_pair(&rows[i], &rows[i + 1], names));
            i += 2;
        } else {
            out.push(row(&rows[i], names, view));
            i += 1;
        }
    }
    out
}

/// The thumbnail's width and height, its corners, and the space between two of them.
const THUMBNAIL: Size = Size::new(108.5, 72.0);
const THUMBNAIL_RADIUS: f64 = 5.0;
const THUMBNAIL_GAP: f64 = 9.5;
/// The room a thumbnail's ring takes on each side of the picture: the strip's spacing leaves it out.
const THUMBNAIL_RING_ROOM: f64 = 3.0;
/// The strip's height: a thumbnail with its ring room and its caption.
const STRIP_HEIGHT: f64 = THUMBNAIL.height + 2.0 * THUMBNAIL_RING_ROOM + 3.0 + 14.0;

/// One row of thumbnails, scrolling sideways: the pictures of `tiles` side by side, the chosen one ringed. The strip
/// runs on past the page's inset to the window's edge.
fn tile_strip(
    id: ControlId,
    tiles: &[crate::controls::Tile],
    selected: Option<usize>,
    enabled: bool,
    view: &View<'_>,
) -> Element<Message> {
    let children = tiles
        .iter()
        .enumerate()
        .map(|(i, tile)| {
            Element::new(ThumbnailTile {
                label: tile.title.clone(),
                image: view.thumbnails.get(&tile.image).cloned(),
                size: THUMBNAIL,
                radius: THUMBNAIL_RADIUS,
                caption: true,
                selected: selected == Some(i),
                enabled,
                on_press: Some(Message::Commit(id, Value::Index(i))),
            })
            .key(i as u64)
        })
        .collect();
    let content = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: THUMBNAIL_GAP - 2.0 * THUMBNAIL_RING_ROOM,
        align: Align::Start,
        children,
    });
    let state = view
        .tile_scrolls
        .iter()
        .find(|(i, _)| *i == id)
        .map_or(UNSCROLLED, |(_, s)| *s);
    Element::from(ScrollView {
        content,
        state,
        horizontal: true,
        vertical: false,
        scroller: Scroller::Overlay,
        on_scroll: Box::new(move |s| Message::TileScroll(id, s)),
    })
    .height(STRIP_HEIGHT)
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

fn row(row: &Row, names: &Names, view: &View<'_>) -> Element<Message> {
    let second = row
        .error
        .as_ref()
        .or(row.disabled.as_ref())
        .or(row.detail.as_ref());
    let label: Vec<Element<Message>> = text(&row.label, TextRole::Body)
        .into_iter()
        .chain(second.and_then(|s| styled(s, TextRole::Caption, true)))
        .collect();
    let name = names.get(row.id).unwrap_or(&row.label);
    // a segmented control takes the whole width of its row, under the row's label
    let content = if matches!(row.control, Control::Segmented { .. }) {
        let mut children = Vec::new();
        if !label.is_empty() {
            children.push(Element::new(Label { lines: label }));
        }
        children.extend(control(row, name, view));
        Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: SEGMENTED_LABEL_GAP,
            align: Align::Stretch,
            children,
        })
    } else {
        let mut children = vec![
            Element::new(Label { lines: label }),
            Element::new(Spacer::default()),
        ];
        children.extend(control(row, name, view));
        Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: ROW_GAP,
            align: Align::Center,
            children,
        })
    };
    match &row.icon {
        Some(icon) => Element::from(AppIconRow {
            icon: icon.clone(),
            size: ROW_ICON,
            two_line: second.is_some(),
            content,
        }),
        None => Element::from(FormRow {
            tile: None,
            content,
        }),
    }
    .key(format!("{:?}", row.id))
}

/// A section title, bold, over its group, after the application's `icon` when it has one.
fn section_title(title: &str, top: f64, icon: Option<&str>) -> Element<Message> {
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
    let content = match icon {
        Some(icon) => Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: TITLE_ICON_GAP,
            align: Align::Center,
            children: vec![
                Element::new(AppIcon {
                    name: icon.to_owned(),
                    size: TITLE_ICON,
                }),
                title,
            ],
        }),
        None => title,
    };
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
        children: vec![content],
    })
}

/// A section: its title, its rows in a group, and its footer. A title at the top of the page (`at_top`) has no space
/// above it beyond the page's own inset, as a header card or a group there has none.
fn section(
    section: &Section,
    names: &Names,
    view: &View<'_>,
    at_top: bool,
) -> Vec<Element<Message>> {
    let mut out = Vec::new();
    if let Some(header) = section.header.as_deref().filter(|h| !h.is_empty()) {
        let top = if at_top { 0.0 } else { SECTION_TITLE_TOP };
        out.push(section_title(
            header,
            top,
            view.icons.get(header).map(String::as_str),
        ));
    }
    if !section.rows.is_empty() {
        out.push(Element::from(FormGroup {
            rows: rows(&section.rows, names, view),
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
    view: &View<'_>,
) -> Element<Message> {
    let names = control_names(sections);
    let at_top = header.is_none();
    let mut children: Vec<Element<Message>> = header.into_iter().collect();
    for (i, block) in blocks.iter().enumerate() {
        let parts = match block {
            Block::Section(s) => section(s, &names, view, at_top && i == 0),
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

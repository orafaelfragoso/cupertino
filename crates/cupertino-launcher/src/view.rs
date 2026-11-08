//! The launcher's windows as CupertinoKit draws them: the panel (a glass capsule holding the query, which grows to
//! show the sectioned results beside a preview of the selected one and its actions), the transparent catcher that
//! takes a press outside the panel, and the empty window the process keeps mapped while the panel is hidden.
//!
//! The panel's window is larger than the panel by room for its shadow on every side; only the panel takes input.
//!
//! The panel's keys never reach the field: [`PanelRoot`] takes them in the capture phase and sends them to the model
//! as [`Input::Key`], so the panel's keyboard table ([`crate::panel`]) holds whatever the field would do with the key.
//! Every other key goes on to Kit's [`TextField`], which reports the edited query as [`Input::Query`].

use crate::engine::{ItemAction, ItemKind, ResultItem};
use crate::panel::{takes_key, Focus, Key as PanelKey, Mods as PanelMods, Panel};
use crate::preview::{Preview, PreviewBody};
use cupertino_kit::controls::{
    BareField, Icon, IconDirection, Image, ImageFit, ImageSource, Separator, Stack, Text,
    TextChange, TextField, TextRole,
};
use cupertino_kit::core::{
    AccessibleStates, Action, Change, CommandRegistry, Element, Environment, Event, EventCx, Glass,
    Key, MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{
    DecorationMode, KeyEvent, Mods, MouseButton, PointerEvent, WindowSpec,
};
use cupertino_kit::render::{Effect, FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{
    AttributedText, ContentPurpose, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign,
    TextStyle, Wrap,
};
use cupertino_kit::theme::{Color, ColorRole, StateVariant, Surface};
use std::collections::HashMap;

/// The `app_id` of the panel's window, which gives it its layer surface.
pub const APP_ID: &str = "cupertino-launcher";
/// The `app_id` of the window that takes a press outside the shown panel.
pub const CATCHER_APP_ID: &str = "cupertino-launcher.catcher";
/// The `app_id` of the window kept mapped while the panel is hidden.
pub const HIDDEN_APP_ID: &str = "cupertino-launcher.hidden";

/// Panel width: the search capsule's.
pub const WIDTH: f64 = 640.0;
/// The query row, which is the whole panel, a capsule, while there are no results.
pub const COLLAPSED_HEIGHT: f64 = 56.0;
/// The least height of the panel while it shows results.
pub const MIN_EXPANDED_HEIGHT: f64 = 430.0;
/// Corner radius of the panel: half the capsule's height, kept as it grows to show results.
pub const CORNER_RADIUS: f64 = COLLAPSED_HEIGHT / 2.0;
/// Between the panel's leading edge and the search symbol, and between the query and the trailing edge.
pub const SIDE_PADDING: f64 = 22.0;
/// Between the search symbol and the query.
pub const QUERY_GAP: f64 = 16.0;
/// The query's text size.
pub const QUERY_SIZE: f64 = 26.0;
/// Above the first row and below the last.
pub const LIST_PADDING: f64 = 8.0;
/// Width of the results list beside the preview.
pub const LIST_WIDTH: f64 = 288.0;
/// A section heading (derived from its paddings and one caption line).
pub const HEADER_HEIGHT: f64 = 32.0;
/// A result row.
pub const ROW_HEIGHT: f64 = 40.0;
/// The calculator's or a conversion's row, whose value is set large.
pub const CALCULATOR_ROW_HEIGHT: f64 = 64.0;
/// Result icons.
pub const ICON_SIZE: f64 = 32.0;
/// The search symbol before the query.
pub const SEARCH_ICON_SIZE: f64 = 22.0;
/// Between a row's selection and the list's edges.
pub const SELECTION_INSET: f64 = 8.0;
/// Between the selection's edge and the icon.
pub const CELL_INSET: f64 = 12.0;
/// Corner radius of a row's selection.
pub const SELECTION_RADIUS: f64 = 12.0;
/// Rows, headings included, shown at once.
pub const MAX_ROWS: usize = 9;
/// Distance of the panel's top edge from the top of the output, as a fraction of its height.
pub const TOP_FRACTION: f64 = 0.2;
/// The query field's placeholder and accessible name.
pub const PLACEHOLDER: &str = "Search";
/// The panel's shadow: its blur, how far below the panel it falls and its opacity in each appearance.
pub const SHADOW_BLUR: f64 = 40.0;
pub const SHADOW_OFFSET: f64 = 16.0;
pub const SHADOW_ALPHA_DARK: f32 = 0.5;
pub const SHADOW_ALPHA_LIGHT: f32 = 0.22;
/// Room the window keeps around the panel for its shadow: the blur reaches three halves of `SHADOW_BLUR` past the
/// shape, moved down by the offset.
pub const SHADOW_SIDE: f64 = 1.5 * SHADOW_BLUR;
pub const SHADOW_TOP: f64 = 1.5 * SHADOW_BLUR - SHADOW_OFFSET;
pub const SHADOW_BOTTOM: f64 = 1.5 * SHADOW_BLUR + SHADOW_OFFSET;
/// The thin dark edge just outside the panel, and its opacity in each appearance.
pub const EDGE_WIDTH: f64 = 0.5;
pub const EDGE_ALPHA_DARK: f32 = 0.55;
pub const EDGE_ALPHA_LIGHT: f32 = 0.12;
/// The preview pane: its padding, the largest picture or icon it shows, the text excerpt's size and lines, and the
/// action buttons' height and gap.
pub const PREVIEW_PADDING: f64 = 16.0;
pub const PREVIEW_IMAGE: Size = Size {
    width: 300.0,
    height: 136.0,
};
pub const PREVIEW_IMAGE_RADIUS: f64 = 6.0;
pub const PREVIEW_ICON_SIZE: f64 = 64.0;
pub const PREVIEW_TEXT_SIZE: f64 = 11.0;
pub const PREVIEW_TEXT_LINES: usize = 9;
pub const PREVIEW_TITLE_SIZE: f64 = 15.0;
pub const PREVIEW_VALUE_SIZE: f64 = 28.0;
/// Between a detail's label and its value.
pub const DETAIL_GAP: f64 = 8.0;
/// The width of a detail's label, which ends where its value starts: the labels end just short of the pane's middle
/// and the values start just past it.
pub const DETAIL_LABEL: f64 = (WIDTH - LIST_WIDTH - 1.0 - 2.0 * PREVIEW_PADDING - DETAIL_GAP) / 2.0;
pub const ACTION_HEIGHT: f64 = 26.0;
pub const ACTION_GAP: f64 = 4.0;
/// Action buttons shown at once.
pub const MAX_ACTIONS: usize = 4;

/// The symbol in front of the query and of the web search row.
pub const SEARCH_SYMBOL: &str = "symbol.magnifyingglass";
/// The raster icon of an application whose entry names none.
pub const GENERIC_APPLICATION_ICON: &str = "application.generic";
/// The raster icon of folder results.
pub const FOLDER_ICON: &str = "folder";
/// The raster icon of file results (they are not typed: the launcher holds no MIME database).
pub const DOCUMENT_ICON: &str = "document.generic";
const RETURN: u32 = 0xff0d;
const KP_ENTER: u32 = 0xff8d;
const ESCAPE: u32 = 0xff1b;
const BACKSPACE: u32 = 0xff08;
const DELETE: u32 = 0xffff;
const TAB: u32 = 0xff09;
const LEFT_TAB: u32 = 0xfe20;
const HOME: u32 = 0xff50;
const LEFT: u32 = 0xff51;
const UP: u32 = 0xff52;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;
const PAGE_UP: u32 = 0xff55;
const PAGE_DOWN: u32 = 0xff56;
const END: u32 = 0xff57;

/// What the windows report to the model.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// A key of the panel's keyboard table.
    Key(PanelKey, PanelMods),
    /// The query field's text changed.
    Query(TextChange),
    /// The pointer moved onto the result at this flat index.
    Hover(usize),
    /// The result at this flat index was clicked, or activated by assistive technology.
    Activate(usize),
    /// Action `.1` of the result at flat index `.0` was clicked, or pressed by assistive technology.
    RunAction(usize, usize),
    /// A press outside the panel, or the panel's window lost the keyboard.
    Dismiss,
}

/// The panel key and modifiers of a key press, or `None` for a release and for keys the panel has no name for
/// (modifiers, function keys). Named keys are recognised by their keysym; a key with a command modifier held (Ctrl+N,
/// Alt+1, Super+Y) is named by its lower-case character, anything else by the character the layout produced.
pub fn panel_key(key: &KeyEvent) -> Option<(PanelKey, PanelMods)> {
    if !key.pressed {
        return None;
    }
    let mods = PanelMods {
        ctrl: key.mods.contains(Mods::CTRL),
        alt: key.mods.contains(Mods::ALT),
        shift: key.mods.contains(Mods::SHIFT),
        primary: key.mods.contains(Mods::PRIMARY),
    };
    let named = match key.keysym.0 {
        RETURN | KP_ENTER => Some(PanelKey::Enter),
        ESCAPE => Some(PanelKey::Escape),
        BACKSPACE => Some(PanelKey::Backspace),
        DELETE => Some(PanelKey::Delete),
        LEFT => Some(PanelKey::Left),
        RIGHT => Some(PanelKey::Right),
        UP => Some(PanelKey::Up),
        DOWN => Some(PanelKey::Down),
        HOME => Some(PanelKey::Home),
        END => Some(PanelKey::End),
        PAGE_UP => Some(PanelKey::PageUp),
        PAGE_DOWN => Some(PanelKey::PageDown),
        TAB | LEFT_TAB => Some(PanelKey::Tab),
        _ => None,
    };
    if let Some(named) = named {
        return Some((named, mods));
    }
    // a chord may type a control character (Ctrl+N) or nothing: the keysym names its key then
    let character = key
        .text
        .filter(|c| !c.is_control())
        .or_else(|| keysym_char(key.keysym.0))?;
    let character = if mods.ctrl || mods.alt || mods.primary {
        character.to_lowercase().next()?
    } else {
        character
    };
    Some((PanelKey::Char(character), mods))
}

/// The character a keysym stands for: Latin-1 keysyms are their code points, Unicode keysyms carry theirs.
fn keysym_char(keysym: u32) -> Option<char> {
    let code = match keysym {
        0x20..=0x7e | 0xa0..=0xff => keysym,
        0x0100_0100..=0x0110_ffff => keysym - 0x0100_0000,
        _ => return None,
    };
    char::from_u32(code).filter(|c| !c.is_control())
}

/// One shown row of the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Header(String),
    Item { flat: usize, calculator: bool },
}

impl Row {
    fn height(&self) -> f64 {
        match self {
            Row::Header(_) => HEADER_HEIGHT,
            Row::Item {
                calculator: true, ..
            } => CALCULATOR_ROW_HEIGHT,
            Row::Item { .. } => ROW_HEIGHT,
        }
    }
}

/// The rows the panel shows: [`MAX_ROWS`] of the headings and results from the panel's scroll position.
pub fn visible_rows(panel: &Panel) -> Vec<Row> {
    let mut rows = Vec::new();
    let scroll = panel.scroll_row().min(panel.total_rows().saturating_sub(1));
    let mut row = 0usize;
    let mut flat = 0usize;
    for section in &panel.results().sections {
        if row >= scroll && rows.len() < MAX_ROWS {
            rows.push(Row::Header(section.title.clone()));
        }
        row += 1;
        for item in &section.items {
            if row >= scroll && rows.len() < MAX_ROWS {
                rows.push(Row::Item {
                    flat,
                    calculator: is_value(item.kind),
                });
            }
            row += 1;
            flat += 1;
        }
    }
    rows
}

/// A calculation's or a conversion's result: its value is set large, without an icon.
fn is_value(kind: ItemKind) -> bool {
    matches!(kind, ItemKind::Calculator | ItemKind::Conversion)
}

/// The panel's height: the query row alone without results, else the rows shown below it, never less than
/// [`MIN_EXPANDED_HEIGHT`].
pub fn panel_height(panel: &Panel) -> f64 {
    let rows = visible_rows(panel);
    if rows.is_empty() {
        return COLLAPSED_HEIGHT;
    }
    let list: f64 = rows.iter().map(Row::height).sum();
    (COLLAPSED_HEIGHT + 1.0 + 2.0 * LIST_PADDING + list).max(MIN_EXPANDED_HEIGHT)
}

/// The panel's window for a panel `height` points tall: the panel and the room around it for its shadow.
pub fn window_size(height: f64) -> Size {
    Size::new(
        WIDTH + 2.0 * SHADOW_SIDE,
        height + SHADOW_TOP + SHADOW_BOTTOM,
    )
}

/// Where the panel lies in its window, `height` points tall.
pub fn panel_rect(height: f64) -> Rect {
    Rect::new(SHADOW_SIDE, SHADOW_TOP, WIDTH, height)
}

/// Where the panel goes on an output `output` points large: horizontally centred, its top edge at [`TOP_FRACTION`]
/// of the height.
pub fn placement(output: Size, height: f64) -> Rect {
    Rect::new(
        ((output.width - WIDTH) / 2.0).round(),
        (output.height * TOP_FRACTION).round(),
        WIDTH,
        height,
    )
}

/// The icon a result shows when the host gave no image for it: its kind's icon (the host normally gives the raster
/// ones as images), none for the calculator.
pub fn fallback_symbol(kind: ItemKind) -> Option<&'static str> {
    match kind {
        ItemKind::Application | ItemKind::ApplicationAction => Some(GENERIC_APPLICATION_ICON),
        ItemKind::Folder => Some(FOLDER_ICON),
        ItemKind::File => Some(DOCUMENT_ICON),
        ItemKind::WebSearch => Some(SEARCH_SYMBOL),
        ItemKind::Calculator | ItemKind::Conversion => None,
    }
}

/// What the panel window draws.
pub struct PanelView<'a> {
    pub panel: &'a Panel,
    /// The revision of the query the model last took from the field.
    pub revision: u64,
    /// The appear animation, 0 (hidden) to 1.
    pub alpha: f64,
    /// Images of the results' icons by [`ResultItem::id`]; a result without one shows its kind's symbol.
    pub icons: &'a HashMap<String, Vec<ImageSource>>,
    /// The selected result's actions, its own activation first.
    pub actions: &'a [ItemAction],
    /// What the host found out about the selected result, once it has.
    pub preview: Option<&'a Preview>,
}

fn window_spec(title: &str, app_id: &str, size: Size) -> WindowSpec {
    WindowSpec {
        title: title.to_owned(),
        app_id: app_id.to_owned(),
        size,
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: None,
    }
}

fn empty<M: 'static>() -> Element<M> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::default(),
        spacing: 0.0,
        align: Align::Start,
        children: Vec::new(),
    })
}

/// The panel's window: transparent, the panel inside it at [`panel_rect`], which alone takes input.
pub fn panel_window<M>(view: &PanelView<'_>) -> Window<M>
where
    M: From<Input> + Clone + 'static,
{
    let height = panel_height(view.panel);
    let mut spec = window_spec("Launcher", APP_ID, window_size(height));
    spec.input_region = Some(vec![panel_rect(height)]);
    Window {
        key: WindowKey(Key::from("panel")),
        spec,
        shortcuts: CommandRegistry::new(),
        on_close: Some(M::from(Input::Dismiss)),
        background: WindowBackground::None,
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets {
                top: SHADOW_TOP,
                left: SHADOW_SIDE,
                bottom: SHADOW_BOTTOM,
                right: SHADOW_SIDE,
            },
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![panel_content(view).key("panel")],
        }),
        popup: None,
        menus: None,
    }
}

/// The transparent window over the output, below the panel, that closes it on a press.
pub fn catcher_window<M>() -> Window<M>
where
    M: From<Input> + Clone + 'static,
{
    Window {
        key: WindowKey(Key::from("catcher")),
        spec: window_spec("Close the launcher", CATCHER_APP_ID, Size::new(1.0, 1.0)),
        shortcuts: CommandRegistry::new(),
        on_close: Some(M::from(Input::Dismiss)),
        background: WindowBackground::None,
        content: Element::new(Catcher),
        popup: None,
        menus: None,
    }
}

/// The window kept mapped while the panel is hidden: one transparent point that takes no input. Each `generation`
/// is a new window; when the server closes one (its output went away) `on_close` arrives and the model asks for the
/// next generation, which the server puts on an output still there.
pub fn hidden_window<M: Clone + 'static>(generation: u64, on_close: M) -> Window<M> {
    Window {
        key: WindowKey(Key::from(format!("hidden-{generation}"))),
        spec: window_spec("Launcher", HIDDEN_APP_ID, Size::new(1.0, 1.0)),
        shortcuts: CommandRegistry::new(),
        on_close: Some(on_close),
        background: WindowBackground::None,
        content: empty(),
        popup: None,
        menus: None,
    }
}

/// The query row, then below a separator the results beside the selected one's preview.
pub fn panel_content<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let field = Element::new(BareField {
        field: TextField {
            label: PLACEHOLDER.to_owned(),
            placeholder: PLACEHOLDER.to_owned(),
            value: panel.query().to_owned(),
            revision: view.revision,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: Some(Box::new(|change| M::from(Input::Query(change)))),
            on_submit: None,
            selection: None,
            initial: true,
        },
        size: QUERY_SIZE,
    })
    .key("query")
    .fill();
    let query_row = Element::new(Bar {
        height: COLLAPSED_HEIGHT,
        spacing: QUERY_GAP,
        children: vec![
            Element::new(Icon {
                name: SEARCH_SYMBOL.to_owned(),
                size: SEARCH_ICON_SIZE,
                direction: IconDirection::Fixed,
                label: None,
                tint: Some(ColorRole::SecondaryText),
            }),
            field,
        ],
    })
    .key("query-row");
    let mut children = vec![query_row];
    let rows = visible_rows(panel);
    if !rows.is_empty() {
        children.push(
            Element::new(Separator {
                axis: Axis::Horizontal,
            })
            .key("separator"),
        );
        let body = vec![
            results(view, &rows).key("results"),
            Element::new(Separator {
                axis: Axis::Vertical,
            })
            .key("divider"),
            preview_pane(view).key("preview"),
        ];
        children.push(Element::new(Plain::row(body, Align::Stretch)).key("body"));
    }
    Element::new(PanelRoot {
        focus: panel.focus(),
        alpha: view.alpha,
        children,
    })
}

/// The shown rows: a list of the sections, each a group named by its heading over its results.
fn results<M>(view: &PanelView<'_>, rows: &[Row]) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let mut sections: Vec<Element<M>> = Vec::new();
    let mut current: Option<(String, Vec<Element<M>>)> = None;
    let flush = |sections: &mut Vec<Element<M>>, current: Option<(String, Vec<Element<M>>)>| {
        if let Some((title, children)) = current {
            let key = format!("section:{title}");
            sections.push(Element::new(SectionGroup { title, children }).key(key));
        }
    };
    for row in rows {
        match row {
            Row::Header(title) => {
                flush(&mut sections, current.take());
                current = Some((
                    title.clone(),
                    vec![Element::new(Quiet::heading(title)).key("heading")],
                ));
            }
            &Row::Item { flat, calculator } => {
                let Some(item) = panel.results().get(flat) else {
                    continue;
                };
                let element = result_row(view, item, flat, calculator, flat == panel.selected());
                match &mut current {
                    Some((_, children)) => children.push(element),
                    // the list scrolled past this section's heading
                    None => {
                        current = Some((
                            panel
                                .results()
                                .locate(flat)
                                .and_then(|(s, _)| panel.results().sections.get(s))
                                .map_or_else(String::new, |s| s.title.clone()),
                            vec![element],
                        ))
                    }
                }
            }
        }
    }
    flush(&mut sections, current);
    Element::new(ResultList {
        count: panel.results().len(),
        children: sections,
    })
}

/// The selected result's preview: a picture of it, its name and what is known of it, and its actions at the foot.
fn preview_pane<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let Some(item) = panel.selected_item() else {
        return Element::new(Plain::pane(Vec::new()));
    };
    let preview = view.preview;
    let mut children: Vec<Element<M>> = Vec::new();
    if is_value(item.kind) {
        children.push(
            Element::new(Label::new(&item.title, PREVIEW_VALUE_SIZE, 500).centred(2)).key("value"),
        );
        children.push(
            Element::new(
                Label::new(item.subtitle.trim_end_matches(" ="), 13.0, 400)
                    .secondary()
                    .centred(2),
            )
            .key("expression"),
        );
    } else {
        children.push(preview_visual(view, item, preview).key("visual"));
        children.push(
            Element::new(Label::new(&item.title, PREVIEW_TITLE_SIZE, 600).centred(2)).key("name"),
        );
        let kind = match (preview, item.kind) {
            (Some(p), _) if !p.kind.is_empty() => p.kind.clone(),
            (_, ItemKind::Application) => "Application".to_owned(),
            (_, ItemKind::ApplicationAction) => item.subtitle.clone(),
            (_, ItemKind::WebSearch) => "Search the web".to_owned(),
            _ => String::new(),
        };
        if !kind.is_empty() {
            children.push(
                Element::new(Label::new(&kind, 12.0, 400).secondary().centred(1)).key("kind"),
            );
        }
        if let Some(p) = preview {
            for (label, value) in &p.details {
                children.push(
                    Element::new(Plain::detail(vec![
                        Element::new(
                            Label::new(label, 11.0, 400)
                                .secondary()
                                .ending(DETAIL_LABEL),
                        ),
                        Element::new(Label::new(value, 11.0, 400).lines(1)).fill(),
                    ]))
                    .key(format!("detail:{label}")),
                );
            }
        } else if matches!(item.kind, ItemKind::Application) && !item.subtitle.is_empty() {
            children.push(
                Element::new(Label::new(&item.subtitle, 11.0, 400).secondary().centred(2))
                    .key("description"),
            );
        }
    }
    children.push(Element::new(Plain::spacer()));
    children.push(action_list(view).key("actions"));
    Element::new(Plain::pane(children))
}

/// The picture at the top of the preview: the file's own, the start of its text, or the result's icon.
fn preview_visual<M: 'static>(
    view: &PanelView<'_>,
    item: &ResultItem,
    preview: Option<&Preview>,
) -> Element<M> {
    match preview.map(|p| &p.body) {
        Some(PreviewBody::Image { sources, size }) => {
            return Element::new(Image {
                sources: sources.clone(),
                size: *size,
                fit: ImageFit::Contain,
                radius: PREVIEW_IMAGE_RADIUS,
                label: None,
            })
        }
        Some(PreviewBody::Text(text)) => return Element::new(TextBlock { text: text.clone() }),
        _ => {}
    }
    let size = Size::new(PREVIEW_ICON_SIZE, PREVIEW_ICON_SIZE);
    let sources = preview
        .map(|p| &p.icon)
        .filter(|icon| !icon.is_empty())
        .or_else(|| view.icons.get(&item.id));
    match (sources, fallback_symbol(item.kind)) {
        (Some(sources), _) => Element::new(Image {
            sources: sources.clone(),
            size,
            fit: ImageFit::Contain,
            radius: 0.0,
            label: None,
        }),
        (None, Some(symbol)) => Element::new(Icon {
            name: symbol.to_owned(),
            size: PREVIEW_ICON_SIZE,
            direction: IconDirection::Fixed,
            label: None,
            tint: (item.kind == ItemKind::WebSearch).then_some(ColorRole::SecondaryText),
        }),
        (None, None) => Element::new(Plain::spacer()),
    }
}

/// The selected result's actions as buttons, the chosen one filled; at most [`MAX_ACTIONS`] of them, the chosen one
/// always among them.
fn action_list<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let chosen = panel.action();
    let first = (chosen + 1).saturating_sub(MAX_ACTIONS);
    let buttons = view
        .actions
        .iter()
        .enumerate()
        .skip(first)
        .take(MAX_ACTIONS)
        .map(|(index, action)| {
            Element::new(ActionButton {
                flat: panel.selected(),
                index,
                label: action.label.clone(),
                chosen: index == chosen,
            })
            .key(format!("action:{index}"))
        })
        .collect();
    Element::new(ActionList {
        count: view.actions.len(),
        children: buttons,
    })
}

fn text<M: 'static>(text: &str, role: TextRole, secondary: bool) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(text, TextStyle::new(13.0)),
        role,
        wrap: false,
        secondary,
        selected: false,
    })
}

fn result_row<M>(
    view: &PanelView<'_>,
    item: &ResultItem,
    flat: usize,
    calculator: bool,
    selected: bool,
) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let icon = match (view.icons.get(&item.id), fallback_symbol(item.kind)) {
        (Some(sources), _) => Some(Element::new(Image {
            sources: sources.clone(),
            size: Size::new(ICON_SIZE, ICON_SIZE),
            fit: ImageFit::Contain,
            radius: 0.0,
            label: None,
        })),
        (None, Some(symbol)) => Some(Element::new(Icon {
            name: symbol.to_owned(),
            size: ICON_SIZE,
            direction: IconDirection::Fixed,
            label: None,
            tint: (item.kind == ItemKind::WebSearch).then_some(ColorRole::SecondaryText),
        })),
        // the calculator's value starts at the icon's place
        (None, None) => None,
    };
    let title_role = if calculator {
        TextRole::Title
    } else {
        TextRole::Body
    };
    let mut lines = vec![text(&item.title, title_role, false)];
    if !item.subtitle.is_empty() {
        lines.push(text(&item.subtitle, TextRole::Caption, true));
    }
    let label = Element::new(Quiet::lines(lines)).fill();
    Element::new(ResultRow {
        flat,
        title: item.title.clone(),
        subtitle: item.subtitle.clone(),
        height: if calculator {
            CALCULATOR_ROW_HEIGHT
        } else {
            ROW_HEIGHT
        },
        selected,
        children: icon.into_iter().chain([label]).collect(),
    })
    .key(format!("item:{}", item.id))
}

/// A container's leaf callbacks: layout sizes it.
fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn column(insets: Insets) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        insets,
        ..LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }
}

/// The panel's content: takes the panel's keys before the query field does, fades its children with the appear
/// animation, and closes the panel when its window loses the keyboard.
struct PanelRoot<M> {
    focus: Focus,
    alpha: f64,
    children: Vec<Element<M>>,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for PanelRoot<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            height: SizeSpec::Fill,
            ..column(Insets::default())
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.alpha != old.alpha {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Key(key, _) if cx.phase() == Phase::Capture || cx.node() == cx.target() => {
                match panel_key(key) {
                    Some((k, mods)) if takes_key(k, mods, self.focus) => {
                        cx.emit(M::from(Input::Key(k, mods)));
                        Propagation::Stop
                    }
                    _ => Propagation::Continue,
                }
            }
            // another window took the keyboard
            Event::FocusWithin { gained: false, .. } if !cx.window_active() => {
                cx.emit(M::from(Input::Dismiss));
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    /// The panel's glass (or the panel material's fill without it), its shadow outside its shape, and its thin dark
    /// edge, all at the appear animation's alpha.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let alpha = self.alpha.clamp(0.0, 1.0);
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = CORNER_RADIUS.min(size.height / 2.0);
        let theme = cx.env().theme.clone();
        let dark = theme.preferences.appearance.is_dark();
        let glass = cx.glass(Glass::regular().radius(radius).alpha(alpha));
        if alpha < 1.0 {
            cx.set_group(alpha as f32, Vec::new());
        }
        let shape = Shape::RoundedRect { rect, radius };
        let shadow = if dark {
            SHADOW_ALPHA_DARK
        } else {
            SHADOW_ALPHA_LIGHT
        };
        // the shadow is cast by an opaque copy of the shape that the clip hides, so it never darkens the glass
        let mut ring = rounded_path(rect.outset(2.0 * SHADOW_SIDE), 0.0);
        ring.extend(rounded_path(rect, radius));
        cx.push(PaintOp::PushClip(Shape::Path {
            verbs: ring,
            fill_rule: FillRule::EvenOdd,
        }));
        cx.push(PaintOp::PushLayer {
            opacity: 1.0,
            effects: vec![Effect::DropShadow {
                offset: Point::new(0.0, SHADOW_OFFSET),
                blur: SHADOW_BLUR,
                color: Color::new(0.0, 0.0, 0.0, shadow),
            }],
        });
        cx.push(PaintOp::Fill {
            shape: shape.clone(),
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)),
        });
        cx.push(PaintOp::PopLayer);
        cx.push(PaintOp::PopClip);
        if !glass {
            if let Some(fill) = theme.surface(Surface::Menu).fill {
                cx.push(PaintOp::Fill {
                    shape,
                    paint: Paint::Solid(fill),
                });
            }
        }
        let edge = if dark {
            EDGE_ALPHA_DARK
        } else {
            EDGE_ALPHA_LIGHT
        };
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect {
                rect: rect.outset(EDGE_WIDTH / 2.0),
                radius: radius + EDGE_WIDTH / 2.0,
            },
            width: EDGE_WIDTH,
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, edge)),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// `rect` with its corners rounded by `radius`, as a path of four quarter circles.
fn rounded_path(rect: Rect, radius: f64) -> Vec<PathVerb> {
    // a quarter circle's control points sit this far along its tangents, as a fraction of the radius
    const KAPPA: f64 = 0.552_284_75;
    let (l, t, r, b) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let k = radius * KAPPA;
    vec![
        PathVerb::MoveTo(Point::new(l + radius, t)),
        PathVerb::LineTo(Point::new(r - radius, t)),
        PathVerb::CubicTo(
            Point::new(r - radius + k, t),
            Point::new(r, t + radius - k),
            Point::new(r, t + radius),
        ),
        PathVerb::LineTo(Point::new(r, b - radius)),
        PathVerb::CubicTo(
            Point::new(r, b - radius + k),
            Point::new(r - radius + k, b),
            Point::new(r - radius, b),
        ),
        PathVerb::LineTo(Point::new(l + radius, b)),
        PathVerb::CubicTo(
            Point::new(l + radius - k, b),
            Point::new(l, b - radius + k),
            Point::new(l, b - radius),
        ),
        PathVerb::LineTo(Point::new(l, t + radius)),
        PathVerb::CubicTo(
            Point::new(l, t + radius - k),
            Point::new(l + radius - k, t),
            Point::new(l + radius, t),
        ),
        PathVerb::Close,
    ]
}

/// A row of its children across the panel, centred vertically, the side padding at both ends.
struct Bar<M> {
    height: f64,
    spacing: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Bar<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.height),
            insets: Insets {
                top: 0.0,
                left: SIDE_PADDING,
                bottom: 0.0,
                right: SIDE_PADDING,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: self.spacing,
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
        if (self.height, self.spacing) != (old.height, old.spacing) {
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

/// The results: a list of every result, of which the shown ones are children of their sections.
struct ResultList<M> {
    count: usize,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ResultList<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(LIST_WIDTH),
            ..column(Insets {
                top: LIST_PADDING,
                left: 0.0,
                bottom: LIST_PADDING,
                right: 0.0,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.count != old.count {
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
        cx.role(Role::List);
        cx.name("Results");
        cx.item_count(self.count);
    }
}

/// A section of the results, named by its heading.
struct SectionGroup<M> {
    title: String,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for SectionGroup<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        column(Insets::default())
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.title != old.title {
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
        cx.role(Role::Group);
        cx.name(self.title.clone());
    }
}

/// Lays its children out as `layout` says and leaves them out of the accessibility tree: a section's heading, which
/// its group is named by, and a row's lines, which the row is named and described by.
struct Quiet<M> {
    layout: LayoutSpec,
    children: Vec<Element<M>>,
}

impl<M: 'static> Quiet<M> {
    /// A section's heading: its title set as a section heading at the rows' text inset, on the heading's lower padding.
    fn heading(title: &str) -> Self {
        Self {
            layout: LayoutSpec {
                width: SizeSpec::Fill,
                height: SizeSpec::Fixed(HEADER_HEIGHT),
                insets: Insets {
                    top: 0.0,
                    left: SELECTION_INSET + CELL_INSET,
                    bottom: 6.0,
                    right: SELECTION_INSET + CELL_INSET,
                },
                ..LayoutSpec::new(ContainerKind::Stack {
                    axis: Axis::Vertical,
                    spacing: 0.0,
                    align: Align::Start,
                    justify: Justify::End,
                })
            },
            children: vec![text(title, TextRole::Section, true)],
        }
    }

    /// A row's title over its subtitle.
    fn lines(children: Vec<Element<M>>) -> Self {
        Self {
            layout: LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Start,
                justify: Justify::Center,
            }),
            children,
        }
    }
}

impl<M: 'static> Widget<M> for Quiet<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        self.layout.clone()
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.layout != old.layout {
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

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// One result: its icon and its title over its subtitle, the selection behind them. Hover selects it and a press
/// activates it.
struct ResultRow<M> {
    flat: usize,
    title: String,
    subtitle: String,
    height: f64,
    selected: bool,
    children: Vec<Element<M>>,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for ResultRow<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.height),
            insets: Insets {
                top: 0.0,
                left: SELECTION_INSET + CELL_INSET,
                bottom: 0.0,
                right: SELECTION_INSET + CELL_INSET,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: CELL_INSET,
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
        if self.height != old.height {
            Change::Layout
        } else if (self.flat, &self.title, &self.subtitle, self.selected)
            != (old.flat, &old.title, &old.subtitle, old.selected)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, self.height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(PointerEvent::Motion { .. }, _) if !self.selected => {
                cx.emit(M::from(Input::Hover(self.flat)));
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(M::from(Input::Activate(self.flat)));
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(M::from(Input::Activate(self.flat)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.selected {
            return;
        }
        let size = cx.size();
        let rect = Rect::new(
            SELECTION_INSET,
            0.0,
            (size.width - 2.0 * SELECTION_INSET).max(0.0),
            size.height,
        );
        let Some(color) = cx.env().theme.colors.get(
            ColorRole::SecondarySelectedControlFill,
            StateVariant::NORMAL,
        ) else {
            return;
        };
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect,
                radius: SELECTION_RADIUS,
            },
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ListItem);
        cx.name(self.title.clone());
        if !self.subtitle.is_empty() {
            cx.description(self.subtitle.clone());
        }
        cx.states(AccessibleStates {
            selected: self.selected,
            ..AccessibleStates::default()
        });
        cx.item_index(self.flat);
        cx.action(Action::Click);
    }
}

/// A container laid out as its `layout` says, with nothing of its own to paint; a named group to assistive
/// technology when it has a name.
struct Plain<M> {
    layout: LayoutSpec,
    group: Option<&'static str>,
    children: Vec<Element<M>>,
}

impl<M: 'static> Plain<M> {
    fn stack(axis: Axis, spacing: f64, align: Align, children: Vec<Element<M>>) -> Self {
        Self {
            layout: LayoutSpec::new(ContainerKind::Stack {
                axis,
                spacing,
                align,
                justify: Justify::Start,
            }),
            group: None,
            children,
        }
    }

    /// Its children side by side, taking the room left below the query row.
    fn row(children: Vec<Element<M>>, align: Align) -> Self {
        let mut row = Self::stack(Axis::Horizontal, 0.0, align, children);
        row.layout.width = SizeSpec::Fill;
        row.layout.height = SizeSpec::Fill;
        row
    }

    /// The preview pane: the width the list leaves, its children centred one above the other inside its padding.
    fn pane(children: Vec<Element<M>>) -> Self {
        let mut pane = Self::stack(Axis::Vertical, 6.0, Align::Center, children);
        pane.layout.width = SizeSpec::Fill;
        pane.layout.height = SizeSpec::Fill;
        pane.layout.flex = Flex::grow(1.0);
        pane.layout.insets = Insets {
            top: PREVIEW_PADDING,
            left: PREVIEW_PADDING,
            bottom: PREVIEW_PADDING,
            right: PREVIEW_PADDING,
        };
        pane.group = Some("Preview");
        pane
    }

    /// A detail of the preview: its label, ending at the value's start, then its value.
    fn detail(children: Vec<Element<M>>) -> Self {
        let mut detail = Self::stack(Axis::Horizontal, DETAIL_GAP, Align::Start, children);
        detail.layout.width = SizeSpec::Fill;
        detail
    }

    /// Takes the room its column leaves.
    fn spacer() -> Self {
        let mut spacer = Self::stack(Axis::Vertical, 0.0, Align::Start, Vec::new());
        spacer.layout.height = SizeSpec::Fill;
        spacer
    }
}

impl<M: 'static> Widget<M> for Plain<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        self.layout.clone()
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.layout != old.layout {
            Change::Layout
        } else if self.group != old.group {
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
        if let Some(name) = self.group {
            cx.role(Role::Group);
            cx.name(name);
        }
    }
}

/// Text at a size and weight of its own, in the label or secondary label colour: the preview's name, value and
/// details, which the text roles' sizes do not cover.
#[derive(Clone, Debug, PartialEq)]
struct Label {
    text: String,
    size: f64,
    weight: u16,
    secondary: bool,
    align: TextAlign,
    lines: u32,
    /// A fixed width, for a detail's label.
    width: Option<f64>,
}

impl Label {
    fn new(text: &str, size: f64, weight: u16) -> Self {
        Self {
            text: text.to_owned(),
            size,
            weight,
            secondary: false,
            align: TextAlign::Start,
            lines: 1,
            width: None,
        }
    }

    fn secondary(self) -> Self {
        Self {
            secondary: true,
            ..self
        }
    }

    /// Centred, wrapping onto at most `lines` lines.
    fn centred(self, lines: u32) -> Self {
        Self {
            align: TextAlign::Center,
            lines,
            ..self
        }
    }

    fn lines(self, lines: u32) -> Self {
        Self { lines, ..self }
    }

    /// `width` points wide, its text against the end.
    fn ending(self, width: f64) -> Self {
        Self {
            align: TextAlign::End,
            width: Some(width),
            ..self
        }
    }

    fn laid_out(&self, env: &Environment, width: f64) -> Option<ParagraphLayout> {
        if self.text.is_empty() {
            return None;
        }
        let style = TextStyle {
            weight: FontWeight(self.weight),
            ..TextStyle::new(self.size as f32)
        };
        let paragraph = ParagraphStyle {
            align: self.align,
            wrap: if self.lines > 1 {
                Wrap::Word
            } else {
                Wrap::None
            },
            max_lines: Some(self.lines.max(1)),
            ellipsis: true,
            ..ParagraphStyle::default()
        };
        let text = env
            .text
            .paragraph(&AttributedText::plain(&self.text, style), &paragraph)
            .ok()?;
        Some(text.layout(width.is_finite().then_some(width.max(0.0) as f32)))
    }
}

impl<M: 'static> Widget<M> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: self.width.map_or(SizeSpec::Fill, SizeSpec::Fixed),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (&self.text, self.size, self.weight, self.lines, self.width)
            != (&old.text, old.size, old.weight, old.lines, old.width)
        {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let width = self.width.unwrap_or(constraints.width.max());
        match self.laid_out(cx.env(), width) {
            Some(layout) => Measured {
                size: Size::new(
                    self.width.unwrap_or(layout.size().width),
                    layout.size().height,
                ),
                baseline: layout.lines().first().map(|line| line.baseline),
            },
            None => no_measure(),
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(layout) = self.laid_out(cx.env(), cx.size().width) else {
            return;
        };
        let role = if self.secondary {
            ColorRole::SecondaryText
        } else {
            ColorRole::Text
        };
        let Some(color) = cx.env().theme.colors.get(role, StateVariant::NORMAL) else {
            return;
        };
        glyphs(cx, &layout, Point::ZERO, color);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
    }
}

/// Draw `layout` with its top-left corner at `origin`.
fn glyphs(cx: &mut PaintCx<'_>, layout: &ParagraphLayout, origin: Point, color: Color) {
    for line in layout.lines() {
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                origin: origin + run.origin,
                run: run.run.clone(),
                color,
            });
        }
    }
}

/// The first lines of a text file, small, on a faint card the size of the preview's picture.
#[derive(Clone, Debug, PartialEq)]
struct TextBlock {
    text: String,
}

/// Between the excerpt's card and its text.
const TEXT_BLOCK_PADDING: f64 = 8.0;

impl TextBlock {
    fn laid_out(&self, env: &Environment) -> Option<ParagraphLayout> {
        let style = TextStyle::new(PREVIEW_TEXT_SIZE as f32);
        let paragraph = ParagraphStyle {
            wrap: Wrap::None,
            max_lines: Some(PREVIEW_TEXT_LINES as u32),
            ..ParagraphStyle::default()
        };
        let text = env
            .text
            .paragraph(&AttributedText::plain(&self.text, style), &paragraph)
            .ok()?;
        Some(text.layout(None))
    }
}

impl<M: 'static> Widget<M> for TextBlock {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(PREVIEW_IMAGE.height),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, PREVIEW_IMAGE.height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let card = Shape::RoundedRect {
            rect: Rect::new(0.0, 0.0, size.width, size.height),
            radius: PREVIEW_IMAGE_RADIUS,
        };
        let colors = cx.env().theme.colors.clone();
        if let Some(fill) = colors.get(ColorRole::QuaternaryLabel, StateVariant::NORMAL) {
            cx.push(PaintOp::Fill {
                shape: card.clone(),
                paint: Paint::Solid(Color::new(fill.r, fill.g, fill.b, fill.a * 0.5)),
            });
        }
        let (Some(layout), Some(color)) = (
            self.laid_out(cx.env()),
            colors.get(ColorRole::SecondaryText, StateVariant::NORMAL),
        ) else {
            return;
        };
        cx.push(PaintOp::PushClip(card));
        glyphs(
            cx,
            &layout,
            Point::new(TEXT_BLOCK_PADDING, TEXT_BLOCK_PADDING),
            color,
        );
        cx.push(PaintOp::PopClip);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
    }
}

/// The selected result's actions, one above the other.
struct ActionList<M> {
    count: usize,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ActionList<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: ACTION_GAP,
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
        if self.count != old.count {
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
        cx.role(Role::Group);
        cx.name("Actions");
        cx.item_count(self.count);
    }
}

/// One action of the selected result: a capsule, filled in the accent colour while it is the chosen one, which a
/// press or Return carries out.
#[derive(Clone, Debug, PartialEq)]
struct ActionButton {
    flat: usize,
    index: usize,
    label: String,
    chosen: bool,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for ActionButton {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(ACTION_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, ACTION_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(M::from(Input::RunAction(self.flat, self.index)));
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(M::from(Input::RunAction(self.flat, self.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let colors = cx.env().theme.colors.clone();
        let (fill, text) = if self.chosen {
            (ColorRole::DefaultButtonFill, ColorRole::DefaultButtonText)
        } else {
            (ColorRole::ControlFill, ColorRole::ControlText)
        };
        if let Some(fill) = colors.get(fill, StateVariant::NORMAL) {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: size.height / 2.0,
                },
                paint: Paint::Solid(fill),
            });
        }
        let label = Label::new(&self.label, 13.0, 500).centred(1);
        let (Some(layout), Some(color)) = (
            label.laid_out(cx.env(), (size.width - size.height).max(0.0)),
            colors.get(text, StateVariant::NORMAL),
        ) else {
            return;
        };
        let top = ((size.height - layout.size().height) / 2.0).round();
        glyphs(cx, &layout, Point::new(size.height / 2.0, top), color);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            selected: self.chosen,
            ..AccessibleStates::default()
        });
        cx.item_index(self.index);
        cx.action(Action::Click);
    }
}

/// Fills the catcher window; a press anywhere dismisses the panel.
struct Catcher;

impl<M: From<Input> + Clone + 'static> Widget<M> for Catcher {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(constraints.width.ideal(), constraints.height.ideal()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _) => {
                cx.emit(M::from(Input::Dismiss));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name("Close the launcher");
    }
}

#[cfg(test)]
mod tests;

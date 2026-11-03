//! The launcher's windows as CupertinoKit draws them: the panel (the query field over the sectioned results, on the
//! panel material), the transparent catcher that takes a press outside the panel, and the empty window the process
//! keeps mapped while the panel is hidden.
//!
//! The panel's keys never reach the field: [`PanelRoot`] takes them in the capture phase and sends them to the model
//! as [`Input::Key`], so the panel's keyboard table ([`crate::panel`]) holds whatever the field would do with the key.
//! Every other key goes on to Kit's [`TextField`], which reports the edited query as [`Input::Query`].

use crate::engine::{ItemKind, ResultItem};
use crate::panel::{takes_key, Focus, Key as PanelKey, Mods as PanelMods, Panel};
use cupertino_kit::controls::{
    Icon, IconDirection, Image, ImageFit, ImageSource, Separator, Stack, Text, TextChange,
    TextField, TextRole,
};
use cupertino_kit::core::{
    AccessibleStates, Action, Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx,
    PaintCx, Phase, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{
    DecorationMode, KeyEvent, Mods, MouseButton, PointerEvent, WindowSpec,
};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};
use cupertino_kit::theme::{ColorRole, StateVariant, Surface};
use std::collections::HashMap;

/// The `app_id` of the panel's window, which gives it its layer surface.
pub const APP_ID: &str = "cupertino-launcher";
/// The `app_id` of the window that takes a press outside the shown panel.
pub const CATCHER_APP_ID: &str = "cupertino-launcher.catcher";
/// The `app_id` of the window kept mapped while the panel is hidden.
pub const HIDDEN_APP_ID: &str = "cupertino-launcher.hidden";

/// Panel width (the analysis: 600 pt).
pub const WIDTH: f64 = 600.0;
/// The query row, which is the whole panel while there are no results (the analysis: 52 pt).
pub const COLLAPSED_HEIGHT: f64 = 52.0;
/// The least height of the panel while it shows results (the analysis: 430 pt).
pub const MIN_EXPANDED_HEIGHT: f64 = 430.0;
/// Corner radius of the panel (the analysis: 16 pt).
pub const CORNER_RADIUS: f64 = 16.0;
/// Between the panel's edges and the query field (the analysis: 20 pt, a 560 pt field).
pub const SIDE_PADDING: f64 = 20.0;
/// Above the first row and below the last (approximate).
pub const LIST_PADDING: f64 = 8.0;
/// A section heading (derived from its paddings and one caption line).
pub const HEADER_HEIGHT: f64 = 32.0;
/// A result row (approximate).
pub const ROW_HEIGHT: f64 = 40.0;
/// The calculator's row, whose value is set large (approximate).
pub const CALCULATOR_ROW_HEIGHT: f64 = 64.0;
/// Result icons (the analysis: 32 pt).
pub const ICON_SIZE: f64 = 32.0;
/// The search symbol before the query (approximate).
pub const SEARCH_ICON_SIZE: f64 = 20.0;
/// Between a row's selection and the panel's edges (approximate; with the cell inset it is the side padding).
pub const SELECTION_INSET: f64 = 8.0;
/// Between the selection's edge and the icon (the analysis: 12 pt cell inset).
pub const CELL_INSET: f64 = 12.0;
/// Corner radius of a row's selection (the analysis: 12 pt).
pub const SELECTION_RADIUS: f64 = 12.0;
/// Rows, headings included, shown at once.
pub const MAX_ROWS: usize = 9;
/// Distance of the panel's top edge from the top of the output, as a fraction of its height.
pub const TOP_FRACTION: f64 = 0.2;
/// The query field's placeholder and accessible name.
pub const PLACEHOLDER: &str = "Search";

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
                    calculator: item.kind == ItemKind::Calculator,
                });
            }
            row += 1;
            flat += 1;
        }
    }
    rows
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
        ItemKind::Calculator => None,
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
    pub icons: &'a HashMap<String, ImageSource>,
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

/// The panel's window on the panel material at the appear animation's alpha.
pub fn panel_window<M>(view: &PanelView<'_>) -> Window<M>
where
    M: From<Input> + Clone + 'static,
{
    let height = panel_height(view.panel);
    Window {
        key: WindowKey(Key::from("panel")),
        spec: window_spec("Launcher", APP_ID, Size::new(WIDTH, height)),
        shortcuts: CommandRegistry::new(),
        on_close: Some(M::from(Input::Dismiss)),
        background: WindowBackground::Surface {
            surface: Surface::Menu,
            radius: CORNER_RADIUS,
            alpha: view.alpha,
        },
        content: panel_content(view),
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

/// The query row, then the results below a separator.
pub fn panel_content<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let field = Element::new(TextField {
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
    })
    .key("query")
    .fill();
    let query_row = Element::new(Bar {
        height: COLLAPSED_HEIGHT,
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
        children.push(results(view, &rows).key("results"));
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
        (Some(source), _) => Some(Element::new(Image {
            sources: vec![source.clone()],
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.alpha < 1.0 {
            cx.set_group(self.alpha.clamp(0.0, 1.0) as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A row of its children across the panel, centred vertically, the side padding at both ends.
struct Bar<M> {
    height: f64,
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
        column(Insets {
            top: LIST_PADDING,
            left: 0.0,
            bottom: LIST_PADDING,
            right: 0.0,
        })
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

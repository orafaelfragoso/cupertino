//! The open and save panel: a file chooser's window content, as a view of the family. A sidebar of places beside the
//! open folder's items as a [`FileList`], the Save As field above them when saving, and under them the filters, the
//! choices and the buttons, or the question whether to replace a file. It takes plain values and reports through one
//! function, as the other views do; the program builds the [`FileList`] (its items from the chooser's model and
//! [`crate::filepicker`]) and keeps every value.

use std::rc::Rc;

use cupertino_geometry::Insets;
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, CheckBox, SegmentedControl, Separator, Sidebar, SidebarLook,
    SidebarSection, Spacer, SplitLayout, SplitView, Stack, Text, TextChange, TextField, TextRole,
    WindowDragArea,
};
use cupertino_ui_core::layout::{Align, Axis, ScrollState};
use cupertino_ui_core::{Checked, Element, Key};
use cupertino_ui_text::{AttributedText, ContentPurpose, TextStyle};

use crate::list::FileList;

/// The sidebar's width when the panel opens, and the narrowest its sidebar and its items pane get.
pub const SIDEBAR_WIDTH: f64 = 180.0;
const MIN_SIDEBAR: f64 = 120.0;
const MIN_ITEMS: f64 = 320.0;
/// The Save As field's width.
const SAVE_AS_WIDTH: f64 = 260.0;
/// Between the controls of a row.
const GAP: f64 = 8.0;
/// Around the rows of controls above and below the panes, which makes the bottom row 52 pt tall with a regular
/// control; an approximate design value.
const BAR_INSETS: Insets = Insets {
    top: 12.0,
    left: 16.0,
    bottom: 12.0,
    right: 16.0,
};
/// Above and below the folder's row over the items.
const FOLDER_ROW_INSET: f64 = 6.0;
/// The room above the sidebar's first row and the folder's row when the panel opens: it has no title bar, and its
/// first rows clear the window's round corners; none under the Save As row. An approximate design value.
const TOP_INSET: f64 = 10.0;
/// The body text's size.
const TEXT_SIZE: f32 = 13.0;

/// What the panel reports, besides what its [`FileList`] reports through its own function.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelEvent {
    /// Place `index` of the sidebar, counted over its sections' rows, was chosen.
    Place(usize),
    PlacesScroll(ScrollState),
    /// The Enclosing Folder button.
    Parent,
    /// The divider between the sidebar and the items moved.
    Split(SplitLayout),
    /// The Save As field's text changed.
    Name(TextChange),
    /// Filter `index` was chosen.
    Filter(usize),
    /// Choice `index` was set.
    Choice(usize, ChoiceValue),
    /// The default button, or Return in the Save As field.
    Accept,
    Cancel,
    /// The person answered the question whether to replace a file: replace it.
    Replace,
    /// The person answered the question whether to replace a file: go back to the name.
    KeepEditing,
}

/// What a choice was set to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceValue {
    /// A check box's state.
    Checked(bool),
    /// The option at this index.
    Option(usize),
}

/// A choice the application offers under the items: a check box, or one of several options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelChoice {
    Check {
        label: String,
        checked: bool,
    },
    Pick {
        label: String,
        options: Vec<String>,
        selected: Option<usize>,
    },
}

/// The Save As field: the name, the revision of it the program last accepted, and how many of its bytes are the
/// stem the field selects as it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveAs {
    pub name: String,
    pub revision: u64,
    pub stem: usize,
}

/// The panel's content; it becomes an element with `Element::from`.
pub struct OpenSavePanel<M> {
    /// The sidebar's places, in sections, and the one that is the open folder.
    pub places: Vec<SidebarSection>,
    pub place: Option<usize>,
    pub places_scroll: ScrollState,
    pub split: SplitLayout,
    /// The open folder as the row over the items names it, and whether it has an enclosing folder.
    pub folder: String,
    pub enclosing: bool,
    /// The last folder asked for could not be read: the panel says so over the items of the open one.
    pub unreadable: bool,
    /// The open folder's items.
    pub list: FileList<M>,
    /// The Save As field when saving; `None` when opening.
    pub save_as: Option<SaveAs>,
    /// The filters' names, and the chosen one.
    pub filters: Vec<String>,
    pub filter: Option<usize>,
    pub choices: Vec<PanelChoice>,
    /// The default button's label, and whether it can be pressed now.
    pub accept_label: String,
    pub can_accept: bool,
    /// The name of the file a save would replace, while the person is asked whether to.
    pub replace: Option<String>,
    pub on_event: Rc<dyn Fn(PanelEvent) -> M>,
}

fn text<M: Clone + 'static>(content: String, role: TextRole, secondary: bool) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(TEXT_SIZE)),
        role,
        wrap: false,
        secondary,
        selected: false,
    })
}

fn button<M: Clone + 'static>(
    label: String,
    role: ButtonRole,
    enabled: bool,
    on_press: M,
) -> Element<M> {
    Element::new(Button {
        label,
        role,
        style: ButtonStyle::Bordered,
        enabled,
        busy: false,
        on_press: Some(on_press),
    })
}

fn row<M: Clone + 'static>(padding: Insets, children: Vec<Element<M>>) -> Element<M> {
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding,
        spacing: GAP,
        align: Align::Center,
        children,
    })
}

fn column<M: Clone + 'static>(top: f64, children: Vec<Element<M>>) -> Element<M> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets {
            top,
            ..Insets::ZERO
        },
        spacing: 0.0,
        align: Align::Stretch,
        children,
    })
}

fn line<M: Clone + 'static>() -> Element<M> {
    Element::new(Separator {
        axis: Axis::Horizontal,
    })
}

/// A handler that reports through the panel's function.
fn report<T: 'static, M: 'static>(
    on_event: &OnEvent<M>,
    event: impl Fn(T) -> PanelEvent + 'static,
) -> Box<dyn Fn(T) -> M> {
    let on_event = Rc::clone(on_event);
    Box::new(move |value| on_event(event(value)))
}

type OnEvent<M> = Rc<dyn Fn(PanelEvent) -> M>;

/// The room above the panes: none under the Save As row, which is the window's top then.
fn top(saving: bool) -> f64 {
    if saving {
        0.0
    } else {
        TOP_INSET
    }
}

fn sidebar<M: Clone + 'static>(
    places: Vec<SidebarSection>,
    place: Option<usize>,
    scroll: ScrollState,
    top: f64,
    on_event: &OnEvent<M>,
) -> Element<M> {
    let accent = vec![true; places.len()];
    let sidebar = Sidebar {
        label: "Places".into(),
        sections: places,
        selected: place,
        scroll,
        on_scroll: report(on_event, PanelEvent::PlacesScroll),
        on_select: Some(report(on_event, PanelEvent::Place)),
        on_accessory: None,
    }
    .with_look(SidebarLook::Neutral { accent })
    .key("places");
    column(top, vec![sidebar])
}

/// The folder's row, the words for a folder that cannot be read, and the items.
fn items<M: Clone + 'static>(
    folder: String,
    enclosing: bool,
    unreadable: bool,
    list: Element<M>,
    top: f64,
    on_event: &OnEvent<M>,
) -> Element<M> {
    // the row at the top of the pane moves the window, which has no title bar
    let folder = row(
        Insets {
            top: FOLDER_ROW_INSET + top,
            left: GAP,
            bottom: FOLDER_ROW_INSET,
            right: GAP,
        },
        vec![
            button(
                "Enclosing Folder".to_owned(),
                ButtonRole::Normal,
                enclosing,
                on_event(PanelEvent::Parent),
            )
            .key("parent"),
            text(folder, TextRole::Caption, true).key("folder"),
        ],
    );
    let mut children = vec![Element::from(WindowDragArea { content: folder })];
    if unreadable {
        children.push(
            text(
                "This folder can't be opened.".to_owned(),
                TextRole::Caption,
                true,
            )
            .key("unreadable"),
        );
    }
    children.push(list.key("items"));
    column(0.0, children)
}

fn save_row<M: Clone + 'static>(save_as: SaveAs, on_event: &OnEvent<M>) -> Element<M> {
    let row = row(
        BAR_INSETS,
        vec![
            Element::new(Spacer { min: 0.0 }),
            text("Save As:".to_owned(), TextRole::Body, false),
            Element::new(TextField {
                label: "Save As".into(),
                placeholder: String::new(),
                selection: Some(0..save_as.stem.min(save_as.name.len())),
                value: save_as.name,
                revision: save_as.revision,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: Some(report(on_event, PanelEvent::Name)),
                on_submit: Some(on_event(PanelEvent::Accept)),
                initial: true,
            })
            .key("name")
            .width(SAVE_AS_WIDTH),
            Element::new(Spacer { min: 0.0 }),
        ],
    );
    // the window's top row moves the window: it has no title bar
    Element::from(WindowDragArea { content: row })
}

/// The question whether to replace the file `name`, and its answers.
fn question<M: Clone + 'static>(name: &str, on_event: &OnEvent<M>) -> Element<M> {
    bar(
        vec![text(
            format!("“{name}” already exists. Do you want to replace it?"),
            TextRole::Body,
            false,
        )
        .key("replace-question")],
        vec![
            button(
                "Cancel".to_owned(),
                ButtonRole::Normal,
                true,
                on_event(PanelEvent::KeepEditing),
            )
            .key("keep"),
            button(
                "Replace".to_owned(),
                ButtonRole::Destructive,
                true,
                on_event(PanelEvent::Replace),
            )
            .key("replace"),
        ],
    )
}

/// The row under the panes: `leading` at its start, `trailing` at its end.
fn bar<M: Clone + 'static>(mut leading: Vec<Element<M>>, trailing: Vec<Element<M>>) -> Element<M> {
    leading.push(Element::new(Spacer { min: GAP }));
    leading.extend(trailing);
    row(BAR_INSETS, leading)
}

/// The filters and the choices.
fn controls<M: Clone + 'static>(
    filters: Vec<String>,
    filter: Option<usize>,
    choices: Vec<PanelChoice>,
    on_event: &OnEvent<M>,
) -> Vec<Element<M>> {
    let mut controls = Vec::new();
    if !filters.is_empty() {
        controls.push(
            Element::new(SegmentedControl {
                label: "Format".into(),
                segments: filters,
                selected: filter,
                enabled: true,
                on_select: Some(report(on_event, PanelEvent::Filter)),
            })
            .key("filters"),
        );
    }
    for (index, choice) in choices.into_iter().enumerate() {
        let element = match choice {
            PanelChoice::Check { label, checked } => Element::new(CheckBox {
                label,
                checked: if checked {
                    Checked::True
                } else {
                    Checked::False
                },
                enabled: true,
                on_toggle: Some(report(on_event, move |on| {
                    PanelEvent::Choice(index, ChoiceValue::Checked(on))
                })),
            }),
            PanelChoice::Pick {
                label,
                options,
                selected,
            } => Element::new(SegmentedControl {
                label,
                segments: options,
                selected,
                enabled: true,
                on_select: Some(report(on_event, move |option| {
                    PanelEvent::Choice(index, ChoiceValue::Option(option))
                })),
            }),
        };
        controls.push(element.key(Key::Int(index as u64)));
    }
    controls
}

impl<M: Clone + 'static> From<OpenSavePanel<M>> for Element<M> {
    fn from(panel: OpenSavePanel<M>) -> Self {
        let OpenSavePanel {
            places,
            place,
            places_scroll,
            split,
            folder,
            enclosing,
            unreadable,
            list,
            save_as,
            filters,
            filter,
            choices,
            accept_label,
            can_accept,
            replace,
            on_event,
        } = panel;
        let top = top(save_as.is_some());
        // an open panel opens on its items; a save on its Save As field
        let list = list.element(save_as.is_none());
        let browser = Element::from(SplitView {
            label: "Sidebar".into(),
            axis: Axis::Horizontal,
            leading: sidebar(places, place, places_scroll, top, &on_event),
            trailing: items(folder, enclosing, unreadable, list, top, &on_event),
            layout: split,
            min_leading: MIN_SIDEBAR,
            max_leading: f64::INFINITY,
            min_trailing: MIN_ITEMS,
            on_change: Some(report(&on_event, PanelEvent::Split)),
        })
        .key("browser");
        let mut children = Vec::new();
        if let Some(save_as) = save_as {
            children.push(save_row(save_as, &on_event));
            children.push(line());
        }
        children.push(browser);
        children.push(line());
        children.push(match replace {
            Some(name) => question(&name, &on_event),
            None => bar(
                controls(filters, filter, choices, &on_event),
                vec![
                    button(
                        "Cancel".to_owned(),
                        ButtonRole::Normal,
                        true,
                        on_event(PanelEvent::Cancel),
                    )
                    .key("cancel"),
                    button(
                        accept_label,
                        ButtonRole::Default,
                        can_accept,
                        on_event(PanelEvent::Accept),
                    )
                    .key("accept"),
                ],
            ),
        });
        column(0.0, children)
    }
}

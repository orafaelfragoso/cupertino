//! The standard CupertinoKit controls. Each is a [`cupertino_ui_core::Widget`] that reads its look from the theme
//! tokens, takes values from the application's model and reports changes as messages.

mod badge;
mod button;
mod checkbox;
mod choice;
mod dialog;
mod disclosure;
mod editor;
mod field;
mod icon;
mod list;
mod look;
mod menu;
mod navigation;
mod pages;
mod popover;
mod press;
mod progress;
mod scroll;
mod sidebar;
mod slider;
mod split;
mod stack;
mod stepper;
mod tabs;
mod text;
mod toggle;
mod tooltip;
mod window_group;

pub use editor::EditorState;
pub use field::{FieldState, SecureState};
pub use list::ListState;
pub use press::Press;
pub use scroll::{BarState, ViewportState};
pub use slider::SliderState;
pub use split::DividerState;
pub use stepper::StepperState;
pub use tooltip::{TooltipChange, TooltipState};

use cupertino_geometry::{Insets, Point};
use cupertino_ui_core::{Checked, CommandRegistry, Element, Key, WindowBackground, WindowKey};
use cupertino_ui_layout::{Align, Axis, ScrollState};
use cupertino_ui_platform::WindowSpec;
use cupertino_ui_text::{AttributedText, ContentPurpose, SecureBuffer};
use cupertino_ui_theme::ColorRole;
use std::ops::Range;

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
    /// Drawn in the secondary text colour, as a list row's second line.
    pub secondary: bool,
    /// On a selected row: drawn in the selected control text colour, whatever `secondary` says.
    pub selected: bool,
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

/// A number stepped by an up and a down arrow, as beside a field; one Tab stop. The arrow that would pass a limit is
/// disabled.
pub struct Stepper<M> {
    /// The accessible name.
    pub label: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub enabled: bool,
    pub on_change: Option<Handler<f64, M>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DisclosureStyle {
    /// A triangle that points along the reading direction while closed and down while open.
    #[default]
    Triangle,
    /// A bordered button with a chevron, for revealing a section.
    Bordered,
}

/// Shows or hides a region, which the application lays out from `expanded`.
pub struct DisclosureButton<M> {
    /// The accessible name.
    pub label: String,
    pub expanded: bool,
    pub style: DisclosureStyle,
    pub enabled: bool,
    /// Asked with the new state.
    pub on_toggle: Option<Handler<bool, M>>,
}

/// A count or a dot on another control, as unread items on a sidebar row. It is no node of its own to accessibility:
/// its description joins that of the control it sits in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Badge {
    /// The count's text, such as `3` or `99+`; `None` draws a dot.
    pub text: Option<String>,
    /// What it says to assistive technology, such as "3 unread".
    pub description: String,
}

/// Which of several pages shows, as a row of dots; one Tab stop, Left and Right change the page and a click picks
/// one.
pub struct PageDots<M> {
    /// The accessible name.
    pub label: String,
    pub count: usize,
    pub current: usize,
    pub enabled: bool,
    pub on_select: Option<Handler<usize, M>>,
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

/// A row of tabs over a panel showing the selected tab's content; one Tab stop, the arrows select. Only `content`,
/// the selected tab's, is mounted.
pub struct Tabs<M> {
    /// The tab list's accessible name.
    pub label: String,
    pub tabs: Vec<String>,
    pub selected: usize,
    pub content: Element<M>,
    pub on_select: Option<Handler<usize, M>>,
}

pub struct Toggle<M> {
    pub label: String,
    pub on: bool,
    pub enabled: bool,
    pub on_toggle: Option<Handler<bool, M>>,
}

/// A short tip for its anchor: shown after the pointer rests on the anchor or the keyboard focus comes to it, and
/// dismissed by a press, the focus leaving, the pointer leaving both anchor and tip, or Escape wherever the focus is
/// unless a field there is composing text. The text is the anchor's description to assistive technology whether the
/// tip shows or not. Key the element, and declare [`Tooltip::window`] with the other windows while the state is shown.
pub struct Tooltip<M> {
    pub text: String,
    pub anchor: Element<M>,
    pub state: TooltipState,
    /// Sent with each change; apply it to the state with [`TooltipState::apply`].
    pub on_change: Handler<TooltipChange, M>,
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
    /// What the field selects each time it takes `value`, in bytes of it, such as a file name's stem; `None` leaves
    /// the caret at the end.
    pub selection: Option<Range<usize>>,
}

/// A multi-line text editor without chrome, as tall as its text: a vertical [`ScrollView`] around it scrolls it.
/// Return inserts a line break and Tab moves the focus.
pub struct TextEditor<M> {
    /// The accessible name, separate from the placeholder.
    pub label: String,
    pub placeholder: String,
    pub value: String,
    /// The revision of `value` the model last accepted, as a [`TextField`]'s.
    pub revision: u64,
    pub enabled: bool,
    pub on_change: Option<Handler<TextChange, M>>,
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

/// How a scroll view shows where it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scroller {
    /// Thin thumbs over the content that show while it moves and fade a second after.
    #[default]
    Overlay,
    /// Tracks beside the content, always shown, that a click pages and a drag moves.
    Always,
}

/// Content larger than its frame, clipped and moved by `state.offset`, which the model keeps: the view reports its
/// size and every move through `on_scroll`. It is a Tab stop only when nothing in it is; keys from a focused child
/// that the child does not use scroll it. Build its element with `Element::from`.
pub struct ScrollView<M> {
    pub content: Element<M>,
    pub state: ScrollState,
    pub horizontal: bool,
    pub vertical: bool,
    pub scroller: Scroller,
    pub on_scroll: Handler<ScrollState, M>,
}

/// Where a navigation stack's back and forward buttons ask to go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Navigation {
    Back,
    Forward,
}

/// A page under a back and forward button group, which the model's history enables. Primary+[ and Primary+] go back
/// and forward from anywhere in the page; Escape does not go back. The page changes at once, without a transition.
/// Build its element with `Element::from`.
pub struct NavigationStack<M> {
    pub content: Element<M>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub on_navigate: Option<Handler<Navigation, M>>,
}

/// One of a split view's two panes: the leading one is at the top, or at the reading direction's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SplitPane {
    Leading,
    Trailing,
}

/// Where a split view's divider stands, as the model keeps it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitLayout {
    /// The leading pane's extent along the split axis. A collapsed pane keeps it, so restoring returns there.
    pub position: f64,
    pub collapsed: Option<SplitPane>,
}

/// Two panes along `axis` with a divider between them, which a drag or the keyboard moves. Dragging a pane below half
/// its minimum collapses it; Return collapses the leading pane or restores a collapsed one. The divider is one Tab
/// stop; a collapsed pane's content is not built. Build its element with `Element::from`.
pub struct SplitView<M> {
    /// The divider's accessible name.
    pub label: String,
    /// Horizontal puts the panes side by side.
    pub axis: Axis,
    pub leading: Element<M>,
    pub trailing: Element<M>,
    pub layout: SplitLayout,
    pub min_leading: f64,
    pub min_trailing: f64,
    pub on_change: Option<Handler<SplitLayout, M>>,
}

/// Windows of one kind, one per open instance (a document, a viewer): Primary+N in any of them sends `on_new`, and
/// Primary+W or the close button sends `on_close` with the id of the window it is pressed in. Each window's key is
/// the group's key and its id, so a window keeps its state while others open and close. New windows open at their
/// default size, where the compositor places them. Declare [`WindowGroup::windows`] with the other windows.
pub struct WindowGroup<M> {
    pub key: String,
    pub windows: Vec<GroupWindow<M>>,
    pub on_new: M,
    pub on_close: Handler<u64, M>,
}

/// One window of a [`WindowGroup`].
pub struct GroupWindow<M> {
    pub id: u64,
    pub spec: WindowSpec,
    /// The window's own shortcuts; Primary+N and Primary+W are the group's.
    pub shortcuts: CommandRegistry<M>,
    pub background: WindowBackground,
    pub content: Element<M>,
}

/// A panel below a control of another window: a non-modal dialog that takes the keyboard while open, moves the focus
/// to its first focusable item, keeps Tab inside, and closes on Escape or a press outside. Closing gives the focus
/// back to the anchor. It is as large as its content up to 400 by 600 pt; taller content scrolls vertically inside
/// the padding, by `scroll`, which the model keeps like a [`ScrollView`]'s. Declare [`Popover::window`] with the
/// other windows while it is open.
pub struct Popover<M> {
    /// The window the anchor is in.
    pub parent: WindowKey,
    /// The key of the node it opens below.
    pub anchor: Key,
    /// What assistive technology calls it, usually the anchor's label.
    pub label: String,
    pub content: Element<M>,
    pub scroll: ScrollState,
    pub on_scroll: Handler<ScrollState, M>,
    /// Sent on Escape or a press outside; the application then stops declaring the popover.
    pub on_close: M,
}

/// A window's content with a modal sheet over it while `dialog` is set. The content behind takes no pointer, keyboard
/// or window shortcut input and is hidden from assistive technology; the sheet takes the focus, keeps Tab inside, and
/// gives the focus back when it closes. Build its element with `Element::from`.
pub struct Modal<M> {
    pub content: Element<M>,
    pub dialog: Option<Dialog<M>>,
}

/// What assistive technology calls a dialog; it looks the same either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DialogKind {
    #[default]
    Dialog,
    /// Asks about something urgent, usually a confirmation.
    Alert,
}

/// A sheet's title, message, optional controls and buttons. Return activates the default button and Escape the cancel
/// button, wherever the focus is in the sheet, except that a focused text field keeps Return for its own submit.
pub struct Dialog<M> {
    pub kind: DialogKind,
    /// Names the sheet.
    pub title: String,
    /// Below the title in the secondary text colour; describes the sheet. May be empty.
    pub message: String,
    /// Controls between the message and the buttons, such as a field.
    pub body: Option<Element<M>>,
    /// Leading to trailing: the default last, so the first focus lands on the safer choice.
    pub buttons: Vec<DialogButton<M>>,
}

/// What a dialog button does besides sending its message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DialogAction {
    #[default]
    Other,
    /// Return activates it.
    Default,
    /// Escape activates it; without one, Escape does nothing.
    Cancel,
    /// Drawn as destructive; never the default.
    Destructive,
}

pub struct DialogButton<M> {
    pub label: String,
    pub action: DialogAction,
    pub on_press: M,
}

/// A row of a [`Menu`].
pub enum MenuItem<M> {
    /// Chosen by a click, Return or Space, which sends `on_select` and closes the menu.
    Item {
        label: String,
        enabled: bool,
        /// Drawn with a check mark: the option a pop-up button shows, or a setting that is on.
        checked: bool,
        on_select: M,
    },
    /// Opens a menu of `items` beside itself on a click, Return, Space or the arrow toward the trailing side.
    Submenu {
        label: String,
        enabled: bool,
        items: Vec<MenuItem<M>>,
    },
    Separator,
}

/// Where a [`Menu`] opens in its parent window.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAnchor {
    /// Below the node with this key: a pop-up button's menu.
    Below(Key),
    /// At a point, opening down and to the trailing side: a context menu.
    At(Point),
}

/// A menu in a popup window that takes the keyboard while open. Up, Down, Home and End move between the enabled items,
/// a typed character moves to the next item it starts, and Escape or the arrow toward the leading side closes a
/// submenu. Escape in the menu itself, a chosen item and a press outside send `on_close`. Declare [`Menu::windows`]
/// with the other windows while it is open.
pub struct Menu<M> {
    /// The window the anchor is in.
    pub parent: WindowKey,
    pub anchor: MenuAnchor,
    /// What assistive technology calls it, usually the anchor's label.
    pub label: String,
    pub items: Vec<MenuItem<M>>,
    /// The item that has the focus and the highlight when the menu opens, instead of the first: a pop-up button's
    /// chosen option.
    pub current: Option<usize>,
    /// The open submenus: the index of each one's item in the menu before it.
    pub open: Vec<usize>,
    /// Asks for other submenus open, as a new `open`.
    pub on_open: Handler<Vec<usize>, M>,
    pub on_close: M,
}

/// A region with a context menu: a secondary click, the Menu key or Shift+F10 sends `on_open` with where the menu
/// opens, in window coordinates, and the application then declares a [`Menu`] anchored there.
pub struct ContextMenu<M> {
    pub content: Element<M>,
    pub on_open: Option<Handler<Point, M>>,
}

/// A button that shows the chosen option and opens a menu of the options below itself on a click, Space, Return, Up
/// or Down. Give its element a key and declare [`PopUpButton::menu`] while `open`.
pub struct PopUpButton<M> {
    /// The button's accessible name; its value is the chosen option.
    pub label: String,
    pub options: Vec<String>,
    pub selected: usize,
    pub open: bool,
    pub enabled: bool,
    pub on_open: Option<M>,
}

/// The selected rows of a list, as indices into the rows as currently shown, and the row the keyboard is on. When the
/// application sorts or changes its rows, it maps the selection to the new indices itself, or [`clamp`]s it.
///
/// [`clamp`]: Self::clamp
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Selection {
    /// Sorted, disjoint and never touching, so selecting every row of any list is one range.
    ranges: Vec<Range<usize>>,
    cursor: Option<usize>,
    /// Where a Shift extension starts.
    anchor: Option<usize>,
}

/// A long list of rows of one height, of which only those near the viewport are built, so scrolling and a change cost
/// the same for any number of rows. One Tab stop at the cursor row; the arrows, Home, End and the page keys move the
/// cursor and select, and typing selects the next row whose text starts with what was typed. Build its element with
/// `Element::from`.
pub struct ListView<M> {
    /// The accessible name.
    pub label: String,
    pub count: usize,
    pub row_height: f64,
    pub scroll: ScrollState,
    pub selection: Selection,
    /// More than one row can be selected: Shift extends, Primary-click toggles, Primary+A selects all.
    pub multiple: bool,
    /// Row `index`'s content; `selected` asks for the selected text colours ([`Text::selected`]).
    pub row: Box<dyn Fn(usize, bool) -> Element<M>>,
    /// Row `index`'s item's identity, unique in the list: its focus and accessible object follow the item while rows
    /// come and go around it. Without it a row is known by its index.
    pub key: Option<Box<dyn Fn(usize) -> Key>>,
    /// Row `index`'s text for typing to select; without it typing selects nothing.
    pub type_select: Option<Box<dyn Fn(usize) -> String>>,
    pub on_scroll: Handler<ScrollState, M>,
    pub on_select: Option<Handler<Selection, M>>,
    /// Return on the cursor row.
    pub on_activate: Option<Handler<usize, M>>,
}

/// One shown row of a tree, as the application's outline describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TreeItem {
    /// 0 at the top level.
    pub level: usize,
    /// Whether its children are shown; `None` for a row without children.
    pub expanded: Option<bool>,
    /// Its place among its siblings, from 0.
    pub position: usize,
    /// How many siblings it has, itself included.
    pub siblings: usize,
}

/// A request to show or hide a tree row's children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TreeToggle {
    pub index: usize,
    pub expanded: bool,
    /// Every row under it too.
    pub recursive: bool,
}

/// A tree of rows of one height, given as its shown rows in order: a row's children follow it while it is expanded.
/// Built and navigated as a [`ListView`], with each row indented by its level after a disclosure triangle. Right
/// opens a row or moves to its first child, Left closes it or moves to its parent, mirrored in right-to-left
/// layouts; with Primary they open or close every row under it. Build its element with `Element::from`.
pub struct TreeView<M> {
    /// The accessible name.
    pub label: String,
    /// The shown rows.
    pub count: usize,
    pub row_height: f64,
    pub scroll: ScrollState,
    pub selection: Selection,
    pub multiple: bool,
    pub item: Box<dyn Fn(usize) -> TreeItem>,
    /// Row `index`'s content, after its indent and triangle.
    pub row: Box<dyn Fn(usize, bool) -> Element<M>>,
    pub type_select: Option<Box<dyn Fn(usize) -> String>>,
    pub on_scroll: Handler<ScrollState, M>,
    pub on_select: Option<Handler<Selection, M>>,
    pub on_activate: Option<Handler<usize, M>>,
    pub on_toggle: Option<Handler<TreeToggle, M>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    Fixed(f64),
    /// A share of the width the fixed columns leave, equal among such columns.
    Fill,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub title: String,
    pub width: ColumnWidth,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

/// A [`ListView`] whose rows lay their cells out in columns under a header. A click on a column's title asks to sort
/// by it, the other way round when it is the sorted column already. It scrolls vertically; the columns share its
/// width. Build its element with `Element::from`.
pub struct TableView<M> {
    /// The accessible name.
    pub label: String,
    pub columns: Vec<Column>,
    pub sort: Option<(usize, SortOrder)>,
    pub on_sort: Option<Handler<(usize, SortOrder), M>>,
    pub count: usize,
    pub row_height: f64,
    pub scroll: ScrollState,
    pub selection: Selection,
    pub multiple: bool,
    /// The cell of row `index` in column `column`, given as `(index, column, selected)`.
    pub cell: Box<dyn Fn(usize, usize, bool) -> Element<M>>,
    pub type_select: Option<Box<dyn Fn(usize) -> String>>,
    pub on_scroll: Handler<ScrollState, M>,
    pub on_select: Option<Handler<Selection, M>>,
    pub on_activate: Option<Handler<usize, M>>,
}

/// A row of a [`Sidebar`].
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarItem {
    /// The row's identity, unique in the sidebar: its focus and accessible object follow it while rows come and go.
    pub key: Key,
    pub label: String,
    /// A system symbol name, drawn before the label in the accent colour.
    pub icon: Option<String>,
}

/// A heading and the rows under it.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarSection {
    /// `None` for rows without a heading, as at the top of a sidebar.
    pub title: Option<String>,
    pub items: Vec<SidebarItem>,
}

/// A window's list of places in its leading pane: rows under section headings, one of them selected. One Tab stop at
/// the selected row, else the first; Up, Down, Home and End move the selection across the sections, and a click or
/// assistive technology's Click selects a row. A row that becomes selected is reported once; selecting the selected
/// row reports nothing. Build its element with `Element::from`.
pub struct Sidebar<M> {
    /// The accessible name.
    pub label: String,
    pub sections: Vec<SidebarSection>,
    /// The selected row, counted over every section's rows in order.
    pub selected: Option<usize>,
    pub scroll: ScrollState,
    pub on_scroll: Handler<ScrollState, M>,
    pub on_select: Option<Handler<usize, M>>,
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

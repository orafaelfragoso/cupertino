//! The standard CupertinoKit controls. Each is a [`cupertino_ui_core::Widget`] that reads its look from the theme
//! tokens, takes values from the application's model and reports changes as messages.

mod badge;
mod bar_chart;
mod button;
mod callout;
mod canvas;
mod checkbox;
mod choice;
mod color_well;
mod combo;
mod date_picker;
mod dialog;
mod disclosure;
mod editor;
mod field;
mod form;
mod frame_clock;
mod glass;
mod glass_edge;
mod grid;
mod icon;
mod image;
mod lens;
mod list;
mod look;
mod menu;
mod module;
mod navigation;
mod pages;
mod popover;
mod press;
mod preview;
mod progress;
mod rich_text;
mod scroll;
pub mod scroll_edge;
mod shell;
mod sidebar;
mod slider;
mod split;
mod stack;
mod stepper;
mod swatch;
mod tabs;
mod text;
mod text_box;
pub use bar_chart::{BarChart, ChartBar, ChartColor};
pub use swatch::{Swatch, SwatchFill, SwatchLook};
mod thumbnail;
mod tile_sidebar;
mod toggle;
mod toolbar;
mod tooltip;
mod window_drag;
mod window_group;

pub use button::CompactButton;
pub use callout::CalloutPointer;
pub use choice::{segment_at, ChoiceState};
pub use date_picker::DatePickerState;
pub use editor::EditorState;
pub use field::{caret_blinking, caret_opacity, FieldState, SecureState};
pub use form::{
    FORM_ICON_ROW_HEIGHT, FORM_ICON_TWO_LINE_ROW_HEIGHT, FORM_ROW_HEIGHT,
    FORM_SMALL_ICON_ROW_HEIGHT, FORM_TILE_ROW_HEIGHT,
};
pub use glass::{nested_radius, paint_glass, GlassMaterial, GlassRole};
pub use glass_edge::{paint_glass_edge, GlassEdge};
pub use list::{striped_element, ListState};
pub use press::Press;
pub use scroll::{BarState, ViewportState};
pub use shell::{
    SidebarWindow, PANEL_WINDOW_BACKGROUND, SIDEBAR_PANEL_INSET, SIDEBAR_TOP, WINDOW_CORNER_RADIUS,
};
pub use slider::SliderState;
pub use split::DividerState;
pub use stepper::StepperState;
pub use text_box::TextBoxState;
pub use tile_sidebar::TILE_SIDEBAR_ROW_HEIGHT;
pub use toggle::SwitchState;
pub use toolbar::{
    unobscured_insets, ToolbarButton, ToolbarGlyph, ToolbarGroup, ToolbarItem, ToolbarMenuButton,
    ToolbarPicker, ToolbarSearch, ToolbarTitle, COMPACT_TOOLBAR_ROW, TOOLBAR_CHOICE_INSET,
    TOOLBAR_CHOICE_SEGMENT, TOOLBAR_CONTROL, TOOLBAR_DIVIDER, TOOLBAR_EDGE, TOOLBAR_GAP,
    TOOLBAR_MENU_BUTTON, TOOLBAR_ROW, TOOLBAR_SEGMENT, TOOLBAR_SYMBOL, TOOLBAR_TITLE_GAP,
    TOOLBAR_TITLE_SIZE, TOOLBAR_TITLE_WEIGHT,
};
pub use tooltip::{TooltipChange, TooltipState};

pub use cupertino_ui_core::SortOrder;

use cupertino_geometry::{Insets, Point, Size};
use cupertino_ui_core::{
    Checked, CommandRegistry, Element, Key, PaintCx, WindowBackground, WindowKey,
};
use cupertino_ui_layout::{Align, Axis, ScrollState, Track};
use cupertino_ui_platform::{KeyEvent, WindowSpec};
use cupertino_ui_render::{ImageResource, ResourceId};
use cupertino_ui_text::{AttributedText, ContentPurpose, SecureBuffer};
use cupertino_ui_theme::{ColorRole, ControlSize};
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

/// Turns a key the rows of a [`TableView`] did not take into the application's message, given the row the keyboard is
/// on; `None` lets the key go on.
pub type RowKeyHandler<M> = Box<dyn Fn(usize, &KeyEvent) -> Option<M>>;

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

/// A link in a [`RichText`]: the bytes of its text and where it leads.
pub struct TextLink<M> {
    pub range: Range<usize>,
    pub url: String,
    /// Sent when the link is activated by a click, Return or assistive technology.
    pub on_open: Option<M>,
}

/// Static text whose links are drawn in the link colour, underlined, and are tab stops in text order.
pub struct RichText<M> {
    pub content: AttributedText,
    pub role: TextRole,
    pub wrap: bool,
    pub secondary: bool,
    pub links: Vec<TextLink<M>>,
}

/// How an image fills its box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImageFit {
    /// The whole image, as large as fits, centred.
    #[default]
    Contain,
    /// The whole box, cropped to it, centred.
    Cover,
}

/// One rendition of an image's pixels; `id` names these pixels for good.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSource {
    pub id: ResourceId,
    pub image: ImageResource,
}

/// A picture of a grid of pictures: the `image` cropped to `size` points with rounded corners, `label` as its caption
/// under it when `caption`, and a neutral ring around it when `selected`. One Tab stop; a click, Space or Return sends
/// `on_press`. It takes up `size` and the room the ring needs on every side, and the caption's line under it.
pub struct ThumbnailTile<M> {
    pub label: String,
    pub image: Option<ImageSource>,
    pub size: Size,
    pub radius: f64,
    pub caption: bool,
    pub selected: bool,
    pub enabled: bool,
    pub on_press: Option<M>,
}

/// A raster image drawn from the rendition that best covers its box at the device's scale.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub sources: Vec<ImageSource>,
    pub size: Size,
    pub fit: ImageFit,
    pub radius: f64,
    /// `None` marks the image decorative, so accessibility skips it.
    pub label: Option<String>,
}

/// Draws itself through `paint`, in node coordinates; `children` are layered over the drawing and are its interactive
/// parts, with their own semantics.
pub struct Canvas<M> {
    /// `None` fills the room offered.
    pub size: Option<Size>,
    pub label: String,
    pub paint: Box<dyn Fn(&mut PaintCx<'_>)>,
    pub children: Vec<Element<M>>,
}

/// Lays its children out in tracks, row by row; a child covers more than one cell with [`Element::span`]. Rows past
/// `rows` are sized to their content.
pub struct Grid<M> {
    pub columns: Vec<Track>,
    pub rows: Vec<Track>,
    /// Space between the grid's edges and its cells.
    pub padding: Insets,
    /// Between columns (`width`) and rows (`height`).
    pub spacing: Size,
    pub children: Vec<Element<M>>,
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
    /// An action on glass of its own: a shaded translucent body with a fine bright rim, as against an ordinary
    /// button's quiet fill. On glass already it draws no glass of its own.
    Glass,
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
    /// A panel's [`SliderStyle::Level`]: fill and knob in the switch thumb's neutral colour, with a system symbol inside
    /// the track's minimum end, as a control centre's brightness.
    PanelLevel(&'static str),
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
    /// Asked with the value when a change ends: the pointer lets go after a drag, or a key or assistive technology
    /// moved the value.
    pub on_end: Option<Handler<f64, M>>,
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

/// A [`SegmentedControl`] that stretches to the full width of its row, as one inside a form's group does, its segments
/// equal shares of it.
pub struct FullWidthSegmentedControl<M>(pub SegmentedControl<M>);

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

/// A [`Toggle`] at the small size of a settings form's rows, made with [`Toggle::small`]: a 36 × 16 pt track and a
/// capsule thumb. It behaves and reads to assistive technology as the toggle does.
pub struct SmallToggle<M> {
    pub toggle: Toggle<M>,
}

/// A [`Toggle`] at the size of a settings pane's header, made with [`Toggle::header`]: a 44.5 × 20 pt track and a
/// capsule thumb. It behaves and reads to assistive technology as the toggle does.
pub struct HeaderToggle<M> {
    pub toggle: Toggle<M>,
}

/// How a [`ModuleToggle`] sets out its symbol and text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ModuleLayout {
    /// A round button with the symbol, then the title over the caption; 40 pt high, as wide as it is offered.
    #[default]
    Row,
    /// The symbol over the title, centred, filling a square module; the caption is only its description.
    Compact,
}

/// The switch of one module of a panel, as Wi-Fi in a control centre: a system symbol on a round button that the accent
/// fills while on, with the title and caption beside it. The whole module is the target; Space toggles.
pub struct ModuleToggle<M> {
    pub label: String,
    /// What the module is on, such as a network's name, or its state.
    pub caption: String,
    /// A system symbol name.
    pub symbol: String,
    pub layout: ModuleLayout,
    pub on: bool,
    pub enabled: bool,
    pub on_toggle: Option<Handler<bool, M>>,
}

/// A group of controls set off on a rounded fill, as a module of a panel; not a Tab stop. The label names the group to
/// assistive technology. Build its element with `Element::from`.
pub struct Tile<M> {
    pub label: String,
    pub content: Element<M>,
}

/// A battery's charge: a battery outline filled to `level` from its leading end, red below a tenth. Not a Tab stop.
#[derive(Clone, Debug, PartialEq)]
pub struct BatteryMeter {
    /// The accessible name.
    pub label: String,
    /// From 0 to 1.
    pub level: f64,
    pub charging: bool,
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
    /// Takes the focus when its window or dialog opens.
    pub initial: bool,
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
    /// Set inside a [`TextBox`], whose edge already insets the editor, so that its text starts where a field's does.
    pub bordered: bool,
    pub on_change: Option<Handler<TextChange, M>>,
}

/// A text field's chrome (background, border and focus ring) around `editor`, a [`TextEditor`] with `bordered` set,
/// which it scrolls vertically as a [`ScrollView`] does. The ring shows while the focus inside it came from the
/// keyboard; a disabled box's scroll bar track is disabled too.
pub struct TextBox<M> {
    pub editor: Element<M>,
    pub enabled: bool,
    pub state: ScrollState,
    pub scroller: Scroller,
    pub on_scroll: Handler<ScrollState, M>,
}

/// A password field. Its text stays in the field's own buffer and reaches the application only through its handlers:
/// `on_change` after each edit, for a form whose button submits it, and `on_submit` on Return. Build it with
/// `..SecureField::default()` after the fields you set: the default is an enabled field without a label, handlers,
/// states or limit.
pub struct SecureField<M> {
    pub label: String,
    pub placeholder: String,
    pub enabled: bool,
    pub on_change: Option<SecureHandler<M>>,
    pub on_submit: Option<SecureHandler<M>>,
    /// The password is being checked: assistive technology hears that the field is busy. Typing goes on as usual.
    pub busy: bool,
    /// The password was refused: assistive technology hears that the field's entry is invalid. The application
    /// clears it, usually on the next edit, and shows and says why itself.
    pub invalid: bool,
    /// The most bytes the field holds. Typed or input-method text past it is cut at a grapheme boundary, and a
    /// keystroke with no room left is refused: no edit is reported and nothing is said. A lower limit keeps text
    /// already past it.
    pub max_len: Option<usize>,
    /// Takes the focus when its window or popup opens.
    pub initial: bool,
}

impl<M> Default for SecureField<M> {
    fn default() -> Self {
        SecureField {
            label: String::new(),
            placeholder: String::new(),
            enabled: true,
            on_change: None,
            on_submit: None,
            busy: false,
            invalid: false,
            max_len: None,
            initial: false,
        }
    }
}

pub struct Stack<M> {
    pub axis: Axis,
    /// Space between the stack's edges and its children.
    pub padding: Insets,
    pub spacing: f64,
    pub align: Align,
    pub children: Vec<Element<M>>,
}

/// Children at their natural sizes in rows across the width it is given, as many in each row as fit, `gaps` apart: a
/// grid of thumbnails that follows the window's width.
pub struct Wrap<M> {
    pub gaps: Size,
    pub children: Vec<Element<M>>,
}

/// How a scroll view shows where it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scroller {
    /// Thin thumbs over the content that show while it moves and fade half a second after.
    #[default]
    Overlay,
    /// Tracks beside the content, always shown, that a click pages and a drag moves.
    Always,
}

/// Content larger than its frame, clipped and moved by `state.offset`, which the model keeps: the view reports its
/// size and every move through `on_scroll`. It is a Tab stop only when its content passes its frame and nothing in it is a stop; keys from a focused child
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

/// A page under a 52 pt toolbar row that starts with a back and forward button group, which the model's history enables:
/// a floating glass capsule ([`ToolbarGroup`]) 8 pt from the row's leading edge, the row's other items following 8 pt
/// apart ([`ToolbarTitle`] keeps 12 pt) and the row painting no fill of its own, so the page's background runs under
/// it. The page takes the height the row leaves. Primary+[ and Primary+] go back and forward from anywhere in the page;
/// Escape does not go back. The page changes at once, without a transition. The toolbar row is the window's drag
/// region, as a [`WindowDragArea`] is: a drag on it where no control takes the press moves the window, and a double
/// click there zooms it. Build its element with `Element::from`.
pub struct NavigationStack<M> {
    pub content: Element<M>,
    /// Items after the button group in the toolbar row, in reading order; a [`Spacer`] pushes the ones after it to
    /// the row's end.
    pub toolbar: Vec<Element<M>>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub on_navigate: Option<Handler<Navigation, M>>,
    /// The page fills the whole pane and the toolbar row lies over its top: the page draws under the row and lays out
    /// below it, with [`unobscured_insets`] for its window's decorations (a [`ScrollView`] whose content starts that far
    /// down scrolls behind the row). Without it the page takes the height the row leaves. The window's material and
    /// glass lie under everything it paints, so a page drawn under the row covers the row's glass where it is opaque.
    pub under_toolbar: bool,
}

/// Marks `content` as a drag region of its window, the size of the content (add `.fill()` to the element for all the
/// room its parent gives): a left press there that no control inside takes starts an interactive move once the
/// pointer travels 5 pt with the button held, and a second press within half a second zooms the window (maximizes it,
/// or restores it). Buttons, fields and other controls inside keep their presses; a disabled control or a label lets
/// the press through. The region adds no role and no Tab stop: keyboard users move and zoom windows with the
/// compositor's shortcuts and the menu bar's Window menu. A toolbar built with [`NavigationStack`] already is one.
/// Build its element with `Element::from`.
pub struct WindowDragArea<M> {
    pub content: Element<M>,
}

/// Wraps `content` (the size of the content) and, while `on_frame` holds a message, sends it once at each frame the
/// compositor gives the window: what the application moves by itself steps with the display's frames, never ahead of
/// them and never waiting on a timer between them. A window the compositor does not show gets no frames, so nothing
/// is sent. Build its element with `Element::from`.
pub struct FrameClock<M> {
    pub content: Element<M>,
    pub on_frame: Option<M>,
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
/// stop; a collapsed pane's content is not built. Each pane's content fills the pane. Build its element with
/// `Element::from`.
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
/// to its first focusable item, or to the dialog when it has none, keeps Tab inside, and closes on Escape or a press outside. Closing gives the focus
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

/// What a [`FilePreview`] draws of a file.
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewContent {
    /// A picture, whole and as large as fits.
    Picture(ImageSource),
    /// The start of a text file, set in a fixed-pitch face on the text background.
    Text(String),
    /// The file's icon, centred, at most its largest size.
    Icon(Vec<ImageSource>),
}

/// Where a [`FilePreview`] shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PreviewStyle {
    /// Beside a list of files: the content in a box 200 pt tall, the name, the summary and the details under it, and
    /// the footer pinned to the bottom edge.
    #[default]
    Pane,
    /// A preview window's content: the content alone filling the room; a file shown by its icon has its name, summary
    /// and details beside the icon.
    Window,
}

/// A file's preview, drawn without opening the file in an application. It reads as a group named "Preview of" the
/// file; a picture is read by the file's name, and a text file's content by its text. Build its element with
/// `Element::from`.
pub struct FilePreview<M> {
    pub content: PreviewContent,
    /// The fixed-pitch family a text file's content is set in; the theme's face without it.
    pub text_family: Option<String>,
    pub name: String,
    /// The line under the name, as the kind and size.
    pub summary: String,
    /// Labelled details, as dates: label, value.
    pub details: Vec<(String, String)>,
    /// Under the details, at the bottom edge of a pane, with a hairline above it: an actions button.
    pub footer: Option<Element<M>>,
    pub style: PreviewStyle,
    /// How far a text file's content is scrolled.
    pub scroll: ScrollState,
    pub on_scroll: Handler<ScrollState, M>,
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
    /// The picture an alert shows at its leading edge, named as the system's pictures are; the running application's own icon
    /// when none.
    pub icon: Option<String>,
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
    /// An [`MenuItem::Item`] that shows the keys that choose it from the keyboard, as "⇧⌘N", at its trailing edge in the
    /// secondary text colour; assistive technology reads them as the item's description. The application handles the
    /// keys itself: the menu only shows them.
    Shortcut {
        label: String,
        shortcut: String,
        enabled: bool,
        checked: bool,
        on_select: M,
    },
    /// Opens a menu of `items` beside itself on a click, Return, Space, the arrow toward the trailing side or once the
    /// pointer rests on it.
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
    /// Over the node with this key, the current item's label where the node's label is: a pop-up button's menu.
    Over(Key),
    /// Above the node with this key, `gap` apart, its leading edge `dx` toward the trailing side of the middle of the
    /// node's top edge: a context menu that stands off to one side of what it belongs to.
    Above { key: Key, gap: f64, dx: f64 },
}

/// The look of a [`Menu`]'s panels that differs between menus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuStyle {
    /// The panels' corner radius in points.
    pub radius: f64,
    /// The least width of a panel in points.
    pub min_width: f64,
    /// A pointer under the first panel, joined to it as one piece of glass.
    pub tail: Option<MenuTail>,
}

/// The pointer of a menu panel that points at what opened it: a small piece of the panel's glass under its bottom edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuTail {
    /// From the panel's leading edge to the middle of the pointer, in points.
    pub from_leading: f64,
    /// The pointer's width where it leaves the panel and its depth below the panel, in points.
    pub width: f64,
    pub depth: f64,
}

impl Default for MenuStyle {
    fn default() -> Self {
        MenuStyle {
            radius: menu::PANEL_RADIUS,
            min_width: menu::MIN_WIDTH,
            tail: None,
        }
    }
}

/// A menu in a popup window that takes the keyboard while open. Up, Down, Home and End move between the enabled items,
/// a typed character moves to the next item it starts, and Escape or the arrow toward the leading side closes a
/// submenu. Escape in the menu itself, a chosen item and a press outside send `on_close`. Declare [`Menu::windows`]
/// with the other windows while it is open. The pointer tracks it as it does every menu: a press that opened it and is
/// dragged onto an item chooses the item on release, and the way from a row into its submenu keeps the submenu open.
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

/// An editable field with a button at its end that opens a menu of choices: the field takes any text, and a choice
/// replaces it. Build its element with `Element::from`, keyed, and declare [`ComboBox::menu`] while `open`; the
/// button's press, Space or Return sends `on_open`.
pub struct ComboBox<M> {
    pub field: TextField<M>,
    pub options: Vec<String>,
    pub open: bool,
    pub on_open: Option<M>,
    /// The field's text size in points.
    pub text_size: f64,
}

/// A day of the proleptic Gregorian calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CalendarDate {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to the month's length.
    pub day: u8,
}

/// A part of a [`DatePicker`]'s date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DatePart {
    Year,
    Month,
    Day,
}

/// A date in a field's well, shown `YYYY-MM-DD`, with a stepper beside it. A click picks the year, month or day, as
/// Left and Right do; Up, Down and the stepper step that part, wrapping within its range, and send `on_change` with
/// the new date.
pub struct DatePicker<M> {
    /// The accessible name.
    pub label: String,
    pub date: CalendarDate,
    pub enabled: bool,
    pub on_change: Option<Handler<CalendarDate, M>>,
}

/// A colour in a capsule set in a quiet rim. A press, Space or Return sends `on_open`, for the application to show a
/// colour picker; its accessible value is the colour as `#RRGGBB`.
pub struct ColorWell<M> {
    /// The accessible name.
    pub label: String,
    pub color: cupertino_ui_theme::Color,
    pub enabled: bool,
    pub on_open: Option<M>,
}

/// A button titled with what it does that opens a menu of actions below itself on a press, Space, Return, Up or Down,
/// marked by a single downward chevron; unlike a [`PopUpButton`] it keeps no chosen value. Give its element a key
/// and declare [`PullDownButton::menu`] while `open`.
pub struct PullDownButton<M> {
    /// The title it shows, also its accessible name.
    pub label: String,
    pub actions: Vec<String>,
    pub open: bool,
    pub enabled: bool,
    pub on_open: Option<M>,
}

/// A button that shows the chosen option and opens a menu of the options over itself on a press, Space, Return, Up
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

/// A [`PopUpButton`] as a row of a grouped form shows it: the chosen value as plain text with no bezel, then a round
/// grey disc holding the up and down chevrons. It behaves as a [`PopUpButton`] does and opens the same menu; use the
/// bordered [`PopUpButton`] outside grouped rows. Give its element a key and declare [`InlinePopUpButton::menu`] while
/// it is open.
pub struct InlinePopUpButton<M>(pub PopUpButton<M>);

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
    /// Takes the focus when its window or dialog opens, on its cursor row.
    pub initial: bool,
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
    /// A key that reached the rows and that the table does not take itself, given as `(row, key)` with the row the
    /// keyboard is on: a message takes the key, `None` lets it go on, so Tab moves the focus to the next control. A key
    /// on a column header never comes here.
    pub on_row_key: Option<RowKeyHandler<M>>,
    /// How far a toolbar row over the table covers its top, for a table that fills a pane under the row
    /// ([`NavigationStack::under_toolbar`], [`unobscured_insets`]); 0 for none. The header then sits below the row and
    /// draws with it, raised, and the rows start below the header and scroll under both.
    pub header_top: f64,
}

/// A [`TableView`] whose rows form an outline, as a [`TreeView`]'s do: each row's first cell comes after its indent and
/// disclosure triangle, and Left and Right open and close rows as in a tree. Build its element with `Element::from`.
pub struct OutlineTable<M> {
    pub table: TableView<M>,
    pub item: Box<dyn Fn(usize) -> TreeItem>,
    /// Row `index`'s key, which its focus and accessible object follow while rows come and go above it; its index
    /// without one.
    pub key: Option<Box<dyn Fn(usize) -> Key>>,
    pub on_toggle: Option<Handler<TreeToggle, M>>,
}

/// A row of a [`Sidebar`].
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarItem {
    /// The row's identity, unique in the sidebar: its focus and accessible object follow it while rows come and go.
    pub key: Key,
    pub label: String,
    /// A system symbol name, drawn before the label in the accent colour.
    pub icon: Option<String>,
    /// What assistive technology reads after the label, as "Removable" for a device.
    pub description: Option<String>,
    /// A button at the row's trailing edge, as a device's eject; the sidebar's `on_accessory` takes its presses.
    pub accessory: Option<SidebarAccessory>,
    /// A call on the row's item is running: the row hides its accessory and dims its label.
    pub busy: bool,
}

/// A row's trailing button: a symbol, and the name assistive technology reads, as "Eject Backup".
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarAccessory {
    pub symbol: String,
    pub label: String,
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
    /// A press on a row's accessory, with the row's key; it does not select the row.
    pub on_accessory: Option<Handler<Key, M>>,
}

/// How a [`Sidebar`] draws its rows. `Element::from` draws [`SidebarLook::Accent`]; [`Sidebar::with_look`] picks
/// another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SidebarLook {
    /// The selected row is filled with the accent and its symbol and label take the selected text colour; the other
    /// rows' symbols are the accent.
    #[default]
    Accent,
    /// A file browser's list of places: taller rows with larger symbols, the selected row filled with a neutral grey
    /// and its label in the semibold weight, its symbol in the accent. The other rows' symbols are the accent in each
    /// section `accent` marks `true` (by the sections' order), the secondary label colour in the others and in a section
    /// it does not reach. In a window that is not active every symbol, the selected row's too, is the secondary label
    /// colour. A section heading without rows is drawn alone.
    Neutral { accent: Vec<bool> },
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

/// The colour of an [`IconTile`]: a system colour, drawn with a slight top-to-bottom shade.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TileColor {
    #[default]
    Blue,
    Gray,
    /// Nearly black, as a tile for display and appearance settings.
    Graphite,
    Green,
    Indigo,
    Orange,
    Pink,
    Purple,
    Red,
    Teal,
    Yellow,
}

/// A system symbol in white on a rounded square of a system colour, as before a settings row. Decorative: the row
/// beside it names it.
#[derive(Clone, Debug, PartialEq)]
pub struct IconTile {
    /// A system symbol name.
    pub symbol: String,
    pub color: TileColor,
    /// The side of the square, in points, where it stands alone; a row draws its tile at the row's own size.
    pub size: f64,
}

/// Rows set off on a rounded fill, as a section of a settings form, with an inset hairline between each row and the
/// next. It is no Tab stop and no node of its own to assistive technology: its rows are. Build its element with
/// `Element::from`.
pub struct FormGroup<M> {
    pub rows: Vec<Element<M>>,
}

/// One row of a [`FormGroup`]: an optional icon tile, then `content`, which takes the rest of the row. The row is at
/// least as tall as a form row (taller with a tile) and pads its content on every side. Build its element with
/// `Element::from`.
pub struct FormRow<M> {
    pub tile: Option<IconTile>,
    pub content: Element<M>,
}

/// An installed application's icon, by the `Icon=` name of its desktop entry; the generic application icon where the
/// icon theme has none. Decorative: the row it stands in names it.
#[derive(Clone, Debug, PartialEq)]
pub struct AppIcon {
    pub name: String,
    /// The side of the square, in points.
    pub size: f64,
}

/// One row of a [`FormGroup`] that starts with an application's or category's icon, `size` points across (20, 22 or the
/// standard 24), then `content`, which takes the rest of the row. The row's height follows the icon (42, 43 or 48 pt),
/// and is taller with `two_line` content. Build its element with `Element::from`.
pub struct AppIconRow<M> {
    pub icon: String,
    pub size: f64,
    pub two_line: bool,
    pub content: Element<M>,
}

/// The header of a settings pane that has a master switch or a short description: on a group's rounded fill, a 26 pt
/// icon tile, the title in bold with its description under it in secondary text, and the optional `switch` (a
/// [`Toggle::header`]) at the trailing edge. Build its element with `Element::from`; the large centred [`FormHeader`]
/// is the card of a pane's first page.
pub struct PaneHeader<M> {
    pub tile: IconTile,
    pub title: String,
    pub description: String,
    pub switch: Option<Element<M>>,
}

/// A row of a [`FormGroup`] that opens a page: an optional icon tile, the label and a chevron at the trailing edge.
/// One Tab stop; a click, Return, Space or assistive technology's Click sends `on_open`.
pub struct NavigationRow<M> {
    pub label: String,
    pub tile: Option<IconTile>,
    pub on_open: Option<M>,
}

/// The card at the top of a settings page: a large icon tile over the page's title in bold and a short description,
/// centred on a group's rounded fill. Assistive technology reads the title, described by the description.
#[derive(Clone, Debug, PartialEq)]
pub struct FormHeader {
    pub tile: IconTile,
    pub title: String,
    pub description: String,
}

/// `content` on the colour behind a form's groups (the window background), filling the room it is given. No node of its
/// own to assistive technology. Build its element with `Element::from`.
pub struct FormBackground<M> {
    pub content: Element<M>,
}

/// A [`TextField`] drawn as a search field: a capsule with a magnifying glass before the text, a light one on a
/// material (a sidebar) and a recessed well on the window's content. It behaves, edits and reads to assistive
/// technology as its field does.
pub struct SearchField<M> {
    pub field: TextField<M>,
    /// Its control size, which sets its height from the size table (24 points at the regular size, 28 at the large);
    /// `None` takes the window's.
    pub size: Option<ControlSize>,
}

/// A [`TextField`] without chrome, its text set at `size` points (whole points, 1 to 200): for a field that stands on a
/// surface drawn around it, such as a launcher's search capsule. It behaves, edits and reads to assistive technology
/// as its field does; its placeholder takes the secondary label colour, as a search field's.
pub struct BareField<M> {
    pub field: TextField<M>,
    pub size: f64,
}

/// A row of a [`TileSidebar`].
#[derive(Clone, Debug, PartialEq)]
pub struct TileSidebarItem {
    /// The row's identity, unique in the sidebar: its focus and accessible object follow it while rows come and go.
    pub key: Key,
    pub label: String,
    /// A system symbol name, drawn in white on the tile.
    pub symbol: String,
    pub tile: TileColor,
    /// A row of the person's own account: the picture or initials in a disc stand in the tile's place, and a second
    /// line follows the label.
    pub account: Option<AccountLook>,
}

/// What an account row of a [`TileSidebar`] shows besides its label: the person's picture, or their initials on a
/// disc of the row's tile colour, and a second line in secondary text.
#[derive(Clone, Debug, PartialEq)]
pub struct AccountLook {
    pub subtitle: String,
    pub initials: String,
    pub picture: Option<ImageSource>,
}

/// A settings window's list of pages in its leading pane: rows with a coloured icon tile before the label, in groups
/// set apart by space, one row selected and filled with the accent colour. `header` (a search field) stands above the
/// rows and does not scroll. One Tab stop at the selected row, else the first; Up, Down, Home and End move the
/// selection across the groups, and a click or assistive technology's Click selects a row. A row that becomes selected
/// is reported once; a key that leaves the selection where it is reports nothing, while a press or Click on the
/// selected row reports it again, so that a window showing a page below that row's page can go back to it. Build its
/// element with `Element::from`.
pub struct TileSidebar<M> {
    /// The list's accessible name.
    pub label: String,
    pub header: Option<Element<M>>,
    pub groups: Vec<Vec<TileSidebarItem>>,
    /// The selected row, counted over every group's rows in order.
    pub selected: Option<usize>,
    pub scroll: ScrollState,
    pub on_scroll: Handler<ScrollState, M>,
    pub on_select: Option<Handler<usize, M>>,
}

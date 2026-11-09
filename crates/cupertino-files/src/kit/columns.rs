//! The column view on CupertinoKit: the chain of folders of [`crate::columns`] side by side in a row that scrolls
//! sideways, each column a list of its folder's items, and a preview of the selected file at the end.
//!
//! The column of the folder the window shows is the window's [`Browser`]: its selection, rename, drag, context menu and
//! type-to-select are the ones of the other views. The other columns are listings of their own, opened as the window's
//! folders are and waking the window as they change: the folders before it, each with the next folder of the chain
//! highlighted, and the contents of the selected folder after it. Choosing a row of one of those columns shows its
//! folder in the window with that row selected, handing the listing to the browser and taking the browser's back, so
//! nothing is read again. Right on a selected folder goes into it and selects its first item; Left goes back to the
//! folder before, selecting the folder left.
//!
//! A column's trailing divider resizes it; with Option held it resizes every column. The chain scrolls to its end as
//! the window goes deeper, in whole columns ([`crate::columns::layout`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use cupertino_kit::controls::{
    FilePreview, Icon, IconDirection, Image, ImageFit, ImageSource, ListView, PreviewContent,
    PreviewStyle, ScrollView, Scroller, Selection, Stack, Text, TextRole,
};
use cupertino_kit::core::{
    Change, Element, Event, EventCx, Key, MeasureCx, PaintCx, Phase, Propagation, SemanticsCx,
    Widget,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
    ScrollState, SizeSpec,
};
use cupertino_kit::platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent};
use cupertino_kit::prelude::{Button, ButtonRole, ButtonStyle, Spacer};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::text::{AttributedText, TextStyle};
use cupertino_kit::theme::{ColorRole, StateVariant};

use super::list::RowCell;
use super::view::{self, popup_key, row_key};
use super::{BrowserWindow, Event as WindowEvent, Message, Popup, Session, NO_SCROLL};
use crate::browser::Browser;
use crate::columns::{self, ChainLayout, ColumnChain, PREVIEW_WIDTH, ROW_HEIGHT};
use crate::metrics::window as window_metrics;
use crate::model::DirectoryModel;
use crate::places::window_title;
use crate::preview::{self as file_preview, PreviewKind};
use crate::Entry;

/// The side of a row's icon.
const ROW_ICON: f64 = 16.0;
/// From a row's selection to its icon, and from its chevron to the selection's far end.
const ROW_LEAD: f64 = 8.0;
/// The side of the chevron that ends a folder's row.
const CHEVRON: f64 = 9.0;
/// How far from a column's trailing edge a press resizes it.
const GRIP: f64 = 4.0;
/// The width of the line between two columns.
const DIVIDER: f64 = 1.0;
/// From a column's sides to a row's selection, which the highlight of a folder of the chain matches.
const SELECTION_INSET: f64 = 10.0;
/// From a list row's content box to the selection drawn around it: the list's content inset less
/// [`SELECTION_INSET`], none as the two are the same.
const HIGHLIGHT_OUTSET: f64 = 0.0;
/// The highlight's corner radius, the list's selection's.
const HIGHLIGHT_RADIUS: f64 = 5.0;
/// The side of an icon in the preview column.
const PREVIEW_ICON: f64 = 128.0;
/// The label of the preview column's actions button.
const MORE: &str = "More…";
const LEFT: Keysym = Keysym(0xff51);
const RIGHT: Keysym = Keysym(0xff53);

/// The columns of a window, beyond its browser's.
pub(super) struct ColumnsView {
    pub chain: ColumnChain,
    /// The listings of the chain's other folders, by folder: those before the folder shown, and the selected folder.
    listings: HashMap<PathBuf, DirectoryModel>,
    /// How far each column is scrolled, by folder.
    scrolls: HashMap<PathBuf, ScrollState>,
    /// What the chain's sideways scroll was last put at the end for.
    pinned: Option<Pin>,
    /// How far the preview's text is scrolled.
    pub preview_scroll: ScrollState,
    /// The divider being dragged: the column's place, and whether it sizes every column; kept when it is let go.
    pub resized: Option<(usize, bool)>,
}

/// What decides where the chain scrolls to: when it changes, the chain scrolls to its end again.
#[derive(Clone, Debug, PartialEq)]
struct Pin {
    folders: Vec<PathBuf>,
    child: Option<PathBuf>,
    preview: bool,
    widths: Vec<u64>,
    viewport: u64,
}

/// The columns the window shows now.
pub(super) struct Chain {
    /// From the root to the folder shown.
    pub folders: Vec<PathBuf>,
    /// The selected folder, whose items follow.
    pub child: Option<PathBuf>,
    /// The selected file, previewed at the end: its index in the browser's entries.
    pub preview: Option<usize>,
}

impl Chain {
    fn of(browser: &Browser, chain: &ColumnChain) -> Chain {
        let folders = chain.folders(browser.dir());
        let selection = browser.selection();
        let single = (selection.len() == 1)
            .then(|| selection.indices().next())
            .flatten()
            .and_then(|i| Some((i, browser.entries().get(i)?)));
        let (child, preview) = match single {
            Some((_, e)) if e.is_dir => (Some(e.path.clone()), None),
            Some((i, _)) => (None, Some(i)),
            None => (None, None),
        };
        Chain {
            folders,
            child,
            preview,
        }
    }

    /// The widths of the columns as the chain keeps them, and how many at the end keep theirs.
    fn widths(&self, chain: &ColumnChain) -> (Vec<f64>, usize) {
        let mut widths = chain.widths(self.folders.len() + usize::from(self.child.is_some()));
        if self.preview.is_some() {
            widths.push(PREVIEW_WIDTH);
        }
        (widths, usize::from(self.preview.is_some()))
    }
}

impl ColumnsView {
    pub fn new(default_width: f64) -> ColumnsView {
        ColumnsView {
            chain: ColumnChain::new(default_width),
            listings: HashMap::new(),
            scrolls: HashMap::new(),
            pinned: None,
            preview_scroll: NO_SCROLL,
            resized: None,
        }
    }

    /// The columns of `browser` now.
    pub fn chain(&self, browser: &Browser) -> Chain {
        Chain::of(browser, &self.chain)
    }

    /// Follow the folder `browser` shows and its selection: listings open for new columns and close for columns
    /// gone, in the browser's order. The chain's sideways scroll `scroll` goes to its end when the columns changed.
    pub fn sync(&mut self, browser: &Browser, scroll: &mut ScrollState) {
        self.chain.follow(browser.dir());
        let chain = self.chain(browser);
        let wanted: Vec<&Path> = chain
            .folders
            .iter()
            .map(PathBuf::as_path)
            .filter(|f| *f != browser.dir())
            .chain(chain.child.as_deref())
            .collect();
        self.listings
            .retain(|dir, _| wanted.contains(&dir.as_path()));
        let folders: Vec<&Path> = chain.folders.iter().map(PathBuf::as_path).collect();
        self.scrolls.retain(|dir, _| {
            folders.contains(&dir.as_path()) || chain.child.as_deref() == Some(dir)
        });
        for dir in wanted {
            if let Some(listing) = self.listings.get_mut(dir) {
                if listing.options().sort != browser.sort() {
                    listing.set_sort(browser.sort());
                }
                continue;
            }
            match browser.open_listing(dir) {
                Ok(listing) => {
                    self.listings.insert(dir.to_path_buf(), listing);
                }
                Err(e) => eprintln!("cupertino-files: cannot list {}: {e}", dir.display()),
            }
        }
        let (widths, _) = chain.widths(&self.chain);
        let pin = Pin {
            folders: chain.folders.clone(),
            child: chain.child.clone(),
            preview: chain.preview.is_some(),
            widths: widths.iter().map(|w| w.to_bits()).collect(),
            viewport: scroll.viewport.width.to_bits(),
        };
        if self.pinned.as_ref() != Some(&pin) {
            let layout = self.layout(&chain, scroll.viewport.width);
            scroll.offset = Point::new(layout.offset, 0.0);
            scroll.content = Size::new(layout.content, scroll.viewport.height);
            if pin.preview != self.pinned.as_ref().is_some_and(|p| p.preview) {
                self.preview_scroll = NO_SCROLL;
            }
            self.pinned = Some(pin);
        }
    }

    fn layout(&self, chain: &Chain, viewport: f64) -> ChainLayout {
        let (widths, fixed) = chain.widths(&self.chain);
        columns::layout(&widths, fixed, viewport)
    }

    /// Apply what the listings' threads queued; whether any changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for listing in self.listings.values_mut() {
            changed |= !listing.poll().is_empty();
        }
        changed
    }

    /// The column of `folder` scrolled.
    pub fn scrolled(&mut self, folder: PathBuf, scroll: ScrollState) {
        self.scrolls.insert(folder, scroll);
    }

    fn scroll(&self, folder: &Path) -> ScrollState {
        self.scrolls.get(folder).copied().unwrap_or(NO_SCROLL)
    }

    /// Show the folder of `folder`'s column in `browser`, its row `index` selected; whether it did.
    pub fn pick(&mut self, browser: &mut Browser, folder: &Path, index: usize) -> bool {
        let Some(listing) = self.listings.remove(folder) else {
            return false;
        };
        let Some(name) = listing.entries().get(index).map(|e| e.name.clone()) else {
            self.listings.insert(folder.to_path_buf(), listing);
            return false;
        };
        let left = browser.go_with(listing, Some(&name));
        self.listings.insert(left.dir().to_path_buf(), left);
        true
    }

    /// Right (`by` 1): into the selected folder, selecting its first item; Left (-1): back to the folder before,
    /// selecting the folder left. The item selected then, or `None` when there is nowhere to go.
    pub fn step(&mut self, browser: &mut Browser, by: isize) -> Option<PathBuf> {
        if by > 0 {
            let child = self.chain(browser).child?;
            let first = self.listings.get(&child)?.entries().first()?.name.clone();
            let listing = self.listings.remove(&child)?;
            let left = browser.go_with(listing, Some(&first));
            self.listings.insert(left.dir().to_path_buf(), left);
            return Some(child.join(first));
        }
        let dir = browser.dir().to_path_buf();
        let root = self.chain.root()?;
        if dir == root || !dir.starts_with(root) || browser.is_virtual() {
            return None;
        }
        let parent = dir.parent()?.to_path_buf();
        let name = dir.file_name()?.to_string_lossy().into_owned();
        let listing = match self.listings.remove(&parent) {
            Some(listing) => listing,
            None => browser
                .open_listing(&parent)
                .map_err(|e| eprintln!("cupertino-files: cannot list {}: {e}", parent.display()))
                .ok()?,
        };
        let left = browser.go_with(listing, Some(&name));
        self.listings.insert(left.dir().to_path_buf(), left);
        Some(dir)
    }

    /// The folders of the rows of the columns other than the browser's, by row key, for drops.
    pub fn row_folders(&self, id: u64) -> Vec<(Key, Option<PathBuf>)> {
        let rows = self
            .listings
            .values()
            .flat_map(|listing| listing.entries())
            .map(|e| (row_key(id, &e.path), e.is_dir.then(|| e.path.clone())));
        let columns = self
            .listings
            .keys()
            .map(|dir| (column_key(dir), Some(dir.clone())));
        rows.chain(columns).collect()
    }

    /// The rows of the browser's column in view or near it, its rows starting `under` points down its view (under a
    /// toolbar row).
    pub fn shown(&self, browser: &Browser, margin: f64, under: f64) -> Vec<usize> {
        let scroll = self.scroll(browser.dir());
        let height = match scroll.viewport.height {
            h if h > 0.0 => h,
            _ => window_metrics::DEFAULT_SIZE.1,
        };
        // the rows start below the toolbar row, and show under it too
        let top = (scroll.offset.y - under - margin).max(0.0);
        let bottom = scroll.offset.y - under + height + margin;
        let count = browser.entries().len();
        let first = (top / ROW_HEIGHT).floor() as usize;
        let last = ((bottom / ROW_HEIGHT).ceil() as usize).min(count);
        (first.min(last)..last).collect()
    }
}

/// The column view of window `id`.
pub(super) fn view(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let browser = &win.browser;
    let cols = &win.columns;
    let chain = cols.chain(browser);
    let layout = cols.layout(&chain, win.scroll.viewport.width);
    let mut children = Vec::new();
    let mut place = 0;
    for (i, folder) in chain.folders.iter().enumerate() {
        let column = if folder == browser.dir() {
            active_column(session, id, win)
        } else {
            let next = chain.folders.get(i + 1).or(chain.child.as_ref());
            match cols.listings.get(folder) {
                Some(listing) => other_column(
                    session,
                    id,
                    cols,
                    folder,
                    listing.entries(),
                    next,
                    view::under_row(win),
                ),
                None => Element::new(Spacer { min: 0.0 }),
            }
        };
        children.push(frame(id, place, &layout, column, folder));
        place += 1;
    }
    if let Some(child) = &chain.child {
        let column = match cols.listings.get(child) {
            Some(listing) => other_column(
                session,
                id,
                cols,
                child,
                listing.entries(),
                None,
                view::under_row(win),
            ),
            None => Element::new(Spacer { min: 0.0 }),
        };
        children.push(frame(id, place, &layout, column, child));
        place += 1;
    }
    if let Some(entry) = chain.preview.and_then(|i| browser.entries().get(i)) {
        let width = layout.columns.get(place).map_or(PREVIEW_WIDTH, |c| c.1);
        // the preview starts below the toolbar row
        let pane = Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets {
                top: view::under_row(win),
                ..Insets::ZERO
            },
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![preview_pane(session, id, win, entry).fill()],
        });
        children.push(pane.key("preview").width(width));
    }
    let content = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children,
    })
    .width(layout.content);
    Element::from(ScrollView {
        content,
        state: win.scroll,
        horizontal: true,
        vertical: false,
        scroller: Scroller::Overlay,
        on_scroll: Box::new(move |s| Message::Window(id, WindowEvent::Scroll(s))),
    })
    .key("columns")
}

/// The column at `place` of the chain, `layout` wide, with the divider after it that resizes it.
fn frame(
    id: u64,
    place: usize,
    layout: &ChainLayout,
    column: Element<Message>,
    folder: &Path,
) -> Element<Message> {
    let width = layout.columns.get(place).map_or(0.0, |c| c.1);
    Element::new(ColumnFrame {
        window: id,
        index: place,
        width,
        children: vec![column.fill(), Element::new(Divider).width(DIVIDER)],
    })
    .key(column_key(folder))
    .width(width)
}

/// The key of the column of `folder`.
fn column_key(folder: &Path) -> Key {
    Key::from(format!("column/{}", folder.display()))
}

/// The column of the folder the window shows: the browser's items, selection and rename.
fn active_column(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let browser = &win.browser;
    let dir = browser.dir().to_path_buf();
    let entries: Rc<[Entry]> = browser.entries().into();
    let names: Rc<[String]> = entries.iter().map(|e| e.name.clone()).collect();
    let selection = view::shown_selection(win);
    let cell_selection = selection.clone();
    let presses = Rc::clone(&win.list_presses);
    let icons = Rc::clone(&session.icons);
    let rename = win.rename.as_ref().map(|r| {
        (
            r.edit.index(),
            r.edit.text().to_owned(),
            r.revision,
            r.selection.clone(),
        )
    });
    let keys = Rc::clone(&entries);
    let folder = dir.clone();
    let list = ListView {
        label: window_title(&dir, &session.places),
        count: entries.len(),
        row_height: ROW_HEIGHT,
        scroll: win.columns.scroll(&dir),
        selection,
        multiple: true,
        initial: false,
        row: Box::new(move |row, selected| {
            let entry = &entries[row];
            let name = match rename.as_ref().filter(|r| r.0 == row) {
                Some((_, value, revision, range)) => {
                    view::name_field(id, value.clone(), *revision, range.clone())
                }
                None => name_text(entry.name.clone(), selected),
            };
            let named = Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::ZERO,
                spacing: crate::window::metrics::PATH_ICON_GAP,
                align: Align::Center,
                children: vec![icon(icons.icon(entry, ROW_ICON)), name.fill()],
            })
            .fill();
            let cell = Element::new(RowCell {
                window: id,
                index: row,
                name: true,
                label: Some(entry.name.clone()),
                selection: cell_selection.clone(),
                presses: Rc::clone(&presses),
                children: vec![named],
            })
            .fill();
            row_content(cell, entry.is_dir, selected)
        }),
        key: Some(Box::new(move |row| row_key(id, &keys[row].path))),
        type_select: Some(Box::new(move |row| names[row].clone())),
        on_scroll: Box::new(move |s| {
            Message::Window(id, WindowEvent::ColumnScroll(folder.clone(), s))
        }),
        on_select: Some(Box::new(move |s| {
            Message::Window(id, WindowEvent::Select(s))
        })),
        on_activate: Some(Box::new(move |i| Message::Window(id, WindowEvent::Open(i)))),
    };
    Element::new(ColumnKeys {
        window: id,
        children: vec![list
            .under_element(SELECTION_INSET, view::under_row(win))
            .fill()],
    })
}

/// The column of a folder other than the one shown, `entries` its items; the row of `next`, the folder of the next
/// column, is highlighted.
fn other_column(
    session: &Session,
    id: u64,
    cols: &ColumnsView,
    folder: &Path,
    entries: &[Entry],
    next: Option<&PathBuf>,
    under: f64,
) -> Element<Message> {
    let highlighted = next.and_then(|n| entries.iter().position(|e| &e.path == n));
    let entries: Rc<[Entry]> = entries.into();
    let names: Rc<[String]> = entries.iter().map(|e| e.name.clone()).collect();
    let keys = Rc::clone(&entries);
    let icons = Rc::clone(&session.icons);
    let picked = folder.to_path_buf();
    let pressed = folder.to_path_buf();
    let scrolled = folder.to_path_buf();
    let list = ListView {
        label: window_title(folder, &session.places),
        count: entries.len(),
        row_height: ROW_HEIGHT,
        scroll: cols.scroll(folder),
        selection: Selection::new([], highlighted, highlighted),
        multiple: false,
        initial: false,
        row: Box::new(move |row, _| {
            let entry = &entries[row];
            let named = Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::ZERO,
                spacing: crate::window::metrics::PATH_ICON_GAP,
                align: Align::Center,
                children: vec![
                    icon(icons.icon(entry, ROW_ICON)),
                    name_text(entry.name.clone(), false).fill(),
                ],
            })
            .fill();
            let content = row_content(named, entry.is_dir, false);
            Element::new(PickCell {
                message: Message::Window(
                    id,
                    WindowEvent::ColumnPick {
                        folder: pressed.clone(),
                        index: row,
                        secondary: true,
                    },
                ),
                highlighted: highlighted == Some(row),
                children: vec![content],
            })
            .fill()
        }),
        key: Some(Box::new(move |row| row_key(id, &keys[row].path))),
        type_select: Some(Box::new(move |row| names[row].clone())),
        on_scroll: Box::new(move |s| {
            Message::Window(id, WindowEvent::ColumnScroll(scrolled.clone(), s))
        }),
        on_select: Some(Box::new(move |s: Selection| {
            Message::Window(
                id,
                WindowEvent::ColumnPick {
                    folder: picked.clone(),
                    index: s.cursor().unwrap_or(0),
                    secondary: false,
                },
            )
        })),
        on_activate: None,
    };
    list.under_element(SELECTION_INSET, under)
}

/// A row: its icon and name, then a folder's chevron at the far end.
fn row_content(named: Element<Message>, folder: bool, selected: bool) -> Element<Message> {
    let mut children = vec![named];
    if folder {
        children.push(Element::new(Icon {
            name: "symbol.chevron.right".into(),
            size: CHEVRON,
            direction: IconDirection::Semantic,
            label: None,
            tint: Some(if selected {
                ColorRole::SelectedControlText
            } else {
                ColorRole::SecondaryText
            }),
        }));
    }
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: 0.0,
            left: ROW_LEAD,
            bottom: 0.0,
            right: ROW_LEAD,
        },
        spacing: crate::window::metrics::PATH_ICON_GAP,
        align: Align::Center,
        children,
    })
    .fill()
}

fn icon(sources: Vec<ImageSource>) -> Element<Message> {
    Element::new(Image {
        sources,
        size: Size::new(ROW_ICON, ROW_ICON),
        fit: ImageFit::Contain,
        radius: 0.0,
        label: None,
    })
}

fn name_text(name: String, selected: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(name, TextStyle::new(13.0)),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected,
    })
}

/// The preview column of the selected file `entry`, which the gallery shows beside its pictures too.
pub(super) fn preview_pane(
    session: &Session,
    id: u64,
    win: &BrowserWindow,
    entry: &Entry,
) -> Element<Message> {
    // the glyph over its label, both in the secondary text colour
    let more = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Center,
        children: vec![
            Element::new(MoreGlyph),
            Element::new(Button {
                label: MORE.into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Borderless,
                enabled: true,
                busy: false,
                on_press: Some(Message::Window(id, WindowEvent::Popup(Popup::More))),
            })
            .key(popup_key(Popup::More)),
        ],
    });
    Element::from(FilePreview {
        text_family: session.text_family.clone(),
        content: preview_content(
            session,
            &win.texts,
            entry,
            super::peek::PICTURE,
            PREVIEW_ICON,
        ),
        name: entry.name.clone(),
        summary: file_preview::summary(entry),
        details: file_preview::details(entry, win.now)
            .into_iter()
            .map(|(label, value)| (label.to_owned(), value))
            .collect(),
        footer: Some(more),
        style: PreviewStyle::Pane,
        scroll: win.columns.preview_scroll,
        on_scroll: Box::new(move |s| Message::Window(id, WindowEvent::PreviewScroll(s))),
    })
}

/// What a preview shows of `entry`: its picture `picture` points square once its thumbnail is made, the start of its
/// text once read, else its icon `icon` points square.
pub(super) fn preview_content(
    session: &Session,
    texts: &super::peek::Texts,
    entry: &Entry,
    picture: f64,
    icon: f64,
) -> PreviewContent {
    let content = match file_preview::kind(entry) {
        PreviewKind::Picture => session
            .icons
            .thumbnail(entry, picture)
            .map(PreviewContent::Picture),
        PreviewKind::Text => texts.get(&entry.path).map(PreviewContent::Text),
        PreviewKind::Icon => None,
    };
    content.unwrap_or_else(|| PreviewContent::Icon(session.icons.icon(entry, icon)))
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn filling(axis: Axis) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        height: SizeSpec::Fill,
        flex: cupertino_kit::layout::Flex::grow(1.0),
        ..LayoutSpec::new(ContainerKind::Stack {
            axis,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }
}

/// Left and Right in the column of the folder shown, which the list leaves alone.
struct ColumnKeys {
    window: u64,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for ColumnKeys {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        filling(Axis::Vertical)
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let Event::Key(
            KeyEvent {
                keysym,
                mods,
                pressed: true,
                ..
            },
            _,
        ) = event
        else {
            return Propagation::Continue;
        };
        if !mods.is_empty() {
            return Propagation::Continue;
        }
        let (forward, back) = match cx.env().direction {
            LayoutDirection::Ltr => (RIGHT, LEFT),
            LayoutDirection::Rtl => (LEFT, RIGHT),
        };
        let by = match *keysym {
            k if k == forward => 1,
            k if k == back => -1,
            _ => return Propagation::Continue,
        };
        cx.emit(Message::Window(self.window, WindowEvent::ColumnStep(by)));
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A column and its divider; a press on the divider, or just before it, drags it to resize the column.
struct ColumnFrame {
    window: u64,
    index: usize,
    /// The column's width as shown.
    width: f64,
    children: Vec<Element<Message>>,
}

/// A divider being dragged: where the press was, the column's width then, and whether Option was held.
#[derive(Default)]
struct Resizing {
    from: Option<(f64, f64, bool)>,
}

impl Widget<Message> for ColumnFrame {
    type State = Resizing;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.width),
            ..filling(Axis::Horizontal)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> Resizing {
        Resizing::default()
    }

    fn update(&self, old: &Self, _: &mut Resizing) -> Change {
        if self.width != old.width {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Resizing, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(
        &self,
        state: &mut Resizing,
        event: &Event,
        cx: &mut EventCx<'_, Message>,
    ) -> Propagation {
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    mods,
                    ..
                },
                _,
            ) if cx.phase() == Phase::Capture => {
                let Some(bounds) = cx.bounds() else {
                    return Propagation::Continue;
                };
                let edge = if rtl { bounds.min_x() } else { bounds.max_x() };
                if (position.x - edge).abs() > GRIP {
                    return Propagation::Continue;
                }
                state.from = Some((position.x, self.width, mods.contains(Mods::ALT)));
                cx.capture_pointer();
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                let Some((from, width, all)) = state.from else {
                    return Propagation::Continue;
                };
                let moved = if rtl {
                    from - position.x
                } else {
                    position.x - from
                };
                cx.emit(Message::Window(
                    self.window,
                    WindowEvent::ColumnResize {
                        index: self.index,
                        width: width + moved,
                        all,
                    },
                ));
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if state.from.is_some() => {
                state.from = None;
                cx.release_pointer();
                cx.emit(Message::Window(self.window, WindowEvent::ColumnResized));
                Propagation::Stop
            }
            Event::Cancel => {
                if state.from.take().is_some() {
                    cx.emit(Message::Window(self.window, WindowEvent::ColumnResized));
                }
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &Resizing, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &Resizing, _: &mut SemanticsCx<'_>) {}
}

/// The side of the More button's glyph: a ring holding three dots in a row.
const MORE_GLYPH: f64 = 18.0;

/// The More button's glyph, drawn here so it never depends on an icon a pack may lack.
struct MoreGlyph;

impl Widget<Message> for MoreGlyph {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(MORE_GLYPH),
            height: SizeSpec::Fixed(MORE_GLYPH),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(MORE_GLYPH, MORE_GLYPH),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let colors = &cx.env().theme.colors;
        let Some(ink) = colors.get(ColorRole::SecondaryText, StateVariant::NORMAL) else {
            return;
        };
        let side = MORE_GLYPH - 2.0;
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect {
                rect: Rect::new(1.0, 1.0, side, side),
                radius: side / 2.0,
            },
            paint: Paint::Solid(ink),
            width: 1.2,
        });
        let middle = MORE_GLYPH / 2.0;
        let dot = 2.0;
        for x in [middle - 4.0, middle, middle + 4.0] {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(x - dot / 2.0, middle - dot / 2.0, dot, dot),
                    radius: dot / 2.0,
                },
                paint: Paint::Solid(ink),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The line between two columns.
struct Divider;

impl Widget<Message> for Divider {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(DIVIDER),
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(DIVIDER, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let color = cx
            .env()
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL);
        if let Some(color) = color {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A row of a column other than the folder shown's: a secondary press on it shows its folder with it selected before
/// the context menu opens, and the row of the next folder of the chain is highlighted.
struct PickCell {
    message: Message,
    highlighted: bool,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for PickCell {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        filling(Axis::Horizontal)
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.highlighted != old.highlighted {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let secondary = match *event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => true,
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    mods,
                    ..
                },
                _,
            ) => mods.contains(Mods::CTRL),
            _ => false,
        };
        if secondary {
            cx.emit(self.message.clone());
            if let Event::Pointer(PointerEvent::Button { position, .. }, _) = *event {
                if let Message::Window(id, _) = self.message {
                    if !matches!(
                        event,
                        Event::Pointer(
                            PointerEvent::Button {
                                button: MouseButton::Right,
                                ..
                            },
                            _
                        )
                    ) {
                        // Ctrl with a press is the secondary click
                        cx.emit(Message::Window(id, WindowEvent::ContextMenu(position)));
                        return Propagation::Stop;
                    }
                }
            }
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.highlighted {
            return;
        }
        let size = cx.size();
        let color = cx.env().theme.colors.get(
            ColorRole::SecondarySelectedControlFill,
            StateVariant::NORMAL,
        );
        if let Some(color) = color {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(
                        -HIGHLIGHT_OUTSET,
                        0.0,
                        size.width + 2.0 * HIGHLIGHT_OUTSET,
                        size.height,
                    ),
                    radius: HIGHLIGHT_RADIUS,
                },
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

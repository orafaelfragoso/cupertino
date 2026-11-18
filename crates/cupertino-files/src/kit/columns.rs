//! The column view on CupertinoKit: the chain of folders of [`crate::columns`] side by side, drawn by Kit's
//! [`FileColumns`], and a preview of the selected file at the end.
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

use cupertino_kit::controls::{FilePreview, PreviewContent, PreviewStyle, Stack};
use cupertino_kit::core::{
    Change, Element, Event, EventCx, Key, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, LayoutSpec, Measured, ScrollState, SizeSpec,
};
use cupertino_kit::prelude::{Button, ButtonRole, ButtonStyle};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::{ColorRole, StateVariant};

use super::view::{self, popup_key};
use super::{BrowserWindow, Event as WindowEvent, Message, Popup, Session, NO_SCROLL};
use crate::browser::Browser;
use crate::columns::{self, ChainLayout, ColumnChain, PREVIEW_WIDTH};
use crate::metrics::window as window_metrics;
use crate::model::DirectoryModel;
use crate::places::window_title;
use crate::preview::{self as file_preview, PreviewKind};
use cupertino_kit::files::columns::{column_key, FileColumn, FileColumns, ROW_HEIGHT, ROW_ICON};
use cupertino_kit::files::entry::Entry;
use cupertino_kit::files::list::row_key;

/// The side of an icon in the preview column.
const PREVIEW_ICON: f64 = 128.0;
/// The label of the preview column's actions button.
const MORE: &str = "More…";

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
    let width = |place: usize| layout.columns.get(place).map_or(0.0, |c| c.1);
    let mut columns = Vec::new();
    for (i, folder) in chain.folders.iter().enumerate() {
        let column = if folder == browser.dir() {
            active_column(session, id, win)
        } else {
            let next = chain.folders.get(i + 1).or(chain.child.as_ref());
            other_column(session, cols, folder, next)
        };
        columns.push((column, width(columns.len())));
    }
    if let Some(child) = &chain.child {
        let column = other_column(session, cols, child, None);
        columns.push((column, width(columns.len())));
    }
    if let Some(entry) = chain.preview.and_then(|i| browser.entries().get(i)) {
        let width = layout
            .columns
            .get(columns.len())
            .map_or(PREVIEW_WIDTH, |c| c.1);
        let preview = FileColumn::Preview(preview_pane(session, id, win, entry));
        columns.push((preview, width));
    }
    let icons = Rc::clone(&session.icons);
    Element::from(FileColumns {
        window: id,
        columns,
        content: layout.content,
        scroll: win.scroll,
        lead: view::under_row(win),
        icon: Rc::new(move |entry| icons.icon(entry, ROW_ICON)),
        on_event: super::view_events(id),
    })
}

/// The column of the folder the window shows: the browser's items, selection and rename.
fn active_column(session: &Session, id: u64, win: &BrowserWindow) -> FileColumn<Message> {
    let browser = &win.browser;
    let dir = browser.dir().to_path_buf();
    FileColumn::Shown {
        label: window_title(&dir, &session.places),
        items: browser.entries().into(),
        scroll: win.columns.scroll(&dir),
        selection: view::shown_selection(win),
        name_field: view::name_field_of(id, win),
        presses: win.list_presses.clone(),
        folder: dir,
    }
}

/// The column of a folder other than the one shown, which the row of `next`, the folder of the next column, is
/// highlighted in; empty until its listing has its items.
fn other_column(
    session: &Session,
    cols: &ColumnsView,
    folder: &Path,
    next: Option<&PathBuf>,
) -> FileColumn<Message> {
    let Some(listing) = cols.listings.get(folder) else {
        return FileColumn::Empty {
            folder: folder.to_path_buf(),
        };
    };
    let entries = listing.entries();
    FileColumn::Other {
        folder: folder.to_path_buf(),
        label: window_title(folder, &session.places),
        highlighted: next.and_then(|n| entries.iter().position(|e| &e.path == n)),
        items: entries.into(),
        scroll: cols.scroll(folder),
    }
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

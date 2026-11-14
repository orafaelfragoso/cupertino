//! The folder as a gallery: a large picture of one item with its name under it, a strip of every item's picture
//! along the bottom, which [`IconGrid`] lays out in one row and scrolls sideways, and the preview pane of the item at
//! the trailing side. The item shown is the selection's cursor, else its first item, else the folder's first item;
//! the keyboard works the strip as it works the icon grid, Left and Right moving to the item beside.

use std::rc::Rc;

use cupertino_kit::controls::{
    Image, ImageFit, PreviewContent, ScrollView, Scroller, Separator, Spacer, Stack, Text, TextRole,
};
use cupertino_kit::core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::text::{AttributedText, TextStyle};

use super::columns;
use super::grid::{IconGrid, ItemIcon};
use super::view::{name_field, shown_selection, under_row};
use super::{BrowserWindow, Event as WindowEvent, Message, Session};
use crate::layout::icon::{self, FontMetrics, IconLayout, IconParams};
use crate::places::window_title;
use crate::settings::GalleryViewSettings;

/// Room around the large picture and its name.
const STAGE_INSETS: Insets = Insets {
    top: 20.0,
    left: 20.0,
    bottom: 12.0,
    right: 20.0,
};
/// Between the large picture and its name.
const NAME_GAP: f64 = 8.0;
/// The size of the name under the large picture.
const NAME_SIZE: f32 = 13.0;
/// The field a name is edited in under the large picture.
const FIELD: (f64, f64) = (240.0, 22.0);
/// The side of the pictures of the strip under the large one; with their padding each takes a 54 point cell.
const STRIP_ICON: u32 = 48;

/// The item the large picture shows: the selection's cursor when it is selected, else the first item selected, else
/// the folder's first item; `None` in an empty folder.
pub(super) fn item(win: &BrowserWindow) -> Option<usize> {
    let selection = win.browser.selection();
    let count = win.browser.entries().len();
    let cursor = selection.focus().filter(|&c| selection.contains(c));
    cursor
        .or_else(|| selection.indices().next())
        .or((count > 0).then_some(0))
        .filter(|&i| i < count)
}

/// The strip's parameters at the gallery's settings, and its row of every item.
fn strip_layout(win: &BrowserWindow) -> (IconParams, IconLayout) {
    let params = IconParams::new(STRIP_ICON, 0, 12, true, false, FontMetrics::estimate(12.0));
    let layout = icon::strip(&params, win.browser.entries().len());
    (params, layout)
}

/// The strip's items in view or within `margin` points of it, the leading first, and the side of their pictures.
pub(super) fn shown(win: &BrowserWindow, margin: f64) -> (Vec<usize>, f64) {
    let scroll = &win.scroll;
    let (params, layout) = strip_layout(win);
    if scroll.viewport.width <= 0.0 {
        return (Vec::new(), params.icon_size);
    }
    let (left, right) = (
        scroll.offset.x - margin,
        scroll.offset.x + scroll.viewport.width + margin,
    );
    let shown = layout
        .cells
        .iter()
        .filter(|c| c.frame.x + c.frame.w >= left && c.frame.x <= right)
        .map(|c| c.index)
        .collect();
    (shown, params.icon_size)
}

/// The gallery of window `id`.
pub(super) fn view(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let entry = item(win).and_then(|i| win.browser.entries().get(i).map(|e| (i, e)));
    let stage = match entry {
        Some((index, entry)) => stage(session, id, win, index, entry),
        None => Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: Vec::new(),
        }),
    };
    let main = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children: vec![
            stage.key("stage").fill(),
            Element::new(Separator {
                axis: Axis::Horizontal,
            }),
            strip(session, id, win),
        ],
    });
    let mut children = vec![main.fill()];
    if let Some((_, entry)) = entry {
        children.push(Element::new(Separator {
            axis: Axis::Vertical,
        }));
        let pane = columns::preview_pane(session, id, win, entry);
        children.push(
            pane.key("preview")
                .width(GalleryViewSettings::default().preview_pane_width),
        );
    }
    // the gallery lies below the toolbar row
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: under_row(win),
            ..Insets::ZERO
        },
        spacing: 0.0,
        align: Align::Stretch,
        children,
    })
    .key("gallery")
}

/// The large picture of item `index`, `entry`, as large as the room allows, and its name under it, or the field it is
/// renamed in.
fn stage(
    session: &Session,
    id: u64,
    win: &BrowserWindow,
    index: usize,
    entry: &crate::Entry,
) -> Element<Message> {
    let picture = super::peek::PICTURE;
    let sources = match columns::preview_content(session, &win.texts, entry, picture, picture) {
        PreviewContent::Picture(source) => vec![source],
        PreviewContent::Icon(sources) => sources,
        PreviewContent::Text(_) => session.icons.icon(entry, picture),
    };
    let image = Element::new(Image {
        sources,
        size: Size::ZERO,
        fit: ImageFit::Contain,
        radius: 0.0,
        label: Some(entry.name.clone()),
    })
    .fill();
    let name = match win.rename.as_ref().filter(|r| r.edit.index() == index) {
        Some(rename) => Element::new(Sized {
            width: Some(FIELD.0),
            height: FIELD.1,
            children: vec![name_field(
                id,
                rename.edit.text().to_owned(),
                rename.revision,
                rename.selection.clone(),
            )],
        }),
        None => Element::new(Text {
            content: AttributedText::plain(entry.name.clone(), TextStyle::new(NAME_SIZE)),
            role: TextRole::Body,
            wrap: false,
            secondary: false,
            selected: false,
        }),
    };
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: STAGE_INSETS,
        spacing: NAME_GAP,
        align: Align::Center,
        children: vec![image, name],
    })
}

/// Every item's picture in a row, which scrolls sideways.
fn strip(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let (params, layout) = strip_layout(win);
    let height = layout.content.h;
    let items: Rc<[crate::Entry]> = win.browser.entries().into();
    let item_icons = Rc::clone(&session.icons);
    let side = params.icon_size;
    let icon: ItemIcon = Rc::new(move |e| item_icons.icon(e, side));
    let grid = Element::new(IconGrid {
        window: id,
        label: window_title(win.browser.dir(), &session.places),
        layout: Rc::new(layout),
        params,
        items,
        icon,
        scroll: win.scroll,
        selection: shown_selection(win),
        ringed: Default::default(),
        band: None,
        lead: 0.0,
        // the strip names no picture: the name is under the large one
        name: Rc::new(|_, _| Element::new(Spacer { min: 0.0 })),
        names: None,
        revision: session.view_revision,
        strip: true,
    });
    let scroll = Element::from(ScrollView {
        content: grid,
        state: win.scroll,
        horizontal: true,
        vertical: false,
        scroller: Scroller::Overlay,
        on_scroll: Box::new(move |s| Message::Window(id, WindowEvent::Scroll(s))),
    })
    .key("strip");
    Element::new(Sized {
        width: None,
        height,
        children: vec![scroll.fill()],
    })
}

/// Its child at a fixed height, and at a fixed width or as wide as its parent.
struct Sized {
    width: Option<f64>,
    height: f64,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Sized {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: self.width.map_or(SizeSpec::Fill, SizeSpec::Fixed),
            height: SizeSpec::Fixed(self.height),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.width, self.height) != (old.width, old.height) {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.width.unwrap_or(0.0), self.height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

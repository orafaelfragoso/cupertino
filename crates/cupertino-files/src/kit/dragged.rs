//! Items dragged out of a window: the files as a `text/uri-list` a drop may copy or move, and the image that follows
//! the pointer. One item shows its icon (or thumbnail) and its name as the grid shows them; several show the pressed
//! item in front of the icons of up to two more, with the count in a badge on the stack. Rows dragged from the list
//! show the pressed row's icon and name as wide as its name cell, with the count in a badge at its trailing end. The
//! image keeps the point the press grabbed under the pointer and is drawn at [`OPACITY`].

use cupertino_kit::controls::{Badge, Image, ImageFit, ImageSource, Text, TextRole};
use cupertino_kit::core::{
    Change, Drag, DragPreview, Element, Event, EventCx, MeasureCx, PaintCx, Propagation,
    SemanticsCx, Widget, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{ClipboardData, DropAction};
use cupertino_kit::prelude::Command;
use cupertino_kit::text::{AttributedText, TextStyle};
use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};

use super::list::RowGrab;
use super::{grid, Event as WindowEvent, Message};
use crate::Entry;

/// How opaque the image of dragged items is: the folder under the pointer shows through it.
pub(super) const OPACITY: f32 = 0.8;
/// How far each icon behind the front one of several dragged items sits up and to the trailing side, in points.
const STACK_STEP: f64 = 4.0;
/// The most icons a stack of dragged items shows behind the front one.
const STACK_BEHIND: usize = 2;
/// Room the count badge takes beyond the top trailing corner of the stack, in points: half the badge's height.
const BADGE_OVERHANG: f64 = 8.0;
/// The widest count badge, in points: room for three digits.
const BADGE_WIDTH: f64 = 32.0;
/// A dragged row's icon, its gap to the name and the name's size, in points.
pub(super) const ROW_ICON: f64 = 16.0;
const ROW_GAP: f64 = 6.0;
const ROW_TEXT_SIZE: f32 = 13.0;
/// The type the dragged files are offered in.
const URI_LIST: &str = "text/uri-list";

/// A press in the icon grid that moved far enough to drag: what is dragged and how its cell is laid out, so the image
/// shows the pressed item as the grid does.
#[derive(Clone, Debug, PartialEq)]
pub struct Grab {
    /// The dragged items, the pressed one first.
    pub indices: Vec<usize>,
    /// The pressed item's name, and the renditions of each dragged item's icon or thumbnail in the order of `indices`.
    pub name: String,
    pub icons: Vec<Vec<ImageSource>>,
    /// Where the press was, from the top left corner of the pressed item's cell, in points.
    pub offset: Point,
    /// The window reads right to left: the image's parts are placed from its right edge.
    pub rtl: bool,
    /// The cell: its size, the room above its icon, the icon's side, the gap to the name, the name's box and size.
    pub cell: Size,
    pub icon_top: f64,
    pub icon_size: f64,
    pub gap: f64,
    pub label: Size,
    pub text_size: f64,
}

/// The drag of `grab`'s items of `entries` out of window `id`.
pub(super) fn start(
    id: u64,
    window: WindowKey,
    grab: &Grab,
    entries: &[Entry],
) -> Command<Message> {
    drag(id, window, &grab.indices, entries, preview(grab))
}

/// The drag of the list's rows of `grab`, items of `entries`, out of window `id`; `icon` is the renditions of the
/// pressed row's icon.
pub(super) fn start_rows(
    id: u64,
    window: WindowKey,
    grab: &RowGrab,
    entries: &[Entry],
    icon: Vec<ImageSource>,
) -> Command<Message> {
    let pressed = grab.indices.first().and_then(|&i| entries.get(i));
    let name = pressed.map_or_else(String::new, |e| e.name.clone());
    let image = row_preview(grab, name, icon);
    drag(id, window, &grab.indices, entries, image)
}

fn drag(
    id: u64,
    window: WindowKey,
    indices: &[usize],
    entries: &[Entry],
    image: DragPreview<Message>,
) -> Command<Message> {
    let uris: String = indices
        .iter()
        .filter_map(|&i| entries.get(i))
        .map(|e| format!("{}\r\n", file_uri(&e.path)))
        .collect();
    Command::StartDrag(Drag {
        window,
        data: vec![ClipboardData {
            mime: URI_LIST.into(),
            bytes: uris.into_bytes(),
        }],
        actions: [DropAction::Copy, DropAction::Move].into(),
        image: Some(image),
        on_progress: Box::new(move |progress| Message::Window(id, WindowEvent::Dragged(progress))),
    })
}

/// The image of rows dragged from the list: the pressed row's icon and name in the size of its name cell, with the
/// count of the rows in a badge at the trailing end when there are several, and the pointer where the press grabbed it.
pub(super) fn row_preview(
    grab: &RowGrab,
    name: String,
    sources: Vec<ImageSource>,
) -> DragPreview<Message> {
    let size = grab.cell;
    let count = grab.indices.len();
    let middle = |height: f64| (size.height - height) / 2.0;
    let badge = if count > 1 { BADGE_WIDTH } else { 0.0 };
    let text_width = (size.width - ROW_ICON - ROW_GAP - badge).max(0.0);
    // right to left, the icon is at the right edge and the badge at the left
    let (icon_x, text_x, badge_x) = if grab.rtl {
        (size.width - ROW_ICON, badge, 0.0)
    } else {
        (0.0, ROW_ICON + ROW_GAP, size.width - BADGE_WIDTH)
    };
    let mut children = vec![
        placed(
            Rect::new(icon_x, middle(ROW_ICON), ROW_ICON, ROW_ICON),
            Insets::ZERO,
            0.0,
            vec![icon(sources, ROW_ICON)],
        ),
        Element::new(Line {
            frame: Rect::new(text_x, 0.0, text_width, size.height),
            children: vec![Element::new(Text {
                content: AttributedText::plain(name, TextStyle::new(ROW_TEXT_SIZE)),
                role: TextRole::Body,
                wrap: false,
                secondary: false,
                selected: false,
            })],
        }),
    ];
    if count > 1 {
        children.push(placed(
            Rect::new(
                badge_x,
                middle(2.0 * BADGE_OVERHANG),
                BADGE_WIDTH,
                2.0 * BADGE_OVERHANG,
            ),
            Insets::ZERO,
            0.0,
            vec![Element::new(Badge {
                text: Some(count.to_string()),
                description: format!("{count} items"),
            })],
        ));
    }
    DragPreview {
        content: Element::new(Preview { children }),
        size,
        hotspot: grab.offset,
    }
}

/// The bytes a `file://` URI of a path keeps as they are: the unreserved ones and `/`.
const URI_PATH: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// A `file://` URI of `path`, every byte but the unreserved ones and `/` percent-encoded.
fn file_uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    format!(
        "file://{}",
        percent_encode(path.as_os_str().as_bytes(), URI_PATH)
    )
}

/// The image of `grab`: the pressed cell where the grid draws it, below and to the leading side of the room the stack
/// and the count take with several items, and the pointer where the press grabbed the cell.
pub(super) fn preview(grab: &Grab) -> DragPreview<Message> {
    let count = grab.indices.len();
    let behind = count.saturating_sub(1).min(STACK_BEHIND);
    let room = if count > 1 {
        behind as f64 * STACK_STEP + BADGE_OVERHANG
    } else {
        0.0
    };
    let size = Size::new(grab.cell.width + room, grab.cell.height + room);
    let icon_left = (grab.cell.width - grab.icon_size) / 2.0;
    let icon_top = room + grab.icon_top;
    let front = grab.icons.first().cloned().unwrap_or_default();
    let mut children = Vec::new();
    // the icons behind, the farthest first, each a step up and to the trailing side of the one before it
    for depth in (1..=behind).rev() {
        let step = depth as f64 * STACK_STEP;
        let sources = grab.icons.get(depth).unwrap_or(&front).clone();
        children.push(placed(
            Rect::new(
                icon_left + step,
                icon_top - step,
                grab.icon_size,
                grab.icon_size,
            ),
            Insets::ZERO,
            0.0,
            vec![icon(sources, grab.icon_size)],
        ));
    }
    children.push(placed(
        Rect::new(0.0, room, grab.cell.width, grab.cell.height),
        Insets {
            top: grab.icon_top,
            ..Insets::ZERO
        },
        grab.gap,
        vec![
            icon(front, grab.icon_size),
            placed(
                Rect::new(0.0, 0.0, grab.label.width, grab.label.height),
                Insets::ZERO,
                0.0,
                vec![grid::name(grab.name.clone(), grab.text_size, true)],
            ),
        ],
    ));
    if count > 1 {
        // centred on the top trailing corner of the farthest icon
        let step = behind as f64 * STACK_STEP;
        let corner = Point::new(icon_left + grab.icon_size + step, icon_top - step);
        children.push(placed(
            Rect::new(
                corner.x - BADGE_WIDTH / 2.0,
                corner.y - BADGE_OVERHANG,
                BADGE_WIDTH,
                2.0 * BADGE_OVERHANG,
            ),
            Insets::ZERO,
            0.0,
            vec![Element::new(Badge {
                text: Some(count.to_string()),
                description: format!("{count} items"),
            })],
        ));
    }
    // right to left, the cell sits at the right edge and the room is on its left
    let left = if grab.rtl { room } else { 0.0 };
    DragPreview {
        content: Element::new(Preview { children }),
        size,
        hotspot: Point::new(grab.offset.x + left, grab.offset.y + room),
    }
}

fn icon(sources: Vec<ImageSource>, size: f64) -> Element<Message> {
    Element::new(Image {
        sources,
        size: Size::new(size, size),
        fit: ImageFit::Contain,
        radius: 0.0,
        label: None,
    })
}

fn placed(
    frame: Rect,
    insets: Insets,
    spacing: f64,
    children: Vec<Element<Message>>,
) -> Element<Message> {
    Element::new(Placed {
        frame,
        insets,
        spacing,
        children,
    })
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

/// The whole image: its parts at their places, drawn as one layer at [`OPACITY`].
struct Preview {
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Preview {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.set_group(OPACITY, Vec::new());
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A line of text at `frame`, from its leading edge and down its middle.
struct Line {
    frame: Rect,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Line {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.frame.size.width),
            height: SizeSpec::Fixed(self.frame.size.height),
            position: self.frame.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A part of the image at `frame`: its children stacked down its middle under `insets`, `spacing` apart.
struct Placed {
    frame: Rect,
    insets: Insets,
    spacing: f64,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Placed {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: self.insets,
            width: SizeSpec::Fixed(self.frame.size.width),
            height: SizeSpec::Fixed(self.frame.size.height),
            position: self.frame.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: self.spacing,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grab(count: usize) -> Grab {
        Grab {
            indices: (0..count).collect(),
            name: "a.txt".into(),
            icons: vec![Vec::new(); count],
            offset: Point::new(30.0, 20.0),
            rtl: false,
            cell: Size::new(90.0, 100.0),
            icon_top: 4.0,
            icon_size: 64.0,
            gap: 2.0,
            label: Size::new(90.0, 30.0),
            text_size: 12.0,
        }
    }

    #[test]
    fn one_item_is_its_cell_with_the_pointer_where_the_press_grabbed_it() {
        let preview = preview(&grab(1));
        assert_eq!(preview.size, Size::new(90.0, 100.0));
        assert_eq!(preview.hotspot, Point::new(30.0, 20.0));
    }

    #[test]
    fn several_items_make_room_above_for_the_stack_and_count_and_keep_the_grab() {
        let room = 2.0 * STACK_STEP + BADGE_OVERHANG;
        for count in [3, 40] {
            let preview = preview(&grab(count));
            assert_eq!(
                preview.size,
                Size::new(90.0 + room, 100.0 + room),
                "{count}"
            );
            assert_eq!(preview.hotspot, Point::new(30.0, 20.0 + room), "{count}");
        }
        let two = preview(&grab(2));
        let room = STACK_STEP + BADGE_OVERHANG;
        assert_eq!(two.hotspot, Point::new(30.0, 20.0 + room));
        let rtl = preview(&Grab {
            rtl: true,
            ..grab(2)
        });
        assert_eq!(
            rtl.hotspot,
            Point::new(30.0 + room, 20.0 + room),
            "the room is on the left"
        );
    }

    #[test]
    fn a_row_is_its_name_cell_with_the_pointer_where_the_press_grabbed_it() {
        let grab = RowGrab {
            indices: vec![4, 1],
            offset: Point::new(40.0, 9.0),
            cell: Size::new(300.0, 24.0),
            rtl: false,
        };
        let preview = row_preview(&grab, "a.txt".into(), Vec::new());
        assert_eq!(preview.size, Size::new(300.0, 24.0));
        assert_eq!(preview.hotspot, Point::new(40.0, 9.0));
    }

    #[test]
    fn a_path_becomes_a_file_uri_with_its_other_bytes_escaped() {
        assert_eq!(
            file_uri(std::path::Path::new("/home/a b/ü%.txt")),
            "file:///home/a%20b/%C3%BC%25.txt"
        );
    }
}

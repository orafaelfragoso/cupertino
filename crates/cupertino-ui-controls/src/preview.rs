//! FilePreview: a file's content drawn without opening it in an application, as a pane beside a list of files or as
//! the content of a preview window.

use crate::look;
use crate::scroll;
use crate::{
    FilePreview, Image, ImageFit, PreviewContent, PreviewStyle, Spacer, Stack, Text, TextRole,
};
use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_core::{
    Change, ControlState, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, Role,
    SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, Overflow,
    SizeSpec,
};
use cupertino_ui_text::{AttributedText, FontWeight, TextStyle};
use cupertino_ui_theme::ColorRole;
use std::rc::Rc;
use std::sync::Arc;

/// The pane's side insets, and the room above its content box.
const PANE_INSET: f64 = 10.0;
const PANE_TOP: f64 = 8.0;
/// The height of the pane's content box.
const PANE_BOX_HEIGHT: f64 = 200.0;
/// The room between the content box and the name.
const PANE_NAME_GAP: f64 = 15.0;
/// The room between the summary and the details' heading, and between detail rows.
const PANE_SECTION_GAP: f64 = 10.0;
const PANE_ROW_GAP: f64 = 4.0;
/// The type of the name, the summary and the details.
const NAME_SIZE: f32 = 15.0;
const BODY_SIZE: f32 = 13.0;
/// The size of a text file's content, set in the fixed-pitch face the preview is given.
const TEXT_SIZE: f32 = 11.0;
/// Between the content box's edges and its text.
const TEXT_INSET: f64 = 8.0;
/// From a window's edges to the box of a text file's content.
const WINDOW_TEXT_INSET: f64 = 4.0;
/// The content box's corner radius and edge.
const BOX_RADIUS: f64 = 6.0;
const BOX_EDGE: f64 = 1.0;
/// The largest an icon shows: in the pane, and in a window with the details beside it.
const PANE_ICON: f64 = 128.0;
const WINDOW_ICON: f64 = 256.0;
/// The heading over the details.
const DETAILS_TITLE: &str = "Information";
/// The footer's height and its separator.
const FOOTER_HEIGHT: f64 = 69.0;

fn style(size: f32, weight: FontWeight) -> TextStyle {
    TextStyle {
        weight,
        ..TextStyle::new(size)
    }
}

fn text<M: 'static>(
    content: &str,
    size: f32,
    weight: FontWeight,
    secondary: bool,
    wrap: bool,
) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(content.to_owned(), style(size, weight)),
        role: TextRole::Body,
        wrap,
        secondary,
        selected: false,
    })
}

fn mono(family: Option<&str>) -> TextStyle {
    TextStyle {
        family: family.map(Arc::from),
        ..TextStyle::new(TEXT_SIZE)
    }
}

fn spacer<M: 'static>(min: f64) -> Element<M> {
    Element::new(Spacer { min })
}

fn column<M: 'static>(padding: Insets, spacing: f64, children: Vec<Element<M>>) -> Element<M> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding,
        spacing,
        align: Align::Stretch,
        children,
    })
}

impl<M: 'static> From<FilePreview<M>> for Element<M> {
    fn from(preview: FilePreview<M>) -> Self {
        let label = format!("Preview of {}", preview.name);
        let body = match preview.style {
            PreviewStyle::Pane => pane(preview),
            PreviewStyle::Window => window(preview),
        };
        Element::new(Group {
            label,
            children: vec![body.fill()],
        })
    }
}

/// The parts of a preview that draw its content.
struct Content<M> {
    content: PreviewContent,
    text_family: Option<String>,
    name: String,
    scroll: cupertino_ui_layout::ScrollState,
    on_scroll: crate::Handler<cupertino_ui_layout::ScrollState, M>,
}

/// The content in its box, as large as the room lets it be, for the pane and for a window; an icon at most `icon`
/// points.
fn content_box<M: 'static>(content: Content<M>, icon: f64, framed: bool) -> Element<M> {
    let Content {
        content,
        text_family,
        name,
        scroll,
        on_scroll,
    } = content;
    let (inner, text) = match content {
        PreviewContent::Picture(source) => (
            Element::new(Image {
                sources: vec![source],
                size: Size::ZERO,
                fit: ImageFit::Contain,
                radius: 0.0,
                label: Some(name),
            })
            .fill(),
            false,
        ),
        PreviewContent::Text(content) => {
            let words = Element::new(Text {
                content: AttributedText::plain(content, mono(text_family.as_deref())),
                role: TextRole::Body,
                wrap: true,
                secondary: false,
                selected: false,
            });
            let padded = column(Insets::uniform(TEXT_INSET), 0.0, vec![words]);
            (
                scroll::frame(
                    padded,
                    scroll,
                    (false, true),
                    crate::Scroller::Overlay,
                    Rc::new(on_scroll),
                )
                .fill(),
                true,
            )
        }
        PreviewContent::Icon(sources) => (
            Element::new(Centred {
                children: vec![Element::new(Image {
                    sources,
                    size: Size::new(icon, icon),
                    fit: ImageFit::Contain,
                    radius: 0.0,
                    label: None,
                })],
            })
            .fill(),
            false,
        ),
    };
    Element::new(ContentBox {
        framed: framed || text,
        filled: text,
        children: vec![inner],
    })
}

/// A preview's content and the rest of it.
fn split<M>(preview: FilePreview<M>) -> (Content<M>, Info<M>) {
    let FilePreview {
        content,
        text_family,
        name,
        summary,
        details,
        footer,
        style: _,
        scroll,
        on_scroll,
    } = preview;
    (
        Content {
            content,
            text_family,
            name: name.clone(),
            scroll,
            on_scroll,
        },
        Info {
            name,
            summary,
            details,
            footer,
        },
    )
}

/// What a preview shows besides its content.
struct Info<M> {
    name: String,
    summary: String,
    details: Vec<(String, String)>,
    footer: Option<Element<M>>,
}

/// The pane: the content in a box, the name, the summary, the details, and the footer pinned to the bottom.
fn pane<M: 'static>(preview: FilePreview<M>) -> Element<M> {
    let (content, mut preview) = split(preview);
    let content = content_box(content, PANE_ICON, true).key("content");
    let mut children = vec![
        Element::new(Sized {
            height: PANE_BOX_HEIGHT,
            clip: false,
            children: vec![content.fill()],
        }),
        spacer(PANE_NAME_GAP),
        text(&preview.name, NAME_SIZE, FontWeight(500), false, false),
        text(
            &preview.summary,
            BODY_SIZE,
            FontWeight::REGULAR,
            true,
            false,
        ),
    ];
    if !preview.details.is_empty() {
        children.push(spacer(PANE_SECTION_GAP));
        children.push(text(
            DETAILS_TITLE,
            BODY_SIZE,
            FontWeight::BOLD,
            false,
            false,
        ));
        for (label, value) in &preview.details {
            children.push(detail(label, value));
        }
    }
    let info = column(
        Insets {
            top: PANE_TOP,
            left: PANE_INSET,
            bottom: PANE_INSET,
            right: PANE_INSET,
        },
        PANE_ROW_GAP,
        children,
    );
    // details the pane has no room for are cut at the footer
    let mut parts = vec![Element::new(Sized {
        height: 0.0,
        clip: true,
        children: vec![info],
    })
    .fill()];
    if let Some(footer) = preview.footer.take() {
        parts.push(Element::new(Footer {
            children: vec![footer],
        }));
    }
    column(Insets::ZERO, 0.0, parts)
}

/// A detail: its label at the leading edge, its value at the trailing one.
fn detail<M: 'static>(label: &str, value: &str) -> Element<M> {
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: 8.0,
        align: Align::Center,
        children: vec![
            text(label, BODY_SIZE, FontWeight::REGULAR, true, false),
            spacer(0.0),
            text(value, BODY_SIZE, FontWeight::SEMIBOLD, false, false),
        ],
    })
}

/// A window: the content alone, filling it; an icon with the name, the summary and the details beside it.
fn window<M: 'static>(preview: FilePreview<M>) -> Element<M> {
    let icon = matches!(preview.content, PreviewContent::Icon(_));
    let (content, preview) = split(preview);
    if !icon {
        let text = matches!(content.content, PreviewContent::Text(_));
        let shown = content_box(content, WINDOW_ICON, false);
        if !text {
            return shown;
        }
        // a text file's box sits inside the window's edges
        return column(
            Insets {
                top: 0.0,
                left: WINDOW_TEXT_INSET,
                bottom: WINDOW_TEXT_INSET,
                right: WINDOW_TEXT_INSET,
            },
            0.0,
            vec![shown.fill()],
        );
    }
    let icon = content_box(content, WINDOW_ICON, false);
    let mut info = vec![
        spacer(0.0),
        text(&preview.name, 20.0, FontWeight::BOLD, false, true),
        text(
            &preview.summary,
            BODY_SIZE,
            FontWeight::REGULAR,
            true,
            false,
        ),
    ];
    for (label, value) in &preview.details {
        info.push(text(
            &format!("{label}: {value}"),
            BODY_SIZE,
            FontWeight::REGULAR,
            true,
            false,
        ));
    }
    info.push(spacer(0.0));
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::uniform(24.0),
        spacing: 24.0,
        align: Align::Stretch,
        children: vec![
            Element::new(Sized {
                height: 0.0,
                clip: false,
                children: vec![icon.fill()],
            })
            .width(WINDOW_ICON)
            .fill(),
            column(Insets::ZERO, PANE_ROW_GAP, info).fill(),
        ],
    })
}

/// The preview as one group, named after its file.
struct Group<M> {
    label: String,
    children: Vec<Element<M>>,
}

fn filling(axis: Axis, align: Align) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        height: SizeSpec::Fill,
        flex: Flex::grow(1.0),
        ..LayoutSpec::new(ContainerKind::Stack {
            axis,
            spacing: 0.0,
            align,
            justify: Justify::Start,
        })
    }
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

impl<M: 'static> Widget<M> for Group<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        filling(Axis::Vertical, Align::Stretch)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.label != old.label {
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
        cx.name(self.label.clone());
    }
}

/// A child `height` points tall, as wide as the room; with a height of 0 it fills the room.
struct Sized<M> {
    height: f64,
    /// What runs past it is cut.
    clip: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Sized<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: if self.height > 0.0 {
                SizeSpec::Fixed(self.height)
            } else {
                SizeSpec::Fill
            },
            overflow: if self.clip {
                Overflow::Clip
            } else {
                Overflow::Visible
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
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
        if (self.height, self.clip) != (old.height, old.clip) {
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

/// Its one child centred in the room.
struct Centred<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Centred<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
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

/// The box around the content: a text file's on the text background, with a hairline edge in rounded corners.
struct ContentBox<M> {
    framed: bool,
    filled: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ContentBox<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::uniform(if self.framed { BOX_EDGE } else { 0.0 }),
            ..filling(Axis::Vertical, Align::Stretch)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.framed, self.filled) != (old.framed, old.filled) {
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.framed {
            return;
        }
        let size = cx.size();
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        if self.filled {
            look::fill(
                cx,
                rect,
                BOX_RADIUS,
                look::color(&theme, ColorRole::TextBackground, &state),
            );
        }
        let half = BOX_EDGE / 2.0;
        look::stroke(
            cx,
            Rect::new(half, half, size.width - BOX_EDGE, size.height - BOX_EDGE),
            BOX_RADIUS - half,
            BOX_EDGE,
            look::color(&theme, ColorRole::Separator, &state),
        );
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The footer: a hairline over it and its content centred, at the bottom of the pane.
struct Footer<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Footer<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(FOOTER_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let width = cx.size().width;
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        look::fill(
            cx,
            Rect::new(0.0, 0.0, width, 1.0),
            0.0,
            look::color(&theme, ColorRole::Separator, &state),
        );
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

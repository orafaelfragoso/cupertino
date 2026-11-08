//! Drop targets: parts of a window that take data dragged onto them from any application. A [`DropTarget`] wraps its
//! content; while a drag offering one of its MIME types is over the content, the runtime asks the target where a drop
//! would land ([`DropTarget::place`]), tells the dragging application the type and action, outlines the place and, at
//! the drop, reads the data and hands it to [`DropTarget::on_drop`]. The innermost target under the pointer that takes
//! one of the drag's types is the one asked; a target that answers `None` there lets the drag pass to the one around
//! it.

use crate::cx::{EventCx, MeasureCx, PaintCx, SemanticsCx};
use crate::widget::{Change, Element, Propagation, Widget};
use crate::{Event, Key};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, Overflow, SizeSpec};
use cupertino_ui_platform::{ClipboardData, DropAction, DropToken, Mods};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, StateVariant};
use std::collections::BTreeSet;

/// Opacity of the accent over the place a drag is over.
const HIGHLIGHT_ALPHA: f32 = 0.3;
/// Corner radius of the place a drag is over.
const HIGHLIGHT_RADIUS: f64 = 5.0;
/// Width of the text-colour outline inside the highlight in a high-contrast appearance, so the place does not rest on a
/// translucent fill alone.
const OUTLINE_WIDTH: f64 = 2.0;

/// Where a drop over a [`DropTarget`] would land.
pub type PlaceHandler = Box<dyn Fn(&DropOver<'_>) -> Option<DropPlace>>;
/// What the place a drag is over becomes in the application: `Some` as the drag enters a place or moves to another,
/// `None` as it leaves the target, drops or ends.
pub type HoverHandler<M> = Box<dyn Fn(Option<DropPlace>) -> M>;
/// What a drop becomes in the application.
pub type DropHandler<M> = Box<dyn Fn(Dropped) -> M>;

/// A part of a window that takes data dragged onto it from any application, this one included. It lays out as its
/// content does and draws nothing of its own but the outline of the place a drag is over: the accent, translucent,
/// with a text-colour outline in a high-contrast appearance.
pub struct DropTarget<M> {
    pub content: Element<M>,
    /// The MIME types it reads, best first; a drag offering none of them passes it by.
    pub mimes: Vec<String>,
    /// What it can do with dropped data. The dragging application's choice among these and its own arrives with the
    /// drop.
    pub actions: BTreeSet<DropAction>,
    /// Where a drop would land, asked as the drag moves (and after each layout while it is over the target); `None`
    /// takes no drop there.
    pub place: PlaceHandler,
    pub on_hover: Option<HoverHandler<M>>,
    pub on_drop: DropHandler<M>,
    /// Sent when a drag that was over the target's window leaves the window, or ends there without being dropped on a
    /// target (released where nothing takes it, refused or cancelled); not after a drop a target took. Over any part
    /// of the window, not only over this target.
    pub on_leave: Option<M>,
}

/// A drag over a [`DropTarget`], as its [`DropTarget::place`] sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropOver<'a> {
    /// The pointer, in the target's coordinates.
    pub position: Point,
    /// The keys of the keyed nodes under the pointer inside the target, the innermost first.
    pub keys: &'a [Key],
    /// The MIME types the drag offers.
    pub mimes: &'a [String],
    /// What the dragging application allows done with the data.
    pub actions: &'a BTreeSet<DropAction>,
    /// The keyboard modifiers held, as the window system last knew them.
    pub mods: Mods,
}

/// Where a drop would land and what it would do there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropPlace {
    /// The keyed node inside the target the drop lands on, outlined while the drag is over it; `None` lands on the
    /// whole target. A key no node under the target has outlines nothing.
    pub key: Option<Key>,
    /// The action it prefers; the dragging application may allow another of the target's actions only.
    pub action: DropAction,
}

/// Data dropped on a [`DropTarget`]: where it landed, at what point of the target, the action chosen, the keyboard
/// modifiers held and the data in the first of the target's types the drag offered. The dragging application learns the
/// drop is done once the turn that hands it over has updated the model, so data it moves is let go only after then. It
/// is told `action` unless the update answers [`Command::FinishDrop`](crate::Command::FinishDrop) with `token` and
/// what was really done.
#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    /// Names the drop to [`Command::FinishDrop`](crate::Command::FinishDrop).
    pub token: DropToken,
    pub place: Option<Key>,
    pub position: Point,
    pub action: DropAction,
    pub mods: Mods,
    pub data: ClipboardData,
}

impl<M: 'static> From<DropTarget<M>> for Element<M> {
    fn from(target: DropTarget<M>) -> Self {
        let DropTarget {
            content,
            mimes,
            actions,
            place,
            on_hover,
            on_drop,
            on_leave,
        } = target;
        // the target takes its content's place in its parent; the content fills it
        let own = content.layout();
        let spec = LayoutSpec {
            width: own.width,
            height: own.height,
            flex: own.flex,
            align_self: own.align_self,
            span: own.span,
            position: own.position,
            insets: Insets::ZERO,
            overflow: Overflow::Visible,
            ..LayoutSpec::new(ContainerKind::Overlay)
        };
        Element::new(DropFrame {
            spec,
            mimes,
            actions,
            place,
            on_hover,
            on_drop,
            on_leave,
            children: vec![content, Element::new(Highlight)],
        })
    }
}

/// The node of a [`DropTarget`]: its content, then the highlight over it.
pub(crate) struct DropFrame<M> {
    spec: LayoutSpec,
    pub(crate) mimes: Vec<String>,
    pub(crate) actions: BTreeSet<DropAction>,
    pub(crate) place: PlaceHandler,
    pub(crate) on_hover: Option<HoverHandler<M>>,
    pub(crate) on_drop: DropHandler<M>,
    pub(crate) on_leave: Option<M>,
    children: Vec<Element<M>>,
}

impl<M> DropFrame<M> {
    /// The first of the target's types the drag offers.
    pub(crate) fn takes(&self, offered: &[String]) -> Option<&str> {
        self.mimes
            .iter()
            .find(|m| offered.contains(m))
            .map(String::as_str)
    }
}

impl<M: 'static> Widget<M> for DropFrame<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        self.spec.clone()
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// What the highlight of a [`DropTarget`] outlines, in its own coordinates; the runtime sets it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Lit(pub(crate) Option<Rect>);

/// The highlight over a [`DropTarget`]'s content. It takes no pointer input and has no semantics.
pub(crate) struct Highlight;

impl<M: 'static> Widget<M> for Highlight {
    type State = Lit;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> Lit {
        Lit::default()
    }

    fn update(&self, _: &Self, _: &mut Lit) -> Change {
        Change::None
    }

    fn measure(&self, _: &Lit, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut Lit, _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, lit: &Lit, cx: &mut PaintCx<'_>) {
        let Some(rect) = lit.0 else { return };
        let theme = std::sync::Arc::clone(&cx.env().theme);
        let colors = &theme.colors;
        let contrast = theme.preferences.appearance.is_high_contrast();
        if let Some(accent) = colors.get(ColorRole::SelectedControlFill, StateVariant::NORMAL) {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect,
                    radius: HIGHLIGHT_RADIUS,
                },
                paint: Paint::Solid(Color {
                    a: HIGHLIGHT_ALPHA,
                    ..accent
                }),
            });
        }
        if contrast {
            if let Some(text) = colors.get(ColorRole::Text, StateVariant::NORMAL) {
                let half = OUTLINE_WIDTH / 2.0;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: rect.outset(-half),
                        radius: HIGHLIGHT_RADIUS - half,
                    },
                    width: OUTLINE_WIDTH,
                    paint: Paint::Solid(Color { a: 1.0, ..text }),
                });
            }
        }
    }

    fn semantics(&self, _: &Lit, _: &mut SemanticsCx<'_>) {}

    fn hit(&self, _: &Lit, _: Point) -> bool {
        false
    }
}

use crate::look;
use crate::scroll;
use crate::{CalloutPointer, Popover};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    Change, CommandRegistry, Element, Event, EventCx, Glass, MeasureCx, PaintCx, Popup,
    PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured,
};
use cupertino_ui_platform::{DecorationMode, Keysym, WindowSpec};
use cupertino_ui_theme::{ColorRole, StateVariant, Surface};
use std::rc::Rc;

const ESCAPE: Keysym = Keysym(0xff1b);
/// The panel's corner radius.
const RADIUS: f64 = 16.0;
const PADDING: f64 = 5.0;
/// The room above the content, less than at its sides: a heading stands close under the panel's top edge.
const PADDING_TOP: f64 = 2.0;
/// How far the tip of the pointer stands below what the popover belongs to.
const GAP: f64 = 6.0;
/// The pointer's width where it leaves the panel and its height above the panel.
const POINTER_WIDTH: f64 = 30.0;
const POINTER_DEPTH: f64 = 10.5;
/// The container the panel and its pointer share as glass, and how far their outlines blend into each other.
const GLASS_CONTAINER: u32 = 1;
const GLASS_BLEND: f64 = 8.0;
/// The panel's own edge, drawn only over the opaque fill a window without the compositor's glass paints.
const EDGE: f64 = 1.0;
/// The widest the panel grows; the height bound is not in the spec, so it stays well under a laptop screen.
const MAX: Size = Size {
    width: 400.0,
    height: 600.0,
};

impl<M: Clone + 'static> Popover<M> {
    /// The popup window that shows the popover; declare it while it is open.
    pub fn window(self, key: WindowKey) -> Window<M> {
        Window {
            key,
            spec: WindowSpec {
                title: self.label.clone(),
                app_id: String::new(),
                size: MAX,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            // the panel and its pointer are one piece of glass the panel's node paints
            background: WindowBackground::None,
            content: Element::new(Callout {
                children: vec![
                    Element::new(PointerRow).key("pointer"),
                    Element::new(Panel {
                        label: self.label,
                        on_close: self.on_close.clone(),
                        children: vec![scroll::fitted(
                            self.content,
                            self.scroll,
                            Rc::new(self.on_scroll),
                        )],
                    })
                    .key("panel"),
                ],
            }),
            popup: Some(Popup {
                parent: self.parent,
                anchor: PopupAnchor::Below {
                    key: self.anchor,
                    gap: GAP,
                },
                grab: true,
                on_dismiss: self.on_close,
            }),
            menus: None,
        }
    }
}

struct Panel<M> {
    label: String,
    on_close: M,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Panel<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::new(PADDING_TOP, PADDING, PADDING, PADDING),
            // content taller than the window's bound gives way under the pointer row and scrolls inside the panel
            flex: Flex {
                grow: 0.0,
                shrink: 1.0,
                basis: None,
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
        if self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Key(key, _) if key.pressed && key.keysym == ESCAPE && key.mods.is_empty() => {
                cx.emit(self.on_close.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    /// The panel is glass joined to its pointer's; without the compositor's material it paints the opaque fill and its
    /// own edge.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        if cx.glass(popover_glass().radius(RADIUS)) {
            return;
        }
        let fill = cx.env().theme.surface(Surface::Popover).fill;
        look::fill(cx, rect, RADIUS, fill);
        let theme = &cx.env().theme;
        let edge = theme.colors.get(ColorRole::Separator, StateVariant::NORMAL);
        look::stroke(
            cx,
            rect.inset(Insets::uniform(EDGE / 2.0)),
            RADIUS - EDGE / 2.0,
            EDGE,
            edge,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Dialog);
        cx.name(self.label.clone());
        cx.focus_scope();
        cx.focus_fallback();
    }
}

fn popover_glass() -> Glass {
    Glass::dense().container(GLASS_CONTAINER, GLASS_BLEND)
}

/// The content of a popover window: the row holding the pointer, and under it the panel.
struct Callout<M> {
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Callout<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
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

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    /// A plain container: the panel under it is the dialog, and its pointer row leaves itself out.
    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The row above the panel that holds its pointer in the middle.
struct PointerRow;

impl<M: Clone + 'static> Widget<M> for PointerRow {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Horizontal,
            spacing: 0.0,
            align: Align::Start,
            justify: Justify::Center,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        vec![CalloutPointer {
            width: POINTER_WIDTH,
            depth: POINTER_DEPTH,
            glass: popover_glass(),
            surface: Surface::Popover,
            fills_without_glass: true,
            up: true,
        }
        .element()
        .key("shape")]
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

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

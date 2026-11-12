use crate::toolbar::{TOOLBAR_EDGE, TOOLBAR_GAP, TOOLBAR_ROW};
use crate::window_drag::DragState;
use crate::{Navigation, NavigationStack, ToolbarGlyph, ToolbarGroup, ToolbarItem};
use cupertino_geometry::{Insets, Size};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_platform::{Keysym, Mods};
use cupertino_ui_theme::Surface;

const BRACKET_LEFT: Keysym = Keysym(0x5b);
const BRACKET_RIGHT: Keysym = Keysym(0x5d);

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

impl<M: Clone + 'static> From<NavigationStack<M>> for Element<M> {
    fn from(stack: NavigationStack<M>) -> Self {
        let message = |to, enabled| {
            stack
                .on_navigate
                .as_ref()
                .filter(|_| enabled)
                .map(|on_navigate| on_navigate(to))
        };
        let back = message(Navigation::Back, stack.can_go_back);
        let forward = message(Navigation::Forward, stack.can_go_forward);
        let arrow = |glyph, label: &str, message: &Option<M>| ToolbarItem {
            key: None,
            glyph,
            label: label.into(),
            on_press: message.clone(),
            menu: None,
        };
        let group = Element::from(ToolbarGroup {
            // the buttons stand in the toolbar on their own, as before: no group of their own
            label: String::new(),
            items: vec![
                arrow(ToolbarGlyph::Back, "Back", &back),
                arrow(ToolbarGlyph::Forward, "Forward", &forward),
            ],
            selected: None,
            dividers: true,
        });
        let header = Element::new(Slot {
            spec: LayoutSpec {
                width: SizeSpec::Fill,
                height: SizeSpec::Fixed(TOOLBAR_ROW),
                align_self: Some(Align::Start),
                insets: Insets {
                    top: 0.0,
                    left: TOOLBAR_EDGE,
                    bottom: 0.0,
                    right: TOOLBAR_EDGE,
                },
                ..LayoutSpec::new(ContainerKind::Stack {
                    axis: Axis::Horizontal,
                    spacing: TOOLBAR_GAP,
                    align: Align::Center,
                    justify: Justify::Start,
                })
            },
            role: Some(Role::Toolbar),
            drag: true,
            // over the page, the row draws in the window's raised layer: the page shows through its glass
            raised: stack.under_toolbar,
            children: std::iter::once(group).chain(stack.toolbar).collect(),
        });
        // the row comes first, for the keyboard and assistive technology; raised over the page, it still draws over it
        // and takes the pointer first
        let children = vec![header, stack.content.fill()];
        Element::new(Navigator {
            back,
            forward,
            under: stack.under_toolbar,
            children,
        })
    }
}

/// A container with a fixed layout and nothing of its own to show; with `drag`, the window's drag region; with
/// `raised`, in the window's raised layer.
struct Slot<M> {
    spec: LayoutSpec,
    role: Option<Role>,
    drag: bool,
    raised: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Slot<M> {
    type State = DragState;

    fn layout(&self) -> LayoutSpec {
        self.spec.clone()
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> DragState {
        DragState::default()
    }

    fn update(&self, old: &Self, _: &mut DragState) -> Change {
        if self.raised != old.raised {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn raised(&self, _: &DragState) -> bool {
        self.raised
    }

    fn measure(&self, _: &DragState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, drag: &mut DragState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if self.drag {
            drag.event(event, cx)
        } else {
            Propagation::Continue
        }
    }

    fn paint(&self, _: &DragState, cx: &mut PaintCx<'_>) {
        // over the page, the row lays a blurred band of the window's colour, there once the page is scrolled
        if self.raised && cx.raised_layer() && self.role == Some(Role::Toolbar) {
            let scrolled = crate::scroll_edge::page_scrolled();
            let shown =
                scrolled && cx.window_active() && cx.material(Surface::ToolbarBand, 0.0, 1.0);
            crate::scroll_edge::set_band_shown(shown);
        }
    }

    fn semantics(&self, _: &DragState, cx: &mut SemanticsCx<'_>) {
        if let Some(role) = self.role {
            cx.role(role);
        }
    }
}

/// The toolbar row over the page; it takes the shortcuts the page leaves.
struct Navigator<M> {
    back: Option<M>,
    forward: Option<M>,
    /// The page fills the pane under the row.
    under: bool,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Navigator<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let kind = if self.under {
            ContainerKind::Overlay
        } else {
            ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            }
        };
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(kind)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.under != old.under {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let Event::Key(key, _) = event else {
            return Propagation::Continue;
        };
        if !key.pressed || !key.mods.contains(Mods::PRIMARY) {
            return Propagation::Continue;
        }
        let message = match key.keysym {
            BRACKET_LEFT => &self.back,
            BRACKET_RIGHT => &self.forward,
            _ => return Propagation::Continue,
        };
        if let Some(message) = message {
            cx.emit(message.clone());
        }
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

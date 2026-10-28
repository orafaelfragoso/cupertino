//! Widgets and an environment for the crate's own tests.

use crate::{
    Change, Element, Environment, Event, EventCx, MeasureCx, PaintCx, Propagation, Role,
    SemanticsCx, Widget,
};
use cupertino_geometry::{Insets, Size};
use cupertino_ui_layout::{
    Constraints, ContainerKind, Flex, LayoutDirection, LayoutSpec, Measured, Overflow, SizeSpec,
};
use cupertino_ui_theme::{
    Appearance, ColorTokens, ControlMetrics, ControlSize, FocusRing, Motion, ThemePreferences,
    ThemeTokens, TypeRole, TypeScale,
};
use std::sync::Arc;

pub(crate) fn env() -> Environment {
    let role = TypeRole {
        size: 13.0,
        weight: 400,
    };
    Environment {
        theme: Arc::new(ThemeTokens {
            preferences: ThemePreferences {
                appearance: Appearance::Light,
                reduce_transparency: false,
                reduce_motion: false,
            },
            colors: ColorTokens::default(),
            metrics: ControlMetrics {
                size: ControlSize::Regular,
                button_height: 20.0,
                button_inset: 10.0,
                button_radius: 5.0,
                field_height: 20.0,
                field_inset: 6.0,
                field_radius: 5.0,
                switch: Size::new(38.0, 22.0),
                switch_thumb: 18.0,
                switch_thumb_inset: 2.0,
                min_target: Size::new(24.0, 24.0),
            },
            type_scale: TypeScale {
                family: "Inter".into(),
                body: role,
                caption: role,
                section: role,
                title: role,
            },
            focus: FocusRing {
                width: 2.0,
                outset: 2.0,
                radius_extra: 3.0,
            },
            motion: Motion {
                reduce: false,
                caret_phase_ms: 500,
            },
        }),
        direction: LayoutDirection::Ltr,
        scale: 1.0,
    }
}

pub(crate) fn spec(kind: ContainerKind) -> LayoutSpec {
    LayoutSpec {
        kind,
        insets: Insets::default(),
        width: SizeSpec::Fit,
        height: SizeSpec::Fit,
        flex: Flex {
            grow: 0.0,
            shrink: 0.0,
            basis: None,
        },
        align_self: None,
        overflow: Overflow::Visible,
    }
}

/// A leaf that records the events it saw and reports its label as a change.
pub(crate) struct Leaf {
    pub(crate) label: &'static str,
    pub(crate) role: Option<Role>,
    pub(crate) focusable: bool,
    pub(crate) stop: bool,
}

pub(crate) fn leaf(label: &'static str) -> Leaf {
    Leaf {
        label,
        role: Some(Role::Button),
        focusable: true,
        stop: false,
    }
}

impl<M: From<String> + 'static> Widget<M> for Leaf {
    /// How many updates the node has seen, so tests can tell a kept node from a new one.
    type State = u32;

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) -> u32 {
        0
    }

    fn update(&self, old: &Self, state: &mut u32) -> Change {
        *state += 1;
        if old.label == self.label {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &u32, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(10.0, 10.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut u32, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let name = match event {
            Event::Pointer(..) => "pointer",
            Event::Key(..) => "key",
            Event::Focus { gained: true, .. } => "focus",
            Event::Focus { .. } => "blur",
            Event::Cancel => "cancel",
            _ => "other",
        };
        cx.emit(M::from(format!("{}:{:?}:{name}", self.label, cx.phase())));
        if self.stop {
            Propagation::Stop
        } else {
            Propagation::Continue
        }
    }

    fn paint(&self, _: &u32, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &u32, cx: &mut SemanticsCx<'_>) {
        if let Some(role) = self.role {
            cx.role(role);
            cx.name(self.label);
        }
        if self.focusable {
            cx.focusable();
        }
    }
}

/// A container with no semantics of its own.
pub(crate) struct Group<M> {
    pub(crate) children: Vec<Element<M>>,
    pub(crate) scope: bool,
}

pub(crate) fn group<M>(children: Vec<Element<M>>) -> Group<M> {
    Group {
        children,
        scope: false,
    }
}

impl<M: From<String> + 'static> Widget<M> for Group<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Overlay)
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
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if matches!(event, Event::Pointer(..) | Event::Key(..)) {
            cx.emit(M::from(format!("group:{:?}", cx.phase())));
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.scope {
            cx.role(Role::Dialog);
            cx.focus_scope();
        }
    }
}

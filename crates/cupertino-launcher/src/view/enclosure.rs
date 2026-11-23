//! The enclosure and its root: the panel's glass, shadow and edge, the root that takes the panel's keys, the colours
//! the rows are drawn in, and the rows that say why nothing shows.

use super::*;

/// The enclosure as it is drawn: the panel's glass (or the panel material's fill without it), its shadow outside its
/// shape and its thin dark edge, at the presented opacity and scale; its content, laid out at the unscaled shape and
/// centred, is cut at its edge.
pub(super) struct Enclosure<M> {
    pub(super) rect: Rect,
    pub(super) radius: f64,
    pub(super) alpha: f64,
    /// How blurred its content is, from 0 to 1 (while it closes).
    pub(super) blur: f64,
    /// Its glass joins the mode buttons' into one shape where they come close.
    pub(super) joins: bool,
    /// The system variant and size class its glass is named by ([`super::enclosure_variant`]).
    pub(super) variant: (GlassVariant, GlassSize),
    /// The mode buttons, in the window, with their corner radii: its edge is not drawn inside them.
    pub(super) joined: Vec<(Rect, f64)>,
    /// Its parts fade themselves at `alpha` ([`Bar`] and the browser's body): the enclosure draws its own edge (and
    /// without glass its shadow and fill) at `alpha` and lays no layer over them.
    pub(super) split: bool,
    /// Its glass, edge and content are layers of their own ([`layers`]): it draws nothing itself.
    pub(super) layered: bool,
    /// It scales in or out: its content keeps its size past its edge, cut off there ([`Overflow::Crop`]).
    pub(super) crops: bool,
    pub(super) content: Size,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Enclosure<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width().max(0.0)),
            height: SizeSpec::Fixed(self.rect.height().max(0.0)),
            position: self.rect.origin,
            overflow: if self.crops {
                Overflow::Crop
            } else {
                Overflow::Clip
            },
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.rect, self.content, self.crops) != (old.rect, old.content, old.crops) {
            Change::Layout
        } else if (
            self.radius,
            self.alpha,
            self.split,
            self.blur,
            self.joins,
            self.layered,
        ) != (
            old.radius,
            old.alpha,
            old.split,
            old.blur,
            old.joins,
            old.layered,
        ) || self.joined != old.joined
            || self.variant != old.variant
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let alpha = self.alpha.clamp(0.0, 1.0);
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = self.radius.min(size.height / 2.0).max(0.0);
        let theme = cx.env().theme.clone();
        let dark = theme.preferences.appearance.is_dark();
        // the compositor draws the large panel's glass and its shadow; a window without glass draws both itself
        let mut material = Glass::large_panel()
            .variant(self.variant.0)
            .size_class(self.variant.1)
            .adaptive(false)
            .radius(radius)
            .alpha(alpha);
        if self.joins {
            material = material.container(modes::JOIN_CONTAINER, modes::JOIN_SPACING);
        }
        if self.layered {
            // the glass layer draws the glass, the compositor the edge along it; the content lies on the glass as in place
            cx.layer_glass(material);
            return;
        }
        let glass = cx.glass(material);
        // what the enclosure's own drawing is multiplied by: the fade itself when its parts fade one by one
        let own = if self.split { alpha as f32 } else { 1.0 };
        let blur = self.blur.clamp(0.0, 1.0) * CLOSE_BLUR;
        let fade = if self.split { 1.0 } else { alpha as f32 };
        if fade < 1.0 || blur > 0.05 {
            let effects = if blur > 0.05 {
                vec![Effect::Blur { radius: blur }]
            } else {
                Vec::new()
            };
            cx.set_group(fade, effects);
        }
        let shape = Shape::RoundedRect { rect, radius };
        if !glass {
            let shadow = if dark {
                SHADOW_ALPHA_DARK
            } else {
                SHADOW_ALPHA_LIGHT
            };
            // the shadow is cast by an opaque copy of the shape that the clip hides, so it never darkens the panel
            let mut ring = rounded_path(rect.outset(2.0 * SHADOW_SIDE), 0.0);
            ring.extend(rounded_path(rect, radius));
            cx.push(PaintOp::PushClip(Shape::Path {
                verbs: ring,
                fill_rule: FillRule::EvenOdd,
            }));
            cx.push(PaintOp::PushLayer {
                opacity: own,
                effects: vec![Effect::DropShadow {
                    offset: Point::new(0.0, SHADOW_OFFSET),
                    blur: SHADOW_BLUR,
                    color: Color::new(0.0, 0.0, 0.0, shadow),
                }],
            });
            cx.push(PaintOp::Fill {
                shape: shape.clone(),
                paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)),
            });
            cx.push(PaintOp::PopLayer);
            cx.push(PaintOp::PopClip);
            if let Some(fill) = theme.surface(Surface::Menu).fill {
                cx.push(PaintOp::Fill {
                    shape,
                    paint: Paint::Solid(Color::new(fill.r, fill.g, fill.b, fill.a * own)),
                });
            }
        }
        let edge = edge_alpha(dark, glass);
        let joined: Vec<(Rect, f64)> = self
            .joined
            .iter()
            .map(|(r, radius)| (r.translate(-self.rect.x(), -self.rect.y()), *radius))
            .collect();
        edge_outside(cx, rect, radius, edge * own, &joined, glass);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The panel's content: takes the panel's keys before the query field does (which keys follows its [`KeyMode`]), and
/// reports when its window loses the keyboard.
pub(super) struct PanelRoot<M> {
    pub(super) keys: KeyMode,
    pub(super) size: Size,
    /// Where it lies in the enclosure: centred across it.
    pub(super) offset: f64,
    /// The compositor runs the close ([`PanelView::handed`]): a key, its release included, reaches nothing inside, as
    /// a field would restart its caret's blink and draw again.
    pub(super) handed: bool,
    /// The compositor runs an open or a fold ([`PanelView::opening`]): a key's release reaches nothing inside.
    pub(super) holding: bool,
    /// A part keeps its own size past its edge, cut off there ([`Overflow::Crop`]): the body as the scope it unfolds
    /// into rests.
    pub(super) crops: bool,
    pub(super) children: Vec<Element<M>>,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for PanelRoot<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width.max(0.0)),
            height: SizeSpec::Fixed(self.size.height.max(0.0)),
            position: Point::new(self.offset, 0.0),
            overflow: if self.crops {
                Overflow::Crop
            } else {
                Overflow::Visible
            },
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    /// It draws nothing: which keys it takes changes no pixel.
    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.size, self.offset, self.crops) != (old.size, old.offset, old.crops) {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Key(..) if self.handed => Propagation::Stop,
            // the primary modifier alone, held and let go: the preview shows a file's location meanwhile
            Event::Key(key, _)
                if matches!(key.keysym.0, SUPER_L | SUPER_R)
                    && (cx.phase() == Phase::Capture || cx.node() == cx.target()) =>
            {
                cx.emit(M::from(Input::Primary(key.pressed)));
                if self.holding && !key.pressed {
                    Propagation::Stop
                } else {
                    Propagation::Continue
                }
            }
            // a release reaches nothing inside while the compositor runs an open or a fold: the field would show its
            // held caret again for it and draw the query row
            Event::Key(key, _) if self.holding && !key.pressed => Propagation::Stop,
            Event::Key(key, _) if cx.phase() == Phase::Capture || cx.node() == cx.target() => {
                match panel_key(key) {
                    Some((k, mods)) if self.keys.takes(k, mods) => {
                        cx.emit(M::from(Input::Key(k, mods)));
                        Propagation::Stop
                    }
                    _ => Propagation::Continue,
                }
            }
            // another window took the keyboard: the model knows whether that was the view options menu; the menu of
            // the query field, a popup the field opened for itself, takes it without the panel closing; when that menu is
            // dismissed and the keyboard went to another surface, the loss is told again with no popup open
            Event::FocusWithin { gained: false, .. }
                if !cx.window_active() && !cx.holds_popup() =>
            {
                cx.emit(M::from(Input::KeyboardLeft));
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    /// The query field is where the panel's keys go: a press on a button or in the content leaves it editing.
    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.keeps_editing();
    }
}

/// The colour of a separator line where `cx` paints: translucent over the panel's glass, the theme's separator over
/// the opaque fill and with Increase Contrast.
pub(crate) fn separator_color(cx: &PaintCx<'_>) -> Option<Color> {
    thin_fill(
        cx,
        GLASS_SEPARATOR_DARK,
        GLASS_SEPARATOR_LIGHT,
        ColorRole::Separator,
    )
}

/// The fill of a selected result, row or cell where `cx` paints: a thin fill of the label's tone over the panel's
/// glass, the theme's selection colour over the opaque fill and with Increase Contrast.
pub(crate) fn selection_color(cx: &PaintCx<'_>) -> Option<Color> {
    thin_fill(
        cx,
        GLASS_SELECTION_DARK,
        GLASS_SELECTION_LIGHT,
        ColorRole::SecondarySelectedControlFill,
    )
}

/// The fill of a category filter that is off where `cx` paints: over the panel's glass a white lift, lighter than the
/// panel around it in either appearance; over the opaque fill and with Increase Contrast none (the caller's own).
pub(crate) fn lift_color(cx: &PaintCx<'_>) -> Option<Color> {
    let appearance = cx.env().theme.preferences.appearance;
    (cx.on_glass() && !appearance.is_high_contrast()).then(|| {
        let a = if appearance.is_dark() {
            GLASS_LIFT_DARK
        } else {
            GLASS_LIFT_LIGHT
        };
        Color::new(1.0, 1.0, 1.0, a)
    })
}

/// Over the glass (and without Increase Contrast), white at `dark` opacity in the dark appearance and black at `light`
/// in the light one; elsewhere the theme's `role`.
pub(super) fn thin_fill(cx: &PaintCx<'_>, dark: f32, light: f32, role: ColorRole) -> Option<Color> {
    let theme = &cx.env().theme;
    let appearance = theme.preferences.appearance;
    if cx.on_glass() && !appearance.is_high_contrast() {
        Some(if appearance.is_dark() {
            Color::new(1.0, 1.0, 1.0, dark)
        } else {
            Color::new(0.0, 0.0, 0.0, light)
        })
    } else {
        theme.colors.get(role, StateVariant::NORMAL)
    }
}

/// A hairline across the panel along `axis`, in [`separator_color`]: under the query row and between the results and
/// the preview.
pub(super) struct Hairline {
    pub(super) axis: Axis,
}

impl<M: 'static> Widget<M> for Hairline {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let (width, height) = match self.axis {
            Axis::Horizontal => (SizeSpec::Fill, SizeSpec::Fixed(1.0)),
            Axis::Vertical => (SizeSpec::Fixed(1.0), SizeSpec::Fill),
        };
        LayoutSpec {
            width,
            height,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.axis != old.axis {
            Change::Layout
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        if let Some(color) = separator_color(cx) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// What the row saying why nothing shows says of `status`; `None` when it does not show.
pub fn status_text(status: &Status) -> Option<&'static str> {
    match status {
        Status::Searching => Some(SEARCHING),
        Status::NoResults => Some(NO_RESULTS),
        Status::Failed(_) => Some(INCOMPLETE),
        Status::Idle | Status::Results => None,
    }
}

/// Why nothing shows, centred in its row: still searching, nothing found, or that a source failed (so what shows may be
/// incomplete); or that the preview did not come in time. A search's status is read where it is, and
/// [`StatusAnnouncer`] says it aloud once the query settled; the preview's is said as it shows (`live`).
pub(super) struct StatusRow {
    pub(super) text: &'static str,
    pub(super) failed: bool,
    pub(super) live: bool,
}

impl<M: 'static> Widget<M> for StatusRow {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(STATUS_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.text, self.failed, self.live) != (old.text, old.failed, old.live) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, STATUS_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let theme = &cx.env().theme;
        let color = theme
            .colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL)
            .unwrap_or(Color::new(0.0, 0.0, 0.0, 0.5));
        let style = TextStyle {
            weight: FontWeight(500),
            ..TextStyle::new(15.0)
        };
        let paragraph = ParagraphStyle {
            wrap: Wrap::None,
            max_lines: Some(1),
            ellipsis: true,
            align: TextAlign::Center,
            ..ParagraphStyle::default()
        };
        let Ok(text) = cx
            .env()
            .text
            .paragraph(&AttributedText::plain(self.text, style), &paragraph)
        else {
            return;
        };
        let layout = text.layout(Some((size.width - 2.0 * SIDE_PADDING).max(0.0) as f32));
        let h = layout.size().height;
        glyphs(
            cx,
            &layout,
            Point::new(SIDE_PADDING, (size.height - h) / 2.0),
            color,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.to_owned());
        if self.live {
            cx.live(Live::Polite);
        }
    }
}

/// The status said aloud for a settled query: a live region of no size that draws nothing, added once per query with
/// what it says, so a screen reader says it once. A failure is said at once, anything else politely.
pub(super) struct StatusAnnouncer {
    pub(super) text: &'static str,
    pub(super) failed: bool,
}

impl<M: 'static> Widget<M> for StatusAnnouncer {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(0.0),
            height: SizeSpec::Fixed(0.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.text, self.failed) != (old.text, old.failed) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
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
        cx.role(Role::StaticText);
        cx.name(self.text.to_owned());
        cx.live(if self.failed {
            Live::Assertive
        } else {
            Live::Polite
        });
    }
}

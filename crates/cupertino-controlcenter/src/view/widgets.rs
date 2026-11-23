//! The panel's widgets: the board, the glass tiles, the pressable controls, the levels and the handles that move a tile.

use super::*;

/// A box at a place in its parent that lays its children out as `kind` says, clipping them.
pub(super) struct Place<M> {
    pub(super) frame: Rect,
    pub(super) kind: ContainerKind,
    /// What the box is to assistive technology, when it is something: a list with its name and its number of rows.
    pub(super) list: Option<(String, usize)>,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Place<M> {
    /// `children` stacked on `axis` in `frame`, centred across it; `centre` also centres them along it.
    pub(super) fn stack(
        frame: Rect,
        axis: Axis,
        centre: bool,
        children: Vec<Element<M>>,
    ) -> Element<M> {
        let justify = if centre || axis == Axis::Vertical {
            Justify::Center
        } else {
            Justify::Start
        };
        let align = if axis == Axis::Vertical && !centre {
            Align::Start
        } else {
            Align::Center
        };
        Element::new(Place {
            frame,
            kind: ContainerKind::Stack {
                axis,
                spacing: 0.0,
                align,
                justify,
            },
            list: None,
            children,
        })
    }

    /// `child` centred in `frame`.
    pub(super) fn centred(frame: Rect, child: Element<M>) -> Element<M> {
        Self::stack(frame, Axis::Horizontal, true, vec![child])
    }

    /// `children` at their own positions in `frame`.
    pub(super) fn absolute(frame: Rect, children: Vec<Element<M>>) -> Element<M> {
        Element::new(Place {
            frame,
            kind: ContainerKind::Absolute,
            list: None,
            children,
        })
    }

    /// The rows of a list named `name`, at their own positions in `frame`.
    pub(super) fn list(frame: Rect, name: &str, children: Vec<Element<M>>) -> Element<M> {
        Element::new(Place {
            frame,
            kind: ContainerKind::Absolute,
            list: Some((name.to_owned(), children.len())),
            children,
        })
    }
}

pub(super) fn spec(kind: ContainerKind, frame: Rect) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fixed(frame.width()),
        height: SizeSpec::Fixed(frame.height()),
        overflow: Overflow::Clip,
        position: frame.origin,
        ..LayoutSpec::new(kind)
    }
}

impl<M: 'static> Widget<M> for Place<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(self.kind.clone(), self.frame)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.frame != old.frame || self.kind != old.kind {
            Change::Layout
        } else if self.list != old.list {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if let Some((name, count)) = &self.list {
            cx.role(Role::List);
            cx.name(name.clone());
            cx.item_count(*count);
        }
    }
}

/// The whole window: the tiles at their places. With Reduce Transparency it draws the one opaque panel under the
/// tiles. A left or right press on it, between or around the tiles, collapses the open detail view or dismisses the
/// panel; while the controls are edited it does nothing, so a press that misses a tile loses no edit.
pub(super) struct Board<M> {
    pub(super) alpha: f64,
    /// Height of the tiles the opaque panel of Reduce Transparency goes around: the grid and Edit Controls, or an open
    /// detail view.
    pub(super) height: f64,
    /// The outline of what the soft shadow goes around, relative to the grid's corner.
    pub(super) shadow: Rect,
    /// The pictures of the shadow's bands the panel keeps, among them perhaps the one of this outline at the window's
    /// scale.
    pub(super) bands: Vec<Bands>,
    /// The tiles' outlines, relative to the grid's corner, that the shadow is cut away over by their opacity.
    pub(super) cutouts: Vec<Cutout>,
    pub(super) on_outside: Option<M>,
    /// What the panel hears when its window loses the keyboard while the focus is on the board or within it.
    pub(super) on_inactive: Option<M>,
    pub(super) children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Board<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            overflow: Overflow::Visible,
            ..spec(
                ContainerKind::Absolute,
                Rect::from_origin_size(Point::ZERO, SURFACE),
            )
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.alpha != old.alpha
            || self.height != old.height
            || self.shadow != old.shadow
            || self.bands != old.bands
            || self.cutouts != old.cutouts
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: SURFACE,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left | MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture => {
                if let Some(outside) = &self.on_outside {
                    cx.emit(outside.clone());
                }
                Propagation::Stop
            }
            // the window lost the keyboard: the focus on the board heard it, or the focus on a control within it
            Event::WindowActive(false) => {
                if let Some(inactive) = &self.on_inactive {
                    cx.emit(inactive.clone());
                }
                Propagation::Continue
            }
            Event::FocusWithin { gained: false, .. } if !cx.window_active() => {
                if let Some(inactive) = &self.on_inactive {
                    cx.emit(inactive.clone());
                }
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let alpha = finite_alpha(self.alpha);
        if alpha < 1.0 {
            cx.set_group(alpha as f32, Vec::new());
        }
        if reduce_transparency(cx) {
            let color = by_appearance(cx, RT_GROUP, RT_GROUP_LIGHT);
            fill(cx, group(self.height), layout::GROUP_RADIUS, color);
        } else {
            // the tiles look the same in both appearances: a mid-toned glass with light labels and symbols
            cx.pin_appearance(true);
        }
        if !reduce_transparency(cx) && !increase_contrast(cx) {
            let cutouts: Vec<Cutout> = self
                .cutouts
                .iter()
                .map(|c| Cutout {
                    rect: c.rect.translate(GRID_ORIGIN.x, GRID_ORIGIN.y),
                    ..*c
                })
                .collect();
            // the shadow fades out by the menu bar's edge above the modules and never reaches into the bar
            let (rect, top_reach) = outline(self.shadow);
            cx.push(PaintOp::PushClip(Shape::Rect(Rect::new(
                -1.0e4,
                bar_edge(),
                2.0e4,
                2.0e4,
            ))));
            shadow(cx, rect, top_reach, &cutouts, &self.bands);
            cx.push(PaintOp::PopClip);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(TITLE);
    }
}

/// The panel's outline around tiles `height` tall, in the window: [`INSET`] around them, with the corners of
/// [`layout::GROUP_RADIUS`].
pub(super) fn group(height: f64) -> Rect {
    Rect::new(
        WINDOW_ORIGIN.x,
        WINDOW_ORIGIN.y,
        WINDOW.width,
        height + 2.0 * INSET,
    )
}

/// The panel behind the tiles: clear glass in the panel's outline ([`group`]), as macOS lays its modules on a clear
/// glass backdrop, on its own so it never blends with the tiles' container. With Reduce Transparency it draws nothing:
/// the board paints the opaque panel there. Presses go through to the board.
pub(super) struct PanelGlass {
    /// Height of the tiles the panel goes around, as the board's.
    pub(super) height: f64,
    /// The panel's opacity.
    pub(super) alpha: f64,
}

impl<M: 'static> Widget<M> for PanelGlass {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf, group(self.height))
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.height != old.height {
            Change::Layout
        } else if self.alpha != old.alpha {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: group(self.height).size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !reduce_transparency(cx) {
            let glass = Glass::clear()
                .radius(layout::GROUP_RADIUS)
                .alpha(finite_alpha(self.alpha));
            cx.glass(glass);
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// One tile: glass under its outline, its content over it. A press on the tile outside its controls does nothing.
pub(super) struct GlassTile<M> {
    pub(super) label: String,
    pub(super) frame: Rect,
    pub(super) radius: f64,
    pub(super) capsule: bool,
    /// The panel's opacity: it reaches the glass but not the content, which the board fades.
    pub(super) panel: f64,
    /// The tile's own opacity, glass and content.
    pub(super) alpha: f64,
    pub(super) light: bool,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for GlassTile<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Absolute, self.frame)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if self.radius != old.radius
            || self.capsule != old.capsule
            || self.panel != old.panel
            || self.alpha != old.alpha
            || self.light != old.light
            || self.label != old.label
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _)
                if cx.phase() != Phase::Capture =>
            {
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let alpha = finite_alpha(self.alpha);
        if alpha < 1.0 {
            cx.set_group(alpha as f32, Vec::new());
        }
        let size = cx.size();
        let rect = Rect::from_origin_size(Point::ZERO, size);
        let radius = if self.capsule {
            size.width.min(size.height) / 2.0
        } else {
            self.radius
        };
        if reduce_transparency(cx) {
            let color = by_appearance(cx, RT_TILE, RT_TILE_LIGHT);
            fill(cx, rect, radius, color);
            return;
        }
        let shape = if self.capsule {
            Glass::module().capsule()
        } else {
            Glass::module().radius(radius)
        };
        let glass = shape
            .container(CONTAINER, BLEND)
            .alpha(finite_alpha(self.panel) * alpha);
        if !cx.glass(glass) {
            let color = by_appearance(cx, FALLBACK, FALLBACK_LIGHT);
            fill(cx, rect, radius, color);
        }
        if increase_contrast(cx) {
            // the glass takes the contrast face; the tile marks its edge with a rim
            let rim_color = by_appearance(cx, IC_RIM_COLOR, IC_RIM_COLOR_LIGHT);
            let width = ic_rim_width(cx.env().scale);
            let rim = Rect::new(
                width / 2.0,
                width / 2.0,
                size.width - width,
                size.height - width,
            );
            cx.push(PaintOp::Stroke {
                shape: Shape::RoundedRect {
                    rect: rim,
                    radius: (radius - width / 2.0).max(0.0),
                },
                width,
                paint: Paint::Solid(rim_color),
            });
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(self.label.clone());
    }
}

/// How a pressable control looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Face {
    /// A disc: white when on, a light veil when off.
    Badge { on: bool },
    /// A whole circular tile: a white disc when on, the glass alone when off.
    Circle { on: bool },
    /// Nothing at rest; a highlight under the pointer when `hover` is set, a veil while pressed.
    Plain { radius: f64, hover: bool },
    /// The mark that removes a tile: a disc with a bar.
    Remove,
    /// A round chip that adds a control: the glass alone, with a small disc with a cross on its corner.
    Add,
}

impl Face {
    pub(super) fn on(self) -> bool {
        matches!(self, Face::Badge { on: true } | Face::Circle { on: true })
    }
}

/// What a pressable control is to assistive technology.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Kind {
    /// A switch with its state.
    Switch(bool),
    Button,
    /// A button that opens a view; `Some(open)` once it can tell.
    Expander(bool),
    /// The row at `index` of a list, `selected` when it is the current one.
    Row {
        index: usize,
        selected: bool,
    },
}

/// Where the arrow keys lead from a control: the controls of the panel with their rects in the window.
pub(super) type Map = Rc<RefCell<Vec<(String, Rect)>>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Press {
    pub(super) hovered: bool,
    pub(super) pointer: bool,
    pub(super) key: bool,
    pub(super) inside: bool,
}

impl Press {
    pub(super) fn pressed(&self) -> bool {
        self.key || (self.pointer && self.inside)
    }
}

/// A control taken by a press, Space or Return, or assistive technology; the arrow keys move the focus to the
/// nearest control that way.
pub(super) struct Pressable<M> {
    pub(super) key: String,
    pub(super) label: String,
    pub(super) description: Option<String>,
    pub(super) kind: Kind,
    pub(super) face: Face,
    pub(super) frame: Rect,
    pub(super) enabled: bool,
    pub(super) on_press: Option<Input>,
    pub(super) map: Map,
    pub(super) to: fn(Input) -> M,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Pressable<M> {
    pub(super) fn radius(&self, size: Size) -> f64 {
        match self.face {
            Face::Badge { .. } | Face::Circle { .. } | Face::Remove | Face::Add => {
                size.width.min(size.height) / 2.0
            }
            Face::Plain { radius, .. } => radius,
        }
    }

    pub(super) fn arrow(&self, keysym: Keysym, cx: &mut EventCx<'_, M>) -> bool {
        let Some(from) = cx.bounds() else {
            return false;
        };
        let direction = match keysym {
            LEFT => (-1.0, 0.0),
            RIGHT => (1.0, 0.0),
            UP => (0.0, -1.0),
            DOWN => (0.0, 1.0),
            _ => return false,
        };
        let map = self.map.borrow();
        if let Some(key) = nearest(&map, &self.key, from, direction) {
            cx.emit((self.to)(Input::Focus(key)));
        }
        true
    }
}

/// A mark in `disc`: a disc with a bar across it, and with `cross` a second bar down it. White with a dark sign where
/// the tiles are dark, dark with a white sign where they are light.
pub(super) fn mark(cx: &mut PaintCx<'_>, disc: Rect, cross: bool) {
    let color = by_appearance(cx, MARK_DISC, MARK_DISC_LIGHT);
    let sign = by_appearance(cx, MARK_SIGN, MARK_SIGN_LIGHT);
    fill(cx, disc, disc.width().min(disc.height()) / 2.0, color);
    let centre = disc.center();
    let (length, width) = (MARK_SIGN_LENGTH.min(disc.width() - 6.0), MARK_SIGN_WIDTH);
    let bar = Rect::new(
        centre.x - length / 2.0,
        centre.y - width / 2.0,
        length,
        width,
    );
    fill(cx, bar, width / 2.0, sign);
    if cross {
        let bar = Rect::new(
            centre.x - width / 2.0,
            centre.y - length / 2.0,
            width,
            length,
        );
        fill(cx, bar, width / 2.0, sign);
    }
}

/// The control of `map` nearest to `from` in `direction`, other than `own`: among those whose centre lies that way,
/// the one with the least distance along the direction plus twice the distance across it.
pub(super) fn nearest(
    map: &[(String, Rect)],
    own: &str,
    from: Rect,
    direction: (f64, f64),
) -> Option<String> {
    let start = from.center();
    map.iter()
        .filter(|(key, _)| key != own)
        .filter_map(|(key, rect)| {
            let c = rect.center();
            let (dx, dy) = (c.x - start.x, c.y - start.y);
            let along = dx * direction.0 + dy * direction.1;
            let across = (dx * direction.1 - dy * direction.0).abs();
            (along > 0.5).then_some((along + 2.0 * across, key))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, key)| key.clone())
}

impl<M: 'static> Widget<M> for Pressable<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        spec(
            ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Center,
            },
            self.frame,
        )
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if self.face != old.face
            || self.kind != old.kind
            || self.enabled != old.enabled
            || self.label != old.label
            || self.description != old.description
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, state: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let enabled = self.enabled && self.on_press.is_some();
        let activate = |cx: &mut EventCx<'_, M>, via: Via| {
            if let Some(input) = self.on_press.clone() {
                let input = match via {
                    Via::Pointer => input,
                    Via::Keyboard => input.by_key(),
                };
                cx.emit((self.to)(input));
            }
        };
        match *event {
            Event::Pointer(PointerEvent::Enter { .. }, _) if cx.node() == cx.target() => {
                state.hovered = true;
                state.inside |= state.pointer;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                state.hovered = false;
                state.inside = false;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                if enabled && !state.pointer && !state.key {
                    state.pointer = true;
                    state.inside = true;
                    cx.capture_pointer();
                    cx.request_paint();
                }
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if state.pointer => {
                let inside = cx.hit(position);
                if inside != state.inside {
                    state.inside = inside;
                    cx.request_paint();
                }
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if state.pointer => {
                cx.release_pointer();
                let inside = cx.hit(position);
                *state = Press {
                    hovered: state.hovered,
                    ..Press::default()
                };
                cx.request_paint();
                if enabled && inside {
                    activate(cx, Via::Pointer);
                }
                Propagation::Stop
            }
            Event::Cancel => {
                *state = Press {
                    hovered: state.hovered,
                    ..Press::default()
                };
                cx.request_paint();
                Propagation::Continue
            }
            Event::Focus { gained: false, .. } if state.key => {
                state.key = false;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Key(ref key, _) if cx.node() == cx.target() => match key.keysym {
                SPACE if key.pressed && !key.repeat && enabled && !state.pointer => {
                    state.key = true;
                    cx.request_paint();
                    Propagation::Stop
                }
                SPACE if !key.pressed && state.key => {
                    state.key = false;
                    cx.request_paint();
                    activate(cx, Via::Keyboard);
                    Propagation::Stop
                }
                RETURN | KP_ENTER if key.pressed && !key.repeat && enabled => {
                    activate(cx, Via::Keyboard);
                    Propagation::Stop
                }
                LEFT | RIGHT | UP | DOWN if key.pressed && self.arrow(key.keysym, cx) => {
                    Propagation::Stop
                }
                _ => Propagation::Continue,
            },
            Event::Action(ref request) => match request.action {
                Action::Click | Action::Expand if enabled => {
                    activate(cx, Via::Keyboard);
                    Propagation::Stop
                }
                Action::Focus => {
                    cx.request_focus();
                    Propagation::Stop
                }
                _ => Propagation::Continue,
            },
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &Press, cx: &mut PaintCx<'_>) {
        if !self.enabled {
            cx.set_group(DISABLED_ALPHA, Vec::new());
        }
        let size = cx.size();
        let rect = Rect::from_origin_size(Point::ZERO, size);
        let radius = self.radius(size);
        match self.face {
            Face::Badge { on: true } | Face::Circle { on: true } => fill(cx, rect, radius, ON_FILL),
            Face::Badge { on: false } => {
                let alpha = if dark(cx) { OFF_BADGE } else { OFF_BADGE_LIGHT };
                fill(cx, rect, radius, veil(cx, alpha))
            }
            Face::Plain { hover: true, .. } if state.hovered && self.enabled => {
                fill(cx, rect, radius, veil(cx, HOVER_ROW))
            }
            Face::Remove => mark(cx, rect, false),
            Face::Add => {
                let corner = Rect::new(0.0, 0.0, editing::CHIP_MARK, editing::CHIP_MARK);
                mark(cx, corner, true)
            }
            _ => {}
        }
        if state.pressed() {
            let shade = if self.face.on() {
                PRESSED_ON
            } else if self.face == Face::Remove {
                // the mark's disc is of the tone a veil would have: it is shaded the other way
                by_appearance(cx, PRESSED_ON, Color::new(1.0, 1.0, 1.0, 0.2))
            } else {
                veil(cx, PRESSED_OFF)
            };
            fill(cx, rect, radius, shade);
        }
        if cx.focus_visible() && cx.window_active() {
            let theme = cx.env().theme.clone();
            let ring = &theme.focus;
            let color = theme
                .colors
                .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL);
            if let Some(color) = color {
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: rect.outset(ring.outset + ring.width / 2.0),
                        radius: radius + ring.outset + ring.width / 2.0,
                    },
                    width: ring.width,
                    paint: Paint::Solid(color),
                });
            }
        }
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        let mut states = AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        };
        match self.kind {
            Kind::Switch(on) => {
                cx.role(Role::Switch);
                cx.value(AccessibleValue::Toggle(if on {
                    Checked::True
                } else {
                    Checked::False
                }));
            }
            Kind::Button => cx.role(Role::Button),
            Kind::Expander(open) => {
                cx.role(Role::Button);
                states.expanded = Some(open);
            }
            Kind::Row { index, selected } => {
                cx.role(Role::ListItem);
                cx.item_index(index);
                states.selected = selected;
            }
        }
        cx.name(self.label.clone());
        if let Some(description) = &self.description {
            cx.description(description.clone());
        }
        cx.states(states);
        if self.enabled && self.on_press.is_some() {
            cx.focusable();
            cx.action(Action::Click);
            if matches!(self.kind, Kind::Expander(false)) {
                cx.action(Action::Expand);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Drag {
    pub(super) dragging: bool,
}

/// A level: a track of glass of its own over its tile's, filled in white up to the level, with no knob. A press or drag
/// puts the level under the pointer; Left and Right, Home and End step it. Where the tile is opaque (Reduce
/// Transparency, Increase Contrast) or the window shows no glass, the track is a veil.
pub(super) struct Level<M> {
    pub(super) key: String,
    pub(super) label: String,
    pub(super) slider: Slider,
    pub(super) frame: Rect,
    pub(super) value: Option<f64>,
    /// The panel's opacity: it reaches the track's glass.
    pub(super) panel: f64,
    /// The level shows and does not act: the controls are being edited.
    pub(super) inert: bool,
    pub(super) map: Map,
    pub(super) to: fn(Input) -> M,
}

impl<M: 'static> Level<M> {
    pub(super) fn at(&self, x: f64, bounds: Rect) -> f64 {
        let width = (bounds.width() - 2.0 * LEVEL_REACH).max(1.0);
        ((x - bounds.x() - LEVEL_REACH) / width).clamp(0.0, 1.0)
    }

    pub(super) fn step(&self, value: f64, cx: &mut EventCx<'_, M>) {
        let value = value.clamp(0.0, 1.0);
        cx.emit((self.to)(Input::Slide(self.slider, value)));
        cx.emit((self.to)(Input::Release(self.slider, value)));
    }
}

impl<M: 'static> Widget<M> for Level<M> {
    type State = Drag;

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf, self.frame)
    }

    fn state(&self) -> Drag {
        Drag::default()
    }

    fn update(&self, old: &Self, _: &mut Drag) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if self.value != old.value
            || self.label != old.label
            || self.panel != old.panel
            || self.inert != old.inert
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Drag, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, state: &mut Drag, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture || self.inert {
            return Propagation::Continue;
        }
        let Some(value) = self.value else {
            return match event {
                Event::Pointer(PointerEvent::Button { .. }, _) => Propagation::Stop,
                _ => Propagation::Continue,
            };
        };
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed,
                    ..
                },
                _,
            ) => {
                if let Some(bounds) = cx.bounds() {
                    if pressed && !state.dragging {
                        state.dragging = true;
                        cx.capture_pointer();
                        cx.emit((self.to)(Input::Slide(
                            self.slider,
                            self.at(position.x, bounds),
                        )));
                    } else if !pressed && state.dragging {
                        state.dragging = false;
                        cx.release_pointer();
                        cx.emit((self.to)(Input::Release(self.slider, value)));
                    }
                }
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if state.dragging => {
                if let Some(bounds) = cx.bounds() {
                    cx.emit((self.to)(Input::Slide(
                        self.slider,
                        self.at(position.x, bounds),
                    )));
                }
                Propagation::Stop
            }
            Event::Cancel if state.dragging => {
                state.dragging = false;
                cx.emit((self.to)(Input::Release(self.slider, value)));
                Propagation::Continue
            }
            Event::Key(ref key, _) if key.pressed && cx.node() == cx.target() => {
                let next = match key.keysym {
                    RIGHT => value + KEY_STEP,
                    LEFT => value - KEY_STEP,
                    HOME => 0.0,
                    END => 1.0,
                    UP | DOWN => {
                        let Some(from) = cx.bounds() else {
                            return Propagation::Continue;
                        };
                        let direction = if key.keysym == UP { -1.0 } else { 1.0 };
                        let found = nearest(&self.map.borrow(), &self.key, from, (0.0, direction));
                        if let Some(key) = found {
                            cx.emit((self.to)(Input::Focus(key)));
                        }
                        return Propagation::Stop;
                    }
                    _ => return Propagation::Continue,
                };
                self.step(next, cx);
                Propagation::Stop
            }
            Event::Action(ref request) => {
                let next = match (request.action, &request.data) {
                    (Action::Increment, _) => value + KEY_STEP,
                    (Action::Decrement, _) => value - KEY_STEP,
                    (Action::SetValue, Some(ActionData::Number(v))) => *v,
                    (Action::Focus, _) => {
                        cx.request_focus();
                        return Propagation::Stop;
                    }
                    _ => return Propagation::Continue,
                };
                self.step(next, cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &Drag, cx: &mut PaintCx<'_>) {
        if self.value.is_none() {
            cx.set_group(DISABLED_ALPHA, Vec::new());
        }
        let size = cx.size();
        let track = Rect::from_origin_size(Point::ZERO, size);
        let radius = size.height / 2.0;
        let glass = !reduce_transparency(cx)
            && !increase_contrast(cx)
            && cx.glass(Glass::track().capsule().alpha(finite_alpha(self.panel)));
        // over its glass the track keeps a faint veil, so its empty part still reads as a track
        fill(
            cx,
            track,
            radius,
            veil(cx, if glass { TRACK_ON_GLASS } else { TRACK_EMPTY }),
        );
        let level = self.value.unwrap_or(0.0).clamp(0.0, 1.0);
        if level > 0.0 {
            // the level runs over the reach the pointer is taken past the track's ends
            let width = LEVEL_REACH + (size.width - 2.0 * LEVEL_REACH) * level;
            let filled = Rect::new(0.0, 0.0, width.max(size.height), size.height);
            fill(cx, filled, radius, TRACK_FILL);
        }
        if cx.focus_visible() && cx.window_active() {
            let theme = cx.env().theme.clone();
            if let Some(color) = theme
                .colors
                .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
            {
                let ring = &theme.focus;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: track.outset(ring.outset + ring.width / 2.0 + 2.0),
                        radius: radius + ring.outset + 2.0,
                    },
                    width: ring.width,
                    paint: Paint::Solid(color),
                });
            }
        }
    }

    fn semantics(&self, _: &Drag, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Slider);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Number {
            value: self.value.unwrap_or(0.0),
            min: 0.0,
            max: 1.0,
            step: Some(KEY_STEP),
        });
        cx.states(AccessibleStates {
            disabled: self.value.is_none(),
            ..AccessibleStates::default()
        });
        if self.value.is_some() && !self.inert {
            cx.focusable();
            cx.action(Action::Increment);
            cx.action(Action::Decrement);
            cx.action(Action::SetValue);
        }
    }
}

/// The pointer on a tile while the controls are edited.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Hold {
    /// Where the button went down, and how far that is from the tile's corner.
    pub(super) pressed: Option<(Point, Point)>,
    /// The pointer moved far enough to hold the tile.
    pub(super) dragging: bool,
}

/// What moves a tile while the controls are edited, over the whole tile and its content: a drag holds the tile and
/// carries it; the arrow keys move it one place earlier or later, and Delete or Backspace removes it. To assistive
/// technology it is the tile's place in the order, stepped down and up.
pub(super) struct Handle<M> {
    pub(super) module: Module,
    pub(super) label: String,
    /// The tile's place in the order, from 1, and how many tiles there are.
    pub(super) place: usize,
    pub(super) count: usize,
    pub(super) frame: Rect,
    pub(super) radius: f64,
    pub(super) to: fn(Input) -> M,
}

impl<M: 'static> Handle<M> {
    pub(super) fn let_go(&self, state: &mut Hold, cx: &mut EventCx<'_, M>) {
        if std::mem::take(state).dragging {
            cx.emit((self.to)(Input::Drop));
        }
    }
}

impl<M: 'static> Widget<M> for Handle<M> {
    type State = Hold;

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf, self.frame)
    }

    fn state(&self) -> Hold {
        Hold::default()
    }

    fn update(&self, old: &Self, _: &mut Hold) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if self.place != old.place
            || self.count != old.count
            || self.label != old.label
            || self.radius != old.radius
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Hold, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, state: &mut Hold, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                if let (None, Some(bounds)) = (state.pressed, cx.bounds()) {
                    let grab = Point::new(position.x - bounds.x(), position.y - bounds.y());
                    state.pressed = Some((position, grab));
                    cx.capture_pointer();
                }
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                let Some((start, grab)) = state.pressed else {
                    return Propagation::Continue;
                };
                let moved = (position.x - start.x).hypot(position.y - start.y);
                if state.dragging || moved >= editing::DRAG_SLOP {
                    state.dragging = true;
                    let origin = Point::new(
                        position.x - grab.x - GRID_ORIGIN.x,
                        position.y - grab.y - GRID_ORIGIN.y,
                    );
                    cx.emit((self.to)(Input::DragTo(self.module, origin)));
                }
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if state.pressed.is_some() => {
                cx.release_pointer();
                self.let_go(state, cx);
                Propagation::Stop
            }
            Event::Cancel => {
                self.let_go(state, cx);
                Propagation::Continue
            }
            Event::Key(ref key, _) if key.pressed && cx.node() == cx.target() => {
                let input = match key.keysym {
                    LEFT | UP => Input::Step(self.module, false),
                    RIGHT | DOWN => Input::Step(self.module, true),
                    DELETE | BACKSPACE if !key.repeat => Input::Remove(self.module, Via::Keyboard),
                    _ => return Propagation::Continue,
                };
                cx.emit((self.to)(input));
                Propagation::Stop
            }
            Event::Action(ref request) => {
                let forward = match request.action {
                    Action::Increment => true,
                    Action::Decrement => false,
                    Action::Focus => {
                        cx.request_focus();
                        return Propagation::Stop;
                    }
                    _ => return Propagation::Continue,
                };
                cx.emit((self.to)(Input::Step(self.module, forward)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &Hold, cx: &mut PaintCx<'_>) {
        if cx.focus_visible() && cx.window_active() {
            let theme = cx.env().theme.clone();
            let ring = &theme.focus;
            let color = theme
                .colors
                .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL);
            if let Some(color) = color {
                // inside the tile's edge: the tile clips what it holds
                let inset = ring.width / 2.0;
                let rect = Rect::from_origin_size(Point::ZERO, cx.size());
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: rect.outset(-inset),
                        radius: (self.radius - inset).max(0.0),
                    },
                    width: ring.width,
                    paint: Paint::Solid(color),
                });
            }
        }
    }

    fn semantics(&self, _: &Hold, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Slider);
        cx.name(self.label.clone());
        cx.description(format!(
            "Place {} of {}. The arrow keys move it, Delete removes it.",
            self.place, self.count
        ));
        cx.value(AccessibleValue::Number {
            value: self.place as f64,
            min: 1.0,
            max: self.count as f64,
            step: Some(1.0),
        });
        cx.focusable();
        if self.place > 1 {
            cx.action(Action::Decrement);
        }
        if self.place < self.count {
            cx.action(Action::Increment);
        }
    }
}

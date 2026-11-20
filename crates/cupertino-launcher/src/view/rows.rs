//! The widgets the panel's rows are made of: the query bar, the result list and its sections, the result rows, their
//! labels and text, the actions and the catcher.

use super::*;

/// `rect` with its corners rounded by `radius`, as a path of four quarter circles.
pub(super) fn rounded_path(rect: Rect, radius: f64) -> Vec<PathVerb> {
    // a quarter circle's control points sit this far along its tangents, as a fraction of the radius
    const KAPPA: f64 = 0.552_284_75;
    let (l, t, r, b) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let k = radius * KAPPA;
    vec![
        PathVerb::MoveTo(Point::new(l + radius, t)),
        PathVerb::LineTo(Point::new(r - radius, t)),
        PathVerb::CubicTo(
            Point::new(r - radius + k, t),
            Point::new(r, t + radius - k),
            Point::new(r, t + radius),
        ),
        PathVerb::LineTo(Point::new(r, b - radius)),
        PathVerb::CubicTo(
            Point::new(r, b - radius + k),
            Point::new(r - radius + k, b),
            Point::new(r - radius, b),
        ),
        PathVerb::LineTo(Point::new(l + radius, b)),
        PathVerb::CubicTo(
            Point::new(l + radius - k, b),
            Point::new(l, b - radius + k),
            Point::new(l, b - radius),
        ),
        PathVerb::LineTo(Point::new(l, t + radius)),
        PathVerb::CubicTo(
            Point::new(l, t + radius - k),
            Point::new(l + radius - k, t),
            Point::new(l + radius, t),
        ),
        PathVerb::Close,
    ]
}

/// A row of its children across the panel, centred vertically, `leading` points in from the leading edge and the
/// side padding in from the trailing one.
pub(super) struct Bar<M> {
    pub(super) height: f64,
    pub(super) leading: f64,
    pub(super) spacing: f64,
    /// The opacity it is drawn at, as one layer when under 1.
    pub(super) alpha: f64,
    /// The compositor layer of its own it draws into, while the panel is drawn as layers ([`layers::ROW_LAYER`]).
    pub(super) layer: Option<u64>,
    /// It lies on whole device pixels as one picture ([`Widget::on_pixels`]): while the panel scales or moves.
    pub(super) on_pixels: bool,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Bar<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.height),
            insets: Insets {
                top: 0.0,
                left: self.leading,
                bottom: 0.0,
                right: SIDE_PADDING,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: self.spacing,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.height, self.leading, self.spacing, self.on_pixels)
            != (old.height, old.leading, old.spacing, old.on_pixels)
        {
            Change::Layout
        } else if (self.alpha, self.layer) != (old.alpha, old.layer) {
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.alpha < 1.0 {
            cx.set_group(self.alpha.clamp(0.0, 1.0) as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn on_pixels(&self, _: &()) -> bool {
        self.on_pixels
    }

    fn surface_layer(&self, _: &()) -> Option<cupertino_kit::core::SurfaceLayerMark> {
        self.layer.map(|key| cupertino_kit::core::SurfaceLayerMark {
            key,
            look: cupertino_kit::platform::SurfaceLayerLook::default(),
        })
    }
}

/// The results: a list of every result, of which the shown ones are children of their sections.
pub(super) struct ResultList<M> {
    pub(super) count: usize,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ResultList<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(LIST_WIDTH),
            ..column(Insets {
                top: LIST_PADDING,
                left: 0.0,
                bottom: LIST_PADDING,
                right: 0.0,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.count != old.count {
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
        cx.role(Role::List);
        cx.name("Results");
        cx.item_count(self.count);
    }
}

/// A section of the results, named by its heading.
pub(super) struct SectionGroup<M> {
    pub(super) title: String,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for SectionGroup<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        column(Insets::default())
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.title != old.title {
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
        cx.name(self.title.clone());
    }
}

/// Lays its children out as `layout` says and leaves them out of the accessibility tree: a section's heading, which
/// its group is named by, and a row's lines, which the row is named and described by.
pub(super) struct Quiet<M> {
    pub(super) layout: LayoutSpec,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Quiet<M> {
    /// A section's heading: its title set as a section heading at the rows' text inset, on the heading's lower padding.
    pub(super) fn heading(title: &str) -> Self {
        Self {
            layout: LayoutSpec {
                width: SizeSpec::Fill,
                height: SizeSpec::Fixed(HEADER_HEIGHT),
                insets: Insets {
                    top: 0.0,
                    left: SELECTION_INSET + CELL_INSET,
                    bottom: 6.0,
                    right: SELECTION_INSET + CELL_INSET,
                },
                ..LayoutSpec::new(ContainerKind::Stack {
                    axis: Axis::Vertical,
                    spacing: 0.0,
                    align: Align::Start,
                    justify: Justify::End,
                })
            },
            children: vec![text(title, TextRole::Section, true)],
        }
    }

    /// A row's title over its subtitle.
    pub(super) fn lines(children: Vec<Element<M>>) -> Self {
        Self {
            layout: LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Start,
                justify: Justify::Center,
            }),
            children,
        }
    }
}

impl<M: 'static> Widget<M> for Quiet<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        self.layout.clone()
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.layout != old.layout {
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

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// One result: its icon and its title over its subtitle, the selection behind them. Hover selects it and a press
/// activates it.
pub(super) struct ResultRow<M> {
    pub(super) flat: usize,
    pub(super) title: String,
    pub(super) subtitle: String,
    pub(super) height: f64,
    pub(super) selected: bool,
    pub(super) children: Vec<Element<M>>,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for ResultRow<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.height),
            insets: Insets {
                top: 0.0,
                left: SELECTION_INSET + CELL_INSET,
                bottom: 0.0,
                right: SELECTION_INSET + CELL_INSET,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: CELL_INSET,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.height != old.height {
            Change::Layout
        } else if (self.flat, &self.title, &self.subtitle, self.selected)
            != (old.flat, &old.title, &old.subtitle, old.selected)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, self.height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(PointerEvent::Motion { .. }, _) if !self.selected => {
                cx.emit(M::from(Input::Hover(self.flat)));
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
                cx.emit(M::from(Input::Activate(self.flat)));
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(M::from(Input::Activate(self.flat)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.selected {
            return;
        }
        let size = cx.size();
        let rect = Rect::new(
            SELECTION_INSET,
            0.0,
            (size.width - 2.0 * SELECTION_INSET).max(0.0),
            size.height,
        );
        let Some(color) = selection_color(cx) else {
            return;
        };
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect,
                radius: SELECTION_RADIUS,
            },
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ListItem);
        cx.name(self.title.clone());
        if !self.subtitle.is_empty() {
            cx.description(self.subtitle.clone());
        }
        cx.states(AccessibleStates {
            selected: self.selected,
            ..AccessibleStates::default()
        });
        cx.item_index(self.flat);
        cx.action(Action::Click);
    }
}

/// A container laid out as its `layout` says, with nothing of its own to paint; a named group to assistive
/// technology when it has a name.
pub(super) struct Plain<M> {
    pub(super) layout: LayoutSpec,
    pub(super) group: Option<&'static str>,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Plain<M> {
    pub(super) fn stack(axis: Axis, spacing: f64, align: Align, children: Vec<Element<M>>) -> Self {
        Self {
            layout: LayoutSpec::new(ContainerKind::Stack {
                axis,
                spacing,
                align,
                justify: Justify::Start,
            }),
            group: None,
            children,
        }
    }

    /// Its children side by side, taking the room left below the query row.
    pub(super) fn row(children: Vec<Element<M>>, align: Align) -> Self {
        let mut row = Self::stack(Axis::Horizontal, 0.0, align, children);
        row.layout.width = SizeSpec::Fill;
        row.layout.height = SizeSpec::Fill;
        row
    }

    /// The preview pane: the width the list leaves, its children centred one above the other inside its padding.
    pub(super) fn pane(children: Vec<Element<M>>) -> Self {
        let mut pane = Self::stack(Axis::Vertical, 6.0, Align::Center, children);
        pane.layout.width = SizeSpec::Fill;
        pane.layout.height = SizeSpec::Fill;
        pane.layout.flex = Flex::grow(1.0);
        pane.layout.insets = Insets {
            top: PREVIEW_PADDING,
            left: PREVIEW_PADDING,
            bottom: PREVIEW_PADDING,
            right: PREVIEW_PADDING,
        };
        pane.group = Some("Preview");
        pane
    }

    /// A detail of the preview: its label, ending at the value's start, then its value.
    pub(super) fn detail(children: Vec<Element<M>>) -> Self {
        let mut detail = Self::stack(Axis::Horizontal, DETAIL_GAP, Align::Start, children);
        detail.layout.width = SizeSpec::Fill;
        detail
    }

    /// Its children placed where they say, over the whole window.
    pub(super) fn absolute(children: Vec<Element<M>>) -> Self {
        Self {
            layout: LayoutSpec {
                width: SizeSpec::Fill,
                height: SizeSpec::Fill,
                ..LayoutSpec::new(ContainerKind::Absolute)
            },
            group: None,
            children,
        }
    }

    /// Its children one above the other, taking the room left below the query row.
    pub(super) fn sheet(children: Vec<Element<M>>) -> Self {
        let mut sheet = Self::stack(Axis::Vertical, 0.0, Align::Stretch, children);
        sheet.layout.width = SizeSpec::Fill;
        sheet.layout.height = SizeSpec::Fill;
        sheet
    }

    /// Takes the room its column leaves.
    pub(super) fn spacer() -> Self {
        let mut spacer = Self::stack(Axis::Vertical, 0.0, Align::Start, Vec::new());
        spacer.layout.height = SizeSpec::Fill;
        spacer
    }
}

impl<M: 'static> Widget<M> for Plain<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        self.layout.clone()
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.layout != old.layout {
            Change::Layout
        } else if self.group != old.group {
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
        if let Some(name) = self.group {
            cx.role(Role::Group);
            cx.name(name);
        }
    }
}

/// Text at a size and weight of its own, in the label or secondary label colour: the preview's name, value and
/// details, which the text roles' sizes do not cover.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Label {
    pub(super) text: String,
    pub(super) size: f64,
    pub(super) weight: u16,
    pub(super) secondary: bool,
    pub(super) align: TextAlign,
    pub(super) lines: u32,
    /// A fixed width, for a detail's label.
    pub(super) width: Option<f64>,
}

impl Label {
    pub(super) fn new(text: &str, size: f64, weight: u16) -> Self {
        Self {
            text: text.to_owned(),
            size,
            weight,
            secondary: false,
            align: TextAlign::Start,
            lines: 1,
            width: None,
        }
    }

    pub(super) fn secondary(self) -> Self {
        Self {
            secondary: true,
            ..self
        }
    }

    /// Centred, wrapping onto at most `lines` lines.
    pub(super) fn centred(self, lines: u32) -> Self {
        Self {
            align: TextAlign::Center,
            lines,
            ..self
        }
    }

    pub(super) fn lines(self, lines: u32) -> Self {
        Self { lines, ..self }
    }

    /// `width` points wide, its text against the end.
    pub(super) fn ending(self, width: f64) -> Self {
        Self {
            align: TextAlign::End,
            width: Some(width),
            ..self
        }
    }

    pub(super) fn laid_out(&self, env: &Environment, width: f64) -> Option<ParagraphLayout> {
        if self.text.is_empty() {
            return None;
        }
        let style = TextStyle {
            weight: FontWeight(self.weight),
            ..TextStyle::new(self.size as f32)
        };
        let paragraph = ParagraphStyle {
            align: self.align,
            wrap: if self.lines > 1 {
                Wrap::Word
            } else {
                Wrap::None
            },
            max_lines: Some(self.lines.max(1)),
            ellipsis: true,
            ..ParagraphStyle::default()
        };
        let text = env
            .text
            .paragraph(&AttributedText::plain(&self.text, style), &paragraph)
            .ok()?;
        Some(text.layout(width.is_finite().then_some(width.max(0.0) as f32)))
    }
}

impl<M: 'static> Widget<M> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: self.width.map_or(SizeSpec::Fill, SizeSpec::Fixed),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (&self.text, self.size, self.weight, self.lines, self.width)
            != (&old.text, old.size, old.weight, old.lines, old.width)
        {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let width = self.width.unwrap_or(constraints.width.max());
        match self.laid_out(cx.env(), width) {
            Some(layout) => Measured {
                size: Size::new(
                    self.width.unwrap_or(layout.size().width),
                    layout.size().height,
                ),
                baseline: layout.lines().first().map(|line| line.baseline),
            },
            None => no_measure(),
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(layout) = self.laid_out(cx.env(), cx.size().width) else {
            return;
        };
        let role = if self.secondary {
            ColorRole::SecondaryText
        } else {
            ColorRole::Text
        };
        let Some(color) = cx.env().theme.colors.get(role, StateVariant::NORMAL) else {
            return;
        };
        glyphs(cx, &layout, Point::ZERO, color);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
    }
}

/// Draw `layout` with its top-left corner at `origin`.
pub(crate) fn glyphs(cx: &mut PaintCx<'_>, layout: &ParagraphLayout, origin: Point, color: Color) {
    for line in layout.lines() {
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                origin: origin + run.origin,
                run: run.run.clone(),
                color,
            });
        }
    }
}

/// The first lines of a text file, small, on a faint card the size of the preview's picture.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TextBlock {
    pub(super) text: String,
}

/// Between the excerpt's card and its text.
pub(super) const TEXT_BLOCK_PADDING: f64 = 8.0;

impl TextBlock {
    pub(super) fn laid_out(&self, env: &Environment) -> Option<ParagraphLayout> {
        let style = TextStyle::new(PREVIEW_TEXT_SIZE as f32);
        let paragraph = ParagraphStyle {
            wrap: Wrap::None,
            max_lines: Some(PREVIEW_TEXT_LINES as u32),
            ..ParagraphStyle::default()
        };
        let text = env
            .text
            .paragraph(&AttributedText::plain(&self.text, style), &paragraph)
            .ok()?;
        Some(text.layout(None))
    }
}

impl<M: 'static> Widget<M> for TextBlock {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(PREVIEW_IMAGE.height),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, PREVIEW_IMAGE.height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let card = Shape::RoundedRect {
            rect: Rect::new(0.0, 0.0, size.width, size.height),
            radius: PREVIEW_IMAGE_RADIUS,
        };
        let colors = cx.env().theme.colors.clone();
        if let Some(fill) = colors.get(ColorRole::QuaternaryLabel, StateVariant::NORMAL) {
            cx.push(PaintOp::Fill {
                shape: card.clone(),
                paint: Paint::Solid(Color::new(fill.r, fill.g, fill.b, fill.a * 0.5)),
            });
        }
        let (Some(layout), Some(color)) = (
            self.laid_out(cx.env()),
            colors.get(ColorRole::SecondaryText, StateVariant::NORMAL),
        ) else {
            return;
        };
        cx.push(PaintOp::PushClip(card));
        glyphs(
            cx,
            &layout,
            Point::new(TEXT_BLOCK_PADDING, TEXT_BLOCK_PADDING),
            color,
        );
        cx.push(PaintOp::PopClip);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
    }
}

/// The selected result's actions, one above the other.
pub(super) struct ActionList<M> {
    pub(super) count: usize,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ActionList<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: ACTION_GAP,
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
        if self.count != old.count {
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
        cx.name("Actions");
        cx.item_count(self.count);
    }
}

/// One action of the selected result: a capsule, filled in the accent colour while it is the chosen one, which a
/// press or Return carries out.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ActionButton {
    pub(super) flat: usize,
    pub(super) index: usize,
    pub(super) label: String,
    pub(super) chosen: bool,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for ActionButton {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(ACTION_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, ACTION_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(M::from(Input::RunAction(self.flat, self.index)));
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(M::from(Input::RunAction(self.flat, self.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let colors = cx.env().theme.colors.clone();
        let (fill, text) = if self.chosen {
            (
                colors.get(ColorRole::DefaultButtonFill, StateVariant::NORMAL),
                ColorRole::DefaultButtonText,
            )
        } else {
            (
                thin_fill(
                    cx,
                    GLASS_CONTROL_DARK,
                    GLASS_CONTROL_LIGHT,
                    ColorRole::ControlFill,
                ),
                ColorRole::ControlText,
            )
        };
        if let Some(fill) = fill {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: size.height / 2.0,
                },
                paint: Paint::Solid(fill),
            });
        }
        let label = Label::new(&self.label, 13.0, 500).centred(1);
        let (Some(layout), Some(color)) = (
            label.laid_out(cx.env(), (size.width - size.height).max(0.0)),
            colors.get(text, StateVariant::NORMAL),
        ) else {
            return;
        };
        let top = ((size.height - layout.size().height) / 2.0).round();
        glyphs(cx, &layout, Point::new(size.height / 2.0, top), color);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            selected: self.chosen,
            ..AccessibleStates::default()
        });
        cx.item_index(self.index);
        cx.action(Action::Click);
    }
}

/// Fills the catcher window; a press anywhere dismisses the panel.
pub(super) struct Catcher;

impl<M: From<Input> + Clone + 'static> Widget<M> for Catcher {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(constraints.width.ideal(), constraints.height.ideal()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _) => {
                cx.emit(M::from(Input::Dismiss));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name("Close the launcher");
    }
}

use super::*;

/// A tree row's indent and its disclosure triangle, before its content; a press on it opens or closes the row.
pub(super) struct Twisty<M> {
    pub(super) index: usize,
    pub(super) item: TreeItem,
    pub(super) selected: bool,
    pub(super) outline: Rc<Outline<M>>,
}

impl<M: 'static> Widget<M> for Twisty<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.item.level as f64 * INDENT + TRIANGLE_COLUMN),
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.item.level != old.item.level {
            Change::Layout
        } else if (self.item.expanded, self.selected) != (old.item.expanded, old.selected) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match (event, self.item.expanded) {
            (
                Event::Pointer(
                    PointerEvent::Button {
                        button: MouseButton::Left,
                        pressed: true,
                        mods,
                        ..
                    },
                    _,
                ),
                Some(expanded),
            ) if cx.node() == cx.target() => {
                let toggle = TreeToggle {
                    index: self.index,
                    expanded: !expanded,
                    recursive: mods.contains(Mods::PRIMARY),
                };
                self.outline.toggle(toggle, cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(open) = self.item.expanded else {
            return;
        };
        let size = cx.size();
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let column = if rtl {
            0.0
        } else {
            size.width - TRIANGLE_COLUMN
        };
        let area = Rect::new(
            column + (TRIANGLE_COLUMN - TRIANGLE) / 2.0,
            (size.height - TRIANGLE) / 2.0,
            TRIANGLE,
            TRIANGLE,
        );
        let points = disclosure::place([(0.25, 0.15), (0.8, 0.5), (0.25, 0.85)], area, open, rtl);
        let state = ControlState {
            enabled: true,
            selected: self.selected,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let role = if self.selected {
            ColorRole::ControlText
        } else {
            ColorRole::SecondaryText
        };
        if let Some(color) = look::color(&cx.env().theme, role, &state) {
            cx.push(PaintOp::Fill {
                shape: disclosure::path(&points, true),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A table row's cells, across its width.
pub(super) struct Cells<M> {
    pub(super) cells: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Cells<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..row_of_cells()
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.cells)
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

pub(super) fn row_of_cells() -> LayoutSpec {
    LayoutSpec::new(ContainerKind::Stack {
        axis: Axis::Horizontal,
        spacing: 0.0,
        align: Align::Center,
        justify: Justify::Start,
    })
}

/// A column's sorting header: what a click on its title asks for.
pub(super) struct Sort<M> {
    index: usize,
    title: String,
    /// The last column draws no divider after it.
    last: bool,
    order: Option<SortOrder>,
    on_sort: Option<Rc<MaybeHandler<(usize, SortOrder), M>>>,
}

/// One column's share of a row or of the header, with its content inset from the column's edges.
pub(super) struct Cell<M> {
    pub(super) width: ColumnWidth,
    pub(super) column: usize,
    /// Set on the header's cells.
    pub(super) header: Option<Sort<M>>,
    pub(super) content: Vec<Element<M>>,
}

pub(super) fn header_cell<M: 'static>(
    index: usize,
    last: bool,
    column: &Column,
    order: Option<SortOrder>,
    on_sort: Option<Rc<MaybeHandler<(usize, SortOrder), M>>>,
) -> Element<M> {
    let style = TextStyle {
        weight: FontWeight(HEADER_TEXT_WEIGHT),
        ..TextStyle::new(HEADER_TEXT_SIZE)
    };
    let mut content = vec![Element::new(Text {
        content: AttributedText::plain(&column.title, style),
        role: TextRole::Caption,
        wrap: false,
        secondary: false,
        selected: false,
    })];
    if let Some(order) = order {
        let name = match order {
            SortOrder::Ascending => "symbol.chevron.up",
            SortOrder::Descending => "symbol.chevron.down",
        };
        content.push(Element::new(Icon {
            name: name.to_owned(),
            size: CHEVRON,
            direction: IconDirection::Fixed,
            label: None,
            tint: Some(ColorRole::SecondaryText),
        }));
    }
    Element::new(Cell {
        width: column.width,
        column: index,
        header: Some(Sort {
            index,
            title: column.title.clone(),
            last,
            order,
            on_sort,
        }),
        content,
    })
}

impl<M: 'static> Widget<M> for Cell<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        let (width, flex) = match self.width {
            ColumnWidth::Fixed(w) => (SizeSpec::Fixed(w), Flex::NONE),
            ColumnWidth::Fill => (SizeSpec::Fit, Flex::grow(1.0)),
        };
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: CELL_INSET,
                bottom: 0.0,
                right: CELL_INSET,
            },
            width,
            height: SizeSpec::Fill,
            flex,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: CHEVRON_GAP,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.content)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        let order = |c: &Self| c.header.as_ref().map(|h| h.order);
        if self.width != old.width {
            Change::Layout
        } else if order(self) != order(old) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let Some(sort) = &self.header else {
            return Propagation::Continue;
        };
        match press.event(event, sort.on_sort.is_some(), Keys::SpaceAndReturn, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => Propagation::Stop,
            Outcome::Activated(_) => {
                let order = match sort.order {
                    Some(SortOrder::Ascending) => SortOrder::Descending,
                    Some(SortOrder::Descending) | None => SortOrder::Ascending,
                };
                if let Some(message) = sort
                    .on_sort
                    .as_ref()
                    .and_then(|on_sort| on_sort((sort.index, order)))
                {
                    cx.emit(message);
                }
                Propagation::Stop
            }
        }
    }

    /// A header cell other than the last draws the divider at its trailing edge; a sorting one shows the keyboard focus.
    fn paint(&self, _: &Press, cx: &mut PaintCx<'_>) {
        let Some(sort) = &self.header else {
            return;
        };
        if sort.on_sort.is_some() {
            let size = cx.size();
            paint_focus_ring(cx, Rect::new(0.0, 0.0, size.width, size.height), 0.0);
        }
        if sort.last {
            return;
        }
        let env = cx.env();
        let Some(color) = env
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL)
        else {
            return;
        };
        let size = cx.size();
        let x = match env.direction {
            LayoutDirection::Ltr => size.width - 1.0,
            LayoutDirection::Rtl => 0.0,
        };
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(x, 0.0, 1.0, size.height)),
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        match &self.header {
            Some(sort) => {
                cx.role(Role::ColumnHeader);
                cx.name(sort.title.clone());
                if sort.on_sort.is_some() {
                    cx.focusable();
                    cx.action(Action::Click);
                }
                if let Some(order) = sort.order {
                    cx.sort(order);
                }
            }
            None => cx.role(Role::Cell),
        }
        cx.column_index(self.column);
    }
}

/// A table's header over its scroll view: the one table assistive technology reads, its header row first.
pub(super) struct TableFrame<M> {
    pub(super) label: String,
    pub(super) count: usize,
    pub(super) multiple: bool,
    /// The table fills a pane under a toolbar row: its header lies over its scroll view, which runs to its top.
    pub(super) under: bool,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for TableFrame<M> {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Table);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            multiselectable: self.multiple,
            ..AccessibleStates::default()
        });
        cx.item_count(self.count);
    }
}

/// A table's header below the toolbar row over the table's top: raised with the row, over the rows that scroll under
/// both. It takes no point above the header, which belongs to the row.
pub(super) struct RaisedHeader<M> {
    pub(super) top: f64,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for RaisedHeader<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: self.top,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.top + HEADER_HEIGHT),
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
        if self.top != old.top {
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

    fn hit(&self, _: &(), point: Point) -> bool {
        point.y >= self.top
    }

    fn raised(&self, _: &()) -> bool {
        true
    }
}

/// A table's column titles, over its scroll view and inset like its rows, with a line under them.
pub(super) struct Header<M> {
    pub(super) cells: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Header<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: CONTENT_INSET,
                bottom: 1.0,
                right: CONTENT_INSET,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(HEADER_HEIGHT),
            ..row_of_cells()
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.cells)
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
        let env = cx.env();
        let Some(color) = env
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL)
        else {
            return;
        };
        let size = cx.size();
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, size.height - 1.0, size.width, 1.0)),
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Row);
    }
}

use super::*;

#[derive(Clone)]
pub(super) enum RowAction<M> {
    Select { on_select: Out<M>, on_close: Out<M> },
    Open { on_open: Out<M>, open: bool },
}

pub(super) struct Row<M> {
    pub(super) label: String,
    /// The keys that choose the row, shown at its trailing edge.
    pub(super) shortcut: Option<String>,
    pub(super) enabled: bool,
    pub(super) checked: bool,
    pub(super) action: RowAction<M>,
    pub(super) columns: Columns,
    /// The labels of the menu's enabled rows, in order, for typing to move.
    pub(super) labels: Rc<[String]>,
    /// What was typed so far, shared by the menu's rows.
    pub(super) typing: Rc<RefCell<TypeAhead>>,
    /// The choice flashing now, which this row's may be.
    pub(super) flash: Flashing<M>,
    /// This row's place among all the menu's items.
    pub(super) index: usize,
    /// This row's place among them, if enabled.
    pub(super) position: usize,
    /// This row's place among all the menu's rows, separators not counted, which a screen reader reads out.
    pub(super) place: usize,
    /// Highlighted while it has the focus, also before the pointer or a key moves it.
    pub(super) current: bool,
}

#[derive(Default)]
pub(super) struct RowState {
    press: Press,
    /// The pointer moved onto the row.
    pointed: bool,
}

impl<M> Row<M> {
    fn open(&self) -> bool {
        matches!(self.action, RowAction::Open { open: true, .. })
    }

    /// Whether the row shows as the menu's selection when it has the focus: a menu opened with the pointer focuses
    /// its first row but selects nothing until the pointer or a key moves.
    fn selected(&self, state: &RowState, focus_visible: bool) -> bool {
        self.open() || focus_visible || state.pointed || self.current
    }
}

impl<M: Clone + 'static> Row<M> {
    fn activate(&self, cx: &mut EventCx<'_, M>) {
        match &self.action {
            RowAction::Select {
                on_select,
                on_close,
            } => choose(
                &self.flash,
                self.index,
                false,
                vec![on_select.clone(), on_close.clone()],
                cx,
            ),
            RowAction::Open { on_open, .. } => on_open.send(cx),
        }
    }

    /// The enabled row that what was typed, with `c` typed at `now`, finds: one character the next row it starts
    /// after this one (from this one when it is not `selected`), more characters the first row from this one that
    /// they start, wrapping around.
    fn typed(&self, c: char, selected: bool, now: f64) -> Option<usize> {
        let count = self.labels.len();
        let titles: Vec<Option<&str>> = self.labels.iter().map(|l| Some(l.as_str())).collect();
        let from = if selected {
            self.position
        } else {
            (self.position + count - 1) % count
        };
        self.typing.borrow_mut().find(c, now, from, &titles)
    }

    fn key(
        &self,
        key: &cupertino_ui_core::platform::KeyEvent,
        selected: bool,
        cx: &mut EventCx<'_, M>,
    ) -> bool {
        let trailing = match cx.env().direction {
            LayoutDirection::Ltr => RIGHT,
            LayoutDirection::Rtl => LEFT,
        };
        let target = match key.keysym {
            RETURN | SPACE if !selected => return true,
            DOWN if !selected => FocusTarget::First,
            UP if !selected => FocusTarget::Last,
            DOWN => FocusTarget::Next,
            UP => FocusTarget::Previous,
            HOME => FocusTarget::First,
            END => FocusTarget::Last,
            k if k == trailing && selected && matches!(self.action, RowAction::Open { .. }) => {
                self.activate(cx);
                return true;
            }
            _ => {
                let plain = !key.mods.contains(Mods::CTRL)
                    && !key.mods.contains(Mods::ALT)
                    && !key.mods.contains(Mods::PRIMARY);
                match key.text.filter(|c| plain && !c.is_control() && *c != ' ') {
                    Some(c) => match self.typed(c, selected, seconds(cx.now())) {
                        Some(index) => FocusTarget::Index(index),
                        None => return true,
                    },
                    None => return false,
                }
            }
        };
        cx.focus_item(target);
        true
    }
}

/// Draws the symbol `name`, `SYMBOL` points square, centred at `centre`.
fn symbol(cx: &mut PaintCx<'_>, name: &str, centre: Point, color: Option<Color>, mirrored: bool) {
    let Some(color) = color else {
        return;
    };
    let scale = cx.env().scale;
    let px = (SYMBOL * scale).round().max(1.0) as u32;
    let icon = cx.env().icons.symbol(name, px, color, mirrored);
    let (width, height) = (
        f64::from(icon.image.width) / scale,
        f64::from(icon.image.height) / scale,
    );
    let dst = Rect::new(
        centre.x - width / 2.0,
        centre.y - height / 2.0,
        width,
        height,
    );
    cx.image(icon.id, &icon.image, dst, Sampling::Linear);
}

impl<M: Clone + 'static> Widget<M> for Row<M> {
    type State = RowState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> RowState {
        RowState::default()
    }

    fn update(&self, old: &Self, _: &mut RowState) -> Change {
        if self.label != old.label
            || self.shortcut != old.shortcut
            || self.columns != old.columns
            || matches!(self.action, RowAction::Open { .. })
                != matches!(old.action, RowAction::Open { .. })
        {
            Change::Layout
        } else if self.enabled != old.enabled
            || self.checked != old.checked
            || self.current != old.current
            || self.open() != old.open()
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &RowState, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let label = look::label(cx.env(), &self.label).map_or(0.0, |l| l.size().width);
        let shortcut = self
            .shortcut
            .as_deref()
            .and_then(|s| look::label(cx.env(), s))
            .map_or(0.0, |l| l.size().width);
        // the shortcuts and chevrons keep a column at the trailing edge when the menu has any
        let trailing = if self.columns.trailing {
            SHORTCUT_GAP + shortcut.max(CHEVRON_WIDTH)
        } else {
            0.0
        };
        let width = self.columns.label + label + trailing + TRAILING;
        Measured {
            size: Size::new(width.max(self.columns.min_width), ROW_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, state: &mut RowState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            // the panel moves the focus to the row the pointer is on
            Event::Pointer(PointerEvent::Enter { .. }, _)
                if self.enabled && cx.node() == cx.target() =>
            {
                state.pointed = true;
                cx.request_paint();
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                state.pointed = false;
                cx.request_paint();
            }
            Event::Focus { .. } if cx.node() == cx.target() => cx.request_paint(),
            Event::Wake(now)
                if cx.node() == cx.target()
                    && flash_woke(
                        &self.flash,
                        |f| !f.by_panel && f.row == self.index,
                        *now,
                        cx,
                    ) =>
            {
                return Propagation::Stop;
            }
            Event::AnimationFrame(_) if cx.node() == cx.target() => cx.request_paint(),
            Event::Key(key, _)
                if self.enabled
                    && key.pressed
                    && cx.node() == cx.target()
                    && cx.phase() != Phase::Capture
                    && self.key(key, self.selected(state, cx.focus_visible()), cx) =>
            {
                return Propagation::Stop;
            }
            _ => {}
        }
        match state
            .press
            .event(event, self.enabled, Keys::SpaceAndReturn, cx)
        {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                self.activate(cx);
                Propagation::Stop
            }
        }
    }

    fn paint(&self, state: &RowState, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let size = cx.size();
        let mut highlighted = self.enabled
            && (self.open() || cx.is_focused() && self.selected(state, cx.focus_visible()));
        // a choice flashes: its row's highlight goes off, then on again, until the choice is sent
        if let Some(flash) = self.flash.borrow().as_ref() {
            cx.request_animation_frame();
            if flash.row == self.index {
                highlighted = flash.lit(cx.now());
            }
        }
        if highlighted {
            let rect = Rect::new(
                HIGHLIGHT_INSET,
                0.0,
                size.width - 2.0 * HIGHLIGHT_INSET,
                size.height,
            );
            look::fill(
                cx,
                rect,
                HIGHLIGHT_RADIUS,
                theme
                    .colors
                    .get(ColorRole::SelectedControlFill, StateVariant::NORMAL),
            );
        }
        let role = if !self.enabled {
            ColorRole::DisabledControlText
        } else if highlighted {
            ColorRole::SelectedControlText
        } else {
            ColorRole::Text
        };
        let color = theme.colors.get(role, StateVariant::NORMAL);
        let rtl = env.direction == LayoutDirection::Rtl;
        let leading = |x: f64, width: f64| if rtl { size.width - x - width } else { x };
        let middle = size.height / 2.0;
        if self.checked {
            let x = leading(MARK_CENTRE, 0.0);
            symbol(cx, "symbol.checkmark", Point::new(x, middle), color, false);
        }
        if let Some(label) = look::label(&env, &self.label) {
            let width = label.size().width;
            let row = Rect::new(0.0, 0.0, size.width, size.height);
            let origin = Point::new(
                leading(self.columns.label, width),
                look::top(&label, row, &env),
            );
            look::glyphs(cx, &label, origin, color);
        }
        if let Some(shortcut) = self.shortcut.as_deref().and_then(|s| look::label(&env, s)) {
            // the keys keep to the trailing edge, in the secondary colour unless the row is highlighted or disabled
            let keys = match (self.enabled, highlighted) {
                (_, true) => color,
                (true, false) => color.map(|c| Color {
                    a: c.a * SHORTCUT_ALPHA,
                    ..c
                }),
                (false, false) => color.map(|c| Color {
                    a: c.a * DISABLED_SHORTCUT_ALPHA,
                    ..c
                }),
            };
            let row = Rect::new(0.0, 0.0, size.width, size.height);
            let text = self.shortcut.as_deref().unwrap_or_default();
            match split_key(text) {
                Some((modifiers, key)) => {
                    // the modifiers end at the key cell's leading side and the key is centred in the cell, so the keys of
                    // different widths line up on the cell's middle
                    let key_label = look::label(&env, key);
                    let modifier_label = look::label(&env, modifiers);
                    if let (Some(key_label), Some(modifier_label)) = (key_label, modifier_label) {
                        let key_width = key_label.size().width;
                        let modifier_width = modifier_label.size().width;
                        let key_x = size.width - KEY_CENTRE - key_width / 2.0;
                        let modifier_x = size.width - KEY_CELL_LEADING - modifier_width;
                        let key_at =
                            Point::new(leading(key_x, key_width), look::top(&key_label, row, &env));
                        let modifier_at = Point::new(
                            leading(modifier_x, modifier_width),
                            look::top(&modifier_label, row, &env),
                        );
                        look::glyphs(cx, &key_label, key_at, keys);
                        look::glyphs(cx, &modifier_label, modifier_at, keys);
                    }
                }
                None => {
                    let width = shortcut.size().width;
                    let origin = Point::new(
                        leading(size.width - TRAILING - width, width),
                        look::top(&shortcut, row, &env),
                    );
                    look::glyphs(cx, &shortcut, origin, keys);
                }
            }
        }
        if matches!(self.action, RowAction::Open { .. }) {
            let x = leading(size.width - KEY_CENTRE, 0.0);
            symbol(
                cx,
                "symbol.chevron.right",
                Point::new(x, middle),
                color,
                rtl,
            );
        }
    }

    fn semantics(&self, _: &RowState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::MenuItem);
        cx.name(self.label.clone());
        if let Some(shortcut) = &self.shortcut {
            cx.description(shortcut.clone());
        }
        cx.item_index(self.place);
        if self.checked {
            cx.value(AccessibleValue::Toggle(Checked::True));
        }
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            expanded: matches!(self.action, RowAction::Open { .. }).then(|| self.open()),
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.roving_item();
            cx.focusable();
            cx.action(Action::Click);
        }
    }
}

/// A line between groups of rows, under the labels: from the label column to near the trailing edge.
pub(super) struct Separator {
    /// From the leading edge to the menu's labels.
    pub(super) leading: f64,
}

impl<M: 'static> Widget<M> for Separator {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.leading != old.leading {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, SEPARATOR_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let theme = &cx.env().theme;
        let appearance = theme.preferences.appearance;
        // over the glass a translucent line that takes its colour; over the opaque fill and with Increase Contrast the
        // theme's separator
        let color = if cx.window_backdrop() && !appearance.is_high_contrast() {
            Some(if appearance.is_dark() {
                GLASS_SEPARATOR_DARK
            } else {
                GLASS_SEPARATOR_LIGHT
            })
        } else {
            theme.colors.get(ColorRole::Separator, StateVariant::NORMAL)
        };
        let Some(color) = color else {
            return;
        };
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let length = (size.width - self.leading - SEPARATOR_TRAILING).max(0.0);
        let x = if rtl {
            SEPARATOR_TRAILING
        } else {
            self.leading
        };
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(x, (size.height - EDGE) / 2.0, length, EDGE)),
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Separator);
    }
}

/// How long a finger rests on a context menu's region before its menu opens there, in seconds: a finger has no
/// secondary button. As long as a press is held before a menu tracks it as held.
const TOUCH_HOLD: f64 = TRACKING_INTERVAL;

/// How far, in points, a resting finger may stray before it is a drag or a scroll rather than a hold.
const TOUCH_SLOP: f64 = 8.0;

/// A context menu's region: the finger resting on it, if one is, with where it went down and when its rest opens the
/// menu. Opaque.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextMenuState {
    hold: Option<(i32, Point, Timestamp)>,
}

impl<M: Clone + 'static> Widget<M> for ContextMenu<M> {
    type State = ContextMenuState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        vec![std::mem::replace(
            &mut self.content,
            Element::new(crate::Stack {
                axis: Axis::Vertical,
                padding: Insets::uniform(0.0),
                spacing: 0.0,
                align: Align::Start,
                children: Vec::new(),
            }),
        )]
    }

    fn state(&self) -> ContextMenuState {
        ContextMenuState::default()
    }

    fn update(&self, _: &Self, _: &mut ContextMenuState) -> Change {
        Change::None
    }

    fn measure(&self, _: &ContextMenuState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(
        &self,
        state: &mut ContextMenuState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let Some(on_open) = &self.on_open else {
            return Propagation::Continue;
        };
        // a finger that rests on the region opens the menu where it went down, and the menu follows it from there; it
        // is watched on its way in, so the content under it still takes it as a press
        match *event {
            Event::Touch(TouchEvent::Down { id, position }, _)
                if cx.phase() == Phase::Capture && state.hold.is_none() =>
            {
                let at = after(cx.now(), TOUCH_HOLD);
                state.hold = Some((id, position, at));
                cx.wake_at(at);
            }
            Event::Touch(TouchEvent::Motion { id, position }, _)
                if cx.phase() == Phase::Capture =>
            {
                if let Some((_, from, _)) = state.hold.filter(|(held, ..)| *held == id) {
                    if (position.x - from.x).hypot(position.y - from.y) > TOUCH_SLOP {
                        state.hold = None;
                    }
                }
            }
            Event::Touch(TouchEvent::Up { id }, _) if cx.phase() == Phase::Capture => {
                state.hold = state.hold.filter(|(held, ..)| *held != id);
            }
            Event::Touch(TouchEvent::Cancel, _) | Event::Cancel => state.hold = None,
            Event::Wake(now) if cx.node() == cx.target() => {
                if let Some((_, at, _)) = state.hold.filter(|(.., due)| *due <= now) {
                    state.hold = None;
                    cx.emit(on_open(at));
                    return Propagation::Stop;
                }
            }
            _ => {}
        }
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let at = match event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => Some(*position),
            Event::Key(key, _)
                if key.pressed
                    && (key.keysym == MENU && key.mods.is_empty()
                        || key.keysym == F10 && key.mods == Mods::SHIFT) =>
            {
                cx.bounds().map(|b| match cx.env().direction {
                    LayoutDirection::Ltr => b.origin,
                    LayoutDirection::Rtl => Point::new(b.max_x(), b.min_y()),
                })
            }
            _ => None,
        };
        match at {
            Some(at) => {
                cx.emit(on_open(at));
                Propagation::Stop
            }
            None => Propagation::Continue,
        }
    }

    fn paint(&self, _: &ContextMenuState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &ContextMenuState, _: &mut SemanticsCx<'_>) {}
}

impl<M: Clone + 'static> PopUpButton<M> {
    /// The menu of the options over the button, which has the key `anchor` in `parent`, the chosen one's label on the
    /// button's: choosing one sends `on_select` with its index, and the menu closes with `on_close`.
    pub fn menu(
        &self,
        parent: WindowKey,
        anchor: Key,
        on_select: Handler<usize, M>,
        on_close: M,
    ) -> Menu<M> {
        let items = self
            .options
            .iter()
            .enumerate()
            .map(|(index, label)| MenuItem::Item {
                label: label.clone(),
                enabled: true,
                checked: index == self.selected,
                on_select: on_select(index),
            })
            .collect();
        let close = on_close.clone();
        Menu {
            parent,
            anchor: MenuAnchor::Over(anchor),
            label: self.label.clone(),
            items,
            current: Some(self.selected),
            open: Vec::new(),
            on_open: Box::new(move |_| close.clone()),
            on_close,
        }
    }

    fn chosen(&self) -> &str {
        self.options.get(self.selected).map_or("", String::as_str)
    }
}

impl<M: Clone + 'static> Widget<M> for PopUpButton<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.options != old.options {
            Change::Layout
        } else if self.selected != old.selected
            || self.enabled != old.enabled
            || self.open != old.open
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    /// Wide enough for the widest option, so choosing another never moves what follows.
    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        menu_button_measure(cx.env(), &self.options)
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        menu_button_event(press, event, cx, self.enabled, self.on_open.as_ref())
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        paint_menu_button(press, cx, self.enabled, self.open, self.chosen(), false);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PopUpButton);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Text {
            value: self.chosen().to_owned(),
            editing: None,
        });
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            expanded: Some(self.open),
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            if self.on_open.is_some() {
                cx.action(Action::Click);
            }
        }
    }
}

/// A pop-up or pull-down button's size: wide enough for the widest of `labels`, so another never moves what follows.
fn menu_button_measure(env: &Environment, labels: &[String]) -> Measured {
    let metrics = env.theme.metrics;
    let widest = labels
        .iter()
        .filter_map(|o| look::label(env, o))
        .map(|l| l.size().width)
        .fold(0.0, f64::max);
    let height = look::geometry(&env.theme, ControlFamily::PopUp).height;
    let width = LABEL_INSET + widest + CHEVRONS_GAP + SYMBOL + LABEL_INSET;
    Measured {
        size: Size::new(
            width.max(metrics.min_target.width),
            height.max(metrics.min_target.height),
        ),
        baseline: None,
    }
}

/// A pop-up or pull-down button's events: its menu opens on a press or a finger put down, so the pointer or the finger
/// can drag to a row and let go there, and on Space, Return, Up or Down.
fn menu_button_event<M: Clone + 'static>(
    press: &mut Press,
    event: &Event,
    cx: &mut EventCx<'_, M>,
    enabled: bool,
    on_open: Option<&M>,
) -> Propagation {
    let Some(on_open) = on_open.filter(|_| enabled) else {
        return Propagation::Continue;
    };
    if let Event::Key(key, _) = event {
        if key.pressed
            && (key.keysym == UP || key.keysym == DOWN)
            && key.mods.is_empty()
            && cx.node() == cx.target()
        {
            cx.emit(on_open.clone());
            return Propagation::Stop;
        }
    }
    // the menu opens on the press, so the pointer can drag to an option and let go there
    let pressed = cx.phase() != Phase::Capture && opens_on_press(event, press);
    if pressed {
        cx.emit(on_open.clone());
    }
    match press.event(event, true, Keys::SpaceAndReturn, cx) {
        Outcome::Ignored if pressed => Propagation::Stop,
        Outcome::Ignored => Propagation::Continue,
        Outcome::Changed
        | Outcome::Activated(ActivationSource::Pointer | ActivationSource::Touch) => {
            cx.request_paint();
            Propagation::Stop
        }
        Outcome::Activated(_) => {
            cx.request_paint();
            cx.emit(on_open.clone());
            Propagation::Stop
        }
    }
}

/// Whether `event` is a press that opens a pop-up or pull-down button's menu, or a combo box's: the primary button
/// going down, or a finger put down while no other holds the control (`press`, before it hears the event).
pub(crate) fn opens_on_press(event: &Event, press: &Press) -> bool {
    match event {
        Event::Pointer(
            PointerEvent::Button {
                button: MouseButton::Left,
                pressed: true,
                ..
            },
            _,
        ) => true,
        Event::Touch(TouchEvent::Down { .. }, _) => !press.armed(),
        _ => false,
    }
}

/// A pop-up or pull-down button: an ordinary button's quiet bezel with `text_label` and opposing chevrons, or a
/// single downward one when `pull_down`.
fn paint_menu_button(
    press: &Press,
    cx: &mut PaintCx<'_>,
    enabled: bool,
    open: bool,
    text_label: &str,
    pull_down: bool,
) {
    let env = cx.env().clone();
    let theme = &env.theme;
    let state = ControlState {
        enabled,
        hovered: press.hovered(),
        pressed: press.pressed() || open,
        focused: cx.is_focused(),
        focus_visible: cx.focus_visible(),
        window_active: cx.window_active(),
        ..ControlState::default()
    };
    let size = cx.size();
    let geometry = look::geometry(theme, ControlFamily::PopUp);
    let height = geometry.height;
    let chrome = Rect::new(0.0, (size.height - height) / 2.0, size.width, height);
    let radius = geometry.corners.radius(height);
    // an ordinary button's quiet bezel
    let bezel = ButtonLook::of(ButtonRole::Normal, ButtonStyle::Bordered, theme, &state);
    look::fill(cx, chrome, radius, bezel.fill);
    look::stroke(
        cx,
        chrome.inset(Insets::uniform(0.5)),
        radius - 0.5,
        1.0,
        bezel.border,
    );
    let text = bezel.text;
    let inset = LABEL_INSET;
    let rtl = env.direction == LayoutDirection::Rtl;
    if let Some(label) = look::label(&env, text_label) {
        let width = label.size().width;
        let x = if rtl {
            chrome.max_x() - inset - width
        } else {
            chrome.min_x() + inset
        };
        look::glyphs(
            cx,
            &label,
            Point::new(x, look::top(&label, chrome, &env)),
            text,
        );
    }
    let x = if rtl {
        chrome.min_x() + inset + SYMBOL / 2.0
    } else {
        chrome.max_x() - inset - SYMBOL / 2.0
    };
    // opposing chevrons: a choice among values; a pull-down's single one points down at its actions
    if let Some(color) = text {
        let mid = chrome.min_y() + chrome.height() / 2.0;
        let chevrons: &[bool] = if pull_down { &[false] } else { &[true, false] };
        for &up in chevrons {
            let offset = match (pull_down, up) {
                (true, _) => 0.0,
                (false, true) => -POPUP_CHEVRON_GAP,
                (false, false) => POPUP_CHEVRON_GAP,
            };
            cx.push(PaintOp::Stroke {
                shape: look::chevron(Point::new(x, mid + offset), POPUP_CHEVRON, up),
                width: POPUP_CHEVRON_STROKE,
                paint: Paint::Solid(color),
            });
        }
    }
    paint_focus_ring(cx, chrome, radius);
}

impl<M: Clone + 'static> InlinePopUpButton<M> {
    /// The menu of the options over the button ([`PopUpButton::menu`]).
    pub fn menu(
        &self,
        parent: WindowKey,
        anchor: Key,
        on_select: Handler<usize, M>,
        on_close: M,
    ) -> Menu<M> {
        self.0.menu(parent, anchor, on_select, on_close)
    }
}

impl<M: Clone + 'static> Widget<M> for InlinePopUpButton<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, state: &mut Press) -> Change {
        self.0.update(&old.0, state)
    }

    /// Wide enough for the widest option and the disc, so choosing another never moves what follows.
    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let env = cx.env();
        let metrics = env.theme.metrics;
        let widest = self
            .0
            .options
            .iter()
            .filter_map(|o| look::label(env, o))
            .map(|l| l.size().width)
            .fold(0.0, f64::max);
        let height = look::geometry(&env.theme, ControlFamily::PopUp).height;
        Measured {
            size: Size::new(
                (widest + INLINE_GAP + INLINE_DISC).max(metrics.min_target.width),
                height.max(metrics.min_target.height),
            ),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        self.0.event(press, event, cx)
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let popup = &self.0;
        let state = ControlState {
            enabled: popup.enabled,
            hovered: press.hovered(),
            pressed: press.pressed() || popup.open,
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let height = look::geometry(theme, ControlFamily::PopUp).height;
        let chrome = Rect::new(0.0, (size.height - height) / 2.0, size.width, height);
        let rtl = env.direction == LayoutDirection::Rtl;
        let disc = Rect::new(
            if rtl {
                chrome.min_x()
            } else {
                chrome.max_x() - INLINE_DISC
            },
            chrome.min_y() + (chrome.height() - INLINE_DISC) / 2.0,
            INLINE_DISC,
            INLINE_DISC,
        );
        look::fill(
            cx,
            disc,
            INLINE_DISC / 2.0,
            Some(look::quiet(theme, look::DISC, &state)),
        );
        let bezel = ButtonLook::of(ButtonRole::Normal, ButtonStyle::Bordered, theme, &state);
        if theme.preferences.appearance.is_high_contrast() {
            look::stroke(
                cx,
                disc.inset(Insets::uniform(0.5)),
                INLINE_DISC / 2.0 - 0.5,
                1.0,
                bezel.border,
            );
        }
        let text = bezel.text;
        // the value, against the disc
        if let Some(label) = look::label(&env, popup.chosen()) {
            let width = label.size().width;
            let x = if rtl {
                disc.max_x() + INLINE_GAP
            } else {
                disc.min_x() - INLINE_GAP - width
            };
            look::glyphs(
                cx,
                &label,
                Point::new(x, look::top(&label, chrome, &env)),
                text,
            );
        }
        if let Some(color) = text {
            let mid = disc.min_y() + disc.height() / 2.0;
            let x = disc.min_x() + disc.width() / 2.0;
            for up in [true, false] {
                let offset = if up {
                    -INLINE_CHEVRON_GAP
                } else {
                    INLINE_CHEVRON_GAP
                };
                cx.push(PaintOp::Stroke {
                    shape: look::chevron(Point::new(x, mid + offset), INLINE_CHEVRON, up),
                    width: POPUP_CHEVRON_STROKE,
                    paint: Paint::Solid(color),
                });
            }
        }
        paint_focus_ring(cx, chrome, height / 2.0);
    }

    fn semantics(&self, press: &Press, cx: &mut SemanticsCx<'_>) {
        self.0.semantics(press, cx);
    }
}

impl<M: Clone + 'static> PullDownButton<M> {
    /// The menu of the actions below the button, which has the key `anchor` in `parent`: choosing one sends
    /// `on_select` with its index, and the menu closes with `on_close`. No action is marked: none is chosen for good.
    pub fn menu(
        &self,
        parent: WindowKey,
        anchor: Key,
        on_select: Handler<usize, M>,
        on_close: M,
    ) -> Menu<M> {
        let items = self
            .actions
            .iter()
            .enumerate()
            .map(|(index, label)| MenuItem::Item {
                label: label.clone(),
                enabled: true,
                checked: false,
                on_select: on_select(index),
            })
            .collect();
        let close = on_close.clone();
        Menu {
            parent,
            anchor: MenuAnchor::Below(anchor),
            label: self.label.clone(),
            items,
            current: None,
            open: Vec::new(),
            on_open: Box::new(move |_| close.clone()),
            on_close,
        }
    }
}

impl<M: Clone + 'static> Widget<M> for PullDownButton<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.label != old.label {
            Change::Layout
        } else if self.enabled != old.enabled || self.open != old.open {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        menu_button_measure(cx.env(), std::slice::from_ref(&self.label))
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        menu_button_event(press, event, cx, self.enabled, self.on_open.as_ref())
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        paint_menu_button(press, cx, self.enabled, self.open, &self.label, true);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PopUpButton);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            expanded: Some(self.open),
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            if self.on_open.is_some() {
                cx.action(Action::Click);
            }
        }
    }
}

/// The container the panel and its pointer share as glass, and how far their outlines blend into each other.
const TAIL_CONTAINER: u32 = 1;
const TAIL_BLEND: f64 = 5.0;

/// A panel with a pointer is a menu all the same: it is menu glass, its pointer joined to it.
pub(super) fn tail_glass() -> Glass {
    Glass::menu().container(TAIL_CONTAINER, TAIL_BLEND)
}

/// The content of a menu window with a pointer: the panel, and under it the row that holds the pointer.
pub(super) struct Callout<M> {
    /// What assistive technology calls the window: the menu's name.
    pub(super) name: String,
    pub(super) children: Vec<Element<M>>,
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

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.name != old.name {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    /// The menu draws its own glass, under its pointer too, and fades out as every menu does.
    fn closes_as_menu(&self) -> bool {
        true
    }

    // the window around the menu: the panel under it is the menu assistive technology reads
    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.name(self.name.clone());
    }
}

/// The row under the panel that holds its pointer at [`MenuTail::from_leading`].
pub(super) struct TailRow {
    pub(super) tail: MenuTail,
}

impl<M: Clone + 'static> Widget<M> for TailRow {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::new(
                0.0,
                self.tail.from_leading - self.tail.width / 2.0,
                0.0,
                0.0,
            ),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Start,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        vec![CalloutPointer {
            width: self.tail.width,
            depth: self.tail.depth,
            glass: tail_glass(),
            surface: Surface::Menu,
            fills_without_glass: true,
            up: false,
        }
        .element()
        .key("shape")]
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.tail != old.tail {
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

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// A shortcut split into its modifier symbols and its one-character key; `None` for any other form (a named key).
fn split_key(text: &str) -> Option<(&str, &str)> {
    let (at, key) = text.char_indices().last()?;
    let modifiers = &text[..at];
    let symbols = modifiers
        .chars()
        .all(|c| matches!(c, '\u{2318}' | '\u{21e7}' | '\u{2325}' | '\u{2303}' | '^'));
    (!modifiers.is_empty()
        && symbols
        && !matches!(key, '\u{2318}' | '\u{21e7}' | '\u{2325}' | '\u{2303}' | '^'))
    .then(|| (modifiers, &text[at..]))
}

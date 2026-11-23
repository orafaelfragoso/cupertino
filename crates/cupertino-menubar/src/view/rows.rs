//! A menu level's window and its rows: the menus' items, the Help menu's search field and the rows of the status
//! panels, with the marks they draw.

use super::*;

pub(super) fn menu<M: Clone + 'static>(
    scene: &BarScene<'_>,
    level: usize,
    rect: geom::Rect,
    at: geom::Rect,
    parent: WindowKey,
    input: fn(Input) -> M,
) -> Window<M> {
    let levels = scene.controller.levels();
    let open: &Level = &levels[level];
    let next = levels.get(level + 1).map(|l| l.menu.id);
    let layout = &open.layout;
    let metrics = scene.controller.menu_metrics.clone();
    let field = scene.controller.search_field().map(|f| Field {
        text: f.text().to_owned(),
        caret: f.caret(),
        selection: f.selection(),
    });
    let mut places = 0;
    let children: Vec<_> = layout
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let searching = field.is_some() && row.item.id == HELP_SEARCH;
            let place = (!row.item.separator && !searching).then(|| {
                places += 1;
                places - 1
            });
            Element::new(RowNode {
                place,
                bar: scene.bar,
                row: row.clone(),
                origin: BarPoint::new(rect.x, rect.y),
                hot: open.selected == Some(i) && row.item.enabled,
                expanded: row.item.has_submenu.then(|| next == Some(row.item.id)),
                icons: Rc::clone(scene.icons),
                icons_revision: scene.icons.revision(),
                key_width: layout.shortcut_key_width,
                panel: layout.panel,
                field: field.clone().filter(|_| row.item.id == HELP_SEARCH),
                metrics: metrics.clone(),
                font: scene.font,
                input,
            })
        })
        .collect();
    let size = Size::new(rect.w, rect.h);
    Window {
        key: scene.bar.menu(level),
        spec: spec(BAR_APP_ID.to_owned(), size),
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Surface {
            surface: Surface::Menu,
            radius: if layout.panel {
                panel::RADIUS
            } else {
                POPUP_RADIUS
            },
            alpha: 1.0,
        },
        content: Element::new(Root {
            bar: scene.bar,
            keyboard: false,
            origin: BarPoint::new(rect.x, rect.y),
            size: Some(size),
            role: Role::Menu,
            name: open.menu.label.clone(),
            items: places,
            input,
            children,
        }),
        popup: Some(Popup {
            parent,
            // `At` hangs the popup off the far corner of the point's one-point square
            anchor: PopupAnchor::At(Point::new(at.x - 1.0, at.y - 1.0)),
            grab: true,
            on_dismiss: input(Input::Dismissed(scene.bar)),
        }),
        menus: None,
    }
}

/// The Help menu's search field as the controller holds it.
#[derive(Clone, Debug, PartialEq)]
struct Field {
    text: String,
    caret: usize,
    selection: Range<usize>,
}

/// One row of a menu.
struct RowNode<M> {
    /// The row's place among the menu's items, separators and the search field not counted.
    place: Option<usize>,
    bar: BarId,
    row: PopupRow,
    /// The menu's top-left corner in the bar's coordinates.
    origin: BarPoint,
    /// Highlighted: selected and enabled.
    hot: bool,
    /// Whether the row's submenu is open; `None` for a row without one.
    expanded: Option<bool>,
    /// Where the rows' icons come from.
    icons: Rc<RowIcons>,
    /// The icons' revision as the view was made: an icon that arrived since repaints the row.
    icons_revision: u64,
    /// The width of the column the shortcuts' keys stand in.
    key_width: f64,
    /// The row belongs to a status panel.
    panel: bool,
    /// The search field, in the Help menu's search row.
    field: Option<Field>,
    metrics: MenuMetrics,
    font: BarFont,
    input: fn(Input) -> M,
}

/// Opacity of the secondary text of a status panel: its headings, notes and the disclosure's label.
const PANEL_SECONDARY: f32 = 0.6;
const PANEL_DISCLOSURE: f32 = 0.75;
/// Size of the panel's headings.
const PANEL_HEADING_PT: f32 = 12.0;
/// Edge of the wireless mark in the joined network's disc, and of the Bluetooth mark in a device's.
const PANEL_MARK_PT: f64 = 14.0;
/// Opacity of the disc of a Bluetooth device that is not connected, a share of the text colour.
const PANEL_IDLE_DISC: f32 = 0.18;
/// Inset of a panel row's highlight from the panel's sides and from the row's top and bottom, so it stands clear of
/// the dividing lines. approximate.
const PANEL_HIGHLIGHT_INSET_X: f64 = 8.0;
const PANEL_HIGHLIGHT_INSET_Y: f64 = 3.0;
/// A signal mark's arc radii, the half-angle they span and their stroke.
const SIGNAL_RADII: [f64; 3] = [4.5, 8.0, 11.5];
const SIGNAL_HALF_ANGLE: f64 = std::f64::consts::FRAC_PI_4;
const SIGNAL_STROKE: f64 = 1.6;
const SIGNAL_DIM: f32 = 0.3;
/// Size of a lock: its width, its body's height and the shackle's height above the body.
const LOCK_WIDTH: f64 = 8.5;
const LOCK_BODY: f64 = 6.5;
const LOCK_SHACKLE: f64 = 6.0;

/// A stroke along an arc of `radius` about `centre`, `half_angle` either side of straight up.
fn arc(cx: &mut PaintCx<'_>, centre: Point, radius: f64, half_angle: f64, width: f64, c: Rgba) {
    const STEPS: usize = 12;
    let points: Vec<Point> = (0..=STEPS)
        .map(|i| {
            let a = -half_angle + 2.0 * half_angle * i as f64 / STEPS as f64;
            Point::new(centre.x + radius * a.sin(), centre.y - radius * a.cos())
        })
        .collect();
    polyline(cx, &points, width, c);
}

/// A padlock with its top-left corner at `at`.
fn lock(cx: &mut PaintCx<'_>, at: Point, c: Rgba) {
    let body = Rect::new(at.x, at.y + LOCK_SHACKLE, LOCK_WIDTH, LOCK_BODY);
    fill(cx, body, 1.6, c);
    let radius = (LOCK_WIDTH - 3.0) / 2.0;
    let centre = Point::new(at.x + LOCK_WIDTH / 2.0, at.y + 0.7 + radius);
    let mut points = vec![Point::new(centre.x - radius, at.y + LOCK_SHACKLE)];
    for i in 0..=10 {
        let a = std::f64::consts::PI * i as f64 / 10.0;
        points.push(Point::new(
            centre.x - radius * a.cos(),
            centre.y - radius * a.sin(),
        ));
    }
    points.push(Point::new(centre.x + radius, at.y + LOCK_SHACKLE));
    polyline(cx, &points, 1.4, c);
}

/// A signal mark `strength` of 3 bars strong, its base point at `base`; `c` is the full colour.
fn signal(cx: &mut PaintCx<'_>, base: Point, strength: u8, c: Rgba) {
    circle(cx, base, 1.4, c);
    for (i, radius) in SIGNAL_RADII.into_iter().enumerate() {
        let on = i < usize::from(strength);
        let c = if on { c } else { faded(c, SIGNAL_DIM) };
        arc(cx, base, radius, SIGNAL_HALF_ANGLE, SIGNAL_STROKE, c);
    }
}

impl<M> RowNode<M> {
    /// A row of a status panel.
    fn paint_panel(&self, cx: &mut PaintCx<'_>, row: &PanelRow, palette: &MenuPalette) {
        let size = cx.size();
        let middle = size.height / 2.0;
        let accent = cx
            .env()
            .theme
            .colors
            .get(ColorRole::SelectedControlFill, StateVariant::NORMAL)
            .map_or(palette.row_highlight, |c| [c.r, c.g, c.b, c.a]);
        let text_c = palette.text;
        let secondary = |a: f32| faded(text_c, a);
        let heading_font = BarFont {
            size: PANEL_HEADING_PT,
            ..self.font
        };
        let x = panel::TEXT_INSET;
        // the title row is chosen through its switch and never highlights
        if self.hot && !matches!(row, PanelRow::Header { .. }) {
            let rect = Rect::new(
                PANEL_HIGHLIGHT_INSET_X,
                PANEL_HIGHLIGHT_INSET_Y,
                size.width - 2.0 * PANEL_HIGHLIGHT_INSET_X,
                size.height - 2.0 * PANEL_HIGHLIGHT_INSET_Y,
            );
            fill(cx, rect, ROW_HIGHLIGHT_RADIUS, palette.bar_highlight);
        }
        match row {
            PanelRow::Header { title, on } => {
                text(cx, &self.font, title, x, 0.0, size.height, true, text_c);
                let track = Rect::new(
                    size.width - panel::SWITCH_INSET - panel::SWITCH_WIDTH,
                    middle - panel::SWITCH_HEIGHT / 2.0,
                    panel::SWITCH_WIDTH,
                    panel::SWITCH_HEIGHT,
                );
                let off = faded(text_c, 0.22);
                fill(
                    cx,
                    track,
                    panel::SWITCH_HEIGHT / 2.0,
                    if *on { accent } else { off },
                );
                let inset = (panel::SWITCH_HEIGHT - panel::KNOB_HEIGHT) / 2.0;
                let knob_x = if *on {
                    track.max_x() - inset - panel::KNOB_WIDTH
                } else {
                    track.x() + inset
                };
                let knob = Rect::new(
                    knob_x,
                    track.y() + inset,
                    panel::KNOB_WIDTH,
                    panel::KNOB_HEIGHT,
                );
                fill(cx, knob, panel::KNOB_HEIGHT / 2.0, [1.0, 1.0, 1.0, 1.0]);
            }
            PanelRow::Section { title } => {
                text(
                    cx,
                    &heading_font,
                    title,
                    x,
                    0.0,
                    size.height,
                    true,
                    secondary(PANEL_SECONDARY),
                );
            }
            PanelRow::Current { name, secured } => {
                let disc = Point::new(panel::DISC_INSET + panel::DISC / 2.0, middle);
                circle(cx, disc, panel::DISC / 2.0, accent);
                let mark = MenuItem {
                    icon_name: Some("symbol.wifi".to_owned()),
                    ..MenuItem::default()
                };
                if let Some(icon) = self.icons.row_icon(&mark, row_icon_px(cx.env().scale)) {
                    image(cx, &icon.tinted([1.0, 1.0, 1.0, 1.0]), disc, PANEL_MARK_PT);
                }
                let name_x = panel::DISC_INSET + panel::DISC + panel::DISC_GAP;
                text(
                    cx,
                    &self.font,
                    name,
                    name_x,
                    0.0,
                    size.height,
                    false,
                    text_c,
                );
                if *secured {
                    let at = Point::new(
                        size.width - panel::LOCK_INSET - LOCK_WIDTH,
                        middle - (LOCK_SHACKLE + LOCK_BODY) / 2.0,
                    );
                    lock(cx, at, secondary(PANEL_DISCLOSURE));
                }
            }
            PanelRow::Device { name, connected } => {
                let disc = Point::new(panel::DISC_INSET + panel::DISC / 2.0, middle);
                let (fill_c, mark_c) = if *connected {
                    (accent, [1.0, 1.0, 1.0, 1.0])
                } else {
                    (secondary(PANEL_IDLE_DISC), text_c)
                };
                circle(cx, disc, panel::DISC / 2.0, fill_c);
                let mark = MenuItem {
                    icon_name: Some("symbol.bluetooth".to_owned()),
                    ..MenuItem::default()
                };
                if let Some(icon) = self.icons.row_icon(&mark, row_icon_px(cx.env().scale)) {
                    image(cx, &icon.tinted(mark_c), disc, PANEL_MARK_PT);
                }
                let name_x = panel::DISC_INSET + panel::DISC + panel::DISC_GAP;
                text(
                    cx,
                    &self.font,
                    name,
                    name_x,
                    0.0,
                    size.height,
                    false,
                    text_c,
                );
            }
            PanelRow::Disclosure { title, expanded } => {
                let c = secondary(PANEL_DISCLOSURE);
                text(cx, &self.font, title, x, 0.0, size.height, true, c);
                let at = size.width - panel::LOCK_INSET + 2.0;
                let chevron = if *expanded {
                    [
                        Point::new(at - 5.0, middle - 2.5),
                        Point::new(at, middle + 2.5),
                        Point::new(at + 5.0, middle - 2.5),
                    ]
                } else {
                    [
                        Point::new(at - 2.5, middle - 5.0),
                        Point::new(at + 2.5, middle),
                        Point::new(at - 2.5, middle + 5.0),
                    ]
                };
                polyline(cx, &chevron, 1.6, c);
            }
            PanelRow::Network {
                name,
                secured,
                strength,
            } => {
                text(cx, &self.font, name, x, 0.0, size.height, false, text_c);
                let signal_x = size.width - panel::LOCK_INSET + 1.0 - SIGNAL_RADII[2] * 0.7;
                let base = Point::new(signal_x, middle + SIGNAL_RADII[2] * 0.4);
                signal(cx, base, *strength, secondary(PANEL_DISCLOSURE));
                if *secured {
                    let at = Point::new(
                        signal_x - SIGNAL_RADII[2] * 0.75 - 9.0 - LOCK_WIDTH,
                        middle - (LOCK_SHACKLE + LOCK_BODY) / 2.0,
                    );
                    lock(cx, at, secondary(PANEL_DISCLOSURE));
                }
            }
            PanelRow::Note { text: note } => {
                text(
                    cx,
                    &self.font,
                    note,
                    x,
                    0.0,
                    size.height,
                    false,
                    secondary(PANEL_SECONDARY),
                );
            }
            PanelRow::Link { title } => {
                text(cx, &self.font, title, x, 0.0, size.height, false, text_c);
            }
        }
    }

    /// The shortcut at the right end of the row's `width`: the key in its column, the modifier glyphs right-aligned
    /// just before it. Returns where the modifier glyphs begin.
    fn shortcut(&self, cx: &mut PaintCx<'_>, sc: &ShortcutGlyphs, width: f64, c: Rgba) -> f64 {
        let height = cx.size().height;
        let key_x = width - self.metrics.trailing_inset - self.key_width;
        // the keys stand centred in their column, so keys of different widths line up on its middle
        let centred =
            key_x + (self.key_width - self::width(cx, &self.font, &sc.key)).max(0.0) / 2.0;
        text(cx, &self.font, &sc.key, centred, 0.0, height, false, c);
        let modifiers_x = key_x - self::width(cx, &self.font, &sc.modifiers);
        text(
            cx,
            &self.font,
            &sc.modifiers,
            modifiers_x,
            0.0,
            height,
            false,
            c,
        );
        modifiers_x
    }

    /// The search field: a box with the text, its selection and a caret that fades in and out, or the dimmed label while it is empty.
    /// The box ends before `chord_left`, where the row's chord begins.
    fn field(
        &self,
        cx: &mut PaintCx<'_>,
        field: &Field,
        palette: &MenuPalette,
        chord_left: Option<f64>,
        caret_opacity: f32,
    ) {
        let size = cx.size();
        let left = FIELD_INSET_X;
        let right = chord_left.map_or(size.width - FIELD_INSET_X, |x| x - FIELD_CHORD_GAP);
        let frame = Rect::new(left, FIELD_TOP, right - left, FIELD_HEIGHT);
        let backing = if dark(cx.env()) {
            FIELD_FILL_DARK
        } else {
            FIELD_FILL_LIGHT
        };
        fill(cx, frame, FIELD_HEIGHT / 2.0, backing);
        let x = frame.min_x() + FIELD_TEXT_INSET;
        if field.text.is_empty() {
            let dimmed = faded(palette.text, palette.disabled_alpha);
            text(
                cx,
                &self.font,
                &self.row.item.label,
                x,
                frame.min_y(),
                FIELD_HEIGHT,
                false,
                dimmed,
            );
        }
        let offset =
            |cx: &PaintCx<'_>, at: usize| x + self::width(cx, &self.font, &field.text[..at]);
        let (y, h) = (
            frame.min_y() + FIELD_INSET_Y,
            frame.height() - 2.0 * FIELD_INSET_Y,
        );
        if field.selection.is_empty() {
            let caret = Rect::new(offset(cx, field.caret), y, CARET_WIDTH, h);
            fill(cx, caret, 0.0, faded(palette.text, caret_opacity));
        } else {
            let (start, end) = (
                offset(cx, field.selection.start),
                offset(cx, field.selection.end),
            );
            let tint = faded(palette.row_highlight, FIELD_SELECTION_ALPHA);
            fill(cx, Rect::new(start, y, end - start, h), 0.0, tint);
        }
        text(
            cx,
            &self.font,
            &field.text,
            x,
            frame.min_y(),
            FIELD_HEIGHT,
            false,
            palette.text,
        );
    }
}

/// What a row remembers between frames: where its search field's caret blink restarted, and the text and caret
/// position it restarted for.
#[derive(Default)]
struct CaretBlink {
    since: Cell<Timestamp>,
    seen: RefCell<Option<(String, usize)>>,
}

impl<M: Clone + 'static> Widget<M> for RowNode<M> {
    type State = CaretBlink;

    fn layout(&self) -> LayoutSpec {
        let r = self.row.rect;
        placed(Rect::new(
            r.x - self.origin.x,
            r.y - self.origin.y,
            r.w,
            r.h,
        ))
    }

    fn state(&self) -> CaretBlink {
        CaretBlink::default()
    }

    fn update(&self, old: &Self, _: &mut CaretBlink) -> Change {
        if self.row.rect != old.row.rect || self.origin != old.origin {
            Change::Layout
        } else if self.row != old.row
            || self.hot != old.hot
            || self.expanded != old.expanded
            || self.place != old.place
            || self.key_width != old.key_width
            || self.panel != old.panel
            || self.field != old.field
            || self.metrics != old.metrics
            || self.font != old.font
            || self.icons_revision != old.icons_revision
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &CaretBlink, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.row.rect.w, self.row.rect.h),
            baseline: None,
        }
    }

    fn event(&self, _: &mut CaretBlink, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        // the caret fades: draw it again each frame while it may change
        if let (Event::AnimationFrame(_), Some(_)) = (event, &self.field) {
            cx.request_paint();
            return Propagation::Continue;
        }
        if self.row.item.separator || !self.row.item.enabled {
            return Propagation::Continue;
        }
        clicked(event, cx, self.bar, centre(self.row.rect), self.input)
    }

    fn paint(&self, blink: &CaretBlink, cx: &mut PaintCx<'_>) {
        let palette = popup_palette(cx.env());
        let size = cx.size();
        let m = &self.metrics;
        let item = &self.row.item;
        if let Some(field) = &self.field {
            // an edit or a move restarts the caret's blink
            let now = cx.now();
            let at = (field.text.clone(), field.caret);
            if blink.seen.borrow().as_ref() != Some(&at) {
                *blink.seen.borrow_mut() = Some(at);
                blink.since.set(now);
            }
            let opacity = cupertino_kit::controls::caret_opacity(cx.env(), blink.since.get(), now);
            if cupertino_kit::controls::caret_blinking(cx.env(), blink.since.get(), now) {
                cx.request_animation_frame();
            }
            let faded_text = faded(palette.text, SHORTCUT_ALPHA);
            let chord_left = self
                .row
                .shortcut
                .as_ref()
                .map(|sc| self.shortcut(cx, sc, size.width, faded_text));
            self.field(cx, field, &palette, chord_left, opacity);
            return;
        }
        if let Some(row) = &item.panel {
            self.paint_panel(cx, row, &palette);
            return;
        }
        if item.separator && self.panel {
            let rect = Rect::new(
                panel::SEPARATOR_INSET,
                size.height / 2.0 - 0.5,
                size.width - 2.0 * panel::SEPARATOR_INSET,
                1.0,
            );
            fill(cx, rect, 0.0, palette.rule);
            return;
        }
        if item.separator {
            // a point thick, from the labels' column to the same distance from the trailing edge
            let rect = Rect::new(
                self.row.label_x,
                size.height / 2.0 - 0.5,
                size.width - self.row.label_x - m.padding_x,
                1.0,
            );
            fill(cx, rect, 0.0, palette.rule);
            return;
        }
        if self.hot {
            let rect = Rect::new(
                ROW_HIGHLIGHT_INSET,
                0.0,
                size.width - 2.0 * ROW_HIGHLIGHT_INSET,
                size.height,
            );
            let accent = cx
                .env()
                .theme
                .colors
                .get(ColorRole::SelectedControlFill, StateVariant::NORMAL)
                .map_or(palette.row_highlight, |c| [c.r, c.g, c.b, c.a]);
            fill(cx, rect, ROW_HIGHLIGHT_RADIUS, accent);
        }
        let base = if self.hot {
            palette.row_highlight_text
        } else {
            palette.text
        };
        let c = faded(
            base,
            if item.enabled {
                1.0
            } else {
                palette.disabled_alpha
            },
        );
        let middle = size.height / 2.0;
        if item.toggle.is_marked() {
            // the mark stands in the column before the labels, centred on the labels' edge of a menu without marks
            let x = m.padding_x;
            match item.toggle {
                Toggle::Radio(_) => circle(cx, Point::new(x, middle), RADIO_DOT_RADIUS, c),
                _ => {
                    let check = [
                        Point::new(x - 4.0, middle),
                        Point::new(x - 1.2, middle + 3.2),
                        Point::new(x + 4.2, middle - 3.8),
                    ];
                    polyline(cx, &check, CHECK_STROKE, c);
                }
            }
        }
        if let Some(icon) = self.icons.row_icon(item, row_icon_px(cx.env().scale)) {
            let origin = self.row.label_x - m.icon_column;
            let centre = Point::new(origin + ROW_ICON_PT / 2.0, middle);
            image(cx, &icon.tinted(c), centre, ROW_ICON_PT);
        }
        text(
            cx,
            &self.font,
            &item.label,
            self.row.label_x,
            0.0,
            size.height,
            false,
            c,
        );
        if let Some(detail) = &self.row.detail {
            let x = self.row.label_x + width(cx, &self.font, &item.label) + m.detail_gap;
            text(
                cx,
                &self.font,
                detail,
                x,
                0.0,
                size.height,
                false,
                faded(c, DETAIL_ALPHA),
            );
        }
        if let Some(sc) = &self.row.shortcut {
            let room = if item.has_submenu {
                size.width - m.arrow_width
            } else {
                size.width
            };
            // white on the highlight; else a share of the label's strength, less on a disabled row
            let keys = match (item.enabled, self.hot) {
                (_, true) => c,
                (true, false) => faded(c, SHORTCUT_ALPHA),
                (false, false) => faded(c, DISABLED_SHORTCUT_ALPHA),
            };
            self.shortcut(cx, sc, room, keys);
        }
        if item.has_submenu {
            let x = size.width - m.trailing_inset - CHEVRON_HALF_WIDTH - CHEVRON_SHIFT;
            let chevron = [
                Point::new(x - CHEVRON_HALF_WIDTH, middle - 3.6),
                Point::new(x + CHEVRON_HALF_WIDTH, middle),
                Point::new(x - CHEVRON_HALF_WIDTH, middle + 3.6),
            ];
            polyline(cx, &chevron, CHEVRON_STROKE, c);
        }
    }

    fn semantics(&self, _: &CaretBlink, cx: &mut SemanticsCx<'_>) {
        let item = &self.row.item;
        if item.separator {
            cx.role(Role::Separator);
            return;
        }
        if let Some(field) = &self.field {
            cx.role(Role::TextInput);
            cx.name(item.label.clone());
            cx.value(AccessibleValue::Text {
                value: field.text.clone(),
                editing: None,
            });
            cx.states(AccessibleStates {
                focused: true,
                ..AccessibleStates::default()
            });
            return;
        }
        let checked =
            |on: bool| AccessibleValue::Toggle(if on { Checked::True } else { Checked::False });
        match item.toggle {
            Toggle::None => cx.role(Role::MenuItem),
            Toggle::Check(on) => {
                cx.role(Role::MenuItemCheckBox);
                cx.value(checked(on));
            }
            Toggle::Indeterminate => {
                cx.role(Role::MenuItemCheckBox);
                cx.value(AccessibleValue::Toggle(Checked::Mixed));
            }
            Toggle::Radio(on) => {
                cx.role(Role::MenuItemRadio);
                cx.value(checked(on));
            }
        }
        cx.name(item.label.clone());
        if let Some(place) = self.place {
            cx.item_index(place);
        }
        if let Some(detail) = &self.row.detail {
            cx.description(detail.clone());
        }
        cx.states(AccessibleStates {
            disabled: !item.enabled,
            selected: self.hot,
            expanded: self.expanded,
            ..AccessibleStates::default()
        });
        if item.enabled {
            cx.action(Action::Click);
        }
    }
}

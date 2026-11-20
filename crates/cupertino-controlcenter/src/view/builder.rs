//! Building the panel's elements from the readings, the arrangement and the presentation.

use super::*;

/// Builds the panel's elements and records where each control lies, for the arrow keys.
pub(super) struct Builder<'a, M> {
    pub(super) controls: &'a Controls,
    pub(super) arrangement: &'a Arrangement,
    pub(super) presentation: Presentation,
    pub(super) to: fn(Input) -> M,
    pub(super) map: Map,
    /// The controls it builds show and do not act: a tile's own content while the controls are edited.
    pub(super) inert: bool,
}

impl<M: Clone + 'static> Builder<'_, M> {
    /// Where `module`'s tile lies, from the grid's corner: where the arrangement puts it. A module it does not show
    /// has no tile; its place with every module shown stands in.
    pub(super) fn frame(&self, module: Module) -> Rect {
        self.arrangement
            .frame(module)
            .unwrap_or_else(|| layout::frame(module))
    }

    /// Height of the tiles' rows.
    pub(super) fn tiles(&self) -> f64 {
        self.arrangement.height()
    }

    /// A builder of controls that show and do not act, and lead no arrow key to them.
    pub(super) fn still(&self) -> Self {
        Builder {
            controls: self.controls,
            arrangement: self.arrangement,
            presentation: self.presentation,
            to: self.to,
            map: Rc::new(RefCell::new(Vec::new())),
            inert: true,
        }
    }

    /// Records a control of `key` at `frame` within a tile at `origin` in the window.
    pub(super) fn record(&self, key: &str, origin: Point, frame: Rect) {
        self.map
            .borrow_mut()
            .push((key.to_owned(), frame.translate(origin.x, origin.y)));
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn pressable(
        &self,
        origin: Point,
        key: &str,
        label: &str,
        kind: Kind,
        face: Face,
        frame: Rect,
        enabled: bool,
        on_press: Option<Input>,
        children: Vec<Element<M>>,
    ) -> Element<M> {
        // while the controls are edited a tile's own controls show and do not act
        let on_press = on_press.filter(|_| !self.inert);
        let interactive = enabled && on_press.is_some();
        if interactive {
            self.record(key, origin, frame);
        }
        Element::new(Pressable {
            key: key.to_owned(),
            label: label.to_owned(),
            description: None,
            kind,
            face,
            frame,
            enabled,
            on_press,
            map: Rc::clone(&self.map),
            to: self.to,
            children,
        })
        .key(key.to_owned())
    }

    pub(super) fn level(
        &self,
        origin: Point,
        key: &str,
        slider: Slider,
        frame: Rect,
    ) -> Element<M> {
        let value = self.controls.fraction(slider);
        if value.is_some() && !self.inert {
            self.record(key, origin, frame);
        }
        Element::new(Level {
            key: key.to_owned(),
            label: slider.module().title().to_owned(),
            slider,
            frame,
            value,
            panel: self.presentation.alpha,
            inert: self.inert,
            map: Rc::clone(&self.map),
            to: self.to,
        })
        .key(key.to_owned())
    }

    /// A glass tile at `frame` in the grid.
    pub(super) fn tile(
        &self,
        label: &str,
        frame: Rect,
        outline: Outline,
        alpha: f64,
        children: Vec<Element<M>>,
    ) -> Element<M> {
        let frame = frame.translate(GRID_ORIGIN.x, GRID_ORIGIN.y);
        Element::new(GlassTile {
            label: label.to_owned(),
            frame,
            radius: outline.radius(frame.size),
            capsule: outline == Outline::Capsule,
            panel: self.presentation.alpha,
            alpha,
            light: self.presentation.light,
            children,
        })
    }

    /// A title over an optional second line, in a block from `x` to the tile's end, centred down the tile.
    pub(super) fn lines(
        &self,
        title: &str,
        line: Option<&str>,
        x: f64,
        width: f64,
        height: f64,
    ) -> Element<M> {
        let mut children = vec![wrapping(title, TypeSize::Body, TITLE_WEIGHT, Tone::Title)];
        if let Some(line) = line {
            children.push(wrapping(line, TypeSize::Caption, LINE_WEIGHT, Tone::Line));
        }
        Place::stack(
            Rect::new(x, 0.0, (width - x - TEXT_END_INSET).max(0.0), height),
            Axis::Vertical,
            false,
            children,
        )
    }

    /// A capsule's content: its badge switch and its title and state. A capsule with a detail view opens it from its
    /// text; the others switch from either.
    pub(super) fn capsule(
        &self,
        module: Module,
        origin: Point,
        key: &str,
        header: bool,
    ) -> Vec<Element<M>> {
        let controls = self.controls;
        let state = controls.tile(module);
        let on = state == TileState::On;
        let enabled = !matches!(state, TileState::Disabled(_));
        let size = layout::frame(module).size;
        let tint = on.then_some(ColorRole::DefaultButtonFill);
        let badge = self.pressable(
            origin,
            key,
            module.title(),
            Kind::Switch(on),
            Face::Badge { on },
            Rect::new(BADGE_INSET, BADGE_INSET, BADGE, BADGE),
            enabled,
            Some(Input::Press(module)),
            vec![icon(symbol(controls, module), BADGE_SYMBOL, tint)],
        );
        match module.detail().filter(|_| !header) {
            Some(detail) => {
                let label = format!("{} {}", module.title(), detail.heading());
                let more = self.pressable(
                    origin,
                    &format!("{key}.more"),
                    &label,
                    Kind::Expander(false),
                    Face::Plain {
                        radius: 0.0,
                        hover: false,
                    },
                    Rect::new(
                        TEXT_START - 4.0,
                        0.0,
                        size.width - TEXT_START + 4.0,
                        size.height,
                    ),
                    true,
                    Some(Input::Expand(detail, Via::Pointer)),
                    vec![Place::absolute(
                        Rect::new(0.0, 0.0, size.width - TEXT_START + 4.0, size.height),
                        vec![self.lines(
                            controls.title(module),
                            controls.subtitle(module),
                            4.0,
                            size.width - TEXT_START + 4.0,
                            size.height,
                        )],
                    )],
                );
                vec![badge, more]
            }
            None => {
                let width = if header {
                    layout::GRID_WIDTH
                } else {
                    size.width
                };
                let lines = self.lines(
                    controls.title(module),
                    controls.subtitle(module),
                    TEXT_START,
                    width,
                    size.height,
                );
                vec![badge, lines]
            }
        }
    }

    /// A circular tile: the whole tile is the switch or button.
    pub(super) fn circle(&self, module: Module, origin: Point) -> Vec<Element<M>> {
        let controls = self.controls;
        let state = controls.tile(module);
        let on = state == TileState::On;
        let enabled = !matches!(state, TileState::Disabled(_));
        let kind = match module {
            Module::LockScreen => Kind::Button,
            _ => Kind::Switch(on),
        };
        let tint = on.then_some(ColorRole::DefaultButtonFill);
        let size = layout::frame(module).size;
        vec![self.pressable(
            origin,
            &focus_key(module),
            module.title(),
            kind,
            Face::Circle { on },
            Rect::from_origin_size(Point::ZERO, size),
            enabled,
            Some(Input::Press(module)),
            vec![icon(symbol(controls, module), CIRCLE_SYMBOL, tint)],
        )]
    }

    /// The media tile: artwork, what plays, and the previous, play / pause and next buttons.
    pub(super) fn media(&self, origin: Point) -> Vec<Element<M>> {
        let size = layout::frame(Module::NowPlaying).size;
        let media = match &self.controls.media {
            Reading::Ready(media) => media.as_ref(),
            Reading::Unavailable(_) => None,
        };
        let (title, artist) = match media {
            Some(media) => (media.title.as_str(), media.artist.as_deref()),
            None => ("Not Playing", None),
        };
        let artwork = Element::new(Artwork {
            frame: Rect::new(BADGE_INSET, BADGE_INSET, ART, ART),
        });
        let block = Place::stack(
            Rect::new(
                BADGE_INSET + 4.0,
                MEDIA_TEXT_TOP,
                size.width - 2.0 * BADGE_INSET - 4.0,
                MEDIA_BUTTONS_Y - MEDIA_BUTTON / 2.0 - MEDIA_TEXT_TOP,
            ),
            Axis::Vertical,
            false,
            [
                Some(text(title, TypeSize::Body, TITLE_WEIGHT, Tone::Title)),
                artist.map(|a| text(a, TypeSize::Caption, LINE_WEIGHT, Tone::Line)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        );
        let playing = media.is_some_and(|m| m.playing);
        let buttons = [
            (
                MediaCommand::Previous,
                "Previous",
                "symbol.backward",
                media.is_some_and(|m| m.can_previous),
            ),
            (
                MediaCommand::PlayPause,
                if playing { "Pause" } else { "Play" },
                if playing {
                    "symbol.pause"
                } else {
                    "symbol.play"
                },
                media.is_some_and(|m| m.can_play_pause),
            ),
            (
                MediaCommand::Next,
                "Next",
                "symbol.forward",
                media.is_some_and(|m| m.can_next),
            ),
        ];
        let mut children = vec![artwork, block];
        for ((command, label, name, enabled), x) in buttons.into_iter().zip(MEDIA_BUTTONS_X) {
            let key = match command {
                MediaCommand::Previous => format!("{}.previous", focus_key(Module::NowPlaying)),
                MediaCommand::PlayPause => focus_key(Module::NowPlaying),
                MediaCommand::Next => format!("{}.next", focus_key(Module::NowPlaying)),
            };
            children.push(self.pressable(
                origin,
                &key,
                label,
                Kind::Button,
                Face::Plain {
                    radius: MEDIA_BUTTON / 2.0,
                    hover: false,
                },
                Rect::new(
                    x - MEDIA_BUTTON / 2.0,
                    MEDIA_BUTTONS_Y - MEDIA_BUTTON / 2.0,
                    MEDIA_BUTTON,
                    MEDIA_BUTTON,
                ),
                enabled,
                Some(Input::Media(command)),
                vec![icon(
                    name,
                    if command == MediaCommand::PlayPause {
                        22.0
                    } else {
                        18.0
                    },
                    None,
                )],
            ));
        }
        children
    }

    /// A slider tile's content: its title (which opens the detail view on the grid) and the level between its
    /// symbols, with the output button on the sound tile.
    pub(super) fn slider(&self, slider: Slider, origin: Point, header: bool) -> Vec<Element<M>> {
        let module = slider.module();
        let detail = module.detail().expect("both sliders have a detail view");
        let title = text(module.title(), TypeSize::Body, TITLE_WEIGHT, Tone::Primary);
        let title_frame = Rect::new(
            SLIDER_TITLE_INSET - 8.0,
            SLIDER_TITLE_TOP - 4.0,
            120.0,
            24.0,
        );
        let title = if header {
            Place::stack(
                title_frame,
                Axis::Horizontal,
                false,
                vec![Element::new(Spacer8), title],
            )
        } else {
            self.pressable(
                origin,
                &format!("{}.more", focus_key(module)),
                &format!("{} {}", module.title(), detail.heading()),
                Kind::Expander(false),
                Face::Plain {
                    radius: 8.0,
                    hover: false,
                },
                title_frame,
                true,
                Some(Input::Expand(detail, Via::Pointer)),
                vec![Place::stack(
                    Rect::from_origin_size(Point::ZERO, title_frame.size),
                    Axis::Horizontal,
                    false,
                    vec![Element::new(Spacer8), title],
                )],
            )
        };
        let (start, end) = match slider {
            Slider::Display => DISPLAY_TRACK,
            Slider::Sound => SOUND_TRACK,
        };
        let key = if header {
            DETAIL_FOCUS.to_owned()
        } else {
            focus_key(module)
        };
        let level = self.level(
            origin,
            &key,
            slider,
            Rect::new(
                start - LEVEL_REACH,
                TRACK_CENTRE - SLIDER_BAND / 2.0,
                end - start + 2.0 * LEVEL_REACH,
                SLIDER_BAND,
            ),
        );
        let symbol_at = |x: f64, size: f64, name: &str| {
            Place::centred(
                Rect::new(x, TRACK_CENTRE - size / 2.0, size, size),
                icon(name, size, None),
            )
        };
        let mut children = vec![title, level];
        match slider {
            Slider::Display => {
                children.push(symbol_at(15.0, 14.0, "symbol.sun"));
                children.push(symbol_at(266.0, 18.0, "symbol.sun"));
            }
            Slider::Sound => {
                children.push(symbol_at(14.0, 14.0, "symbol.speaker.low"));
                children.push(symbol_at(219.0, 20.0, "symbol.speaker.high"));
                let output = Rect::new(
                    OUTPUT_CENTRE_X - OUTPUT_BUTTON / 2.0,
                    TRACK_CENTRE - OUTPUT_BUTTON / 2.0,
                    OUTPUT_BUTTON,
                    OUTPUT_BUTTON,
                );
                children.push(self.pressable(
                    origin,
                    &format!("{}.output", focus_key(module)),
                    "Output",
                    Kind::Expander(header),
                    Face::Badge { on: false },
                    output,
                    true,
                    Some(if header {
                        Input::Collapse(Via::Pointer)
                    } else {
                        Input::Expand(Detail::Sound, Via::Pointer)
                    }),
                    vec![icon("symbol.speaker", 14.0, None)],
                ));
            }
        }
        children
    }

    /// The content of `module`'s tile at the tile's origin in the window.
    pub(super) fn content(&self, module: Module, origin: Point) -> Vec<Element<M>> {
        match module {
            Module::WiFi | Module::Bluetooth | Module::Microphone | Module::FocusMode => {
                self.capsule(module, origin, &focus_key(module), false)
            }
            Module::DockHiding | Module::LockScreen | Module::Mute | Module::KeyboardBrightness => {
                self.circle(module, origin)
            }
            Module::NowPlaying => self.media(origin),
            Module::Display => self.slider(Slider::Display, origin, false),
            Module::Sound => self.slider(Slider::Sound, origin, false),
        }
    }

    /// The rows of `detail`'s list: label, symbol, whether it is the current one, and what a press does.
    pub(super) fn rows(&self, detail: Detail) -> Result<Vec<Row>, String> {
        let controls = self.controls;
        let rows = match detail {
            Detail::WiFi => match &controls.networks {
                Reading::Ready(networks) => networks
                    .iter()
                    .map(|n| Row {
                        label: n.ssid.clone(),
                        symbol: "symbol.wifi",
                        trailing: if n.connected {
                            Some("symbol.checkmark")
                        } else if n.secured {
                            Some("symbol.lock")
                        } else {
                            None
                        },
                        current: n.connected,
                        on_press: n.joinable().then(|| Input::Join(n.ssid.clone())),
                    })
                    .collect(),
                Reading::Unavailable(reason) => return Err(reason.clone()),
            },
            Detail::Bluetooth => match &controls.devices {
                Reading::Ready(devices) => devices
                    .iter()
                    .map(|d| Row {
                        label: d.name.clone(),
                        symbol: "symbol.bluetooth",
                        trailing: d.connected.then_some("symbol.checkmark"),
                        current: d.connected,
                        on_press: Some(Input::Connect(d.id.clone())),
                    })
                    .collect(),
                Reading::Unavailable(reason) => return Err(reason.clone()),
            },
            Detail::Sound => match &controls.outputs {
                Reading::Ready(outputs) => outputs
                    .iter()
                    .map(|o| Row {
                        label: o.name.clone(),
                        symbol: "symbol.speaker",
                        trailing: o.current.then_some("symbol.checkmark"),
                        current: o.current,
                        on_press: (!o.current).then_some(Input::Output(o.id)),
                    })
                    .collect(),
                Reading::Unavailable(reason) => return Err(reason.clone()),
            },
            Detail::Display => {
                let module = Module::KeyboardBrightness;
                match controls.tile(module) {
                    TileState::Disabled(reason) => return Err(reason),
                    state => vec![Row {
                        label: module.title().to_owned(),
                        symbol: "symbol.keyboard-brightness",
                        trailing: (state == TileState::On).then_some("symbol.checkmark"),
                        current: state == TileState::On,
                        on_press: Some(Input::Press(module)),
                    }],
                }
            }
        };
        Ok(rows)
    }

    /// The number of list rows `detail` shows.
    pub(super) fn row_count(&self, detail: Detail) -> usize {
        self.rows(detail)
            .map_or(1, |rows| rows.len())
            .clamp(1, layout::detail_rows())
    }

    /// The open view's content at the view's origin in the window.
    pub(super) fn detail(&self, detail: Detail, origin: Point) -> Vec<Element<M>> {
        let module = detail.module();
        let mut children = match detail {
            Detail::WiFi | Detail::Bluetooth => self.capsule(module, origin, DETAIL_FOCUS, true),
            Detail::Display => self.slider(Slider::Display, origin, true),
            Detail::Sound => self.slider(Slider::Sound, origin, true),
        };
        let width = layout::GRID_WIDTH;
        let row_width = width - 2.0 * rows::ROW_INSET;
        children.push(Place::stack(
            Rect::new(
                rows::ROW_INSET + rows::TEXT_INSET,
                rows::HEADER,
                row_width - 2.0 * rows::TEXT_INSET,
                rows::HEADING,
            ),
            Axis::Horizontal,
            false,
            vec![text(
                detail.heading(),
                TypeSize::Caption,
                TITLE_WEIGHT,
                Tone::Secondary,
            )],
        ));
        let mut y = rows::HEADER + rows::HEADING;
        match self.rows(detail) {
            Ok(list) if !list.is_empty() => {
                let list: Vec<_> = list.into_iter().take(layout::detail_rows()).collect();
                let at = Point::new(origin.x, origin.y + y);
                let items = list
                    .into_iter()
                    .enumerate()
                    .map(|(i, row)| {
                        let frame = Rect::new(
                            rows::ROW_INSET,
                            i as f64 * rows::ITEM,
                            row_width,
                            rows::ITEM,
                        );
                        self.row(at, i, frame, row)
                    })
                    .collect::<Vec<_>>();
                let height = items.len() as f64 * rows::ITEM;
                children.push(Place::list(
                    Rect::new(0.0, y, width, height),
                    detail.heading(),
                    items,
                ));
                y += height;
            }
            outcome => {
                let message = match outcome {
                    Err(reason) => format!("Unavailable: {reason}"),
                    Ok(_) => "None found".to_owned(),
                };
                children.push(Place::stack(
                    Rect::new(
                        rows::ROW_INSET + rows::TEXT_INSET,
                        y,
                        row_width - 2.0 * rows::TEXT_INSET,
                        rows::ITEM,
                    ),
                    Axis::Horizontal,
                    false,
                    vec![text(&message, TypeSize::Body, ROW_WEIGHT, Tone::Secondary)],
                ));
                y += rows::ITEM;
            }
        }
        children.push(Element::new(Line {
            frame: Rect::new(
                rows::ROW_INSET + rows::TEXT_INSET,
                y + (rows::SEPARATOR - 1.0) / 2.0,
                row_width - 2.0 * rows::TEXT_INSET,
                1.0,
            ),
        }));
        y += rows::SEPARATOR;
        children.push(self.pressable(
            origin,
            "detail.settings",
            detail.settings_label(),
            Kind::Button,
            Face::Plain {
                radius: ROW_RADIUS,
                hover: true,
            },
            Rect::new(rows::ROW_INSET, y, row_width, rows::ITEM),
            true,
            Some(Input::Settings(detail)),
            vec![Place::stack(
                Rect::new(0.0, 0.0, row_width, rows::ITEM),
                Axis::Horizontal,
                false,
                vec![
                    Element::new(Spacer8),
                    text(
                        detail.settings_label(),
                        TypeSize::Body,
                        ROW_WEIGHT,
                        Tone::Primary,
                    ),
                ],
            )],
        ));
        children
    }

    pub(super) fn row(&self, origin: Point, index: usize, frame: Rect, row: Row) -> Element<M> {
        let key = &format!("detail.row.{index}");
        let width = frame.width();
        let mut parts = vec![
            Place::centred(
                Rect::new(
                    rows::TEXT_INSET,
                    (rows::ITEM - ROW_SYMBOL) / 2.0,
                    ROW_SYMBOL,
                    ROW_SYMBOL,
                ),
                icon(row.symbol, ROW_SYMBOL, None),
            ),
            Place::stack(
                Rect::new(
                    rows::TEXT_INSET + ROW_SYMBOL + 10.0,
                    0.0,
                    width - 2.0 * rows::TEXT_INSET - 2.0 * ROW_SYMBOL - 20.0,
                    rows::ITEM,
                ),
                Axis::Horizontal,
                false,
                vec![text(&row.label, TypeSize::Body, ROW_WEIGHT, Tone::Primary)],
            ),
        ];
        if let Some(trailing) = row.trailing {
            parts.push(Place::centred(
                Rect::new(
                    width - rows::TEXT_INSET - ROW_SYMBOL,
                    (rows::ITEM - ROW_SYMBOL) / 2.0,
                    ROW_SYMBOL,
                    ROW_SYMBOL,
                ),
                icon(trailing, ROW_SYMBOL - 2.0, None),
            ));
        }
        let enabled = row.on_press.is_some() || row.current;
        self.pressable(
            origin,
            key,
            &row.label,
            Kind::Row {
                index,
                selected: row.current,
            },
            Face::Plain {
                radius: ROW_RADIUS,
                hover: true,
            },
            frame,
            enabled,
            row.on_press,
            vec![Place::absolute(
                Rect::from_origin_size(Point::ZERO, frame.size),
                parts,
            )],
        )
    }

    /// A glass capsule at `frame` in the grid that is one button: Edit Controls, or Cancel and Done while editing.
    pub(super) fn capsule_button(
        &self,
        frame: Rect,
        key: &str,
        label: &str,
        weight: u16,
        alpha: f64,
        on_press: Input,
    ) -> Element<M> {
        let origin = frame.origin;
        let origin = Point::new(origin.x + GRID_ORIGIN.x, origin.y + GRID_ORIGIN.y);
        let size = frame.size;
        self.tile(
            label,
            frame,
            Outline::Capsule,
            alpha,
            vec![self.pressable(
                origin,
                key,
                label,
                Kind::Button,
                Face::Plain {
                    radius: size.height / 2.0,
                    hover: false,
                },
                Rect::from_origin_size(Point::ZERO, size),
                true,
                Some(on_press),
                vec![text(label, TypeSize::Body, weight, Tone::Primary)],
            )],
        )
    }

    /// The Edit Controls capsule under the tiles.
    pub(super) fn edit(&self, alpha: f64) -> Element<M> {
        self.capsule_button(
            layout::edit_frame_under(self.tiles()),
            EDIT_FOCUS,
            "Edit Controls",
            EDIT_WEIGHT,
            alpha,
            Input::EditControls,
        )
    }

    /// The panel while its controls are edited. Every shown tile lies where the edit puts it, the held one where
    /// the pointer holds it and over the others; its content shows and does not act, under one control that moves
    /// the tile (a drag; the arrow keys; Delete removes it) and a mark that removes it. Cancel and Done take the
    /// place of Edit Controls, Done under its key so the focus stays, and the controls not shown are chips under
    /// them that add.
    pub(super) fn edit_board(&self, edit: Editing) -> Vec<Element<M>> {
        let still = self.still();
        let places = self.arrangement.places();
        let count = places.len();
        let mut children = Vec::new();
        let mut held = None;
        for (index, (module, slot)) in places.into_iter().enumerate() {
            let holding = edit.held.filter(|held| held.module == module);
            let frame = holding.map_or(slot, |h| Rect::from_origin_size(h.origin, slot.size));
            let origin = Point::new(frame.x() + GRID_ORIGIN.x, frame.y() + GRID_ORIGIN.y);
            let whole = Rect::from_origin_size(Point::ZERO, frame.size);
            let outline = layout::outline(module);
            let key = edit_key(module);
            self.record(&key, origin, whole);
            let content = Element::new(Fade {
                frame: whole,
                alpha: EDITED_CONTENT,
                hidden: true,
                children: still.content(module, origin),
            });
            let handle = Element::new(Handle {
                module,
                label: module.title().to_owned(),
                place: index + 1,
                count,
                frame: whole,
                radius: outline.radius(frame.size),
                to: self.to,
            })
            .key(key);
            let remove = self.pressable(
                origin,
                &format!("edit.remove.{}", slug(module)),
                &format!("Remove {}", module.title()),
                Kind::Button,
                Face::Remove,
                editing::mark(module, frame.size),
                true,
                Some(Input::Remove(module, Via::Pointer)),
                Vec::new(),
            );
            let tile = self
                .tile(
                    module.title(),
                    frame,
                    outline,
                    1.0,
                    vec![content, handle, remove],
                )
                .key(format!("tile.{}", slug(module)));
            if holding.is_some() {
                held = Some(tile);
            } else {
                children.push(tile);
            }
        }
        let (cancel, done) = editing::buttons(self.tiles());
        children.push(
            self.capsule_button(
                cancel,
                "edit-cancel",
                "Cancel",
                EDIT_WEIGHT,
                1.0,
                Input::Cancel,
            )
            .key("tile.cancel"),
        );
        children.push(
            self.capsule_button(done, EDIT_FOCUS, "Done", TITLE_WEIGHT, 1.0, Input::Done)
                .key("tile.edit"),
        );
        let offered = self.arrangement.available();
        for (index, module) in offered.iter().copied().enumerate() {
            let frame = editing::chip(index, offered.len(), self.tiles());
            let origin = Point::new(frame.x() + GRID_ORIGIN.x, frame.y() + GRID_ORIGIN.y);
            let add = self.pressable(
                origin,
                &format!("edit.add.{}", slug(module)),
                &format!("Add {}", module.title()),
                Kind::Button,
                Face::Add,
                Rect::from_origin_size(Point::ZERO, frame.size),
                true,
                Some(Input::Add(module, Via::Pointer)),
                vec![icon(symbol(self.controls, module), CHIP_SYMBOL, None)],
            );
            children.push(
                self.tile(
                    &format!("Add {}", module.title()),
                    frame,
                    Outline::Capsule,
                    1.0,
                    vec![add],
                )
                .key(format!("chip.{}", slug(module))),
            );
        }
        // last, so it is drawn over what it passes
        children.extend(held);
        children
    }

    /// Height of what the tiles cover: the tiles with the Edit Controls capsule under them, going to the open detail
    /// view's height as it opens; while the controls are edited, down to the controls offered.
    pub(super) fn height(&self) -> f64 {
        if self.presentation.edit.is_some() {
            return editing::bottom(self.arrangement.available().len(), self.tiles());
        }
        let grid = layout::edit_frame_under(self.tiles()).max_y();
        self.presentation.detail.map_or(grid, |o| {
            let open = layout::detail_frame(self.row_count(o.detail)).height();
            grid + (open - grid) * o.progress.clamp(0.0, 1.0)
        })
    }

    /// The tiles' outlines from the grid's corner, each with the opacity of its glass: what the soft shadow is cut
    /// away over. They follow the tiles as [`Builder::board`] lays them out, so the shadow changes with them and never
    /// at once.
    pub(super) fn cutouts(&self) -> Vec<Cutout> {
        let opened = self.presentation.detail;
        let others = opened.map_or(1.0, |o| visibility(o.progress).others);
        let source = opened.map(|o| o.detail.module());
        let cutout = |rect: Rect, radius: f64, alpha: f64| Cutout {
            rect,
            radius,
            alpha,
        };
        let mut cutouts = Vec::new();
        if let Some(edit) = self.presentation.edit {
            for (module, slot) in self.arrangement.places() {
                let frame = edit
                    .held
                    .filter(|held| held.module == module)
                    .map_or(slot, |held| Rect::from_origin_size(held.origin, slot.size));
                cutouts.push(cutout(
                    frame,
                    layout::outline(module).radius(frame.size),
                    1.0,
                ));
            }
            let (cancel, done) = editing::buttons(self.tiles());
            cutouts.push(cutout(cancel, cancel.height() / 2.0, 1.0));
            cutouts.push(cutout(done, done.height() / 2.0, 1.0));
            let offered = self.arrangement.available().len();
            for index in 0..offered {
                let chip = editing::chip(index, offered, self.tiles());
                cutouts.push(cutout(chip, chip.height() / 2.0, 1.0));
            }
            return cutouts;
        }
        if others > 0.0 {
            for (module, frame) in self.arrangement.places() {
                if Some(module) == source {
                    continue;
                }
                cutouts.push(cutout(
                    frame,
                    layout::outline(module).radius(frame.size),
                    others,
                ));
            }
            let edit = layout::edit_frame_under(self.tiles());
            cutouts.push(cutout(edit, edit.height() / 2.0, others));
        }
        if let Some(opened) = opened {
            let module = opened.detail.module();
            let p = opened.progress.clamp(0.0, 1.0);
            if opened.morph {
                let (frame, radius) = self.morph(opened);
                cutouts.push(cutout(frame, radius, 1.0));
            } else {
                let tile = self.frame(module);
                if p < 1.0 {
                    cutouts.push(cutout(
                        tile,
                        layout::outline(module).radius(tile.size),
                        1.0 - p,
                    ));
                }
                if p > 0.0 {
                    let open = layout::detail_frame(self.row_count(opened.detail));
                    cutouts.push(cutout(open, LARGE_RADIUS, p));
                }
            }
        }
        cutouts
    }

    /// The frame and corner radius of the tile growing into the open view, from the grid's corner.
    pub(super) fn morph(&self, opened: Opened) -> (Rect, f64) {
        let module = opened.detail.module();
        let tile = self.frame(module);
        let frame = layout::morph_from(tile, self.row_count(opened.detail), opened.progress);
        let from = layout::outline(module).radius(tile.size);
        let p = opened.progress.clamp(0.0, 1.0);
        (frame, from + (LARGE_RADIUS - from) * p)
    }

    /// What the soft shadow goes around: the grid with the Edit Controls capsule, going to the open detail view's
    /// outline as it opens. It fades with the panel.
    pub(super) fn shadow(&self) -> Rect {
        Rect::new(0.0, 0.0, GRID_WIDTH, self.height())
    }

    /// Every tile, with the detail view opening, open or closing. Every tile keeps the same module glass in the one
    /// container through the motion, only its opacity changing, so nothing changes look the moment a motion starts,
    /// turns or ends.
    pub(super) fn board(&self) -> Vec<Element<M>> {
        if let Some(edit) = self.presentation.edit {
            return self.edit_board(edit);
        }
        let opened = self.presentation.detail;
        let shown = opened.map_or(
            Visibility {
                others: 1.0,
                tile_content: 1.0,
                detail_content: 0.0,
            },
            |o| visibility(o.progress),
        );
        let source = opened.map(|o| o.detail.module());
        let mut children = Vec::new();
        for (module, frame) in self.arrangement.places() {
            if Some(module) == source {
                continue;
            }
            if shown.others <= 0.0 {
                continue;
            }
            let origin = Point::new(frame.x() + GRID_ORIGIN.x, frame.y() + GRID_ORIGIN.y);
            let content = self.content(module, origin);
            children.push(
                self.tile(
                    module.title(),
                    frame,
                    layout::outline(module),
                    shown.others,
                    content,
                )
                .key(format!("tile.{}", slug(module))),
            );
        }
        if shown.others > 0.0 {
            children.push(self.edit(shown.others).key("tile.edit"));
        }
        if let Some(opened) = opened {
            children.extend(self.opening(opened, shown));
        }
        children
    }

    /// The tile of the detail view: grown from its place with its content giving way to the view's, or with Reduce
    /// Motion the tile fading in place and the view fading in at its own place.
    pub(super) fn opening(&self, opened: Opened, shown: Visibility) -> Vec<Element<M>> {
        let detail = opened.detail;
        let module = detail.module();
        let rows = self.row_count(detail);
        let open = layout::detail_frame(rows);
        let tile_frame = self.frame(module);
        let at = |frame: Rect| Point::new(frame.x() + GRID_ORIGIN.x, frame.y() + GRID_ORIGIN.y);
        let faded = |alpha: f64, children: Vec<Element<M>>| {
            if alpha >= 1.0 {
                children
            } else {
                vec![Element::new(Fade {
                    frame: Rect::from_origin_size(
                        Point::ZERO,
                        Size::new(open.width(), open.height()),
                    ),
                    alpha,
                    hidden: false,
                    children,
                })]
            }
        };
        let label = module.title();
        if !opened.morph {
            let mut tiles = Vec::new();
            let p = opened.progress.clamp(0.0, 1.0);
            if p < 1.0 {
                let content = self.content(module, at(tile_frame));
                tiles.push(
                    self.tile(label, tile_frame, layout::outline(module), 1.0 - p, content)
                        .key(format!("tile.{}", slug(module))),
                );
            }
            if p > 0.0 {
                let content = self.detail(detail, at(open));
                tiles.push(
                    self.tile(label, open, Outline::Rounded(LARGE_RADIUS), p, content)
                        .key("tile.detail"),
                );
            }
            return tiles;
        }
        let (frame, radius) = self.morph(opened);
        let mut content = Vec::new();
        if shown.tile_content > 0.0 {
            content.extend(faded(shown.tile_content, self.content(module, at(frame))));
        }
        if shown.detail_content > 0.0 {
            content.extend(faded(shown.detail_content, self.detail(detail, at(frame))));
        }
        vec![self
            .tile(label, frame, Outline::Rounded(radius), 1.0, content)
            .key(format!("tile.{}", slug(module)))]
    }
}

/// A row of a detail view's list.
pub(super) struct Row {
    pub(super) label: String,
    pub(super) symbol: &'static str,
    pub(super) trailing: Option<&'static str>,
    pub(super) current: bool,
    pub(super) on_press: Option<Input>,
}

/// An 8 point gap before a label.
pub(super) struct Spacer8;

impl<M: 'static> Widget<M> for Spacer8 {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(8.0),
            height: SizeSpec::Fixed(1.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(8.0, 1.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A drawn shape with no input: the media tile's artwork placeholder, or a detail view's separator.
macro_rules! shape_widget {
    ($name:ident, $alpha:expr, $radius:expr) => {
        struct $name {
            frame: Rect,
        }

        impl<M: 'static> Widget<M> for $name {
            type State = ();

            fn layout(&self) -> LayoutSpec {
                spec(ContainerKind::Leaf, self.frame)
            }

            fn state(&self) {}

            fn update(&self, old: &Self, _: &mut ()) -> Change {
                if self.frame != old.frame {
                    Change::Layout
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

            fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
                let rect = Rect::from_origin_size(Point::ZERO, cx.size());
                let color = veil(cx, $alpha);
                fill(cx, rect, $radius, color);
            }

            fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
        }
    };
}

shape_widget!(Artwork, ARTWORK, ARTWORK_RADIUS);
shape_widget!(Line, SEPARATOR, 0.0);

/// Content faded as one layer.
pub(super) struct Fade<M> {
    pub(super) frame: Rect,
    pub(super) alpha: f64,
    /// The content only shows: assistive technology and the Tab order leave it out.
    pub(super) hidden: bool,
    pub(super) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Fade<M> {
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
        } else if self.alpha != old.alpha || self.hidden != old.hidden {
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.set_group(finite_alpha(self.alpha) as f32, Vec::new());
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.hidden {
            cx.hidden();
        }
    }
}

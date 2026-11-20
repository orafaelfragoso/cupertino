//! What the panel does for the person: the selected result's preview, the browser's icons, starting and bringing
//! forward what is activated, dragging an application out, the keys and the input of the panel's widgets.

use super::*;

impl Model {
    /// The selected result's preview: asked once per selection, with a deadline.
    pub(super) fn ask_preview(&mut self) -> Vec<Command<Message>> {
        let selected = self.panel.selected_item().cloned();
        if selected.as_ref().map(|item| &item.id) == self.previewed.as_ref() {
            return Vec::new();
        }
        self.previewed = selected.as_ref().map(|item| item.id.clone());
        self.preview = PreviewState::None;
        let Some(item) = selected else {
            return Vec::new();
        };
        let mut commands = self.host.preview(&item);
        if !commands.is_empty() {
            self.preview = PreviewState::Waiting(item.id.clone());
            commands.extend(
                self.host
                    .after(PREVIEW_TIMEOUT, Message::PreviewOverdue(item.id.clone())),
            );
        }
        commands
    }

    /// Ask for the icons of the browser's applications near the view, nearest first, while fewer than
    /// [`MAX_APP_ICON_LOADS`] are being loaded. The host loads and decodes them off the loop; each that arrives
    /// ([`Message::AppIcon`]) makes room for the next.
    pub(super) fn load_app_icons(&mut self) -> Vec<Command<Message>> {
        let Some(browser) = self
            .browser
            .as_ref()
            .filter(|_| self.nav.view == View::Browse)
        else {
            return Vec::new();
        };
        let room = MAX_APP_ICON_LOADS.saturating_sub(self.app_icons_loading.len());
        let wanted: Vec<App> = browser
            .nearby()
            .into_iter()
            .filter_map(|i| browser.item(i))
            .filter(|a| {
                !self.app_icons.contains_key(&a.id) && !self.app_icons_loading.contains(&a.id)
            })
            .take(room)
            .cloned()
            .collect();
        let mut commands = Vec::new();
        for app in &wanted {
            self.app_icons_loading.insert(app.id.clone());
            commands.extend(self.host.app_icon(app));
        }
        commands
    }

    /// The applications whose icons are being loaded now.
    pub fn app_icons_loading(&self) -> usize {
        self.app_icons_loading.len()
    }

    /// Carry out what the panel asked for.
    pub(super) fn outcome(&mut self, outcome: PanelEvent) -> Vec<Command<Message>> {
        match outcome {
            PanelEvent::None | PanelEvent::Changed | PanelEvent::Edit => Vec::new(),
            PanelEvent::Close => {
                self.hide(false);
                Vec::new()
            }
            PanelEvent::Activate(activation) => self.activated(activation),
            PanelEvent::Command(command) => self.command(command),
        }
    }

    /// The panel closed itself for `activation`: the clipboard is set, or the token asked for, while its window
    /// still exists.
    pub(super) fn activated(&mut self, activation: Activation) -> Vec<Command<Message>> {
        let commands = match activation {
            Activation::CopyToClipboard(text) => vec![Command::SetClipboard(text)],
            Activation::Run(spec) => self.ask_token(spec, None),
            Activation::OpenApp { desktop_id, spec } => self.ask_token(spec, Some(desktop_id)),
            Activation::Nothing => {
                eprintln!("cupertino-launcher: the entry has no usable command");
                Vec::new()
            }
        };
        self.hide(false);
        commands
    }

    /// Ask for the activation token of `spec` while the panel's window still exists. `opens` is the application it
    /// starts when that is to be brought forward instead when it runs.
    pub(super) fn ask_token(
        &mut self,
        spec: CommandSpec,
        opens: Option<String>,
    ) -> Vec<Command<Message>> {
        let launch = self.next_launch;
        self.next_launch += 1;
        let app_id = spec.desktop_id.clone();
        self.pending.insert(launch, Pending { spec, opens });
        vec![Command::ActivationToken {
            window: WindowKey(Key::from("panel")),
            app_id,
            on_token: Box::new(move |token| Message::Launch(launch, token)),
        }]
    }

    /// The token of `launch` arrived: an application to open is first brought forward if it runs (the server is
    /// asked), anything else starts.
    pub(super) fn launch(&mut self, launch: u64, token: Option<String>) {
        let Some(Pending { spec, opens }) = self.pending.remove(&launch) else {
            return;
        };
        if let Some(desktop_id) = opens {
            if self.host.bring_forward(&desktop_id, launch) {
                self.opening.insert(
                    launch,
                    Opening {
                        desktop_id,
                        spec,
                        token,
                    },
                );
                return;
            }
        }
        self.host.run(spec, token);
    }

    /// The server's answer for the application opened as `launch`, as a click on its dock tile goes on: nothing more
    /// when it came forward; a resident one is asked for a window over D-Bus when its entry allows, else started again;
    /// one that does not run is started.
    pub(super) fn opened(&mut self, launch: u64, running: Running) -> Vec<Command<Message>> {
        let Some(opening) = self.opening.remove(&launch) else {
            return Vec::new();
        };
        match running {
            Running::Raised => Vec::new(),
            Running::NotRunning => {
                self.host.run(opening.spec, opening.token);
                Vec::new()
            }
            Running::Resident => {
                let source = self
                    .launcher
                    .entry(&opening.desktop_id)
                    .and_then(|e| e.source.clone());
                let bus_id = opening
                    .desktop_id
                    .strip_suffix(".desktop")
                    .unwrap_or(&opening.desktop_id)
                    .to_owned();
                match self
                    .host
                    .ask_for_window(&bus_id, source.as_deref(), opening.token.clone())
                {
                    Some(call) => {
                        self.opening.insert(launch, opening);
                        vec![Command::Perform(Box::pin(async move {
                            Message::Asked(launch, call.await)
                        }))]
                    }
                    None => {
                        self.host.run(opening.spec, opening.token);
                        Vec::new()
                    }
                }
            }
        }
    }

    /// Open the browser's application `id` as the search would.
    /// The application at `index` of the browser dragged out as its desktop file, for the dock to keep or the desktop
    /// to hold; its icon follows the pointer, held at `grab`.
    pub(super) fn drag_app(
        &mut self,
        index: usize,
        grab: cupertino_kit::geometry::Point,
    ) -> Vec<Command<Message>> {
        use cupertino_kit::controls::{Image, ImageFit};
        use cupertino_kit::core::{Drag, DragPreview};
        use cupertino_kit::platform::{ClipboardData, DropAction};
        let Some(id) = self
            .browser
            .as_ref()
            .and_then(|b| b.item(index))
            .map(|a| a.id.clone())
        else {
            return Vec::new();
        };
        let Some(path) = self.launcher.entry(&id).and_then(|e| e.source.clone()) else {
            return Vec::new();
        };
        let icon = BrowseLayout::ICON;
        let image = DragPreview {
            content: Element::new(Image {
                sources: self.app_icons.get(&id).cloned().unwrap_or_default(),
                size: Size::new(icon, icon),
                fit: ImageFit::Contain,
                radius: 0.0,
                label: None,
            }),
            size: Size::new(icon, icon),
            hotspot: grab,
        };
        self.dragging = true;
        vec![Command::StartDrag(Drag {
            window: view::panel_window_key(),
            data: vec![ClipboardData {
                mime: "text/uri-list".into(),
                bytes: format!("{}\r\n", file_uri(&path)).into_bytes(),
            }],
            actions: [DropAction::Copy].into(),
            image: Some(image),
            on_progress: Box::new(Message::Dragged),
        })]
    }

    /// The drag of an application went on: once it ends the catcher comes back, and a drop something took closes
    /// the panel, as a launch does.
    pub(super) fn dragged(
        &mut self,
        progress: cupertino_kit::platform::DragProgress,
    ) -> Vec<Command<Message>> {
        use cupertino_kit::platform::DragProgress;
        match progress {
            DragProgress::Finished => {
                self.dragging = false;
                self.intent(Intent::Dismiss)
            }
            DragProgress::Cancelled => {
                self.dragging = false;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn launch_app(&mut self, id: &str) -> Vec<Command<Message>> {
        let now = self.now();
        match self.launcher.app_result(id) {
            Some(item) => {
                let activation = self.launcher.activate(&item, "", now.unix);
                self.activated(activation)
            }
            None => Vec::new(),
        }
    }

    /// Shortcut commands of the panel that need the host.
    pub(super) fn command(&mut self, command: PanelCommand) -> Vec<Command<Message>> {
        match command {
            PanelCommand::Reveal => {
                let reveal = self.panel.selected_item().and_then(|item| {
                    self.launcher
                        .actions(item)
                        .iter()
                        .position(|a: &ItemAction| a.kind == ActionKind::Reveal)
                });
                if let Some(index) = reveal {
                    let now = self.now();
                    let flat = self.panel.selected();
                    let outcome = self
                        .panel
                        .activate_action(flat, index, &mut self.launcher, now);
                    return self.outcome(outcome);
                }
            }
            PanelCommand::WebSearch => {
                match (&self.launcher.config.web_search_url, self.panel.query()) {
                    (Some(url), query) if !query.is_empty() => {
                        let target = url.replace("{}", &percent_encode(query));
                        self.hide(false);
                        self.host.open(&target);
                    }
                    _ => eprintln!(
                        "cupertino-launcher: no web search URL configured (`web_search_url`)"
                    ),
                }
            }
            // Space or Primary+Y: the selected result's preview, enlarged; again to go back
            PanelCommand::Preview => {
                if self.nav.nested == Some(Nested::Preview) {
                    return self.intent(Intent::Done);
                }
                if self.panel.selected_item().is_some() {
                    self.panel_focus_list();
                    return self.intent(Intent::Open(Nested::Preview));
                }
            }
            other => eprintln!("cupertino-launcher: {other:?} is not provided"),
        }
        Vec::new()
    }

    /// The keyboard is in the result list (a preview opens over it).
    pub(super) fn panel_focus_list(&mut self) {
        self.nav.apply(Intent::Focus(NavFocus::Content));
    }

    /// How the panel's keys are split between the panel and the query field now.
    pub fn key_mode(&self) -> KeyMode {
        match (self.nav.view, self.nav.nested) {
            (_, Some(Nested::ViewOptions)) => KeyMode::Menu,
            (_, Some(Nested::Preview)) => KeyMode::Preview,
            (View::Search, _) if self.mode_focus.on.is_some() => KeyMode::Modes,
            (View::Files, _) => KeyMode::Files {
                selecting: self
                    .scope
                    .as_ref()
                    .is_some_and(|s| s.zone() != scope::Zone::Content || s.selected().is_some()),
            },
            (View::Browse, _) => KeyMode::Browse {
                selecting: self
                    .browser
                    .as_ref()
                    .is_some_and(|b| b.zone() != Zone::Content || b.selected().is_some()),
            },
            _ => KeyMode::Search(self.panel.focus()),
        }
    }

    /// A key of the panel.
    pub(super) fn key(&mut self, key: PanelKey, mods: PanelMods) -> Vec<Command<Message>> {
        let now = self.now();
        if mods.only_primary() {
            if let PanelKey::Char(c) = key {
                if let Some(index) = modes::index_of_digit(c) {
                    return match self.modes().get(index) {
                        Some(&mode) => self.press_mode(mode),
                        None => Vec::new(),
                    };
                }
            }
        }
        if let Some(commands) = self.mode_key(key, mods) {
            return commands;
        }
        match self.nav.nested {
            Some(Nested::ViewOptions) => return self.menu_key(key),
            Some(Nested::Preview) => match key {
                PanelKey::Escape | PanelKey::Char(' ') => return self.intent(Intent::Escape),
                _ => {}
            },
            _ => {}
        }
        if key == PanelKey::Escape {
            return self.intent(Intent::Escape);
        }
        if self.nav.view == View::Browse {
            return self.browse_key(key, mods);
        }
        if self.nav.view == View::Files {
            return self.scope_key(key, mods);
        }
        let before = self.panel.query().to_owned();
        let outcome = self.panel.handle_key(key, mods, &mut self.launcher, now);
        if self.panel.query() != before {
            // a recalled search shows selected, so typing replaces it; a kind's token goes in with the caret after it
            self.held = false;
            self.select_query = key == PanelKey::Up;
            self.revision += 1;
        }
        let mut commands = self.outcome(outcome);
        // a shortcut that closes the panel (settings, help)
        if !self.panel.is_visible() && self.nav.is_visible() {
            self.hide(false);
        }
        if self.nav.nested == Some(Nested::Preview) && self.panel.selected_item().is_none() {
            commands.extend(self.intent(Intent::Done));
        }
        commands
    }

    /// Switch to the scope of `mode`, as its button or its shortcut asks.
    pub(super) fn press_mode(&mut self, mode: Mode) -> Vec<Command<Message>> {
        self.mode_hover = None;
        self.mode_focus = modes::Focus::default();
        match mode {
            Mode::Applications => self.intent(Intent::Browse),
            Mode::Files => self.intent(Intent::Files),
        }
    }

    /// A key while the buttons stand beside the plain bar: Tab goes to them, along them and back to the field; Left and
    /// Right move between them, Return and Space press the one the keys are on, Escape gives the keys back to the field.
    /// `None` when the key is not the buttons'.
    pub(super) fn mode_key(
        &mut self,
        key: PanelKey,
        mods: PanelMods,
    ) -> Option<Vec<Command<Message>>> {
        let count = self.modes().len();
        let shown = self.nav.view == View::Search
            && self.nav.nested.is_none()
            && self.motion.modes_wanted() > 0
            && {
                let m = self.presented().modes;
                m.count > 0 && m.spread > 0.5
            };
        if !shown {
            self.mode_focus = modes::Focus::default();
            return None;
        }
        match key {
            PanelKey::Tab => {
                match self.mode_focus.tab(mods.shift, count) {
                    modes::Move::To(i) => self.mode_focus.on = Some(i),
                    _ => self.mode_focus.on = None,
                }
                Some(Vec::new())
            }
            _ if self.mode_focus.on.is_none() => None,
            PanelKey::Left | PanelKey::Right => {
                if let Some(modes::Move::To(i)) =
                    self.mode_focus.step(key == PanelKey::Right, count)
                {
                    self.mode_focus.on = Some(i);
                }
                Some(Vec::new())
            }
            PanelKey::Enter | PanelKey::Char(' ') => {
                let mode = self
                    .mode_focus
                    .on
                    .and_then(|i| self.modes().get(i).copied());
                Some(match mode {
                    Some(mode) => self.press_mode(mode),
                    None => Vec::new(),
                })
            }
            PanelKey::Escape => {
                self.mode_focus.on = None;
                Some(Vec::new())
            }
            // anything else is typing: the keys go back to the field
            _ => {
                self.mode_focus.on = None;
                None
            }
        }
    }

    /// A key in the files scope: the arrows, Home, End and the pages move along the files, Return opens the selected
    /// one, Tab moves between the files and the filters.
    pub(super) fn scope_key(&mut self, key: PanelKey, mods: PanelMods) -> Vec<Command<Message>> {
        let now = self.now().mono;
        let key = match (key, mods.shift) {
            (PanelKey::Left, _) => browse::Key::Left,
            (PanelKey::Right, _) => browse::Key::Right,
            (PanelKey::Up, _) => browse::Key::Up,
            (PanelKey::Down, _) => browse::Key::Down,
            (PanelKey::Home, _) => browse::Key::Home,
            (PanelKey::End, _) => browse::Key::End,
            (PanelKey::PageUp, _) => browse::Key::PageUp,
            (PanelKey::PageDown, _) => browse::Key::PageDown,
            (PanelKey::Enter, _) => browse::Key::Return,
            (PanelKey::Char(' '), _) => browse::Key::Space,
            (PanelKey::Tab, false) => browse::Key::Tab,
            (PanelKey::Tab, true) => browse::Key::BackTab,
            _ => return Vec::new(),
        };
        let Some(files) = self.scope.as_mut() else {
            return Vec::new();
        };
        match files.key(key, now) {
            Some(scope::Outcome::Open(path)) => self.open_file(&path),
            None => Vec::new(),
        }
    }

    /// Open the scope's file or folder with its default handler; the launcher closes.
    pub(super) fn open_file(&mut self, path: &std::path::Path) -> Vec<Command<Message>> {
        self.hide(false);
        self.host.open(&path.to_string_lossy());
        Vec::new()
    }

    pub(super) fn browse_key(&mut self, key: PanelKey, mods: PanelMods) -> Vec<Command<Message>> {
        let now = self.now().mono;
        let key = match (key, mods.shift) {
            (PanelKey::Left, _) => browse::Key::Left,
            (PanelKey::Right, _) => browse::Key::Right,
            (PanelKey::Up, _) => browse::Key::Up,
            (PanelKey::Down, _) => browse::Key::Down,
            (PanelKey::Home, _) => browse::Key::Home,
            (PanelKey::End, _) => browse::Key::End,
            (PanelKey::PageUp, _) => browse::Key::PageUp,
            (PanelKey::PageDown, _) => browse::Key::PageDown,
            (PanelKey::Enter, _) => browse::Key::Return,
            (PanelKey::Char(' '), _) => browse::Key::Space,
            (PanelKey::Tab, false) => browse::Key::Tab,
            (PanelKey::Tab, true) => browse::Key::BackTab,
            _ => return Vec::new(),
        };
        let Some(browser) = self.browser.as_mut() else {
            return Vec::new();
        };
        match browser.key(key, now) {
            Some(browse::Outcome::Launch(id)) => self.launch_app(&id),
            Some(browse::Outcome::ViewOptions) => self.intent(Intent::Open(Nested::ViewOptions)),
            None => Vec::new(),
        }
    }

    /// A key that reaches the panel while the view options are open. Their menu holds the keyboard and moves between
    /// its items itself, so only a key the panel still hears matters: Escape closes the menu, everything else waits.
    pub(super) fn menu_key(&mut self, key: PanelKey) -> Vec<Command<Message>> {
        match key {
            PanelKey::Escape => self.intent(Intent::Escape),
            _ => Vec::new(),
        }
    }

    /// Lay the browser out as `arrangement`; the view options close.
    pub(super) fn arrange(&mut self, arrangement: Arrangement) -> Vec<Command<Message>> {
        self.arrangement = arrangement;
        if let Some(b) = self.browser.as_mut() {
            b.set_arrangement(arrangement);
        }
        if self.nav.nested == Some(Nested::ViewOptions) {
            self.intent(Intent::Done)
        } else {
            Vec::new()
        }
    }

    pub(super) fn input(&mut self, input: Input) -> Vec<Command<Message>> {
        let now = self.now();
        match input {
            Input::Key(key, mods) => self.key(key, mods),
            Input::Query(change) => {
                self.revision = change.revision;
                if self.nav.nested == Some(Nested::Preview) && change.text != self.panel.query() {
                    self.nav.apply(Intent::Done);
                }
                // an edit ends the kept query: from here on it filters and is searched
                self.held = false;
                self.select_query = false;
                self.mode_focus = modes::Focus::default();
                if self.nav.view == View::Browse {
                    self.panel.set_text_quiet(&change.text);
                    if let Some(b) = self.browser.as_mut() {
                        b.set_query(&change.text);
                    }
                } else if self.nav.view == View::Files {
                    self.panel.set_text_quiet(&change.text);
                    if let Some(files) = self.scope.as_mut() {
                        files.set_query(&change.text);
                    }
                } else {
                    self.panel
                        .edit(&self.launcher, now, |field| field.set_text(&change.text));
                }
                Vec::new()
            }
            Input::Hover(flat) => {
                self.panel.set_selected(flat);
                Vec::new()
            }
            Input::Activate(flat) => {
                let outcome = self.panel.activate_index(flat, &mut self.launcher, now);
                self.outcome(outcome)
            }
            Input::RunAction(flat, index) => {
                let outcome = self
                    .panel
                    .activate_action(flat, index, &mut self.launcher, now);
                self.outcome(outcome)
            }
            Input::Dismiss => self.intent(Intent::Dismiss),
            // the view options menu, a popup of the panel's window, holds the keyboard while it is open; when the
            // compositor dismisses it because something else took the keyboard, the panel's leave comes after the
            // menu's close and closes the panel
            Input::KeyboardLeft if self.nav.nested == Some(Nested::ViewOptions) => Vec::new(),
            Input::KeyboardLeft => self.intent(Intent::Dismiss),
            Input::BrowseHover(index) => {
                if let Some(b) = self.browser.as_mut() {
                    b.hover(index);
                }
                Vec::new()
            }
            Input::BrowseActivate(index) => {
                let id = self
                    .browser
                    .as_ref()
                    .and_then(|b| b.item(index))
                    .map(|a| a.id.clone());
                match id {
                    Some(id) => self.launch_app(&id),
                    None => Vec::new(),
                }
            }
            Input::BrowseDrag(index, grab) => self.drag_app(index, grab),
            Input::BrowseCategory(category) => {
                if let Some(b) = self.browser.as_mut() {
                    b.set_zone(Zone::Categories);
                    b.toggle_category(category);
                }
                Vec::new()
            }
            Input::BrowseScroll(dy) => {
                if self.nav.view == View::Files {
                    if let Some(files) = self.scope.as_mut() {
                        files.scroll_by(dy, now.mono);
                    }
                } else if let Some(b) = self.browser.as_mut() {
                    b.scroll_by(dy, now.mono);
                }
                self.animate()
            }
            Input::CaretShown(blink) => {
                // a caret shown again by an edit, a move or a press, or a blink other than the one the compositor
                // shows, is the field's own from now on
                if self
                    .caret_layered
                    .is_some_and(|shown| blink.restarted || blink.phase_ms != shown.phase_ms)
                {
                    self.caret_layered = None;
                }
                self.caret_blink = cupertino_kit::controls::CaretBlink {
                    restarted: false,
                    ..blink
                };
                Vec::new()
            }
            Input::ModeHover(index) => {
                self.mode_hover = index;
                Vec::new()
            }
            Input::ModePress(index) => match self.modes().get(index) {
                Some(&mode) => self.press_mode(mode),
                None => Vec::new(),
            },
            Input::ScopeHover(index) => {
                if let Some(files) = self.scope.as_mut() {
                    files.hover(index);
                }
                Vec::new()
            }
            Input::ScopeActivate(index) => {
                let path = self
                    .scope
                    .as_ref()
                    .and_then(|s| s.item(index))
                    .map(|f| f.path.clone());
                match path {
                    Some(path) => self.open_file(&path),
                    None => Vec::new(),
                }
            }
            Input::Primary(held) => {
                self.primary_held = held;
                Vec::new()
            }
            Input::ScopeFilter(filter) => {
                if let Some(files) = self.scope.as_mut() {
                    files.set_zone(scope::Zone::Filters);
                    files.toggle_filter(filter);
                }
                Vec::new()
            }
            // the pointer (or assistive technology) pressed the button: the keys stay where they were, so the button
            // shows no focus ring once the menu closes
            Input::ViewOptions => {
                if self.nav.nested == Some(Nested::ViewOptions) {
                    self.intent(Intent::Done)
                } else {
                    self.intent(Intent::Open(Nested::ViewOptions))
                }
            }
            Input::Arrange(arrangement) => self.arrange(arrangement),
            Input::CloseMenu => {
                if self.nav.nested == Some(Nested::ViewOptions) {
                    self.intent(Intent::Done)
                } else {
                    Vec::new()
                }
            }
            Input::ClosePreview => self.intent(Intent::Done),
        }
    }
}

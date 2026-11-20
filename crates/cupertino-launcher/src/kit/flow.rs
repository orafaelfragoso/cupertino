//! How the panel moves through its views: the navigation steps, the opens, folds and closes handed to the compositor,
//! the searches and what they say, the mode buttons, the files scope's images and the window's size.

use super::*;

impl Model {
    /// Carry out a navigation step.
    pub(super) fn step(&mut self, step: Step) -> Vec<Command<Message>> {
        let now = self.now();
        let mut commands = Vec::new();
        match step {
            Step::Stay | Step::Noted => {}
            Step::Show(view) => {
                // the previous query is kept and shown selected, so typing replaces it; nothing filters by it
                let kept = self.panel.query().to_owned();
                self.apply_launcher_settings();
                self.panel.show(&self.launcher, now);
                self.searched = None;
                self.held = !kept.is_empty();
                self.select_query = self.held;
                self.mode_hover = None;
                self.mode_focus = modes::Focus::default();
                self.just_shown = true;
                // a close the compositor ran is turned around by the panel itself, which draws its opening
                let from_hidden = self.handed.is_none() && self.motion.gone(now.mono);
                self.handed = None;
                self.end_open();
                if from_hidden {
                    self.shows += 1;
                    self.blink_start = Some(self.shows);
                }
                self.hand_fold = false;
                self.layer_changes.clear();
                match view {
                    View::Browse => {
                        self.panel.set_text_quiet(&kept);
                        self.open_browser();
                        self.last_scope = Some(View::Browse);
                    }
                    View::Files => {
                        self.panel.set_text_quiet(&kept);
                        self.open_scope();
                        self.last_scope = Some(View::Files);
                    }
                    _ => {}
                }
                self.motion
                    .show_in(self.target_shape(), header_of(view), now.mono);
                if view == View::Search {
                    // handed over once the mode buttons are placed
                    self.hand_field = from_hidden;
                } else {
                    self.hand_open(from_hidden, now.mono);
                }
                if let (Some(config), false, None) = (&self.files, self.indexing, &self.file_index)
                {
                    self.indexing = true;
                    let config = config.clone();
                    commands.extend(self.host.index(config));
                }
            }
            Step::Hide => self.hide(false),
            Step::Fold => {
                // a scope drawn as layers at rest folds on the compositor's clock, once the buttons are placed
                self.hand_fold = self.hand_folds && self.layered() && self.open_rest.is_none();
                // the field shows the cleared query; one already empty and not selected is shown as it is, so the
                // query row draws nothing new
                if !self.panel.query().is_empty() || self.select_query {
                    self.revision += 1;
                }
                self.panel.set_query("", &self.launcher, now);
                self.searched = None;
                self.held = false;
                self.select_query = false;
                self.motion.fold(self.target_shape(), now.mono);
            }
            Step::Unfold => {
                // the search field drawn as layers at rest unfolds on the compositor's clock, once the buttons merge
                self.hand_fold = self.unfolds_from_field();
                let query = self.filter_text();
                let text = self.panel.query().to_owned();
                self.panel.set_text_quiet(&text);
                self.last_scope = Some(View::Browse);
                self.mode_hover = None;
                self.tracker.cancel();
                self.preview = PreviewState::None;
                let height = self.browse_height();
                match self.browser.as_mut() {
                    Some(browser) => {
                        browser.set_height(height);
                        browser.set_query(&query);
                        browser.deselect();
                        if !self.settled {
                            browser.reveal_scroller(now.mono);
                        }
                    }
                    None => self.open_browser(),
                }
                self.motion.unfold(self.target_shape(), now.mono);
            }
            Step::UnfoldFiles => {
                self.hand_fold = self.unfolds_from_field();
                let text = self.panel.query().to_owned();
                self.panel.set_text_quiet(&text);
                self.tracker.cancel();
                self.preview = PreviewState::None;
                self.last_scope = Some(View::Files);
                self.mode_hover = None;
                let height = self.scope_height();
                match self.scope.as_mut() {
                    Some(files) => files.set_height(height),
                    None => self.open_scope(),
                }
                let query = self.filter_text();
                if let Some(files) = self.scope.as_mut() {
                    files.set_query(&query);
                    files.deselect();
                }
                self.motion
                    .unfold_to(self.target_shape(), Header::Files, now.mono);
            }
            Step::Leave(_, _) => {}
            Step::Enter(_) => {}
            Step::FocusField => self.panel.focus_field(),
            Step::ClearQuery => {
                self.held = false;
                self.select_query = false;
                match self.nav.view {
                    View::Browse => {
                        self.panel.set_text_quiet("");
                        if let Some(b) = self.browser.as_mut() {
                            b.set_query("");
                        }
                    }
                    View::Files => {
                        self.panel.set_text_quiet("");
                        if let Some(files) = self.scope.as_mut() {
                            files.set_query("");
                        }
                    }
                    _ => self.panel.set_query("", &self.launcher, now),
                }
                // the field shows the cleared query
                self.revision += 1;
            }
            Step::ClearCategory => {
                if let Some(b) = self.browser.as_mut() {
                    b.set_category(None);
                }
            }
        }
        if self.settled {
            self.motion.settle(now.mono);
        }
        commands.extend(self.animate());
        commands
    }

    pub(super) fn intent(&mut self, intent: Intent) -> Vec<Command<Message>> {
        let step = self.nav.apply(intent);
        self.step(step)
    }

    /// Close: the window fades out (at once when `now_gone`, for a session lock); the launch history is written.
    pub(super) fn hide(&mut self, now_gone: bool) {
        let now = self.now();
        // the navigation has already left the panel: it was drawn as layers if it rested, shown
        let layered = self.compositor_layers
            && !self.motion.reduce_motion()
            && self.handed.is_none()
            && !self.motion.animating(now.mono)
            && !self.motion.gone(now.mono);
        self.dragging = false;
        if self.nav.is_visible() {
            self.nav = Nav::HIDDEN;
        }
        self.mode_hover = None;
        self.mode_focus = modes::Focus::default();
        self.primary_held = false;
        self.panel.hide();
        self.tracker.cancel();
        self.searched = None;
        self.said = None;
        self.said_before.clear();
        self.browse_filter = None;
        self.browse_said = None;
        self.browse_said_before.clear();
        self.preview = PreviewState::None;
        self.previewed = None;
        // a panel drawn as layers at rest closes on the compositor's clock: its layers keep what they show and the
        // compositor grows, blurs and fades them on the curves the panel draws them on itself
        let rest = (layered && !now_gone && !self.settled).then(|| self.motion.frame(now.mono));
        self.motion.hide(now.mono);
        if now_gone || self.settled {
            self.motion.settle(now.mono);
        }
        if let Some(from) = rest {
            let to = self
                .motion
                .frame(now.mono + crate::motion::CLOSE.duration());
            self.layer_changes =
                view::layers::close_changes(self.extent, &from, &to, self.modes().len());
            self.handed = Some(from);
        }
        if let Some(history) = self.launcher.frecency.unsaved() {
            self.history_to_save = Some(history);
        }
    }

    /// An open from hidden, drawn as layers, runs on the compositor's clock: its layers show the panel at rest from the
    /// first frame and the compositor moves them from where the open starts: a scope's
    /// ([`view::layers::open_changes`]), or the search field's with its mode buttons budding out of it
    /// ([`view::layers::field_open_changes`]). Never under Reduce Motion, nor without the layers, nor where the panel
    /// settles at once.
    pub(super) fn hand_open(&mut self, from_hidden: bool, now: f64) {
        if !(from_hidden
            && self.hand_opens
            && self.compositor_layers
            && !self.motion.reduce_motion()
            && !self.settled)
        {
            return;
        }
        let first = self.motion.frame(now);
        let rest = self.resting(now);
        let field = self.hand_field_opens && rest.header == Header::Search && rest.modes.count > 0;
        let scope = rest.header.is_scope() && first.modes.count == 0 && rest.modes.count == 0;
        if !(field || scope) {
            return;
        }
        // the window's size through the open, which places the layers
        let _ = self.resize();
        // the caret blinks in a layer of its own, from the open's start, where the field's theme blinks it
        self.caret_layered = Some(self.caret_blink)
            .filter(|b| field && b.phase_ms > 0 && self.blink_start.is_some());
        self.layer_changes = if field {
            view::layers::field_open_changes(
                self.extent,
                &first,
                &rest,
                &self.motion,
                now,
                self.caret_layered,
            )
        } else {
            view::layers::open_changes(self.extent, &first, &rest, &self.motion, now)
        };
        self.open_rest = Some(rest);
        self.open_timer = false;
        self.open_number += 1;
        self.open_length = self.motion.moving_left(now);
        self.open_end = now + self.open_length;
    }

    /// A fold of a scope that rested as layers into the search field, or an unfold of the field into a scope, runs on
    /// the compositor's clock ([`Model::hand_folds`], [`Model::hand_unfolds`]): its layers keep what they show as it
    /// begins and the compositor moves the glass and the content and fades the body through the panel's own values
    /// ([`view::layers::fold_changes`]), the edge drawn along the glass; the panel draws its query row once more, when
    /// the header switches. An unfold draws the scope's body at once, at the scope's shape at rest, which the compositor
    /// fades in and cuts to the glass as it grows, and the mode buttons merge into the field through the panel's own
    /// values. It ends when the compositor tells its layers came to rest past the enclosure's own motion; after a fold
    /// the mode buttons then bud out of the field drawn by the panel. Never under Reduce Motion, nor where the panel
    /// settles at once.
    pub(super) fn hand_fold(&mut self, now: f64) -> Vec<Command<Message>> {
        if !(self.compositor_layers
            && !self.motion.reduce_motion()
            && !self.settled
            && self.handed.is_none())
        {
            return Vec::new();
        }
        let first = self.motion.frame(now);
        // the enclosure's own motion; an unfold's buttons merge well within it
        let duration = self
            .motion
            .shaping_left(now)
            .max(crate::motion::MERGE.duration())
            .max(1e-3);
        let switch = self.motion.header_switch(now);
        let rest = self.resting(now);
        let unfold = rest.header.is_scope();
        let _ = self.resize();
        self.layer_changes = view::layers::fold_changes(
            self.extent,
            &self.motion,
            now,
            duration,
            first.modes.count,
            unfold,
        );
        // an unfold's body shows from its first frame, as it rests, the compositor fading it in
        let shown = if unfold {
            Frame {
                content: rest.content,
                ..first
            }
        } else {
            first
        };
        let (at, header) = switch.unwrap_or((now, shown.header));
        self.fold_shown = Some((shown, at, Frame { header, ..shown }));
        self.unfold_body = unfold.then_some(rest.shape);
        self.open_rest = Some(rest);
        self.open_timer = false;
        self.open_number += 1;
        self.open_end = now + duration;
        match switch {
            // the row's symbol and placeholder switch part way: its one frame of the fold
            Some((at, _)) => self
                .host
                .after(at - now, Message::HeaderSwitch(self.open_number)),
            None => Vec::new(),
        }
    }

    pub(super) fn animate(&mut self) -> Vec<Command<Message>> {
        self.frame += 1;
        let now = self.now().mono;
        let scrolling = match self.nav.view {
            View::Browse => self.browser.as_ref().is_some_and(|b| b.animating(now)),
            View::Files => self.scope.as_ref().is_some_and(|s| s.animating(now)),
            _ => false,
        };
        // the panel's window sends the frame's message at its next frame callback ([`view`]); a close the compositor
        // runs asks for none
        // nor does an open it runs: the scroll indicator the browser shows as it comes into view lingers through it,
        // and its frames would draw the field's held caret again
        self.frame_waiting = self.handed.is_none()
            && self.open_rest.is_none()
            && (self.motion.animating(now) || scrolling);
        Vec::new()
    }

    /// Whether the panel moves by itself and its window asks the compositor for frames.
    pub fn animating(&self) -> bool {
        self.frame_waiting
    }

    /// What the preview shows of the selected result, once it has arrived.
    pub fn preview(&self) -> Option<&Preview> {
        match &self.preview {
            PreviewState::Ready(p) => Some(p),
            _ => None,
        }
    }

    pub fn preview_state(&self) -> &PreviewState {
        &self.preview
    }

    /// The query changed in search: ask the slow providers about it, cancelling what they still do for the old one.
    pub(super) fn search(&mut self) -> Vec<Command<Message>> {
        let text = self.panel.query().to_owned();
        if self.searched.as_deref() == Some(text.as_str()) {
            return Vec::new();
        }
        self.searched = Some(text.clone());
        // nothing is said of the new query until it has stood a while
        self.query_change += 1;
        let mut commands = self
            .host
            .after(STATUS_SETTLE, Message::Settled(self.query_change));
        let now = self.now();
        let index = self.file_index.clone().filter(|_| !text.trim().is_empty());
        let Some(index) = index else {
            self.tracker.begin(&[], now.mono);
            return commands;
        };
        let generation = self.tracker.begin(&[Provider::Files], now.mono);
        let request = SearchRequest {
            generation,
            text,
            index,
            config: self.launcher.config.clone(),
            cancel: self.tracker.token(),
            unix: now.unix,
        };
        commands.extend(self.host.search(request));
        commands.extend(
            self.host
                .after(crate::providers::DEADLINE, Message::Overdue(generation)),
        );
        commands
    }

    /// The query of change `change` has stood for [`STATUS_SETTLE`]: say what the panel says of it, once. Providers past
    /// their deadline give up first, so what is said is final.
    pub(super) fn settle(&mut self, change: u64) {
        if change != self.query_change || self.said.as_ref().is_some_and(|(c, _)| *c == change) {
            return;
        }
        self.tracker.expire(self.now().mono);
        let status = self.status();
        let said = (self.panel.query().to_owned(), status.clone());
        if view::status_text(&status).is_some() && !self.said_before.contains(&said) {
            self.said = Some((change, status));
            if self.said_before.len() == MAX_SAID {
                self.said_before.remove(0);
            }
            self.said_before.push(said);
        }
    }

    /// The status said aloud for the query now, while the panel still says it: `None` before the query settled, once
    /// results show, and after the query changed.
    pub fn said(&self) -> Option<Status> {
        let (change, said) = self.said.as_ref()?;
        let now = self.status();
        (*change == self.query_change && view::status_text(&now).is_some()).then(|| said.clone())
    }

    /// How many times the query changed ([`Message::Settled`]).
    pub fn query_change(&self) -> u64 {
        self.query_change
    }

    /// A browser whose filter (query or category) changed waits [`STATUS_SETTLE`] before "No Results" may be said of it.
    pub(super) fn watch_browse_filter(&mut self) -> Vec<Command<Message>> {
        let Some(b) = self
            .browser
            .as_ref()
            .filter(|_| self.nav.view == View::Browse)
        else {
            return Vec::new();
        };
        let filter = (b.query().to_owned(), b.category());
        if self.browse_filter.as_ref() == Some(&filter) {
            return Vec::new();
        }
        self.browse_filter = Some(filter);
        self.browse_change += 1;
        self.host
            .after(STATUS_SETTLE, Message::BrowseSettled(self.browse_change))
    }

    /// The browser's filter of change `change` has stood for [`STATUS_SETTLE`]: say "No Results" when nothing shows,
    /// once, and never again for the same filter while the panel is open.
    pub(super) fn settle_browse(&mut self, change: u64) {
        if change != self.browse_change || self.browse_said == Some(change) {
            return;
        }
        let Some(filter) = self.browse_filter.clone() else {
            return;
        };
        let empty = self.browser.as_ref().is_some_and(|b| b.item_count() == 0);
        if empty && !self.browse_said_before.contains(&filter) {
            self.browse_said = Some(change);
            if self.browse_said_before.len() == MAX_SAID {
                self.browse_said_before.remove(0);
            }
            self.browse_said_before.push(filter);
        }
    }

    /// Whether the browser says "No Results" aloud now: its filter stood for [`STATUS_SETTLE`] with nothing to show,
    /// and it was not said for that filter before.
    pub fn browse_no_results_said(&self) -> bool {
        self.browse_said == Some(self.browse_change)
            && self.browser.as_ref().is_some_and(|b| b.item_count() == 0)
    }

    /// How many times the browser's filter changed ([`Message::BrowseSettled`]).
    pub fn browse_change(&self) -> u64 {
        self.browse_change
    }

    /// After every change: the providers, the shown icons, the selected result's preview, the window's size and the
    /// keyboard's place.
    pub(super) fn changed(&mut self) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        if !self.nav.is_visible() {
            return commands;
        }
        if self.nav.view == View::Search {
            if !self.held {
                commands.extend(self.search());
            }
            let focus = match self.panel.focus() {
                Focus::Field => NavFocus::Field,
                Focus::List => NavFocus::Content,
            };
            self.nav
                .apply(Intent::Query(!self.panel.query().is_empty()));
            self.nav.apply(Intent::Focus(focus));
        }
        if let Some(b) = self
            .browser
            .as_ref()
            .filter(|_| self.nav.view == View::Browse)
        {
            let focus = match b.zone() {
                Zone::Categories | Zone::Options => NavFocus::Categories,
                Zone::Content if b.selected().is_some() => NavFocus::Content,
                Zone::Content => NavFocus::Field,
            };
            let (query, category) = (!self.panel.query().is_empty(), b.category().is_some());
            self.nav.apply(Intent::Query(query));
            self.nav.apply(Intent::Category(category));
            self.nav.apply(Intent::Focus(focus));
        }
        if self.nav.view == View::Files {
            let text = self.filter_text();
            if let Some(files) = self.scope.as_mut() {
                files.set_query(&text);
                let focus = match files.zone() {
                    scope::Zone::Filters => NavFocus::Categories,
                    scope::Zone::Content if files.selected().is_some() => NavFocus::Content,
                    scope::Zone::Content => NavFocus::Field,
                };
                let (query, category) = (!self.panel.query().is_empty(), files.filter().is_some());
                self.nav.apply(Intent::Query(query));
                self.nav.apply(Intent::Category(category));
                self.nav.apply(Intent::Focus(focus));
            }
            commands.extend(self.ask_scope());
            commands.extend(self.load_file_images());
        }
        commands.extend(self.place_modes());
        if std::mem::take(&mut self.hand_fold) {
            commands.extend(self.hand_fold(self.now().mono));
        }
        if self.icons.len() > MAX_ICONS {
            self.icons.clear();
        }
        if self.nav.view == View::Search {
            for row in view::visible_rows(&self.panel) {
                let view::Row::Item { flat, .. } = row else {
                    continue;
                };
                let Some(item) = self.panel.results().get(flat) else {
                    continue;
                };
                if !self.icons.contains_key(&item.id) {
                    let loads = self.host.icon(item);
                    if !loads.is_empty() {
                        // blank until its renditions arrive, rather than its kind's symbol for a moment first
                        self.icons.insert(item.id.clone(), Vec::new());
                        commands.extend(loads);
                    }
                }
            }
            commands.extend(self.ask_preview());
        }
        commands.extend(self.watch_browse_filter());
        commands.extend(self.load_app_icons());
        let now = self.now().mono;
        let target = self.target_shape();
        if self.settled {
            self.motion.reshape(target, now);
            self.motion.settle(now);
        } else {
            // a change within the view shown (results, the output's size) reshapes it; a fold or unfold under way
            // keeps its course
            let header = self.motion.frame(now).header;
            let settled_in = match self.nav.view {
                View::Search => header == crate::motion::Header::Search,
                View::Browse => header == crate::motion::Header::Browse,
                View::Files => header == crate::motion::Header::Files,
                View::Hidden => false,
            };
            if settled_in {
                self.motion.reshape(target, now);
            }
        }
        commands.extend(self.resize());
        commands
    }

    /// The buttons stand beside the plain bar, and merge into the field when anything opens below it.
    pub(super) fn place_modes(&mut self) -> Vec<Command<Message>> {
        let now = self.now().mono;
        let want = if self.wants_modes() {
            self.modes().len()
        } else {
            0
        };
        let first = std::mem::take(&mut self.just_shown);
        let mut commands = Vec::new();
        if want != self.motion.modes_wanted() || (first && want > 0) {
            let delay = if first { 0.0 } else { RESPLIT_DELAY };
            self.motion.set_modes(want, now, delay);
            if self.settled {
                self.motion.settle(now);
            }
            // the search field opening from hidden with its buttons runs on the compositor's clock
            if first && std::mem::take(&mut self.hand_field) {
                self.hand_open(true, now);
            }
            // the buttons' own motion needs frames
            commands.extend(self.animate());
        }
        self.hand_field = false;
        if want == 0 {
            self.mode_hover = None;
            self.mode_focus = modes::Focus::default();
        }
        commands
    }

    /// Ask the host for the files the scope shows, when its query or filter changed since it last asked. The answer
    /// comes back as [`Message::Scoped`]; the scope shows no spinner while it waits.
    pub(super) fn ask_scope(&mut self) -> Vec<Command<Message>> {
        let (Some(files), Some(index)) = (self.scope.as_ref(), self.file_index.clone()) else {
            return Vec::new();
        };
        let asked = (files.query().to_owned(), files.filter());
        if self.scope_asked.as_ref() == Some(&asked) {
            return Vec::new();
        }
        self.scope_asked = Some(asked.clone());
        self.scope_generation += 1;
        let request = ScopeRequest {
            generation: self.scope_generation,
            text: asked.0,
            filter: asked.1,
            index,
            config: self.launcher.config.clone(),
            unix: self.now().unix,
        };
        self.host.scope_search(request)
    }

    /// Give the scope's files near the view their type's icon, and ask for the thumbnails of those that have one, a
    /// few at a time. A thumbnail that arrives ([`Message::FileThumb`]) takes the icon's place and makes room for the
    /// next.
    pub(super) fn load_file_images(&mut self) -> Vec<Command<Message>> {
        let Some(files) = self.scope.as_ref() else {
            return Vec::new();
        };
        if self.file_images.len() > MAX_FILE_IMAGES {
            self.file_images.clear();
            self.file_thumbed.clear();
            self.file_icons_asked.clear();
        }
        let near: Vec<FileItem> = files
            .nearby()
            .into_iter()
            .filter_map(|i| files.item(i).cloned())
            .collect();
        let mut commands = Vec::new();
        for item in near {
            if !self.file_images.contains_key(&item.path)
                && self.file_icons_asked.insert(item.path.clone())
            {
                commands.extend(self.host.file_icon(&item));
            }
            if !item.is_dir
                && self.file_thumbs_loading < MAX_FILE_THUMB_LOADS
                && !self.file_thumbed.contains(&item.path)
            {
                self.file_thumbed.insert(item.path.clone());
                self.file_thumbs_loading += 1;
                commands.extend(self.host.file_thumb(&item));
            }
        }
        commands
    }

    /// Whether the panel's window keeps [`Model::largest_extent`] now ([`Model::window_held`]).
    pub(super) fn holds_window(&self) -> bool {
        self.window_held && !self.motion.reduce_motion()
    }

    /// The largest window the panel takes: the browser's and the files scope's on the output known (their tallest
    /// before it is known) and the search field's with the tallest list of results it shows, each with the most its
    /// shape overshoots on its way from the search field, within the room the output known leaves below the panel.
    pub fn largest_extent(&self) -> Size {
        let compact = view::compact_shape();
        let rows = [
            view::ROW_HEIGHT,
            view::HEADER_HEIGHT,
            view::CALCULATOR_ROW_HEIGHT,
        ]
        .into_iter()
        .fold(0.0, f64::max);
        let list = view::COLLAPSED_HEIGHT + 1.0 + 2.0 * view::LIST_PADDING + MAX_ROWS as f64 * rows;
        let search = Shape::new(
            view::WIDTH,
            view::on_shape_grid(list.max(view::MIN_EXPANDED_HEIGHT)),
            compact.radius,
        );
        let mut largest = view::window_extent(compact);
        for (shape, header) in [
            (view::browse_shape(self.browse_height()), Header::Browse),
            (view::scope_shape(self.scope_height()), Header::Files),
            (search, Header::Search),
        ] {
            let mut motion = Motion::new(compact);
            if header == Header::Search {
                motion.reshape(shape, 0.0);
            } else {
                motion.unfold_to(shape, header, 0.0);
            }
            let steps = (motion.shaping_left(0.0) / SHAPE_STEP).ceil() as usize;
            for i in 0..=steps {
                let on_way =
                    view::window_extent(motion.frame(i as f64 * SHAPE_STEP).shape.union(shape));
                largest = Size::new(
                    largest.width.max(on_way.width),
                    largest.height.max(on_way.height),
                );
            }
        }
        // a whole number of grid steps wide, as each panel's own window is (the panels are, and the room for their
        // shadows), so the window centred on the output and each panel centred in it lie on whole device pixels at
        // the scales the panel's own windows did
        let largest = Size::new(view::on_shape_grid(largest.width), largest.height);
        // no larger than the room the output leaves below the panel's top, which the window system holds it to
        match self.output {
            Some(output) => {
                let top = (output.height * view::TOP_FRACTION - view::SHADOW_TOP)
                    .round()
                    .max(0.0);
                Size::new(
                    largest.width.min(output.width.floor()),
                    largest.height.min((output.height - top).floor()),
                )
            }
            None => largest,
        }
    }

    /// The window holds the shape presented and the one it heads for; it shrinks once the shape settled. Where it is
    /// held ([`Model::window_held`]) it is the largest the panel takes ([`Model::largest_extent`]) from the panel's
    /// first frame on, and it never shrinks while the panel shows: a fold that settles leaves it as it is.
    pub(super) fn resize(&mut self) -> Vec<Command<Message>> {
        let now = self.now().mono;
        let target = self.motion.target();
        // settled, the window is the target's (what the motion presents may be a hair off it); while the shape
        // changes, it holds both and the largest the shape grows to on its way, a spring's overshoot included, from
        // the motion's start, so it is not configured again part way, and never shrinks
        let mut needed = if self.motion.animating(now) {
            let mut both = view::window_extent(target.union(self.motion.frame(now).shape));
            let left = self.motion.shaping_left(now);
            let steps = (left / SHAPE_STEP).ceil() as usize;
            for i in 1..=steps {
                let shape = self.motion.frame(now + i as f64 * SHAPE_STEP).shape;
                let on_way = view::window_extent(shape);
                both = Size::new(both.width.max(on_way.width), both.height.max(on_way.height));
            }
            Size::new(
                both.width.max(self.extent.width),
                both.height.max(self.extent.height),
            )
        } else {
            view::window_extent(target)
        };
        if self.holds_window() {
            // worked out again only when the output changes
            let largest = match self.largest {
                Some((output, largest)) if output == self.output => largest,
                _ => {
                    let largest = self.largest_extent();
                    self.largest = Some((self.output, largest));
                    largest
                }
            };
            // hidden, the panel has no window: the next one starts at the largest as it is then
            let held = if self.presenting() {
                Size::new(
                    largest.width.max(self.extent.width),
                    largest.height.max(self.extent.height),
                )
            } else {
                largest
            };
            needed = Size::new(
                view::on_shape_grid(needed.width.max(held.width)),
                needed.height.max(held.height),
            );
        }
        if needed != self.extent {
            self.extent = needed;
            self.host.resize(needed);
        }
        Vec::new()
    }

    /// The window's size as last given.
    pub fn extent(&self) -> Size {
        self.extent
    }
}

use super::*;

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    pub(super) fn platform_event(&mut self, event: PlatformEvent) -> Result<(), UiError> {
        let (window, event) = match event {
            PlatformEvent::Window { window, event } => (window, event),
            PlatformEvent::ActivationToken { request, token } => {
                if let Some(on_token) = self.tokens.remove(&request) {
                    self.messages.push_back(on_token(token));
                }
                return Ok(());
            }
            PlatformEvent::Drag { token, progress } => {
                let ended = matches!(progress, DragProgress::Finished | DragProgress::Cancelled);
                if let Some(on_progress) = self.drags.get_mut(&token) {
                    self.messages.push_back(on_progress(progress));
                }
                if ended {
                    self.drags.remove(&token);
                }
                return Ok(());
            }
            PlatformEvent::Clipboard { token, data } => {
                if self.pastes.contains_key(&token) {
                    self.paste_data(token, data);
                } else {
                    self.drop_data(token, data);
                }
                return Ok(());
            }
            PlatformEvent::ClipboardOffered { kind, mimes } => {
                if kind == ClipboardKind::Clipboard {
                    self.clipboard_offers(mimes.as_deref());
                }
                return Ok(());
            }
            PlatformEvent::Wake => {
                if let Some(theme) = self.personal_refresh.as_mut().and_then(|refresh| refresh()) {
                    self.set_theme(theme);
                    // the views read the choices as they are built, and the controls draw them
                    self.view_dirty = true;
                    for w in &mut self.windows {
                        w.invalidate();
                        w.invalidate_damage();
                    }
                }
                return Ok(());
            }
            PlatformEvent::Appearance(appearance) => {
                if let Some(theme_for) = &mut self.theme_for {
                    let theme = theme_for(&appearance);
                    self.set_theme(theme);
                }
                return Ok(());
            }
            PlatformEvent::SessionLock(locked) => {
                if let Some(adapter) = &mut self.options.accessibility {
                    adapter.session_locked(locked);
                }
                return Ok(());
            }
            PlatformEvent::ScrollBars(bars) => {
                // the views pick their bars as they are built: build every window again
                if crate::scroll::set_session_scroll_bars(Some(bars)) {
                    self.view_dirty = true;
                    for w in &mut self.windows {
                        w.invalidate();
                    }
                }
                return Ok(());
            }
            PlatformEvent::BuffersLost => {
                // the window system holds nothing of what it showed: every window presents a whole frame
                for w in &mut self.windows {
                    w.invalidate();
                    w.invalidate_damage();
                }
                return Ok(());
            }
            PlatformEvent::RendererChanged(_) => {
                // the factory makes renderers of the new kind; the old ones go with their frames, layers and caches
                for w in &mut self.windows {
                    w.renderer = Arc::new(Mutex::new((self.options.renderers)()));
                    w.layer.renderer = None;
                    w.surface_layers.drop_renderers();
                    w.invalidate();
                    w.invalidate_damage();
                }
                return Ok(());
            }
        };
        let Some(i) = self.windows.iter().position(|w| w.id == window) else {
            return Ok(());
        };
        // a closing menu hears nothing of the user, nor of the compositor dismissing it
        if self.windows[i].fade.is_some()
            && matches!(
                event,
                WindowEvent::Pointer(..)
                    | WindowEvent::Touch(..)
                    | WindowEvent::Key(..)
                    | WindowEvent::Focus(_)
                    | WindowEvent::Ime(_)
                    | WindowEvent::PopupDone
                    | WindowEvent::PopupPlaced(_)
                    | WindowEvent::CloseRequested
                    | WindowEvent::MenuActivated(_)
                    | WindowEvent::Drop(_)
            )
        {
            return Ok(());
        }
        let Some(i) = self.ungrabbed_input(i, &event) else {
            return Ok(());
        };
        match &event {
            WindowEvent::Key(key, _) if key.pressed => self.keyboard = true,
            WindowEvent::Pointer(PointerEvent::Button { pressed: true, .. }, _)
            | WindowEvent::Touch(TouchEvent::Down { .. }, _) => self.keyboard = false,
            _ => {}
        }
        let now = self.options.clock.now();
        // the press that ends here, if it opened popups the release is not on
        let ended_elsewhere = match event {
            WindowEvent::Pointer(
                PointerEvent::Button {
                    pressed: false,
                    button,
                    serial,
                    mods,
                    ..
                },
                time,
            ) => self
                .press
                .filter(|&start| self.windows[i].opening_press != Some(start))
                .map(|start| (start, button, serial, mods, time)),
            _ => None,
        };
        if let WindowEvent::Pointer(PointerEvent::Button { pressed, .. }, _) = event {
            self.press = pressed.then_some(now);
        }
        if let Some((start, button, serial, mods, time)) = ended_elsewhere {
            self.release_elsewhere(start, button, serial, mods, time, now);
        }
        let w = &mut self.windows[i];
        let mut effects = Effects::default();
        match event {
            WindowEvent::Configure(c) => {
                let size = c.size.unwrap_or(w.spec.size);
                // a window shown at this size and in these states keeps its layout and its pixels: the next frame
                // only acknowledges the configure, with a commit that damages nothing
                let same = w.configured
                    && w.committed
                    && w.failed.is_none()
                    && size == w.size
                    && w.maximized == c.states.maximized
                    && w.fullscreen == c.states.fullscreen;
                let shown_active = w.active || w.keyboard_in_popup;
                // a window nobody sees (minimized, or covered whole) keeps its frame and lets go of its layers
                if c.states.suspended {
                    if w.in_flight.is_some() {
                        w.layers_owed = true;
                    } else {
                        w.drop_layers();
                    }
                }
                w.size = size;
                w.ack = Some(c.id);
                w.configured = true;
                w.free_size = c.size.is_none();
                w.maximized = c.states.maximized;
                w.fullscreen = c.states.fullscreen;
                // no longer the window in use, whatever its popups last had
                w.keyboard_in_popup &= c.states.activated;
                w.frame_ready = true;
                if same {
                    w.dirty = true;
                    if w.active == c.states.activated
                        && shown_active != (w.active || w.keyboard_in_popup)
                    {
                        self.invalidate_window(i);
                    }
                } else {
                    w.invalidate();
                    w.invalidate_damage();
                }
                let w = &mut self.windows[i];
                if w.active != c.states.activated {
                    w.active = c.states.activated;
                    w.begin_activation(now);
                    if let Some(node) = w.focus.node() {
                        let event = Event::WindowActive(w.active);
                        route(&mut self.tree, w, now, &mut effects).notify(node, &event);
                    }
                    self.invalidate_window(i);
                    self.apply(i, effects);
                }
                return Ok(());
            }
            WindowEvent::CloseRequested => {
                match w.on_close.clone() {
                    Some(message) => self.messages.push_back(message),
                    None => self.exit = true,
                }
                return Ok(());
            }
            WindowEvent::Scale(scale) => {
                if scale.is_finite() && scale > 0.0 && scale != w.env.scale {
                    w.env.scale = scale;
                    w.invalidate();
                    w.invalidate_damage();
                }
                return Ok(());
            }
            WindowEvent::Shown(shown) => {
                let Some(flight) = w.in_flight.take() else {
                    return Ok(());
                };
                if std::mem::take(&mut w.layers_owed) {
                    w.drop_layers();
                }
                match shown {
                    Shown::Presented(id) => {
                        if let Some(t) = &mut self.frame_timing {
                            t.presented();
                        }
                        w.failed = None;
                        // a configure that arrived meanwhile waits for the next frame
                        if w.ack == flight.ack {
                            w.ack = None;
                        }
                        if let Some(height) = flight.raised {
                            w.layer.shown = height;
                        }
                        if let Some(layers) = &flight.surface_layers {
                            w.surface_layers.shown(layers);
                        }
                        w.owed = false;
                        w.committed = true;
                        if let Some(timing) = &mut self.scroll_timing {
                            timing.submitted(w.id, id, self.options.clock.now());
                        }
                    }
                    Shown::NotReady => {
                        w.dirty = true;
                        w.owed = true;
                        w.damage.invalidate();
                        w.layer.damage.invalidate();
                        w.surface_layers.not_shown();
                    }
                    Shown::Failed(e) => {
                        let error = UiError::Render(e);
                        // as a frame that fails on the UI thread: drawn again whole on the next change
                        w.invalidate();
                        w.dirty = false;
                        w.invalidate_damage();
                        w.surface_layers.not_shown();
                        if w.failed.as_ref() != Some(&error) {
                            if let Some(report) = &mut self.options.on_window_error {
                                report(w.id, &error);
                            }
                        }
                        w.failed = Some(error);
                    }
                }
                // the next frame waits for the compositor to ask for it
                w.frame_ready = false;
                self.system.request_frame(w.id);
                return Ok(());
            }
            WindowEvent::Frame(time) => {
                w.frame_ready = true;
                w.frame_clock.frame(now);
                // the callback of a committed frame: the compositor has shown it, not only received it
                w.presented |= w.committed;
                let animate = std::mem::take(&mut w.animate);
                // a node animating from its paint draws its next step: without it a node that does not answer the
                // frame with a paint of its own keeps its cached paint, presents nothing and so asks for no more frames
                effects.changes.extend(
                    std::mem::take(&mut w.paint_animate)
                        .into_iter()
                        .map(|node| (node, crate::Change::Paint)),
                );
                if w.activated_at.is_some() {
                    // the colours of an activated window are still crossing: paint it again
                    w.repaint();
                    w.invalidate_damage();
                }
                if !animate.is_empty() {
                    w.repaint();
                    let mut route = route(&mut self.tree, w, now, &mut effects);
                    for node in animate {
                        route.notify(node, &Event::AnimationFrame(time));
                    }
                }
            }
            WindowEvent::Pointer(event, time) => {
                if let Some(root) = w.root {
                    let mut pointer = std::mem::take(&mut w.pointer);
                    let mut route = route(&mut self.tree, w, now, &mut effects);
                    let scroll = matches!(event, PointerEvent::Scroll { .. });
                    pointer.handle(&mut route, root, &w.geometry, event, time);
                    w.pointer = pointer;
                    if let (true, Some(timing)) = (scroll, &mut self.scroll_timing) {
                        timing.handled(w.id, time, self.options.clock.now());
                    }
                    if let PointerEvent::Button {
                        pressed: true,
                        position,
                        ..
                    } = event
                    {
                        Self::press_away_from_text(
                            &mut self.tree,
                            w,
                            root,
                            position,
                            &mut effects,
                            now,
                        );
                    }
                }
            }
            WindowEvent::Touch(event, time) => self.touch(i, event, time, now, &mut effects),
            WindowEvent::Key(key, time) => {
                self.key(i, key, time, now, &mut effects);
            }
            WindowEvent::Focus(focused) => {
                // a window the compositor still states activated stays the window in use while a surface of the desktop
                // (the overview, the launcher) has the keyboard: it neither looks inactive nor paints again for it
                let active = focused || self.system.activated(w.id) == Some(true);
                let changed = w.active != active;
                // a window whose popup had the keyboard was drawn active all along: its colours have nothing to cross
                let shown = w.active || w.keyboard_in_popup;
                w.active = active;
                if changed && shown != active {
                    w.begin_activation(now);
                }
                if !focused {
                    let held: BTreeSet<NodeId> = w.held.drain(..).map(|(_, node)| node).collect();
                    w.dismissed = None;
                    let mut pointer = std::mem::take(&mut w.pointer);
                    let mut route = route(&mut self.tree, w, now, &mut effects);
                    pointer.revoke(&mut route);
                    for node in held {
                        if route.tree.contains(node) {
                            route.notify(node, &Event::Cancel);
                        }
                    }
                    w.pointer = pointer;
                }
                if let Some(node) = w.focus.node().filter(|_| changed) {
                    let mut ancestors = self.tree.path(node);
                    ancestors.pop();
                    if !active {
                        ancestors.reverse();
                    }
                    let within = Event::FocusWithin {
                        gained: active,
                        visible: active && w.focus.visible(),
                    };
                    let mut route = route(&mut self.tree, w, now, &mut effects);
                    route.notify(node, &Event::WindowActive(active));
                    for ancestor in ancestors {
                        route.notify(ancestor, &within);
                    }
                }
                if let Some(adapter) = &mut self.options.accessibility {
                    adapter.window_focus(w.id, focused);
                }
                self.keyboard_moved(i, focused);
                // what the window shows changes with its active state alone (a popup's opener in `keyboard_moved`)
                self.activity_moved(i);
            }
            WindowEvent::Ime(ime) => {
                if let Some(node) = w.text_input.or(w.focus.node()) {
                    route(&mut self.tree, w, now, &mut effects).notify(node, &Event::Ime(ime));
                }
            }
            WindowEvent::PopupDone => {
                if let Some(dismiss) = w.popup.as_ref().map(|p| p.on_dismiss.clone()) {
                    if let (Dismiss::Owned, Some(owned)) = (&dismiss, &self.owned) {
                        self.owned_dismissed = Some(owned.parent.clone());
                    }
                    self.dismissed(dismiss);
                }
                return Ok(());
            }
            WindowEvent::PopupPlaced(rect) => {
                let key = w.key.clone();
                if let Some(parent) = w.popup.as_ref().map(|p| p.parent.clone()) {
                    if let Some(parent) = self.windows.iter_mut().find(|w| w.key == parent) {
                        parent.popups.retain(|(k, _)| *k != key);
                        parent.popups.push((key, rect));
                    }
                }
                return Ok(());
            }
            WindowEvent::MenuActivated(action) => {
                if let Some(on_menu) = &w.on_menu {
                    self.messages.push_back(on_menu(action));
                }
                return Ok(());
            }
            WindowEvent::Presented { frame, time } => {
                if let Some(timing) = &mut self.scroll_timing {
                    timing.presented(w.id, frame, Some(time));
                }
                return Ok(());
            }
            WindowEvent::Discarded { frame } => {
                if let Some(timing) = &mut self.scroll_timing {
                    timing.presented(w.id, frame, None);
                }
                return Ok(());
            }
            WindowEvent::GlassTone { region, tone } => {
                let tones = &mut w.painter.tones;
                let slot = match region {
                    None => &mut tones.window,
                    Some(i) if i < w.materials.len() => {
                        if tones.regions.len() <= i {
                            tones.regions.resize(i + 1, GlassTone::Appearance);
                        }
                        &mut tones.regions[i]
                    }
                    Some(_) => return Ok(()),
                };
                if *slot != tone {
                    *slot = tone;
                    // what lies on the glass paints again with the colours of its face
                    w.repaint();
                }
                return Ok(());
            }
            WindowEvent::Drop(event) => {
                self.drop_event(i, event);
                return Ok(());
            }
            WindowEvent::LayersSettled => {
                if let Some(root) = w.root {
                    route(&mut self.tree, w, now, &mut effects).notify(root, &Event::LayersSettled);
                }
            }
        }
        self.apply(i, effects);
        Ok(())
    }

    /// The press that began at `start` was let go away from the popups it opened: each one's root hears of the release
    /// at a point outside it, so a menu dragged through and let go elsewhere can close.
    pub(super) fn release_elsewhere(
        &mut self,
        start: Timestamp,
        button: MouseButton,
        serial: u32,
        mods: Mods,
        time: Timestamp,
        now: Timestamp,
    ) {
        let opened: Vec<usize> = (0..self.windows.len())
            .filter(|&j| {
                let w = &self.windows[j];
                w.popup.is_some() && w.fade.is_none() && w.opening_press == Some(start)
            })
            .collect();
        let outside = Point::new(-f64::from(u16::MAX), -f64::from(u16::MAX));
        let release = Event::Pointer(
            PointerEvent::Button {
                position: outside,
                button,
                pressed: false,
                serial,
                mods,
            },
            time,
        );
        for j in opened {
            let mut effects = Effects::default();
            let w = &mut self.windows[j];
            if let Some(root) = w.root {
                route(&mut self.tree, w, now, &mut effects).notify(root, &release);
            }
            self.apply(j, effects);
        }
    }

    /// Repaints every node of window `i`: its active state changed, which every control may show.
    pub(super) fn invalidate_window(&mut self, i: usize) {
        let w = &mut self.windows[i];
        w.invalidate();
        w.invalidate_damage();
    }

    /// Window `i` gained or lost the keyboard: a popup that gains it keeps the window its chain opens from drawn as
    /// active, until that window hears of the keyboard itself. A popup still open that loses it lets go: the keyboard
    /// went to another popup of the chain, which takes the window up again in the same dispatch, or somewhere else,
    /// which a compositor need not tell the window. One that closed before is no longer here to hear of it.
    pub(super) fn keyboard_moved(&mut self, i: usize, focused: bool) {
        if self.windows[i].popup.is_none() {
            self.windows[i].keyboard_in_popup = false;
            return;
        }
        let mut key = self.windows[i].key.clone();
        // each popup opens after its parent, so the chain is never longer than the windows open
        for _ in 0..self.windows.len() {
            let Some(w) = self.windows.iter().find(|w| w.key == key) else {
                return;
            };
            match &w.popup {
                Some(popup) => key = popup.parent.clone(),
                None => break,
            }
        }
        if let Some(opener) = self.windows.iter().position(|w| w.key == key) {
            self.windows[opener].keyboard_in_popup = focused;
            self.activity_moved(opener);
        }
    }

    /// Window `i` may show another active state than it was painted in: its next frame compares the two and paints
    /// the window again only when they differ. The keyboard going from a popup to the next of its chain passes
    /// through the window it opens from as inactive within one dispatch, which leaves its pixels as they are.
    pub(super) fn activity_moved(&mut self, i: usize) {
        let w = &mut self.windows[i];
        if w.painted_active
            .is_some_and(|p| p != (w.active || w.keyboard_in_popup))
        {
            w.dirty = true;
        }
    }

    pub(super) fn key(
        &mut self,
        i: usize,
        key: KeyEvent,
        time: Timestamp,
        now: Timestamp,
        effects: &mut Effects<A::Message>,
    ) {
        self.settle_focus_request(i, now);
        let w = &mut self.windows[i];
        let Some(root) = w.root else { return };
        let path = match w.focus.node() {
            Some(node) if self.tree.contains(node) => self.tree.path(node),
            _ => vec![root],
        };
        let held = HeldKey::of(&key);
        if !key.pressed && w.dismissed == Some(held) {
            w.dismissed = None;
            w.held.retain(|&(k, _)| k != held);
            return;
        }
        let escape = key.pressed && key.keysym == ESCAPE && key.mods.is_empty();
        if escape && !path.last().is_some_and(|&n| composing(&self.tree, n)) {
            let nodes = dismissible(&self.tree, root);
            let mut route = route(&mut self.tree, w, now, effects);
            let taken = nodes
                .into_iter()
                .rev()
                .find(|&node| route.offer(node, &Event::Dismiss) == Propagation::Stop);
            if let Some(node) = taken {
                w.held.retain(|&(k, _)| k != held);
                w.held.push((held, node));
                w.dismissed = Some(held);
                return;
            }
        }
        let handled =
            route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Key(key, time));
        w.held.retain(|&(k, _)| k != held);
        if handled == Propagation::Stop && key.pressed {
            w.held.extend(path.last().map(|&node| (held, node)));
        }
        if handled == Propagation::Stop || !key.pressed {
            return;
        }
        if let Some(message) = w.shortcuts.lookup(&key) {
            effects.messages.push(message);
            return;
        }
        if escape {
            Self::escape_from_text(&mut self.tree, w, effects, now);
            return;
        }
        if key.keysym == TAB || key.keysym == LEFT_TAB {
            let forward = key.keysym == TAB && !key.mods.contains(crate::platform::Mods::SHIFT);
            let next = w.focus.step(&self.tree, &w.focusable, forward);
            let change = w.focus.set(&self.tree, next, true);
            let rest = Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
            effects.absorb(rest);
        }
    }

    /// A press at `position` that lands outside the focused text input, where no node took the focus, ends its editing:
    /// the window keeps no focus, so typing goes nowhere until a field is chosen again.
    pub(super) fn press_away_from_text(
        tree: &mut Tree<A::Message>,
        w: &mut WindowState<A::Message>,
        root: NodeId,
        position: Point,
        effects: &mut Effects<A::Message>,
        now: Timestamp,
    ) {
        if effects.focus.is_some() {
            return;
        }
        let Some(node) = w
            .focus
            .node()
            .filter(|&n| tree.contains(n) && text_input(tree, n))
        else {
            return;
        };
        let path = hit_test(tree, root, &w.geometry, position).unwrap_or_default();
        if path.contains(&node) || path.iter().any(|&n| keeps_editing(tree, n)) {
            return;
        }
        let change = w.focus.set(tree, None, false);
        let mut messages = VecDeque::new();
        let rest = Self::focus_events(tree, w, change, &mut messages, now);
        effects.messages.extend(messages);
        effects.absorb(rest);
    }

    /// Escape that nothing took, in a focused text input, ends its editing as a press away from it does.
    pub(super) fn escape_from_text(
        tree: &mut Tree<A::Message>,
        w: &mut WindowState<A::Message>,
        effects: &mut Effects<A::Message>,
        now: Timestamp,
    ) {
        if !w
            .focus
            .node()
            .is_some_and(|n| tree.contains(n) && text_input(tree, n))
        {
            return;
        }
        let change = w.focus.set(tree, None, false);
        let mut messages = VecDeque::new();
        let rest = Self::focus_events(tree, w, change, &mut messages, now);
        effects.messages.extend(messages);
        effects.absorb(rest);
    }

    /// Moves the focus a roving item asked for before the next frame when the item is already a Tab stop, so a key
    /// that follows at once, such as Return after Down in a menu, goes to the item the first key chose.
    pub(super) fn settle_focus_request(&mut self, i: usize, now: Timestamp) {
        let w = &mut self.windows[i];
        let Some(FocusRequest::Node(node)) = w.focus_request else {
            return;
        };
        if !self.tree.contains(node) || !w.focusable.contains(&node) {
            return;
        }
        w.focus_request = None;
        let change = w.focus.set(&self.tree, Some(node), true);
        let effects = Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
        self.apply(i, effects);
    }

    pub(super) fn touch(
        &mut self,
        i: usize,
        event: TouchEvent,
        time: Timestamp,
        now: Timestamp,
        effects: &mut Effects<A::Message>,
    ) {
        let window = self.windows[i].id;
        // the first finger down is the one a popup opened meanwhile follows; one whose window or path went away while
        // it was held no longer counts
        let gone = self.touch.is_some_and(|t| {
            !self
                .windows
                .iter()
                .any(|w| w.id == t.window && w.touches.contains_key(&t.id))
        });
        if gone {
            self.touch = None;
        }
        let held = self.touch.filter(|t| {
            t.window == window
                && match event {
                    TouchEvent::Motion { id, .. } | TouchEvent::Up { id } => id == t.id,
                    TouchEvent::Cancel => true,
                    TouchEvent::Down { .. } => false,
                }
        });
        if let (Some(t), TouchEvent::Motion { position, .. }) = (self.touch.as_mut(), event) {
            if held.is_some() {
                t.at = position;
            }
        }
        // the popups the held touch opened, each with its top-left corner in this window
        let opened: Vec<(usize, Point)> = held.map_or_else(Vec::new, |t| self.opened_by(i, t));
        if matches!(event, TouchEvent::Up { .. } | TouchEvent::Cancel) && held.is_some() {
            self.touch = None;
        }
        let w = &mut self.windows[i];
        let Some(root) = w.root else { return };
        // a popup follows the touch that opened it while it is held: another finger put down on it goes nowhere
        let following = self
            .touch
            .is_some_and(|t| w.opening_touch == Some((t.id, t.start)));
        let path = match event {
            TouchEvent::Down { .. } if following => return,
            TouchEvent::Down { id, position } => {
                let path = hit_test(&self.tree, root, &w.geometry, position).unwrap_or_default();
                w.touches.insert(id, path.clone());
                if self.touch.is_none() {
                    self.touch = Some(HeldTouch {
                        window,
                        id,
                        start: now,
                        at: position,
                    });
                }
                path
            }
            TouchEvent::Motion { id, .. } => w.touches.get(&id).cloned().unwrap_or_default(),
            TouchEvent::Up { id } => w.touches.remove(&id).unwrap_or_default(),
            TouchEvent::Cancel => {
                // every node a touch went through, the touched one first: a scroll view around it ends its drag too
                let mut targets: Vec<NodeId> = Vec::new();
                for (_, path) in w.touches.drain() {
                    for &node in path.iter().rev() {
                        if !targets.contains(&node) {
                            targets.push(node);
                        }
                    }
                }
                let mut route = route(&mut self.tree, w, now, effects);
                for node in targets {
                    route.notify(node, &Event::Cancel);
                }
                // the popups it opened end its interaction too, with nothing chosen
                for (j, _) in opened {
                    self.tell_popup(j, &Event::Touch(TouchEvent::Cancel, time), now);
                }
                return;
            }
        };
        let Some(t) = held else {
            route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Touch(event, time));
            return;
        };
        let size = |runtime: &Self, j: usize| runtime.windows[j].size;
        let over = opened
            .iter()
            .rev()
            .find(|&&(j, origin)| {
                Rect::new(
                    origin.x,
                    origin.y,
                    size(self, j).width,
                    size(self, j).height,
                )
                .contains(t.at)
            })
            .copied();
        match (event, over) {
            // lifted over a popup it opened, the touch is that popup's, as a press let go there: what it went through
            // here lets go of it without acting
            (TouchEvent::Up { id }, Some((j, origin))) => {
                let w = &mut self.windows[i];
                let mut route = route(&mut self.tree, w, now, effects);
                for &node in path.iter().rev() {
                    route.notify(node, &Event::Cancel);
                }
                // where it lifts, which the popup has not heard of when the touch never moved over it
                let at = Point::new(t.at.x - origin.x, t.at.y - origin.y);
                let moved = TouchEvent::Motion { id, position: at };
                self.tell_popup(j, &Event::Touch(moved, time), now);
                self.tell_popup(j, &Event::Touch(TouchEvent::Up { id }, time), now);
            }
            // lifted anywhere else, it goes where it lifts, and every popup it opened hears it lift at that point,
            // outside it, so a menu dragged through and let go elsewhere can close
            (TouchEvent::Up { id }, None) => {
                let w = &mut self.windows[i];
                route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Touch(event, time));
                for (j, origin) in opened {
                    let at = Point::new(t.at.x - origin.x, t.at.y - origin.y);
                    let moved = TouchEvent::Motion { id, position: at };
                    self.tell_popup(j, &Event::Touch(moved, time), now);
                    self.tell_popup(j, &Event::Touch(TouchEvent::Up { id }, time), now);
                }
            }
            // it moves over the popups it opened as well as here, at each one's own point
            (TouchEvent::Motion { id, position }, _) => {
                let w = &mut self.windows[i];
                route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Touch(event, time));
                for (j, origin) in opened {
                    let at = Point::new(position.x - origin.x, position.y - origin.y);
                    let moved = TouchEvent::Motion { id, position: at };
                    self.tell_popup(j, &Event::Touch(moved, time), now);
                }
            }
            _ => {
                let w = &mut self.windows[i];
                route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Touch(event, time));
            }
        }
    }

    /// The held touch opened a popup: in a window that is not a popup, what it went through lets go of it without
    /// acting, as the popup follows it from now on, so a button it rests on is not clicked as it lifts and a scroll view
    /// does not scroll under the menu. In a menu it went down in, the menu goes on following it.
    pub(super) fn touch_taken(&mut self) {
        let Some(t) = self.touch else { return };
        let Some(k) = self.windows.iter().position(|w| w.id == t.window) else {
            return;
        };
        let now = self.options.clock.now();
        let mut effects = Effects::default();
        let w = &mut self.windows[k];
        if w.popup.is_some() {
            return;
        }
        let path = w.touches.get(&t.id).cloned().unwrap_or_default();
        let mut route = route(&mut self.tree, w, now, &mut effects);
        for &node in path.iter().rev() {
            route.notify(node, &Event::Cancel);
        }
        self.apply(k, effects);
    }

    /// The popups the touch `t`, held in window `i`, opened, each with its top-left corner in that window, the ones
    /// opened first first. A popup not placed yet, or not under that window, is not among them.
    pub(super) fn opened_by(&self, i: usize, t: HeldTouch) -> Vec<(usize, Point)> {
        (0..self.windows.len())
            .filter(|&j| {
                let w = &self.windows[j];
                w.popup.is_some() && w.fade.is_none() && w.opening_touch == Some((t.id, t.start))
            })
            .filter_map(|j| self.origin_in(j, i).map(|origin| (j, origin)))
            .collect()
    }

    /// Where the compositor placed popup `j`'s top-left corner, in the coordinates of window `i` it opens from,
    /// directly or through other popups.
    pub(super) fn origin_in(&self, j: usize, i: usize) -> Option<Point> {
        let mut at = Point::new(0.0, 0.0);
        let mut k = j;
        // each popup opens after its parent, so the chain is never longer than the windows open
        for _ in 0..=self.windows.len() {
            if k == i {
                return Some(at);
            }
            let w = &self.windows[k];
            let parent = &w.popup.as_ref()?.parent;
            let p = self.windows.iter().position(|o| &o.key == parent)?;
            let placed = self.windows[p]
                .popups
                .iter()
                .find(|(key, _)| *key == w.key)?
                .1;
            at = Point::new(at.x + placed.min_x(), at.y + placed.min_y());
            k = p;
        }
        None
    }

    /// Popup `j`'s root hears `event`, as a popup a press opened hears of its release elsewhere.
    pub(super) fn tell_popup(&mut self, j: usize, event: &Event, now: Timestamp) {
        let mut effects = Effects::default();
        let w = &mut self.windows[j];
        if let Some(root) = w.root {
            route(&mut self.tree, w, now, &mut effects).notify(root, event);
        }
        self.apply(j, effects);
    }

    /// Carries out what handlers asked for while one event was routed.
    /// Wakes every node whose time has come, before this turn's input.
    pub(super) fn wake_due(&mut self) {
        let now = self.options.clock.now();
        while let Some(entry) = self.timers.first_entry() {
            if entry.key().0 > now {
                break;
            }
            self.messages.push_back(entry.remove());
        }
        for i in 0..self.windows.len() {
            let w = &mut self.windows[i];
            let due: Vec<NodeId> = w
                .wakes
                .iter()
                .filter(|&(_, &at)| at <= now)
                .map(|(&node, _)| node)
                .collect();
            if due.is_empty() {
                continue;
            }
            w.wakes.retain(|_, at| *at > now);
            let mut effects = Effects::default();
            let mut route = route(&mut self.tree, w, now, &mut effects);
            for node in due {
                if route.tree.contains(node) {
                    route.notify(node, &Event::Wake(now));
                }
            }
            self.apply(i, effects);
        }
    }

    pub(super) fn apply(&mut self, i: usize, mut effects: Effects<A::Message>) {
        let now = self.options.clock.now();
        self.apply_owned(i, &mut effects);
        let w = &mut self.windows[i];
        self.messages.extend(effects.messages);
        // a layout change is the tree's, which the next frame lays out
        for (node, change) in effects.changes {
            if self.tree.contains(node) {
                self.tree.mark(node, change);
                w.repaint();
            }
        }
        if let Some(cursor) = effects.cursor {
            self.system.set_cursor(w.id, cursor);
        }
        // a fullscreen window stays where it is: a maximize request would take it out of fullscreen
        for gesture in effects.window.into_iter().filter(|_| !w.fullscreen) {
            let request = match gesture {
                WindowGesture::Move => WindowRequest::Move,
                WindowGesture::Zoom => match crate::personal().titlebar_double_click {
                    cupertino_defaults::appearance::TitlebarAction::Zoom => {
                        WindowRequest::Maximize(!w.maximized)
                    }
                    cupertino_defaults::appearance::TitlebarAction::Minimize => {
                        WindowRequest::Minimize
                    }
                    cupertino_defaults::appearance::TitlebarAction::None => continue,
                },
            };
            // a platform without the request, or a press the compositor no longer holds, leaves the window where it is
            let _ = self.system.request(w.id, request);
        }
        w.wakes.extend(effects.wakes);
        // the overlays draw again over the paint the window keeps
        w.dirty |= effects.overlay;
        if let Some((node, rect)) = effects.reveal {
            w.reveal = Some((node, Some(rect)));
            w.repaint();
        }
        if !effects.animate.is_empty() {
            w.animate.extend(effects.animate);
            w.repaint();
            // a window whose frame is away asks for the next once it is back ([`WindowEvent::Shown`])
            if w.failed.is_none() && w.in_flight.is_none() {
                self.system.request_frame(w.id);
            }
        }
        if let Some((node, mut state)) = effects.text_input {
            if let Some(state) = &mut state {
                if protected(&self.tree, node) {
                    state.surrounding = None;
                }
            }
            // another field takes the input method: the one before lets go of it first, so the input method starts
            // afresh and nothing it still composes for that field reaches this one
            if state.is_some() && w.text_input.is_some_and(|before| before != node) {
                self.system.set_text_input(w.id, None);
            }
            w.text_input = state.as_ref().map(|_| node);
            self.system.set_text_input(w.id, state.as_ref());
        }
        if let Some((node, target)) = effects.focus_item {
            w.focus_request = roving_target(&self.tree, w, node, target);
            w.repaint();
        }
        if let Some(node) = effects.focus {
            let change = w.focus.set(&self.tree, Some(node), false);
            let rest = Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
            self.apply(i, rest);
            return;
        }
        if w.text_input.is_some() && w.text_input != w.focus.node() {
            w.text_input = None;
            self.system.set_text_input(w.id, None);
        }
    }

    /// Hands each action assistive technology asked for to its node, in the window whose published tree holds it.
    pub(super) fn assistive_actions(&mut self) {
        let mut requests = Vec::new();
        if let Some(adapter) = &mut self.options.accessibility {
            adapter.drain_actions(&mut requests);
        }
        let now = self.options.clock.now();
        for request in requests {
            let target = request.target;
            let Some(i) = self
                .windows
                .iter()
                .position(|w| w.accessibility.contains_key(&target))
            else {
                continue;
            };
            if !self.tree.contains(target) {
                continue;
            }
            // whatever came last, a menu the action opens shows its selection, as one the Menu key opens does
            self.keyboard = true;
            let mut effects = Effects::default();
            route(&mut self.tree, &self.windows[i], now, &mut effects)
                .notify(target, &Event::Action(request));
            self.apply(i, effects);
        }
    }

    pub(super) fn set_clipboard(system: &mut W, text: String) {
        system.set_clipboard(
            ClipboardKind::Clipboard,
            ClipboardData {
                mime: "text/plain;charset=utf-8".into(),
                bytes: text.into_bytes(),
            },
        );
    }

    /// Tells the nodes a focus move concerns, and repaints them. Returns what else their handlers asked for, such as
    /// the input method; a focus request made while focus moves is dropped, so two nodes cannot pass it back and forth.
    pub(super) fn focus_events(
        tree: &mut Tree<A::Message>,
        w: &mut WindowState<A::Message>,
        change: Option<FocusChange>,
        messages: &mut VecDeque<A::Message>,
        now: Timestamp,
    ) -> Effects<A::Message> {
        let Some(change) = change else {
            return Effects::default();
        };
        w.repaint();
        if change.visible {
            w.reveal = change.to.map(|node| (node, None));
        }
        // the same node with its ring shown or hidden keeps its focus: no events, only a repaint
        if change.from == change.to {
            return Effects::default();
        }
        let mut effects = Effects::default();
        let mut route = route(tree, w, now, &mut effects);
        if let Some(from) = change.from {
            route.notify(
                from,
                &Event::Focus {
                    gained: false,
                    visible: false,
                },
            );
        }
        for &node in &change.left {
            route.notify(
                node,
                &Event::FocusWithin {
                    gained: false,
                    visible: false,
                },
            );
        }
        for &node in &change.entered {
            route.notify(
                node,
                &Event::FocusWithin {
                    gained: true,
                    visible: change.visible,
                },
            );
        }
        if let Some(to) = change.to {
            route.notify(
                to,
                &Event::Focus {
                    gained: true,
                    visible: change.visible,
                },
            );
        }
        messages.extend(effects.messages.drain(..));
        for (node, c) in effects.changes.drain(..) {
            if tree.contains(node) {
                tree.mark(node, c);
            }
        }
        effects.focus = None;
        effects
    }
}

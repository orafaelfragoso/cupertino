use super::*;

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    /// Rebuilds every window from the model and reconciles it with the tree.
    pub(super) fn view(&mut self) -> Result<(), UiError> {
        if let Some(timing) = &mut self.scroll_timing {
            timing.updated(self.options.clock.now());
        }
        let mut old: Vec<WindowState<A::Message>> = std::mem::take(&mut self.windows);
        let before: Vec<WindowKey> = old.iter().map(|w| w.key.clone()).collect();
        let (mut popups, toplevels): (Vec<_>, Vec<_>) = A::view(&self.model)
            .into_iter()
            .partition(|w| w.popup.is_some());
        for Window {
            key,
            spec,
            shortcuts,
            on_close,
            background,
            content,
            popup: _,
            menus,
        } in toplevels
        {
            let mut w = match old.iter().position(|w| w.key == key && w.popup.is_none()) {
                Some(i) => {
                    let mut w = old.remove(i);
                    if w.spec.title != spec.title {
                        let _ = self
                            .system
                            .request(w.id, WindowRequest::SetTitle(spec.title.clone()));
                        if let Some(adapter) = &mut self.options.accessibility {
                            if w.accessible {
                                adapter.window_title(w.id, &spec.title);
                            }
                        }
                    }
                    if w.spec.min_size != spec.min_size {
                        let _ = self
                            .system
                            .request(w.id, WindowRequest::SetMinSize(spec.min_size));
                    }
                    if w.spec.max_size != spec.max_size {
                        let _ = self
                            .system
                            .request(w.id, WindowRequest::SetMaxSize(spec.max_size));
                    }
                    if w.spec.input_region != spec.input_region {
                        let _ = self.system.request(
                            w.id,
                            WindowRequest::SetInputRegion(spec.input_region.clone()),
                        );
                    }
                    // the window system leases its buffers at the size it knows: told of the new one, both agree, and
                    // a window whose size is its own takes it at once
                    if w.spec.size != spec.size {
                        let _ = self.system.request(w.id, WindowRequest::SetSize(spec.size));
                        if w.free_size && w.size != spec.size {
                            w.size = spec.size;
                            w.invalidate();
                            w.invalidate_damage();
                        }
                    }
                    w.spec = spec;
                    w
                }
                None => {
                    self.system.opening(&key);
                    let id = self
                        .system
                        .create_window(&spec)
                        .map_err(UiError::Platform)?;
                    for (_, request) in self.to_open.extract_if(.., |(k, _)| *k == key) {
                        let _ = self.system.request(id, request);
                    }
                    self.new_window(key, id, spec)
                }
            };
            w.shortcuts = shortcuts;
            w.on_close = on_close;
            set_background(&mut w, background);
            sync_backdrop(&mut self.system, &mut w);
            sync_menus(&mut self.system, &mut w, menus);
            w.root = Some(self.tree.reconcile(w.root, content)?);
            // what changed size the tree marked for the next layout
            w.repaint();
            self.windows.push(w);
        }
        // a refused popup no longer declared may be asked for again
        self.refused_popups
            .retain(|key| popups.iter().any(|w| &w.key == key));
        // each popup after its parent, which may be a popup declared later; one without an open parent stays closed
        while let Some(i) = popups.iter().position(|w| {
            w.popup
                .as_ref()
                .is_some_and(|p| self.windows.iter().any(|open| open.key == p.parent))
        }) {
            let Window {
                key,
                spec,
                shortcuts,
                on_close,
                background,
                content,
                popup,
                menus: _,
            } = popups.remove(i);
            let popup = popup.expect("partitioned as a popup");
            if self.refused_popups.contains(&key) {
                continue;
            }
            // a new popup that takes the grab must stand on top of the grab's chain: the popups going away (a closing
            // menu among them) go first
            let reused = old
                .iter()
                .any(|w| w.key == key && w.popup.is_some() && w.fade.is_none());
            if popup.grab && !reused {
                let declared: Vec<WindowKey> = popups.iter().map(|w| w.key.clone()).collect();
                self.close_leftover_popups(&mut old, &declared);
            }
            let popup = Declared {
                parent: popup.parent,
                anchor: popup.anchor,
                grab: popup.grab,
                on_dismiss: Dismiss::Message(popup.on_dismiss),
            };
            if let Some(mut w) = self.popup(key, spec, popup, content, &mut old)? {
                w.shortcuts = shortcuts;
                w.on_close = on_close;
                set_background(&mut w, background);
                sync_backdrop(&mut self.system, &mut w);
                w.invalidate();
                self.windows.push(w);
            }
        }
        self.declare_owned(&mut old)?;
        if std::mem::take(&mut self.touch_opened) {
            self.touch_taken();
        }
        // a menu the view opened that the last one did not have: the menus that close with it go at once
        let reopened = self.windows.iter().any(|w| {
            w.fade.is_none()
                && w.popup.is_some()
                && matches!(
                    w.background,
                    WindowBackground::Surface {
                        surface: Surface::Menu,
                        ..
                    }
                )
                && !before.contains(&w.key)
        });
        let mut old = self.fade_closed(old, reopened);
        // popups go before their parents, the latest first, as the protocol destroys them
        old.reverse();
        old.sort_by_key(|w| w.popup.is_none());
        for w in old {
            self.close(w);
        }
        let removed = self.tree.take_removed();
        if !removed.is_empty() {
            for w in &mut self.windows {
                w.pointer.retain(&self.tree);
                w.animate.retain(|n| self.tree.contains(*n));
                w.paint_animate.retain(|n| self.tree.contains(*n));
                w.wakes.retain(|n, _| self.tree.contains(*n));
                w.touches
                    .retain(|_, path| path.last().is_some_and(|&n| self.tree.contains(n)));
                if w.text_input.is_some_and(|n| !self.tree.contains(n)) {
                    w.text_input = None;
                    self.system.set_text_input(w.id, None);
                }
            }
        }
        // a message waits only for a window the view has: one closed, or never opened, presents nothing
        let windows = &self.windows;
        self.on_presented
            .retain(|(key, _)| windows.iter().any(|w| &w.key == key));
        // a request for a window the view did not open goes nowhere
        self.to_open.clear();
        if self.windows.is_empty() && !A::RESIDENT {
            self.exit = true;
        }
        Ok(())
    }

    /// Opens or keeps the popup a control opened for itself, after the application's windows: its windows are built
    /// anew as a declared popup's are. It is gone with its control, with the window the control is in, and when the
    /// window system refuses it.
    pub(super) fn declare_owned(
        &mut self,
        old: &mut Vec<WindowState<A::Message>>,
    ) -> Result<(), UiError> {
        let declared = self.declare_owned_windows(old);
        self.mark_owner();
        declared
    }

    /// Tells each window whether a control of it has a popup of its own open.
    pub(super) fn mark_owner(&mut self) {
        let parent = self.owned.as_ref().map(|owned| &owned.parent);
        for w in &mut self.windows {
            w.owned_popup = parent == Some(&w.key);
        }
    }

    pub(super) fn declare_owned_windows(
        &mut self,
        old: &mut Vec<WindowState<A::Message>>,
    ) -> Result<(), UiError> {
        let Some(mut owned) = self.owned.take() else {
            return Ok(());
        };
        let open = self.tree.contains(owned.node)
            && self
                .windows
                .iter()
                .any(|w| w.key == owned.parent && w.fade.is_none());
        if !open {
            return Ok(());
        }
        owned.keys.clear();
        for OwnedWindow {
            key,
            spec,
            background,
            content,
            anchor,
            grab,
        } in (owned.build)()
        {
            let reused = old
                .iter()
                .any(|w| w.key == key && w.popup.is_some() && w.fade.is_none());
            if grab && !reused {
                self.close_leftover_popups(old, &[]);
            }
            let popup = Declared {
                parent: owned.parent.clone(),
                anchor,
                grab,
                on_dismiss: Dismiss::Owned,
            };
            owned.keys.push(key.clone());
            if let Some(mut w) = self.popup(key, spec, popup, content, old)? {
                set_background(&mut w, background);
                sync_backdrop(&mut self.system, &mut w);
                w.invalidate();
                self.windows.push(w);
            }
        }
        // one the window system refused is not asked for again
        if owned.keys.iter().any(|k| self.refused_popups.contains(k)) {
            self.refused_popups.retain(|k| !owned.keys.contains(k));
            return Ok(());
        }
        self.owned = Some(owned);
        Ok(())
    }

    /// The compositor dismissed a popup, or it could not open: its application hears so, and a popup a control opened
    /// for itself is closed.
    pub(super) fn dismissed(&mut self, dismiss: Dismiss<A::Message>) {
        match dismiss {
            Dismiss::Message(message) => self.messages.push_back(message),
            Dismiss::Owned => self.close_owned(),
        }
    }

    /// Closes the popup a control opened for itself, at the next view.
    pub(super) fn close_owned(&mut self) {
        if self.owned.take().is_some() {
            self.view_dirty = true;
            self.mark_owner();
        }
    }

    /// The compositor dismissed the popup a control of a window opened for itself, and every event it sent with that is
    /// handled. A window that lost the keyboard to that popup was told so while [`EventCx::holds_popup`] was true, and
    /// took it for the popup's. When the dismissal gave the keyboard back, the window heard so among those events;
    /// when it did not (another surface took the keyboard, which is what dismissed the popup), no event says so, as
    /// the keyboard never passed through the window. The ancestors of its focused node then hear the loss again, now
    /// with no popup open, so a panel that closes without the keyboard closes.
    pub(super) fn keyboard_after_dismissal(&mut self) {
        let Some(key) = self.owned_dismissed.take() else {
            return;
        };
        let Some(i) = self.windows.iter().position(|w| w.key == key) else {
            return;
        };
        let w = &mut self.windows[i];
        if w.active || w.keyboard_in_popup || w.owned_popup || w.fade.is_some() {
            return;
        }
        let Some(node) = w.focus.node() else {
            return;
        };
        let mut ancestors = self.tree.path(node);
        ancestors.pop();
        ancestors.reverse();
        let lost = Event::FocusWithin {
            gained: false,
            visible: false,
        };
        let now = self.options.clock.now();
        let mut effects = Effects::default();
        let mut route = route(&mut self.tree, w, now, &mut effects);
        for ancestor in ancestors {
            route.notify(ancestor, &lost);
        }
        self.apply(i, effects);
    }

    /// The clipboard offers these kinds of data, or none: whether it holds text, and of which kind, for every window.
    pub(super) fn clipboard_offers(&mut self, mimes: Option<&[String]>) {
        self.clipboard_text = mimes.and_then(|mimes| {
            TEXT_MIMES
                .iter()
                .find(|text| mimes.iter().any(|m| m == *text))
                .map(|text| (*text).to_owned())
        });
        let holds = self.clipboard_text.is_some();
        for w in &mut self.windows {
            w.clipboard_text = holds;
        }
    }

    /// Whether `read` is a clipboard read the runtime started for a paste, and then whether the control it is for
    /// protects its input, as a password field does: what is read is then a secret. This is for what keeps a record
    /// of the events it hands the runtime, as a test driver's trace does, to leave the secret out of it; the runtime
    /// itself keeps no record, and treats what every paste read as it would a secret
    /// ([`PlatformEvent::Clipboard`] in `platform_event`).
    pub fn paste_protected(&self, read: ClipboardToken) -> Option<bool> {
        self.pastes
            .get(&read)
            .map(|(_, node)| self.tree.contains(*node) && protected(&self.tree, *node))
    }

    /// The text of a paste arrived, or `None` when it could not be read: the control that asked hears it, unless it
    /// or its window went meanwhile. What was read may be a password on its way into its field, so the bytes are
    /// zeroed here, where they end, and the text when the event drops ([`crate::Pasted`]): neither stays in freed
    /// memory. `read` is a paste's ([`Runtime::pastes`]).
    pub(super) fn paste_data(&mut self, read: ClipboardToken, data: Option<ClipboardData>) {
        let Some((key, node)) = self.pastes.remove(&read) else {
            return;
        };
        let pasted = data.map(|mut data| {
            let pasted = crate::Pasted(clipboard_text(&data.bytes));
            cupertino_ui_text::wipe_bytes(&mut data.bytes);
            pasted
        });
        let window = self.windows.iter().position(|w| w.key == key);
        if let (Some(pasted), Some(i)) = (pasted.filter(|p| !p.0.is_empty()), window) {
            if self.tree.contains(node) {
                let now = self.options.clock.now();
                let mut effects = Effects::default();
                route(&mut self.tree, &self.windows[i], now, &mut effects)
                    .notify(node, &Event::Paste(pasted));
                self.apply(i, effects);
            }
        }
    }

    /// What a control asked of the clipboard and of a popup of its own, in window `i`.
    pub(super) fn apply_owned(&mut self, i: usize, effects: &mut Effects<A::Message>) {
        if let Some(text) = effects.clipboard.take() {
            Self::set_clipboard(&mut self.system, text);
            self.clipboard_replaced = None;
            self.clipboard_offers(Some(&[TEXT_MIMES[0].to_owned()]));
        }
        if let Some(node) = effects.paste.take() {
            if let Some(mime) = self.clipboard_text.clone() {
                let read = self.system.read_clipboard(ClipboardKind::Clipboard, &mime);
                self.pastes
                    .insert(read, (self.windows[i].key.clone(), node));
            }
        }
        if let Some((node, build)) = effects.popup.take() {
            self.owned = Some(Owned {
                parent: self.windows[i].key.clone(),
                node,
                build,
                keys: Vec::new(),
            });
            self.view_dirty = true;
            self.mark_owner();
        }
        let key = &self.windows[i].key;
        let inside = self
            .owned
            .as_ref()
            .is_some_and(|owned| owned.keys.contains(key));
        let opener = self
            .owned
            .as_ref()
            .is_some_and(|owned| &owned.parent == key);
        if let Some(answer) = effects.popup_answer.take().filter(|_| inside) {
            let owner = self.owned.as_ref().and_then(|owned| {
                let p = self.windows.iter().position(|w| w.key == owned.parent)?;
                self.tree.contains(owned.node).then_some((p, owned.node))
            });
            if let Some((p, node)) = owner {
                let now = self.options.clock.now();
                let mut heard = Effects::default();
                route(&mut self.tree, &self.windows[p], now, &mut heard)
                    .notify(node, &Event::PopupAnswer(answer));
                self.apply(p, heard);
            }
        }
        if std::mem::take(&mut effects.popup_close) && (inside || opener) {
            self.close_owned();
        }
    }

    pub(super) fn new_window(
        &mut self,
        key: WindowKey,
        id: WindowId,
        spec: WindowSpec,
    ) -> WindowState<A::Message> {
        WindowState {
            key,
            id,
            size: spec.size,
            spec,
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            backdrop: None,
            materials: Vec::new(),
            materials_refused: false,
            solid_shown: None,
            solid_refused: false,
            menus: None,
            on_menu: None,
            root: None,
            env: Environment {
                theme: Arc::clone(&self.options.theme),
                text: Arc::clone(&self.options.text),
                icons: Arc::clone(&self.options.icons),
                direction: self.options.direction,
                scale: 1.0,
            },
            configured: false,
            free_size: true,
            ack: None,
            active: false,
            keyboard_in_popup: false,
            painted_active: None,
            maximized: false,
            fullscreen: false,
            frame_ready: false,
            committed: false,
            owed: false,
            presented: false,
            dirty: true,
            prepared: false,
            laid_out: None,
            focus: Focus::default(),
            pointer: Pointer::default(),
            touches: HashMap::new(),
            held: Vec::new(),
            dismissed: None,
            animate: BTreeSet::new(),
            paint_animate: BTreeSet::new(),
            idle: BTreeSet::new(),
            frame_clock: idle::FrameClock::default(),
            wakes: BTreeMap::new(),
            text_input: None,
            geometry: Geometry::default(),
            accessibility: BTreeMap::new(),
            accessibility_focus: None,
            semantic_revision: 0,
            semantic_snapshot: None,
            semantic_sent: None,
            semantic_due: None,
            accessible: false,
            focusable: Vec::new(),
            roving_groups: Vec::new(),
            roving_items: Vec::new(),
            focus_request: None,
            reveal: None,
            scroll_heard: HashMap::new(),
            held_paints: 0,
            layout: LayoutStats::default(),
            layout_cache: LayoutCache::new(),
            placements: KeptPlacements::default(),
            semantics: KeptSemantics::default(),
            measured_in: None,
            painter: Painter::default(),
            damage: DamageTracker::default(),
            raised: Vec::new(),
            obscuring: Vec::new(),
            layer: LayerState::default(),
            surface_layers: surface_layers::SurfaceLayers::default(),
            display_list: Arc::default(),
            resources: Arc::default(),
            renderer: Arc::new(Mutex::new((self.options.renderers)())),
            in_flight: None,
            layers_owed: false,
            failed: None,
            popup: None,
            opening_press: None,
            opening_touch: None,
            popups: Vec::new(),
            drop: None,
            fade: None,
            grows_from: None,
            grow_followed: None,
            layouts: 0,
            grow: None,
            activated_at: None,
            opaque: Vec::new(),
            clipboard_text: self.clipboard_text.is_some(),
            owned_popup: false,
        }
    }

    /// Opens or keeps a declared popup of an open parent. `None` when the parent has not laid out its anchor yet.
    pub(super) fn popup(
        &mut self,
        key: WindowKey,
        spec: WindowSpec,
        popup: Declared<A::Message>,
        content: Element<A::Message>,
        old: &mut Vec<WindowState<A::Message>>,
    ) -> Result<Option<WindowState<A::Message>>, UiError> {
        let mut existing = old
            .iter()
            .position(|w| w.key == key && w.popup.is_some() && w.fade.is_none())
            .map(|i| old.remove(i));
        let parent = self
            .windows
            .iter()
            .find(|w| w.key == popup.parent)
            .expect("a popup is opened after its parent");
        let root = self
            .tree
            .reconcile(existing.as_mut().and_then(|w| w.root.take()), content)?;
        let env = parent.env.clone();
        let measured = layout(
            &TreeView::new(&self.tree, &env),
            root,
            Constraints::loose(spec.size).map_err(UiError::Layout)?,
            &LayoutOptions {
                direction: env.direction,
                node_limit: self.options.node_limit,
            },
        )
        .map_err(UiError::Layout)?;
        let size = measured.nodes.get(&root).map_or(spec.size, |n| n.rect.size);
        if let Some(mut w) = existing {
            let state = w.popup.as_mut().expect("only popups are taken");
            state.parent = popup.parent;
            state.anchor = popup.anchor;
            state.size = size;
            state.on_dismiss = popup.on_dismiss;
            w.spec = spec;
            w.root = Some(root);
            return Ok(Some(w));
        }
        let grab = self.popup_grabs(popup.grab, parent);
        let Some(placed) = place(&self.tree, parent, &popup.anchor, grab, size) else {
            // a parent laid out without the anchor waits for the next view; retrying at once would never settle
            if parent.geometry.is_empty() && !self.awaiting_layout.contains(&popup.parent) {
                self.awaiting_layout.push(popup.parent.clone());
            }
            // an anchor node that came with this view has no place before the parent lays out: ask once more after it
            if anchor_declared(&self.tree, parent, &popup.anchor)
                && !self.awaiting_anchor.iter().any(|(k, _)| *k == popup.parent)
            {
                self.awaiting_anchor.push((popup.parent.clone(), false));
            }
            self.tree.unmount(root);
            return Ok(None);
        };
        self.system.opening(&key);
        let id = match self.system.create_popup(parent.id, &placed) {
            Ok(id) => id,
            Err(e) => {
                let parent = parent.id;
                self.tree.unmount(root);
                self.popup_refused(key, parent, popup.on_dismiss, e)?;
                return Ok(None);
            }
        };
        let mut w = self.new_window(key, id, spec);
        w.focus = Focus::shown(self.keyboard);
        w.opening_press = self.press;
        w.opening_touch = self.touch.map(|t| (t.id, t.start));
        self.touch_opened |= w.opening_touch.is_some();
        w.env = env;
        w.size = size;
        w.popup = Some(PopupState {
            parent: popup.parent,
            anchor: popup.anchor,
            size,
            on_dismiss: popup.on_dismiss,
            ungrabbed: popup.grab && !placed.grab,
            placed,
        });
        w.root = Some(root);
        Ok(Some(w))
    }
}

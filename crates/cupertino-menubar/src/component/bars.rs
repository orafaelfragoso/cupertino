//! The bars: one per output, each with its controller and its feed; what the person does on a bar and its menus, and
//! what the controller asks for in return.

use std::collections::VecDeque;
use std::time::Instant;

use cupertino_apps::Launch;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_keyboard::modifiers::{Modifier, Physical, Profile};
use cupertino_kit::core::{Command as Do, WindowKey};
use cupertino_kit::geometry::Rect as Area;
use cupertino_kit::platform::{KeyEvent, Keysym, Mods};
use cupertino_kit::text::KeyInput;
use cupertino_kit_shell::bus::{self, Message as Published};
use cupertino_kit_shell::outputs::OutputId;
use cupertino_kit_shell::{menu_bar, Keyboard, ServerWindowId};

use super::{fail, presented, Message, Model, PROCESS};
use crate::controller::{MenuController, Mode, Outcome};
use crate::extras::{ExtraAction, Passphrase, SessionAction};
use crate::feed::BarFeed;
use crate::geom::{Point, Rect};
use crate::layout::{panel, SlotKey};
use crate::routing::{self, menu_key, Action, Context};
use crate::tone::{Backdrop as Wallpaper, BarTone, SlotTones};
use crate::view::{BarId, BarPointer, Input, KitMeasure, PromptEvent, PromptPhase};
use crate::worker::Request;

/// The keysyms of the Alt keys, which show the alternates of the open menus' items while held.
const ALT_L: Keysym = Keysym(0xffe9);
const ALT_R: Keysym = Keysym(0xffea);

/// How long a join may take before the prompt says it failed.
const JOIN_TIME: std::time::Duration = std::time::Duration::from_secs(30);

/// How long the Wi-Fi panel says it is looking for networks after a scan it asked for, if none is listed by then.
pub(super) const WIFI_SCAN_TIME: std::time::Duration = std::time::Duration::from_secs(8);

/// The join prompt of one bar: which network, what is typed and how far the attempt stands.
pub(super) struct JoinPrompt {
    pub(super) bar: BarId,
    pub(super) ssid: String,
    pub(super) show: bool,
    pub(super) text: Passphrase,
    pub(super) phase: PromptPhase,
    /// What the service had answered and what the device had failed with when the join was asked, so an earlier
    /// attempt's answer is not taken for this one's.
    joins_at_send: u32,
    failure_at_send: String,
    /// The service took the request: the device's state tells the rest.
    answered: bool,
    saw_busy: bool,
    pub(super) deadline: Option<Instant>,
}

/// The line that says why a join failed, from the device's failure.
pub(super) fn failure_line(failure: &str) -> String {
    match failure {
        "wrong-passphrase" => "The password is not correct.",
        "network-not-found" => "The network is not in range.",
        "no-address" => "The network gave no address.",
        "connection-lost" => "The connection was lost.",
        _ => "The network could not be joined.",
    }
    .to_owned()
}

/// The bar of one output.
pub(super) struct Bar {
    pub(super) id: BarId,
    pub(super) output: OutputId,
    /// The output's place and size in the compositor space, in points.
    geometry: Area,
    pub(super) controller: MenuController,
    pub(super) feed: BarFeed,
    /// Whether the bar's layer surface takes the keyboard.
    pub(super) keyboard: Keyboard,
    /// The bar's own surface has the keyboard.
    focused: bool,
    /// Alt is held on a surface of the bar.
    alt: bool,
    /// The wallpaper under the bar, from the server; `None` before it is known.
    pub(super) backdrop: Option<Wallpaper>,
    /// The palette of the bar's strip.
    pub(super) tone: BarTone,
    /// The tone of each item of the status area over the wallpaper under it, while `tone.per_item` is on, the bar is
    /// see-through and the server tells the wallpaper band by band; empty otherwise.
    pub(super) slot_tones: SlotTones,
    /// Groups of the menu-bar object that wait for the name of the application they make active, in order.
    pub(super) held: VecDeque<menu_bar::Update>,
    /// The bar gives way to an overview of its output: faded out, without input (`Model::give_way`).
    pub(super) hidden: bool,
}

impl Bar {
    /// The bar's width: its output's.
    pub(super) fn width(&self) -> f64 {
        self.geometry.width()
    }

    /// The relative luminance of the wallpaper under the whole bar, once known.
    fn luminance(&self) -> Option<f64> {
        self.backdrop.as_ref().map(Wallpaper::luminance)
    }

    /// The bar's tone chosen afresh, under `display`; each item takes its tone afresh, at once.
    pub(super) fn retone(&mut self, display: DisplayOptions) {
        self.tone = BarTone::fresh(self.luminance(), display);
        self.slot_tones.clear();
    }

    /// A surface of this bar has the keyboard: the bar, or one of its menus, which take it with their grab.
    fn has_keyboard(&self) -> bool {
        self.focused || self.controller.is_open()
    }

    /// The output moved or changed its size: menus are laid out on its screen.
    pub(super) fn set_geometry(&mut self, geometry: Area) {
        if self.geometry != geometry {
            self.geometry = geometry;
            self.controller
                .set_output(Rect::new(0.0, 0.0, geometry.width(), geometry.height()));
        }
    }

    /// Where the join prompt's top-left corner is on the bar: where the Wi-Fi panel's is, its right edge level with
    /// the Wi-Fi item's, a little below the bar.
    pub(super) fn prompt_origin(&self, bar_height: f64) -> Point {
        let right = self
            .controller
            .layout()
            .slots
            .iter()
            .find(|slot| matches!(&slot.key, SlotKey::Status(id) if id == "extra:wifi"))
            .map_or(self.width() - 8.0, |slot| slot.rect.x + slot.rect.w);
        Point::new((right - panel::WIDTH).max(8.0), bar_height + panel::GAP)
    }
}

/// The modifiers of `key` on the keys that carry them, as the search field's chords are written.
fn key_input(key: &KeyEvent, profile: &Profile) -> KeyInput {
    let held: Vec<Physical> = [
        (Mods::SHIFT, Modifier::Shift),
        (Mods::CTRL, Modifier::Ctrl),
        (Mods::ALT, Modifier::Alt),
        (Mods::PRIMARY, Modifier::Primary),
    ]
    .into_iter()
    .filter(|(mods, _)| key.mods.contains(*mods))
    .map(|(_, role)| profile.physical_for(role))
    .collect();
    let has = |p: Physical| held.contains(&p);
    KeyInput::new(
        key.code,
        has(Physical::Shift),
        has(Physical::Ctrl),
        has(Physical::Alt),
        has(Physical::Super),
        key.text,
    )
}

/// What a launch starts, for messages.
pub(super) fn launch_name(launch: &Launch) -> String {
    match launch {
        Launch::Program(argv) => argv.join(" "),
        Launch::App(app) => app.id.clone(),
        Launch::Open { app, file } => format!("{} with {}", file.display(), app.id),
    }
}

fn client_session_action(action: SessionAction) -> menu_bar::SessionAction {
    match action {
        SessionAction::Sleep => menu_bar::SessionAction::Sleep,
        SessionAction::Restart => menu_bar::SessionAction::Restart,
        SessionAction::ShutDown => menu_bar::SessionAction::ShutDown,
        SessionAction::Lock => menu_bar::SessionAction::Lock,
        SessionAction::LogOut => menu_bar::SessionAction::LogOut,
        SessionAction::SwitchUser => menu_bar::SessionAction::SwitchUser,
        SessionAction::RefreshCapabilities => menu_bar::SessionAction::RefreshCapabilities,
    }
}

impl Model {
    pub(super) fn index(&self, bar: BarId) -> Option<usize> {
        self.bars.iter().position(|b| b.id == bar)
    }

    pub(super) fn create_bar(&mut self, output: OutputId, geometry: Area) -> Vec<Do<Message>> {
        let id = BarId(self.next_bar);
        self.next_bar += 1;
        let mut controller = MenuController::new(
            Rect::new(0.0, 0.0, geometry.width(), geometry.height()),
            Box::new(KitMeasure {
                text: std::sync::Arc::clone(&self.text),
                font: self.font,
            }),
        );
        controller.bar_metrics = self.metrics.clone();
        controller.menu_metrics.shortcut_style = self.shared.chords.style();
        controller.cfg.flash_choice = !self.display.reduce_motion;
        let tone = BarTone::fresh(None, self.tone_display());
        // a bar made while an overview of its output is shown starts hidden, with no motion
        let hidden = self.overview.contains(&output);
        self.bars.push(Bar {
            id,
            output,
            geometry,
            controller,
            // the server's menu bar role is of a version that sends the Edit items' chords and force quits
            feed: BarFeed::new(id.0, true, true),
            keyboard: Keyboard::None,
            focused: false,
            alt: false,
            backdrop: None,
            tone,
            slot_tones: SlotTones::default(),
            held: VecDeque::new(),
            hidden,
        });
        let mut commands = if hidden {
            presented(&id.window(), true, None)
        } else {
            Vec::new()
        };
        commands.extend(self.refresh());
        if self.bars.len() == 1 {
            commands.extend(self.request(Request::AppMenuRegistrar(true)));
        }
        commands
    }

    /// Drop a bar; its window closes with it.
    pub(super) fn destroy_bar(&mut self, i: usize) -> Vec<Do<Message>> {
        let bar = self.bars.remove(i);
        if self.join.as_ref().is_some_and(|j| j.bar == bar.id) {
            self.join = None;
        }
        self.x11.waiting.retain(|q| q.key != bar.id.0);
        let mut commands = self.refresh();
        if self.bars.is_empty() {
            commands.extend(self.request(Request::AppMenuRegistrar(false)));
        }
        commands
    }

    /// A group of the bar `key`'s menu-bar object, applied once the name of the application it makes active is known:
    /// groups wait, in order, while the name is read.
    pub(super) fn menu_bar_update(
        &mut self,
        key: &WindowKey,
        update: menu_bar::Update,
    ) -> Vec<Do<Message>> {
        let Some(i) = self.bars.iter().position(|b| b.id.window() == *key) else {
            return Vec::new();
        };
        self.bars[i].held.push_back(update);
        self.apply_held(i)
    }

    /// Apply the bar's waiting groups up to the first whose application's name is not read yet, which is asked for.
    pub(super) fn apply_held(&mut self, i: usize) -> Vec<Do<Message>> {
        let mut commands = Vec::new();
        while let Some(update) = self.bars[i].held.front() {
            let app_id = update
                .active_app
                .as_ref()
                .map(|a| a.app_id.clone())
                .filter(|id| !id.is_empty());
            if let Some(app_id) = &app_id {
                if !self.names.contains_key(app_id) {
                    commands.extend(self.read_name(app_id));
                    break;
                }
            }
            let Some(update) = self.bars[i].held.pop_front() else {
                break;
            };
            if let Some(app_id) = app_id {
                // the entry may have changed since it was read: it is read again, and a new name shown
                commands.extend(self.read_name(&app_id));
            }
            commands.extend(self.apply_update(i, &update));
        }
        commands
    }

    fn apply_update(&mut self, i: usize, update: &menu_bar::Update) -> Vec<Do<Message>> {
        let names = &self.names;
        let query = self.bars[i].feed.apply(update, &mut self.shared, |id| {
            names
                .get(id)
                .cloned()
                .unwrap_or_else(|| crate::desktop::provisional_name(id))
        });
        let mut commands = self.ask_x11(query);
        let tone_display = self.tone_display();
        let backdrop = match (&update.backdrop_bands, update.backdrop_lightness) {
            (Some(bands), _) => Some(Wallpaper::Bands(bands.clone())),
            (None, Some(lightness)) => Some(Wallpaper::Lightness(lightness)),
            (None, None) => None,
        };
        if let Some(backdrop) = backdrop {
            let bar = &mut self.bars[i];
            bar.backdrop = Some(backdrop);
            // a new wallpaper: each item takes its tone afresh, at once
            bar.retone(tone_display);
        }
        if update.shortcuts.is_some() {
            let style = self.shared.chords.style();
            for bar in &mut self.bars {
                bar.controller.menu_metrics.shortcut_style = style;
            }
        }
        if update.engage {
            let outcome = self.bars[i].controller.focus_bar();
            commands.extend(self.apply(i, outcome));
        }
        if update.engage_help {
            let bar = &mut self.bars[i];
            let outcome = bar.controller.focus_help_search(bar.has_keyboard());
            commands.extend(self.apply(i, outcome));
        }
        commands.extend(self.refresh());
        commands
    }

    pub(super) fn input(&mut self, input: Input) -> Vec<Do<Message>> {
        let now = self.now();
        // a bar that gives way to an overview takes no input, also what was on its way as it hid
        if let Input::Pointer { bar, .. } | Input::Key { bar, .. } = &input {
            if self.index(*bar).is_some_and(|i| self.bars[i].hidden) {
                return Vec::new();
            }
        }
        match input {
            Input::Pointer { bar, pointer, time } => {
                let Some(i) = self.index(bar) else {
                    return Vec::new();
                };
                let controller = &mut self.bars[i].controller;
                controller.set_input_time(time, now);
                let outcomes = match pointer {
                    BarPointer::Move(p) => vec![controller.pointer_move(p, now)],
                    BarPointer::Press(p, button) => vec![controller.button_press(p, button, now)],
                    BarPointer::Release(p, button) => {
                        vec![controller.button_release(p, button, now)]
                    }
                    BarPointer::Scroll { at, steps } => {
                        vec![controller.pointer_scroll(at, steps, now)]
                    }
                    BarPointer::Click(p) => vec![
                        controller.pointer_move(p, now),
                        controller.pointer_press(p, now),
                        controller.pointer_release(p, now),
                    ],
                };
                outcomes
                    .into_iter()
                    .flat_map(|outcome| self.apply(i, outcome))
                    .collect()
            }
            Input::Key { bar, key, time } => match self.index(bar) {
                Some(i) => {
                    self.bars[i].controller.set_input_time(time, now);
                    self.key(i, &key, now)
                }
                None => Vec::new(),
            },
            Input::Dismissed(bar) => match self.index(bar) {
                // the server dismissed the grab: the menus are over
                Some(i) => {
                    let outcome = self.bars[i].controller.close();
                    self.apply(i, outcome)
                }
                None => Vec::new(),
            },
            Input::Prompt { bar, event } => self.prompt_event(bar, event),
            Input::Keyboard { bar, focused } => match self.index(bar) {
                Some(i) => self.keyboard_focus(i, focused),
                None => Vec::new(),
            },
            Input::Closed(bar) => {
                // the server closed the bar: a bar of an output still present is made anew
                let Some(i) = self.index(bar) else {
                    return Vec::new();
                };
                let mut commands = self.destroy_bar(i);
                commands.extend(self.follow_outputs());
                commands
            }
        }
    }

    fn key(&mut self, i: usize, key: &KeyEvent, now: f64) -> Vec<Do<Message>> {
        self.bars[i].alt = match key.keysym {
            ALT_L | ALT_R => key.pressed,
            _ => key.mods.contains(Mods::ALT),
        };
        let mut commands = self.sync_alt(i);
        if !key.pressed {
            return commands;
        }
        let command = key.mods.contains(Mods::CTRL)
            || key.mods.contains(Mods::ALT)
            || key.mods.contains(Mods::PRIMARY);
        let code = menu_key(key.code).or_else(|| routing::typed_key(key.text, command));
        let controller = &mut self.bars[i].controller;
        let searched = self.keymap.as_mut().and_then(|keymap| {
            let input = key_input(key, keymap.profile());
            controller.search_key(code, keymap, input, Instant::now())
        });
        let outcome = searched.or_else(|| code.map(|code| controller.key(code, now)));
        if let Some(outcome) = outcome {
            commands.extend(self.apply(i, outcome));
        }
        commands
    }

    fn keyboard_focus(&mut self, i: usize, focused: bool) -> Vec<Do<Message>> {
        self.bars[i].focused = focused;
        if focused {
            let outcome = self.bars[i].controller.keyboard_arrived();
            return self.apply(i, outcome);
        }
        self.bars[i].alt = false;
        // the bar held the keyboard for the shortcut and lost it to something else
        if self.bars[i].controller.mode() == Mode::TitleFocus {
            let outcome = self.bars[i].controller.close();
            self.apply(i, outcome)
        } else {
            self.sync_alt(i)
        }
    }

    /// Open the join prompt for `ssid` on the bar `i`.
    fn open_prompt(&mut self, i: usize, ssid: String) {
        let link = &self.shared.extras.wifi_link;
        self.join = Some(JoinPrompt {
            bar: self.bars[i].id,
            ssid,
            show: false,
            text: Passphrase::new(""),
            phase: PromptPhase::Editing,
            joins_at_send: link.joins,
            failure_at_send: link.failure.clone(),
            answered: false,
            saw_busy: false,
            deadline: None,
        });
    }

    /// Something happened in the join prompt.
    fn prompt_event(&mut self, bar: BarId, event: PromptEvent) -> Vec<Do<Message>> {
        let Some(join) = self.join.as_mut().filter(|j| j.bar == bar) else {
            return Vec::new();
        };
        match event {
            PromptEvent::Typed(text) => {
                join.text = text;
                if matches!(join.phase, PromptPhase::Failed(_)) {
                    join.phase = PromptPhase::Editing;
                }
            }
            PromptEvent::Show(on) => {
                join.show = on;
                // the hidden field starts empty: nothing typed there is handed back to it
                if !on {
                    join.text = Passphrase::new("");
                }
            }
            PromptEvent::Join(text) => {
                if text.as_str().is_empty() || join.phase == PromptPhase::Joining {
                    return Vec::new();
                }
                join.text = text.clone();
                let link = &self.shared.extras.wifi_link;
                join.joins_at_send = link.joins;
                join.failure_at_send = link.failure.clone();
                join.answered = false;
                join.saw_busy = false;
                join.phase = PromptPhase::Joining;
                join.deadline = Some(Instant::now() + JOIN_TIME);
                let ssid = join.ssid.clone();
                return match &self.services.extras {
                    Some(extras) => extras.join(ssid, text).err().map_or_else(Vec::new, fail),
                    None => {
                        join.phase = PromptPhase::Failed(failure_line(""));
                        join.deadline = None;
                        Vec::new()
                    }
                };
            }
            PromptEvent::Cancel => {
                let joining = join.phase == PromptPhase::Joining;
                self.join = None;
                if joining {
                    return self.act(ExtraAction::CancelJoin);
                }
            }
        }
        Vec::new()
    }

    /// The service's state moved: see how far the join stands.
    pub(super) fn follow_join(&mut self) {
        let Some(join) = self.join.as_mut() else {
            return;
        };
        if join.phase != PromptPhase::Joining {
            return;
        }
        let link = &self.shared.extras.wifi_link;
        if link.joins > join.joins_at_send {
            if let Some(refused) = &link.refused {
                join.phase = PromptPhase::Failed(refused.clone());
                join.deadline = None;
                return;
            }
            join.answered = true;
        }
        if !join.answered {
            return;
        }
        if link.joined(&join.ssid) {
            self.join = None;
        } else if link.busy() {
            join.saw_busy = true;
        } else if !link.failure.is_empty()
            && (join.saw_busy || link.failure != join.failure_at_send)
        {
            join.phase = PromptPhase::Failed(failure_line(&link.failure));
            join.deadline = None;
        } else if join.saw_busy {
            join.phase = PromptPhase::Failed(failure_line(""));
            join.deadline = None;
        }
    }

    /// Show the alternates of the open menus' items while Alt is held on a surface of their bar.
    fn sync_alt(&mut self, i: usize) -> Vec<Do<Message>> {
        let bar = &mut self.bars[i];
        let held = bar.alt && bar.has_keyboard();
        let outcome = bar.controller.set_alt(held);
        if outcome.effects.is_empty() {
            Vec::new()
        } else {
            self.apply(i, outcome)
        }
    }

    /// Hand an action of a bar's own item to the extras' task; do not disturb goes over the host bus.
    fn act(&mut self, action: ExtraAction) -> Vec<Do<Message>> {
        match action {
            ExtraAction::SetDoNotDisturb(on) => {
                bus::publish(Published::SetDoNotDisturb { on, lifetime: None });
                Vec::new()
            }
            ExtraAction::DoNotDisturbFor(id) => {
                bus::publish(Published::SetDoNotDisturb {
                    on: true,
                    lifetime: Some(id),
                });
                Vec::new()
            }
            action => match &self.services.extras {
                Some(extras) => extras.act(action).err().map_or_else(Vec::new, fail),
                None => {
                    eprintln!("{PROCESS}: {action:?} dropped: the services are not read yet");
                    Vec::new()
                }
            },
        }
    }

    /// Give every bar the content its feed describes, load menus it needs and forget the rest.
    pub(super) fn refresh(&mut self) -> Vec<Do<Message>> {
        let mut commands = Vec::new();
        for i in 0..self.bars.len() {
            let content = self.bars[i].feed.content(&self.shared);
            if self.bars[i].controller.content() != &content {
                let outcome = self.bars[i].controller.set_content(content);
                commands.extend(self.apply(i, outcome));
            }
            if let Some(request) = self.bars[i].feed.request_layout(&mut self.shared) {
                commands.extend(self.request(request));
            }
        }
        let gone = self.shared.take_gone();
        if !gone.is_empty() {
            commands.extend(self.request(Request::ForgetRegistrations(gone)));
        }
        let in_use: Vec<_> = self
            .bars
            .iter()
            .map(|b| b.feed.source(&self.shared))
            .collect();
        for source in self.shared.prune_layouts(&in_use) {
            match source {
                crate::source::MenuSource::Kde(menu) => {
                    commands.extend(self.request(Request::ReleaseDbusMenu(menu)));
                }
                crate::source::MenuSource::Gtk(props) => {
                    commands.extend(self.request(Request::ReleaseGtk(props)));
                }
                crate::source::MenuSource::Synthetic => {}
            }
        }
        commands
    }

    /// Carry out what the controller of bar `i` asked for, then bring the keyboard in line.
    pub(super) fn apply(&mut self, i: usize, outcome: Outcome) -> Vec<Do<Message>> {
        let actions: Vec<Action> = {
            let bar = &self.bars[i];
            let cx = Context {
                feed: &bar.feed,
                shared: &self.shared,
                layout: bar.controller.layout(),
                origin: (bar.geometry.x() as i32, bar.geometry.y() as i32),
            };
            outcome
                .effects
                .into_iter()
                .filter_map(|e| routing::resolve(e, &cx))
                .collect()
        };
        let (window, output) = (self.bars[i].id.window(), self.bars[i].output);
        let mut commands = Vec::new();
        for action in actions {
            match action {
                // the windows show the controller as it is after each update
                Action::Redraw => {}
                Action::SetMenuOpen(open) => {
                    if !open {
                        // the menus took the keyboard with them: an Alt released over them never reaches the bar
                        self.bars[i].alt = false;
                        self.bars[i].controller.set_alt(false);
                    }
                    menu_bar::set_menu_open(&window, open);
                }
                // a window the action opens takes the focus with the bar's activation token
                Action::Request(Request::ActivateGtk { props, item, .. }) => {
                    commands.push(Do::ActivationToken {
                        window: window.clone(),
                        app_id: props.application_id.clone(),
                        on_token: Box::new(move |token| {
                            Message::GtkToken(Box::new(props.clone()), item, token)
                        }),
                    });
                }
                Action::Request(request) => commands.extend(self.request(request)),
                Action::TogglePanel(panel) => bus::publish(Published::TogglePanel {
                    panel,
                    output: Some(output),
                }),
                Action::WindowCommand(command) => menu_bar::window_command(&window, command),
                Action::MoveToWorkspace(workspace) => {
                    menu_bar::move_to_workspace(&window, workspace);
                }
                Action::ActivateWindow(id) => {
                    menu_bar::activate_window(&window, ServerWindowId(id));
                }
                Action::Edit(action) => menu_bar::edit(&window, action),
                Action::ForceQuitApp(app_id) => menu_bar::force_quit_app(&window, &app_id),
                Action::Launch(launch) => commands.push(self.begin_launch(&window, launch)),
                Action::OpenDocument(file) => {
                    commands.push(self.open_document(window.clone(), file))
                }
                Action::SystemMenuOpening {
                    refresh_capabilities,
                } => {
                    commands.extend(self.read_recent_items());
                    if refresh_capabilities {
                        menu_bar::session_action(
                            &window,
                            client_session_action(SessionAction::RefreshCapabilities),
                        );
                    }
                }
                Action::ClearRecentItems => commands.extend(self.clear_recent_items()),
                Action::ShowAbout => {
                    eprintln!("{PROCESS}: about requested (there is no about panel yet)");
                }
                Action::Extra(ExtraAction::Session(action)) => {
                    menu_bar::session_action(&window, client_session_action(action));
                }
                Action::Extra(ExtraAction::PromptNetwork(ssid)) => self.open_prompt(i, ssid),
                Action::Extra(ExtraAction::ToggleWifiList) => {
                    self.shared.extras.wifi_list_open = !self.shared.extras.wifi_list_open;
                    commands.extend(self.refresh());
                }
                Action::Extra(action) => {
                    if matches!(
                        action,
                        ExtraAction::ScanWifi | ExtraAction::SetWifiEnabled(true)
                    ) {
                        self.shared.extras.wifi_scanning = true;
                        self.wifi_scan_until = Some(Instant::now() + WIFI_SCAN_TIME);
                        commands.extend(self.refresh());
                    }
                    commands.extend(self.act(action));
                }
            }
        }
        self.sync_keyboard(i);
        commands
    }

    /// The bar takes the keyboard exclusively while a title is focused by the shortcut (the server gives a layer the
    /// keyboard only for exclusive interactivity); open menus get it through their popup grab. The change applies with
    /// the bar's next frame, which the title's highlight brings.
    fn sync_keyboard(&mut self, i: usize) {
        let bar = &mut self.bars[i];
        bar.keyboard = if bar.controller.mode() == Mode::TitleFocus {
            Keyboard::Exclusive
        } else {
            Keyboard::None
        };
    }
}

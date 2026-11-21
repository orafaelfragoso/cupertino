//! What the server tells the dock roles, the outputs the dock moves between, the switcher's panels, the hot corners
//! and gestures, `dock.conf` as it changes, and where each window of the view goes ([`Placed`]).

use std::collections::HashSet;

use cupertino_defaults::dock::DockSettings;
use cupertino_kit::core::{Command, Key, WindowKey};
use cupertino_kit::geometry::Size;

use super::{DockEvent, IconKey, Message, Model, OutputInfo, Placement, Request, APP_GRID_ACTION};
use crate::actions::{ActionTarget, Trigger};
use crate::conf::dock_config;
use crate::config::Orientation;
use crate::switcher_kit::{SwitcherEntry, SwitcherInput, SwitcherRequest, SwitcherSurface};
use crate::view;

/// Where a window of the view goes: the layer surface the shell gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placed {
    /// The bar, placed as `placement` says, with the keyboard while `keyboard`; it holds the dock role of its output.
    Bar {
        placement: Placement,
        keyboard: bool,
    },
    /// The edge strip of the output with key `output`, along the dock's edge; it holds that output's dock role.
    Edge {
        output: u32,
        orientation: Orientation,
    },
    /// Over the whole output with key `output`: the tile the pointer holds and the removal puff.
    Drag { output: u32 },
    /// Centred on the output, `size` points large, with the keyboard: the Force Quit dialog and the trash's alerts.
    Dialog { output: u32, size: Size },
    /// Centred on the output, `size` points large, the keyboard left to the server: the switcher's panel.
    Panel { output: u32, size: Size },
}

impl Model {
    /// Where the window `key` of the view goes; `None` for a popup of the bar and a window the view does not show.
    pub fn placed(&self, key: &WindowKey) -> Option<Placed> {
        if self.bar_key().as_ref() == Some(key) {
            return Some(Placed::Bar {
                placement: self.placement()?,
                keyboard: self.engaged,
            });
        }
        let orientation = self.session.dock.cfg.orientation;
        if let Some(output) = self
            .outputs
            .iter()
            .map(|o| o.key)
            .find(|&o| view::edge::key(o) == *key)
        {
            return Some(Placed::Edge {
                output,
                orientation,
            });
        }
        if *key == view::drag::key() {
            return Some(Placed::Drag {
                output: self.output?,
            });
        }
        if *key == view::force_quit::key() {
            let open = self.force_quit.as_ref()?;
            let (width, height) = open.layout.size();
            return Some(Placed::Dialog {
                output: open.output,
                size: Size::new(width, height),
            });
        }
        if let Some(output) = self.output.filter(|&o| view::confirm::key(o) == *key) {
            return Some(Placed::Dialog {
                output,
                size: view::confirm::WINDOW,
            });
        }
        let Key::Str(name) = &key.0 else {
            return None;
        };
        let (_, output) = name.rsplit_once('@')?;
        let output: u32 = output.parse().ok()?;
        let panel = self.switchers.get(&output)?.panel()?;
        Some(Placed::Panel {
            output,
            size: panel.size,
        })
    }

    /// The output whose dock role the window `key` holds: the bar's or an edge strip's.
    pub fn role_output(&self, key: &WindowKey) -> Option<u32> {
        match self.placed(key)? {
            Placed::Bar { placement, .. } => Some(placement.output),
            Placed::Edge { output, .. } => Some(output),
            _ => None,
        }
    }

    /// The pictures the bar's minimized tiles show: each tile's picture layer ([`view::bar::picture_layer`]) and the
    /// server's id of its window.
    pub fn pictures(&self) -> Vec<(u64, u32)> {
        self.session
            .dock
            .tiles()
            .into_iter()
            .filter(|t| t.kind == crate::tiles::TileKind::MinimizedWindow)
            .filter_map(|t| {
                Some((
                    view::bar::picture_layer(t.id.0),
                    u32::try_from(t.window?.0).ok()?,
                ))
            })
            .collect()
    }

    // ---- the window feed ------------------------------------------------------------------------------------

    pub(super) fn dock_events(
        &mut self,
        output: u32,
        events: Vec<DockEvent>,
    ) -> Vec<Command<Message>> {
        let now = self.now();
        let from_bar = Some(output) == self.output;
        if from_bar && std::mem::take(&mut self.awaiting_list) {
            // a new bar role: its first group lists every window, so one that closed meanwhile is gone
            let listed = listed(&events);
            let known: Vec<u32> = self.session.window_ids();
            for id in known.into_iter().filter(|id| !listed.contains(id)) {
                self.host.window_closed(id);
                self.session.window_closed(id, now);
            }
        }
        let first_group = from_bar && self.awaiting_residents;
        let mut residents = std::collections::BTreeSet::new();
        let mut commands = Vec::new();
        for event in events {
            if event.is_shared() && !from_bar {
                continue;
            }
            match event {
                DockEvent::Window {
                    id,
                    app_id,
                    title,
                    minimized,
                } => {
                    self.opening.remove(&self.session.key_of(&app_id));
                    self.session.window(id, &app_id, &title, minimized, now)
                }
                DockEvent::Resident { app_id, resident } => {
                    if resident {
                        residents.insert(app_id.clone());
                    } else {
                        self.opening.remove(&self.session.key_of(&app_id));
                    }
                    self.session.set_resident(&app_id, resident, now);
                }
                DockEvent::WindowIcon(icon) => {
                    if let Some(w) = self.session.window_info(icon.id) {
                        let apps = [w.app_id.clone(), w.key.clone()];
                        self.icons_go_stale();
                        self.host.window_icon(apps, icon);
                    }
                }
                DockEvent::WindowClosed(id) => {
                    if self.session.window_info(id).is_some() {
                        self.icons_go_stale();
                    }
                    self.host.window_closed(id);
                    self.session.window_closed(id, now);
                }
                DockEvent::Launching(app_id) => self.session.launching(&app_id, now),
                DockEvent::ToggleAutoHide => self.session.dock.toggle_autohide(now),
                // an older server's forward of the app-grid action, which the launcher now answers
                DockEvent::ToggleAppGrid => {}
                DockEvent::Switcher {
                    output,
                    apps,
                    selected,
                } => self.switcher_changed(output, apps, selected),
                DockEvent::SwitcherClosed { committed } => self.switcher_closed(committed),
                DockEvent::Trigger(trigger) => commands.extend(self.run_trigger(output, trigger)),
                // a refused activation: the tile hops once (informational attention)
                DockEvent::Attention(app_id) => self.session.attention(&app_id, now),
                DockEvent::Unresponsive {
                    app_id,
                    unresponsive,
                } => {
                    if unresponsive {
                        self.unresponsive.insert(app_id);
                    } else {
                        self.unresponsive.remove(&app_id);
                    }
                    self.refresh_force_quit();
                }
                DockEvent::ForceQuitDialog => self.toggle_force_quit(),
                DockEvent::Engage if !self.locked => {
                    self.engaged = true;
                    self.focus_first = true;
                }
                DockEvent::Engage => {}
            }
        }
        if from_bar {
            if first_group {
                // the first group of a new role lists every resident application
                self.awaiting_residents = false;
                self.session.residents_listed(&residents, now);
            }
            self.session.list_taken();
            self.refresh_force_quit();
        }
        commands
    }

    /// The icons of the applications were loaded before a window's own icon changed: they are drawn until the new
    /// ones come, which load in the background.
    pub(super) fn icons_go_stale(&mut self) {
        let apps: Vec<IconKey> = self
            .icons
            .keys()
            .filter(|k| matches!(k, IconKey::App(_)))
            .cloned()
            .collect();
        for key in apps {
            if let Some(sides) = self.icons.remove(&key) {
                self.stale_icons.insert(key.clone(), sides);
            }
            self.requested.retain(|(k, _)| *k != key);
        }
    }

    // ---- moving between outputs ----------------------------------------------------------------------------

    /// Put the dock on the output with key `output`: what was under way on the bar ends. A bar that was on another
    /// output hands its role over first ([`Model::hand_off`]).
    pub(super) fn relocate(&mut self, output: u32) {
        if let Some(from) = self.output {
            self.hand_off(Some(from).filter(|&o| o != output && self.output_info(o).is_some()));
        }
        self.output = Some(output);
        self.awaiting_list = true;
        self.awaiting_residents = true;
        self.menu = None;
        self.press = None;
        self.poof = None;
        self.pointer = None;
        self.engaged = false;
        self.edge = None;
        self.session.dock.cancel_drag();
    }

    /// The next view leaves the bar out, and the strip of `left`, the output the bar leaves: the server allows one
    /// dock role per output, and Kit opens a new window before it closes the one it replaces. The view after
    /// ([`Message::HandedOff`]) shows them where they go.
    pub(super) fn hand_off(&mut self, left: Option<u32>) {
        self.handoff = true;
        self.left = left;
        self.later
            .push(Command::Perform(Box::pin(std::future::ready(
                Message::HandedOff,
            ))));
    }

    pub(super) fn outputs_changed(&mut self, outputs: Vec<OutputInfo>) {
        self.outputs = outputs;
        let first = self.outputs.first().map(|o| o.key);
        match self.output {
            Some(o) if self.output_info(o).is_some() => {}
            _ => match first {
                Some(o) => self.relocate(o),
                None => self.output = None,
            },
        }
        let there = |o: u32, outputs: &[OutputInfo]| outputs.iter().any(|i| i.key == o);
        if self
            .force_quit
            .as_ref()
            .is_some_and(|f| !there(f.output, &self.outputs))
        {
            self.force_quit = None;
        }
        // the panel of an output that went away goes with it; a new output gets its own
        self.switchers.retain(|&o, _| there(o, &self.outputs));
        for out in &self.outputs {
            self.switchers.entry(out.key).or_default();
        }
        if self.edge.is_some_and(|(o, _)| !there(o, &self.outputs)) {
            self.edge = None;
        }
    }

    /// The compositor closed a window the model still shows.
    pub(super) fn closed(&mut self, key: WindowKey) {
        if Some(&key) == self.bar_key().as_ref() {
            self.bar_generation += 1;
            self.hand_off(None);
            self.awaiting_list = true;
            self.awaiting_residents = true;
            self.menu = None;
            self.press = None;
            self.engaged = false;
        } else if key == view::force_quit::key() {
            self.force_quit = None;
        } else if Some(&key) == self.output.map(view::confirm::key).as_ref() {
            self.confirm = None;
        }
    }

    // ---- the application switcher --------------------------------------------------------------------------

    /// The server's list and selection for the panel on `output`: a step of the switch on screen moves the selection,
    /// anything else is a new switch, which ends one still waiting on another output.
    pub(super) fn switcher_changed(&mut self, output: u32, apps: Vec<String>, selected: usize) {
        let now = self.now();
        let Some(out) = self.output_info(output) else {
            eprintln!("cupertino-dock: the output of the switcher went away");
            return;
        };
        self.learn_names(apps.iter().map(String::as_str));
        let entries = apps
            .into_iter()
            .map(|app_id| SwitcherEntry {
                name: self
                    .entry_name(&app_id)
                    .map_or_else(|| app_id.clone(), str::to_owned),
                app_id,
            })
            .collect();
        for (&o, switcher) in self.switchers.iter_mut().filter(|(&o, _)| o != output) {
            if switcher.output() == Some(o) && !switcher.is_done(now) {
                switcher.closed(false, now);
            }
        }
        let Some(switcher) = self.switchers.get_mut(&output) else {
            return;
        };
        if let Err(e) = switcher.changed(output, (out.width, out.height), entries, selected, now) {
            eprintln!("cupertino-dock: {e}");
        }
    }

    /// The switch ended. A panel that was on screen fades out after a commit; a cancel, or a commit before the panel
    /// appeared, removes it at once.
    pub(super) fn switcher_closed(&mut self, committed: bool) {
        let now = self.now();
        for switcher in self.switchers.values_mut() {
            switcher.closed(committed, now);
        }
    }

    /// What the panel on `output` reported; its requests go through the dock role of that output.
    pub(super) fn switcher_input(&mut self, output: u32, input: SwitcherInput) {
        let now = self.now();
        let Some(switcher) = self.switchers.get_mut(&output) else {
            return;
        };
        let requests = switcher.input(input, now);
        let role = self.role(output);
        for request in requests {
            self.host.request(
                &role,
                match request {
                    SwitcherRequest::Select(i) => Request::SwitcherSelect(i),
                    SwitcherRequest::Activate(i) => Request::SwitcherActivate(i),
                    SwitcherRequest::Cancel => Request::SwitcherCancel,
                },
            );
        }
    }

    /// The switcher's panel of every output.
    pub(super) fn switchers(&self) -> impl Iterator<Item = (u32, &SwitcherSurface)> {
        self.switchers.iter().map(|(&o, s)| (o, s))
    }

    // ---- hot corners and gestures ---------------------------------------------------------------------------

    pub(super) fn run_trigger(&mut self, output: u32, trigger: Trigger) -> Vec<Command<Message>> {
        let role = self.role(output);
        match self.actions.get(trigger).clone() {
            ActionTarget::None => {}
            // the launcher's application browser, which the server asks for
            ActionTarget::AppGrid => self
                .host
                .request(&role, Request::RunAction(APP_GRID_ACTION.to_owned())),
            ActionTarget::Exec(command) => self.host.shell(&command),
            ActionTarget::Server(name) => self.host.request(&role, Request::RunAction(name)),
        }
        Vec::new()
    }

    // ---- dock.conf -------------------------------------------------------------------------------------------

    /// Apply the keys whose value differs between the file as it was and as it is; a key removed from the file goes
    /// back to its default. The pins of `dock.conf` apply at the next start (the dock keeps the pins the user changes).
    pub(super) fn conf_changed(&mut self, after: DockSettings) {
        let before = std::mem::replace(&mut self.conf, after.clone());
        let height = self
            .output
            .and_then(|o| self.output_info(o))
            .map_or(0.0, |o| o.height);
        let target = dock_config(Some(&after), height);
        let now = self.now();
        let cfg = &mut self.session.dock.cfg;
        if before.tile_size != after.tile_size {
            cfg.tile_size = target.tile_size;
        }
        if before.large_size != after.large_size {
            cfg.large_size = target.large_size;
        }
        if before.magnification != after.magnification {
            cfg.magnification = target.magnification;
        }
        if before.position != after.position {
            cfg.orientation = target.orientation;
        }
        if before.recents != after.recents {
            cfg.show_recents = target.show_recents;
            self.recents_basis = None;
        }
        if before.minimize_to_app != after.minimize_to_app {
            cfg.minimize_to_app = target.minimize_to_app;
        }
        if before.bounce != after.bounce {
            cfg.launch_animation = target.launch_animation;
        }
        if before.indicators != after.indicators {
            cfg.show_running_indicators = target.show_running_indicators;
        }
        if before.autohide != after.autohide && cfg.autohide != target.autohide {
            self.session.dock.toggle_autohide(now);
        }
        if before.actions != after.actions {
            self.actions = after.actions;
        }
    }
}

/// The keys an update of the role's first group lists, for telling a window that closed meanwhile.
fn listed(events: &[DockEvent]) -> HashSet<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            DockEvent::Window { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

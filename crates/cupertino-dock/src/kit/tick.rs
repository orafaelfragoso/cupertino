//! After every message: the icons the next view draws, the bar's focus, the trash's state, the recents, what the
//! shell has to know, and the next wake.

use cupertino_kit::core::Command;

use super::{
    IconKey, Message, Model, Request, EDGE_DWELL, FORCE_QUIT_ROW_ICON, FRAME_FALLBACK,
    MENU_FADE_OUT, TOKEN_TIMEOUT, TRASH_CHECK_EVERY,
};
use crate::fan;
use crate::force_quit;
use crate::stack::{self, StackLayout};
use crate::switcher_kit::SwitcherSurface;
use crate::tiles::{TileId, TileKind};
use crate::view::{self, IconSet};

impl Model {
    /// The icon of `key` at every size it is drawn at: those the host has at hand at once, the others gathered for
    /// the next load in the background ([`Model::preload`]).
    pub(super) fn icon_set(&mut self, key: &IconKey, sides: &[f64]) {
        for &side in sides {
            let px = side.ceil().max(1.0) as u32;
            let known = self.icons.get(key).is_some_and(|k| k.contains_key(&px))
                || self.requested.contains(&(key.clone(), px))
                || self.wanted.iter().any(|(k, p)| k == key && *p == px);
            if known {
                continue;
            }
            match self.host.icon(self.session.desktop(), key, f64::from(px)) {
                Some(renditions) => {
                    self.icons
                        .entry(key.clone())
                        .or_default()
                        .insert(px, renditions);
                }
                None => self.wanted.push((key.clone(), px)),
            }
        }
    }

    /// The loaded renditions of `key` at `sides`; while they load after a window's icon changed, the ones before.
    pub(super) fn icon(&self, key: &IconKey, sides: &[f64]) -> IconSet {
        let Some(loaded) = self.icons.get(key).or_else(|| self.stale_icons.get(key)) else {
            return IconSet::new();
        };
        sides
            .iter()
            .filter_map(|&s| loaded.get(&(s.ceil().max(1.0) as u32)))
            .flat_map(|set| set.iter().cloned())
            .collect()
    }

    /// The sides the tiles are drawn at: resting, and fully magnified. While the magnified side loads, a magnified
    /// tile draws the resting side's largest rendition.
    pub(super) fn drawn_sides(&self) -> Vec<f64> {
        let cfg = &self.session.dock.cfg;
        vec![cfg.tile_size, cfg.large_size.max(cfg.tile_size)]
    }

    /// Ask the host to load in the background the icons the view wants ([`Model::icon_set`]), each at each side once.
    pub(super) fn preload(&mut self) -> Vec<Command<Message>> {
        if self.wanted.is_empty() {
            return Vec::new();
        }
        let asked = std::mem::take(&mut self.wanted);
        let jobs: Vec<(IconKey, f64)> = asked
            .iter()
            .map(|(k, px)| (k.clone(), f64::from(*px)))
            .collect();
        match self.host.preload(self.session.desktop(), jobs) {
            Some(load) => {
                self.requested.extend(asked.iter().cloned());
                vec![Command::Perform(Box::pin(async move {
                    Message::IconsLoaded {
                        asked,
                        loaded: load.await,
                    }
                }))]
            }
            // a host that loads nothing: the icons stay empty, and are not asked for again
            None => {
                for (key, px) in asked {
                    self.icons.entry(key).or_default().entry(px).or_default();
                }
                Vec::new()
            }
        }
    }

    /// Icons loaded in the background arrived; one the host could not load stays empty, and is not asked for again.
    pub(super) fn icons_loaded(
        &mut self,
        asked: Vec<(IconKey, u32)>,
        loaded: Vec<(IconKey, u32, IconSet)>,
    ) {
        for (key, px, set) in loaded {
            self.stale_icons.remove(&key);
            self.icons.entry(key).or_default().insert(px, set);
        }
        for (key, px) in asked {
            self.requested.remove(&(key.clone(), px));
            self.icons.entry(key).or_default().entry(px).or_default();
        }
    }

    /// The icon key of the tile with `id`.
    pub(super) fn tile_icon(&self, id: TileId) -> Option<IconKey> {
        let tile = self.session.dock.tile(id)?;
        Some(match tile.kind {
            TileKind::Trash => IconKey::Trash(self.trash_full),
            TileKind::Folder => IconKey::Named(
                self.folder_icons
                    .get(&tile.key)
                    .cloned()
                    .unwrap_or_else(|| stack::FOLDER_ICON.to_owned()),
            ),
            _ => IconKey::App(tile.key.clone()),
        })
    }

    /// Load what the next view draws, and the names it shows, so the view only reads.
    pub(super) fn prefill(&mut self) {
        let tiles: Vec<(TileId, TileKind, String)> = self
            .session
            .dock
            .tiles()
            .iter()
            .map(|t| (t.id, t.kind, t.key.clone()))
            .collect();
        self.learn_names(tiles.iter().map(|t| t.2.as_str()));
        let sides = self.drawn_sides();
        let mut keys: Vec<IconKey> = tiles
            .iter()
            .filter(|t| t.1 != TileKind::Separator)
            .filter_map(|t| self.tile_icon(t.0))
            .collect();
        keys.dedup();
        for key in keys {
            self.icon_set(&key, &sides);
        }
        if let Some(open) = &self.force_quit {
            let fq: Vec<IconKey> = open
                .dialog
                .entries()
                .iter()
                .map(|e| IconKey::App(e.app_id.clone()))
                .collect();
            for key in &fq {
                self.icon_set(key, &[FORCE_QUIT_ROW_ICON, force_quit::ICON_SIZE]);
            }
        }
        let switched: Vec<(String, f64)> = self
            .switchers
            .values()
            .filter_map(|s| Some((s.apps()?, s.icon_side()?)))
            .flat_map(|(apps, side)| apps.iter().map(move |a| (a.clone(), side)))
            .collect();
        for (app, side) in switched {
            self.icon_set(&IconKey::App(app), &[side]);
        }
        if let Some(entries) = self.stack.as_ref().and_then(|s| s.entries.as_ref()) {
            let keys: Vec<IconKey> = entries
                .iter()
                .map(|e| IconKey::Named(e.icon.clone()))
                .collect();
            for key in keys {
                self.icon_set(&key, &[StackLayout::ICON]);
            }
        }
    }

    /// The bar's keyboard focus follows the model: Focus Dock focuses the first tile, with its ring; a focused tile
    /// that went away hands the focus to its neighbour (the next one, else the one before), never to the bar.
    pub(super) fn focus_commands(&mut self) -> Vec<Command<Message>> {
        let order: Vec<TileId> = self
            .session
            .dock
            .tiles()
            .iter()
            .filter(|t| t.kind != TileKind::Separator)
            .map(|t| t.id)
            .collect();
        let before = std::mem::replace(&mut self.tile_order, order);
        let target = if self.engaged && std::mem::take(&mut self.focus_first) {
            self.tile_order.first().copied()
        } else {
            match self.focused_tile {
                Some(gone) if !self.tile_order.contains(&gone) => {
                    let at = before.iter().position(|&t| t == gone).unwrap_or(0);
                    before[at..]
                        .iter()
                        .chain(before[..at].iter().rev())
                        .copied()
                        .find(|t| self.tile_order.contains(t))
                        .or_else(|| self.tile_order.first().copied())
                }
                _ => None,
            }
        };
        self.focus_first &= self.engaged;
        let Some(tile) = target else {
            return Vec::new();
        };
        self.focused_tile = Some(tile);
        vec![Command::Focus(view::bar::tile_key(tile.0))]
    }

    /// Advance the animations, carry out what is due and tell the host what changed; then schedule the next wake.
    pub(super) fn changed(&mut self) -> Vec<Command<Message>> {
        let now = self.now();
        // the locked session, the Force Quit dialog and the trash's alert hold the keyboard while they are up: a Focus
        // Dock meanwhile is dropped, not kept for when they go
        if self.locked || self.force_quit.is_some() || self.confirm.is_some() {
            self.engaged = false;
        }
        // an auto-hidden bar shows while it has the keyboard or one of its menus or stacks is open
        self.session.dock.keep_shown = self.engaged || self.stack.is_some() || self.menu.is_some();
        if let Some(screen) = self.screen() {
            self.session.dock.pointer_moved(self.pointer, screen, now);
        }
        self.session.dock.tick(now);
        let mut trash_look = None;
        if self
            .trash_checked
            .is_none_or(|at| now - at >= TRASH_CHECK_EVERY)
        {
            self.trash_checked = Some(now);
            let look = self.host.trash_full();
            // a host that cannot look has an empty trash
            self.trash_looked |= look.is_empty();
            self.later.extend(look);
            trash_look = self.host.look_at_trash();
        }
        // a press held in place on a tile opens its menu; the press is then neither a click nor a drag
        if let Some(id) = self
            .press
            .as_mut()
            .and_then(|p| p.held(now).then_some(p.id))
        {
            self.press = None;
            self.open_menu(id);
        }
        for switcher in self.switchers.values_mut() {
            switcher.tick(now);
        }
        if self.poof.is_some_and(|p| p.progress(now).is_none()) {
            self.poof = None;
        }
        if let Some((output, since)) = self.edge {
            if now - since >= EDGE_DWELL
                && Some(output) != self.output
                && !self.session.dock.is_dragging()
                && !self.locked
            {
                self.relocate(output);
            }
        }
        let mut commands = self.expire_tokens(now);
        commands.extend(trash_look.map(|look| {
            Command::Perform(Box::pin(async move { Message::TrashLooked(look.await) }))
        }));
        self.ask_folders();
        if self
            .stack
            .as_ref()
            .is_some_and(|s| self.session.dock.tile(s.tile).is_none())
            || self
                .stack
                .as_ref()
                .and_then(|s| s.closing)
                .is_some_and(|c| now >= c.start + fan::CLOSE_S)
        {
            self.stack = None;
        }
        self.refresh_recents(now);
        self.tell_host();
        // what the next view draws, and the icons of it not at hand asked for at once
        self.prefill();
        commands.extend(self.preload());
        commands.extend(self.focus_commands());
        commands.extend(self.schedule(now));
        commands
    }

    /// The bar's recents section: the applications that quit since the dock started, then the launcher's history, each
    /// without a tile among the applications, at most [`crate::config::DockConfig::recents_max`]. Worked out again, the
    /// history read again, only when an application gets or loses its tile or one quits.
    pub(super) fn refresh_recents(&mut self, now: f64) {
        if !self.session.dock.cfg.show_recents {
            return;
        }
        let mut basis: Vec<String> = self
            .session
            .dock
            .tiles()
            .iter()
            .filter(|t| t.kind == TileKind::App && !t.is_leaving())
            .map(|t| t.key.clone())
            .collect();
        basis.push(String::new());
        basis.extend(self.session.recently_quit().iter().cloned());
        if self.recents_basis.as_ref() == Some(&basis) {
            return;
        }
        self.recents_basis = Some(basis);
        let read = self.host.recent_apps();
        self.recents_reading += read.len();
        if read.is_empty() {
            // a host without the launcher's history: the recents are the applications that quit
            let recents = self.session.recent_apps(&[]);
            let keys: Vec<&str> = recents.iter().map(String::as_str).collect();
            self.session.dock.set_recents(&keys, now);
        }
        self.later.extend(read);
    }

    /// Follow what the shell has to know: a change of the bar's placement or keyboard makes the bar present again,
    /// so its surface takes it; the slot the next minimized window flies to goes to the server through the bar.
    pub(super) fn tell_host(&mut self) {
        let placement = self.placement();
        if placement.is_some() && placement != self.placed {
            self.placed = placement;
            self.surface_changes += 1;
        }
        if self.keyboard != self.engaged {
            self.keyboard = self.engaged;
            self.surface_changes += 1;
        }
        let target = match (self.bar_key(), self.screen()) {
            (Some(bar), Some(screen)) if !self.handoff => {
                Some((bar, self.session.dock.next_minimized_slot(screen)))
            }
            _ => None,
        };
        if let Some((bar, slot)) = target.filter(|t| Some(t) != self.minimize_target.as_ref()) {
            self.host.request(&bar, Request::MinimizeTarget(slot));
            self.minimize_target = Some((bar, slot));
        }
    }

    /// Whether the bar moves by itself: a bounce, the magnification, auto-hide, a drag settling or the removal puff.
    pub(super) fn bar_moving(&self, now: f64) -> bool {
        self.session.dock.wants_tick(now) || self.poof.is_some() || self.menu_fading(now).is_some()
    }

    /// The tile whose closed menu still fades out now, and its darkened icon.
    pub(super) fn menu_fading(&self, now: f64) -> Option<(TileId, &IconSet)> {
        self.menu_fade
            .as_ref()
            .filter(|(_, _, closed)| now - closed < MENU_FADE_OUT)
            .map(|(id, dark, _)| (*id, dark))
    }

    /// The darkened icon a tile shows now: the one the button is held on, or the one whose menu is up or fading out.
    pub(super) fn pressed_icon(&self, id: TileId, now: f64) -> Option<&IconSet> {
        let held = self
            .held
            .as_ref()
            .filter(|(h, dark)| *h == id && !dark.is_empty());
        let menu = self
            .menu
            .as_ref()
            .filter(|m| m.tile == id && !m.dark.is_empty());
        held.map(|(_, dark)| dark).or(menu.map(|m| &m.dark)).or(self
            .menu_fading(now)
            .filter(|(t, d)| *t == id && !d.is_empty())
            .map(|(_, d)| d))
    }

    /// Whether the switcher's panel of an output moves by itself now (it wants the next frame).
    pub(super) fn switcher_moving(switcher: &SwitcherSurface, now: f64) -> bool {
        let soon = |at: Option<f64>| at.is_some_and(|at| at - now <= FRAME_FALLBACK);
        // the switcher's wake is its delay until it shows, or the next frame of its fade
        soon(switcher.next_wake(now)) && switcher.is_shown(now)
    }

    /// Whether anything moves on its own now.
    pub(super) fn animating(&self, now: f64) -> bool {
        self.bar_moving(now)
            || self
                .switchers
                .values()
                .any(|s| Self::switcher_moving(s, now))
    }

    /// While something moves its window's frames drive it, with a fallback wake; otherwise one wake at the next thing
    /// that is due.
    pub(super) fn schedule(&mut self, now: f64) -> Vec<Command<Message>> {
        let panel = self
            .switchers
            .values()
            .filter(|s| !s.is_shown(now))
            .filter_map(|s| s.next_wake(now))
            .map(|at| (at - now).max(0.0));
        let hold = self.press.as_ref().and_then(|p| p.hold_due(now));
        let edge = self
            .edge
            .filter(|(o, _)| Some(*o) != self.output)
            .map(|(_, since)| (since + EDGE_DWELL - now).max(0.0));
        let moving = (self.animating(now) || self.stack_moving(now)).then_some(FRAME_FALLBACK);
        let token = self
            .pending
            .values()
            .map(|p| (p.asked + TOKEN_TIMEOUT - now).max(0.0))
            .reduce(f64::min);
        let due = panel
            .chain(hold)
            .chain(edge)
            .chain(moving)
            .chain(token)
            .reduce(f64::min);
        let Some(due) = due else {
            return Vec::new();
        };
        let at = now + due;
        if self.wake_at.is_some_and(|w| w <= at + 1e-3) {
            return Vec::new();
        }
        self.wake_generation += 1;
        self.wake_at = Some(at);
        self.host.wake(due, self.wake_generation)
    }
}

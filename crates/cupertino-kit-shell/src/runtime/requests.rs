//! The requests a program queued ([`ShellRequest`]), sent on its objects before its loop next waits: the session's,
//! each role's, transactions', the lock's, consent's and captures'.

use cupertino_kit::core::WindowKey;
use cupertino_protocols::client::cupertino_window_manager_v1::Placement as WirePlacement;
use cupertino_shell_client::{ConsentAnswer, ControlAuthority};
use cupertino_wayland_client::{Connection, SurfaceId};

use super::capture::Key;
use super::protocols::{wire_gesture, TransactionData, WindowsData};
use super::roles::{wire_output, ShellRoles};
use crate::context::ShellRequest;
use crate::window_manager::Placement;
use crate::{consent, transaction};

/// The longest idle time a watch is asked for, in the protocol's milliseconds.
const MAX_IDLE_MILLIS: u32 = u32::MAX;

impl ShellRoles {
    /// Sends `request` on the object it is for; one for an object the program does not have is logged and dropped.
    pub(super) fn execute(&mut self, conn: &mut Connection, request: ShellRequest) {
        use ShellRequest as R;
        match request {
            R::SessionActivateApp { .. }
            | R::SessionWatchIdle { .. }
            | R::SessionUnwatchIdle { .. } => {
                self.session_request(conn, request);
            }
            R::MenuBarSetOpen { .. }
            | R::MenuBarWindowCommand { .. }
            | R::MenuBarActivateWindow { .. }
            | R::MenuBarMoveToWorkspace { .. }
            | R::MenuBarSessionAction { .. }
            | R::MenuBarEdit { .. }
            | R::MenuBarForceQuitApp { .. } => self.menu_bar_request(request),
            R::DockActivateApp { .. }
            | R::DockActivateWindow { .. }
            | R::DockRestoreWindow { .. }
            | R::DockSetMinimizeTarget { .. }
            | R::DockForceQuitApp { .. }
            | R::DockAppCommand { .. }
            | R::DockRunAction { .. }
            | R::DockSwitcherSelect { .. }
            | R::DockSwitcherActivate { .. }
            | R::DockSwitcherCancel { .. }
            | R::DockSwitchWorkspace { .. }
            | R::DockAddWorkspace { .. }
            | R::DockRemoveWorkspace { .. }
            | R::DockMoveWorkspace { .. } => self.dock_request(request),
            R::WindowManagerPlace { .. }
            | R::WindowManagerCommit { .. }
            | R::WindowManagerActivateWindow { .. }
            | R::OverviewActivateWindow { .. }
            | R::OverviewSwitchWorkspace { .. }
            | R::OverviewAddWorkspace { .. }
            | R::OverviewRemoveWorkspace { .. }
            | R::OverviewMoveWorkspace { .. }
            | R::OverviewMoveWindow { .. } => self.arrangement_request(request),
            R::TxCreate { .. }
            | R::TxJoin { .. }
            | R::TxExpect { .. }
            | R::TxDeadline { .. }
            | R::TxStage { .. }
            | R::TxBeginWindowChanges { .. }
            | R::TxEndWindowChanges { .. }
            | R::TxBindGesture { .. }
            | R::TxBindReverseGesture { .. }
            | R::TxSubmit { .. }
            | R::TxCancel { .. }
            | R::TxDestroy { .. } => self.transaction_request(conn, request),
            R::LockRequest | R::LockUnlock => self.lock_request(conn, request),
            R::AccentPick { serial, index } => match &self.keyboard_panels {
                Some(panels) => panels.accent_pick(serial, index),
                None => tracing::warn!(serial, "a pick without the keyboard panels' object"),
            },
            R::ConsentBind
            | R::ConsentSetWindow { .. }
            | R::ConsentAnswer { .. }
            | R::ConsentStop { .. } => self.consent_request(conn, request),
            R::CaptureWindows
            | R::CaptureOnce { .. }
            | R::CaptureFollowStart { .. }
            | R::CaptureFollowStop { .. } => self.capture_request(conn, request),
        }
    }

    fn session_request(&mut self, conn: &Connection, request: ShellRequest) {
        match request {
            ShellRequest::SessionActivateApp { serial, desktop_id } => {
                let asked = self
                    .globals
                    .session
                    .as_ref()
                    .is_some_and(|session| session.activate_app(serial, &desktop_id));
                if !asked {
                    tracing::warn!(serial, "the server cannot bring an application forward");
                }
            }
            ShellRequest::SessionWatchIdle { id, after } => {
                let (Some(idle), Some(seat)) = (&self.globals.idle, conn.seat()) else {
                    tracing::warn!(id, "the server offers no idle notice");
                    return;
                };
                let millis = u32::try_from(after.as_millis()).unwrap_or(MAX_IDLE_MILLIS);
                self.idle_watches
                    .insert(id, idle.watch(conn, &seat, millis));
            }
            ShellRequest::SessionUnwatchIdle { id } => {
                self.idle_watches.remove(&id);
            }
            _ => {}
        }
    }

    fn menu_bar_request(&mut self, request: ShellRequest) {
        use ShellRequest as R;
        let window = match &request {
            R::MenuBarSetOpen { window, .. }
            | R::MenuBarWindowCommand { window, .. }
            | R::MenuBarActivateWindow { window, .. }
            | R::MenuBarMoveToWorkspace { window, .. }
            | R::MenuBarSessionAction { window, .. }
            | R::MenuBarEdit { window, .. }
            | R::MenuBarForceQuitApp { window, .. } => window.clone(),
            _ => return,
        };
        let Some(bar) = self.win_of_key(&window).and_then(|w| w.menu_bar.as_ref()) else {
            tracing::warn!(
                ?window,
                "a menu bar request for a window that is no menu bar"
            );
            return;
        };
        match request {
            R::MenuBarSetOpen { open, .. } => bar.set_menu_open(open),
            R::MenuBarWindowCommand { command, .. } => bar.window_command(command),
            R::MenuBarActivateWindow { id, .. } => bar.activate_window(id.0),
            R::MenuBarMoveToWorkspace { workspace, .. } => bar.move_to_workspace(workspace),
            R::MenuBarSessionAction { action, .. } => bar.session_action(action),
            R::MenuBarEdit { action, .. } => bar.edit(action),
            R::MenuBarForceQuitApp { app_id, .. } => bar.force_quit_app(&app_id),
            _ => {}
        }
    }

    fn dock_request(&mut self, request: ShellRequest) {
        use ShellRequest as R;
        let window = match &request {
            R::DockActivateApp { window, .. }
            | R::DockActivateWindow { window, .. }
            | R::DockRestoreWindow { window, .. }
            | R::DockSetMinimizeTarget { window, .. }
            | R::DockForceQuitApp { window, .. }
            | R::DockAppCommand { window, .. }
            | R::DockRunAction { window, .. }
            | R::DockSwitcherSelect { window, .. }
            | R::DockSwitcherActivate { window }
            | R::DockSwitcherCancel { window }
            | R::DockSwitchWorkspace { window, .. }
            | R::DockAddWorkspace { window }
            | R::DockRemoveWorkspace { window, .. }
            | R::DockMoveWorkspace { window, .. } => window.clone(),
            _ => return,
        };
        let Some(win) = self.win_of_key(&window).filter(|w| w.dock.is_some()) else {
            tracing::warn!(?window, "a dock request for a window that is no dock");
            return;
        };
        let Some(dock) = win.dock.as_ref() else {
            return;
        };
        let shaped = match request {
            R::DockActivateApp { app_id, .. } => {
                dock.activate_app(&app_id);
                Ok(())
            }
            R::DockActivateWindow { id, .. } => {
                if !dock.activate_window(id.0) {
                    tracing::warn!(id = id.0, "the server cannot show one window");
                }
                Ok(())
            }
            R::DockRestoreWindow { id, .. } => {
                let rect = win.minimize_targets.get(&id.0).copied().unwrap_or_default();
                dock.restore_window(id.0, &rect)
            }
            R::DockSetMinimizeTarget { id, rect, .. } => {
                win.minimize_targets.insert(id.0, rect);
                dock.set_minimize_target(&rect)
            }
            R::DockForceQuitApp { app_id, .. } => {
                dock.force_quit_app(&app_id);
                Ok(())
            }
            R::DockAppCommand {
                app_id, command, ..
            } => {
                dock.app_command(&app_id, command);
                Ok(())
            }
            R::DockRunAction { action, .. } => {
                dock.run_action(&action);
                Ok(())
            }
            R::DockSwitcherSelect { index, .. } => {
                win.switcher = index as usize;
                dock.switcher_select(index as usize)
            }
            R::DockSwitcherActivate { .. } => dock.switcher_activate(win.switcher),
            R::DockSwitcherCancel { .. } => {
                dock.switcher_cancel();
                Ok(())
            }
            R::DockSwitchWorkspace { workspace, .. } => {
                dock.switch_workspace(workspace.0);
                Ok(())
            }
            R::DockAddWorkspace { .. } => {
                dock.add_workspace();
                Ok(())
            }
            R::DockRemoveWorkspace { workspace, .. } => {
                dock.remove_workspace(workspace.0);
                Ok(())
            }
            R::DockMoveWorkspace {
                workspace, index, ..
            } => dock.move_workspace(workspace.0, index as usize),
            _ => Ok(()),
        };
        if let Err(e) = shaped {
            tracing::warn!(?window, error = %e, "a dock request the protocol cannot carry");
        }
    }

    fn arrangement_request(&mut self, request: ShellRequest) {
        use ShellRequest as R;
        match request {
            R::WindowManagerPlace {
                output,
                window,
                placement,
            } => {
                if let Some(wm) = self.window_managers.get(&output) {
                    let placement = match placement {
                        Placement::Stage => WirePlacement::Stage,
                        Placement::Hidden => WirePlacement::Hidden,
                    };
                    wm.set_placement(window.0, placement);
                    return;
                }
            }
            R::WindowManagerCommit { output } => {
                if let Some(wm) = self.window_managers.get(&output) {
                    wm.commit();
                    return;
                }
            }
            R::WindowManagerActivateWindow { output, window } => {
                if let Some(wm) = self.window_managers.get(&output) {
                    wm.activate_window(window.0);
                    return;
                }
            }
            R::OverviewActivateWindow { output, window } => {
                if let Some(overview) = self.overviews.get(&output) {
                    overview.activate_window(window.0);
                    return;
                }
            }
            R::OverviewSwitchWorkspace { output, workspace } => {
                if let Some(overview) = self.overviews.get(&output) {
                    overview.switch_workspace(workspace.0);
                    return;
                }
            }
            R::OverviewAddWorkspace { output } => {
                if let Some(overview) = self.overviews.get(&output) {
                    overview.add_workspace();
                    return;
                }
            }
            R::OverviewRemoveWorkspace { output, workspace } => {
                if let Some(overview) = self.overviews.get(&output) {
                    overview.remove_workspace(workspace.0);
                    return;
                }
            }
            R::OverviewMoveWorkspace {
                output,
                workspace,
                index,
            } => {
                if let Some(overview) = self.overviews.get(&output) {
                    overview.move_workspace(workspace.0, index);
                    return;
                }
            }
            R::OverviewMoveWindow {
                output,
                window,
                workspace,
            } => {
                if let Some(overview) = self.overviews.get(&output) {
                    overview.move_window(window.0, workspace.0);
                    return;
                }
            }
            _ => return,
        }
        tracing::warn!("a window manager or overview request for an output without the role");
    }

    fn transaction_request(&mut self, conn: &mut Connection, request: ShellRequest) {
        use ShellRequest as R;
        match request {
            R::TxCreate { id, output } | R::TxJoin { id, output } => {
                let joins = id.component != self.component.unwrap_or(crate::ComponentName::NONE)
                    || id.instance != crate::context::with_ctx(|ctx| ctx.instance).unwrap_or(0);
                let (Some(manager), Some(wl)) = (
                    self.globals.transactions.as_ref(),
                    conn.wl_output(wire_output(output)),
                ) else {
                    tracing::warn!(?id, "no transaction manager or no such output");
                    return;
                };
                let qh = conn.queue_handle();
                let object = if joins {
                    manager.join(
                        id.component.as_str().to_owned(),
                        id.instance,
                        id.serial,
                        &wl,
                        &qh,
                        TransactionData(id),
                    )
                } else {
                    manager.create(id.serial, &wl, &qh, TransactionData(id))
                };
                if let Some(old) = self.transactions.insert(id, object) {
                    old.destroy();
                }
            }
            R::TxStage { id, window } => match self.surface_of_key(&window) {
                Some(surface) => self.stage(conn, id, surface),
                None => self.pending_stages.push((id, window)),
            },
            R::TxDestroy { id } => {
                self.pending_stages.retain(|(staged, _)| *staged != id);
                if let Some(object) = self.transactions.remove(&id) {
                    object.destroy();
                }
            }
            request => self.transaction_part(request),
        }
    }

    /// The requests on a transaction part the program holds.
    fn transaction_part(&mut self, request: ShellRequest) {
        use ShellRequest as R;
        let id = match &request {
            R::TxExpect { id, .. }
            | R::TxDeadline { id, .. }
            | R::TxBeginWindowChanges { id }
            | R::TxEndWindowChanges { id }
            | R::TxBindGesture { id, .. }
            | R::TxBindReverseGesture { id, .. }
            | R::TxSubmit { id }
            | R::TxCancel { id } => *id,
            _ => return,
        };
        let Some(object) = self.transactions.get(&id) else {
            tracing::warn!(
                ?id,
                "a request on a transaction part the program does not hold"
            );
            return;
        };
        match request {
            R::TxExpect { component, .. } => object.expect(component.as_str().to_owned()),
            R::TxDeadline { deadline, .. } => {
                object.set_deadline(u32::try_from(deadline.as_millis()).unwrap_or(u32::MAX));
            }
            R::TxBeginWindowChanges { .. } => object.begin_window_changes(),
            R::TxEndWindowChanges { .. } => object.end_window_changes(),
            R::TxBindGesture {
                gesture,
                response,
                damping,
                ..
            } => object.bind_gesture(wire_gesture(gesture), response, damping),
            R::TxBindReverseGesture { gesture, .. } => {
                object.bind_reverse_gesture(wire_gesture(gesture));
            }
            R::TxSubmit { .. } => object.submit(),
            R::TxCancel { .. } => object.cancel(),
            _ => {}
        }
    }

    fn surface_of_key(&mut self, key: &WindowKey) -> Option<SurfaceId> {
        self.win_of_key(key).and_then(|w| w.surface)
    }

    /// Stages `surface`'s next commit in transaction `id`.
    fn stage(&self, conn: &Connection, id: transaction::Id, surface: SurfaceId) {
        let (Some(object), Ok(wl)) = (self.transactions.get(&id), conn.wl_surface(surface)) else {
            return;
        };
        object.stage(&wl);
    }

    /// What waited for `key`'s surface: its stages and its being the consent window, before its first commit.
    pub(super) fn stage_pending(&mut self, conn: &Connection, key: &WindowKey, surface: SurfaceId) {
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_stages)
            .into_iter()
            .partition(|(_, window)| window == key);
        self.pending_stages = later;
        for (id, _) in now {
            self.stage(conn, id, surface);
        }
        if self.consent_window.as_ref() == Some(key) {
            self.consent_window = None;
            if let Some(authority) = &self.globals.consent {
                let _ = authority.set_surface(conn, surface);
            }
        }
    }

    fn lock_request(&mut self, conn: &Connection, request: ShellRequest) {
        match request {
            ShellRequest::LockRequest => {
                let Some(manager) = self.globals.lock_manager.as_mut() else {
                    tracing::warn!("the server offers no session lock to this program");
                    self.inbox
                        .push(crate::Event::Lock(crate::lock::Event::Finished));
                    return;
                };
                if self.lock.is_none() {
                    self.lock = Some(manager.lock(conn));
                }
            }
            ShellRequest::LockUnlock => {
                if let Some(lock) = self.lock.take() {
                    lock.unlock();
                }
            }
            _ => {}
        }
    }

    fn consent_request(&mut self, conn: &mut Connection, request: ShellRequest) {
        match request {
            ShellRequest::ConsentBind => {
                if self.globals.consent.is_none() {
                    self.globals.consent = ControlAuthority::bind(conn);
                }
                if self.globals.consent.is_none() {
                    tracing::warn!("the server offers no consent authority to this program");
                }
            }
            ShellRequest::ConsentSetWindow { window } => match self.surface_of_key(&window) {
                Some(surface) => {
                    if let Some(authority) = &self.globals.consent {
                        let _ = authority.set_surface(conn, surface);
                    }
                }
                None => self.consent_window = Some(window),
            },
            ShellRequest::ConsentAnswer { run, answer } => {
                let (Some(authority), Some(&epoch)) =
                    (&self.globals.consent, self.consent_runs.get(&run.raw()))
                else {
                    return;
                };
                let answer = match answer {
                    consent::Answer::Allow => ConsentAnswer::Allow,
                    consent::Answer::Deny => ConsentAnswer::Deny,
                    consent::Answer::Dismiss => ConsentAnswer::Dismiss,
                };
                authority.answer(epoch, answer, conn.input_serial().unwrap_or(0));
            }
            ShellRequest::ConsentStop { run } => {
                let (Some(authority), Some(epoch)) =
                    (&self.globals.consent, self.consent_runs.remove(&run.raw()))
                else {
                    return;
                };
                authority.stop(epoch);
            }
            _ => {}
        }
    }

    fn capture_request(&mut self, conn: &mut Connection, request: ShellRequest) {
        match request {
            ShellRequest::CaptureWindows => {
                if self.globals.windows.is_none() {
                    self.globals.windows = conn.bind_extension(1..=1, WindowsData::default());
                }
                if self.globals.windows.is_none() {
                    tracing::warn!("the server offers no window list to this program");
                }
            }
            ShellRequest::CaptureOnce {
                id,
                source,
                options,
            } => self.captures.start(conn, Key::Once(id), source, options),
            ShellRequest::CaptureFollowStart {
                id,
                source,
                options,
            } => self.captures.start(conn, Key::Follow(id), source, options),
            ShellRequest::CaptureFollowStop { id } => self.captures.stop(Key::Follow(id)),
            _ => {}
        }
    }
}

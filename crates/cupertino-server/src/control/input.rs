use super::{protected_surface, session, Cupertino};
use cupertino_control_model::{Epoch, Input, InputSource, Operation, Transition};
use smithay::backend::input::{AxisRelativeDirection, AxisSource, ButtonState, KeyState};
use smithay::desktop::layer_map_for_output;
use smithay::input::keyboard::Keycode;
use smithay::input::pointer::MotionEvent;
use smithay::reexports::wayland_server::WEnum;
use smithay::utils::{Point, SERIAL_COUNTER};

/// Keyboard codes supported by the native control wire, excluding Linux's pointer-button range.
const MAX_KEY_CODE: u32 = 255;
/// Largest scroll delta accepted in one request, in logical points.
const MAX_AXIS_POINTS: f64 = 4096.0;

impl Cupertino {
    fn control_protected_input(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) -> bool {
        use smithay::reexports::wayland_server::Resource;
        protected_surface(surface)
            || crate::authorization::is_prompt(surface)
            || surface.client().is_some_and(|client| {
                crate::authorization::authorized(
                    &client,
                    crate::authorization::ShellRole::SessionLock,
                )
            })
    }
    fn control_mark_target(&mut self, keyboard: bool) {
        use smithay::reexports::wayland_server::Resource;
        let targets = if keyboard {
            vec![self.seat.get_keyboard().and_then(|k| k.current_focus())]
        } else {
            vec![
                self.surface_under(self.pointer.current_location())
                    .map(|(s, _)| s),
                self.pointer.current_focus(),
                self.pointer
                    .grab_start_data()
                    .and_then(|g| g.focus.map(|(s, _)| s)),
            ]
        };
        for client in targets.into_iter().flatten().filter_map(|s| s.client()) {
            if crate::authorization::authorized(&client, crate::authorization::ShellRole::MenuBar)
                && !self.control.injected_clients.contains(&client.id())
            {
                self.control.injected_clients.push(client.id());
            }
        }
    }
    pub(crate) fn control_physical_transition(&mut self, input: Input, pressed: bool) -> bool {
        if self.control.dispatching {
            return true;
        }
        match self
            .control
            .owners
            .set(InputSource::Physical(0), input, pressed)
        {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => {
                tracing::warn!("input ownership table is full");
                false
            }
        }
    }
    fn control_point(
        &self,
        index: usize,
        x: f64,
        y: f64,
    ) -> Result<Point<f64, smithay::utils::Logical>, session::Refusal> {
        if !x.is_finite() || !y.is_finite() {
            return Err(session::Refusal::InvalidInput);
        }
        let g = self.control.runs[index]
            .geometry
            .as_ref()
            .ok_or(session::Refusal::NotGranted)?;
        if x < 0.0 || y < 0.0 || x >= g.rect.2 || y >= g.rect.3 {
            return Err(session::Refusal::OutsideScope);
        }
        if self
            .pointer
            .grab_start_data()
            .and_then(|g| g.focus)
            .is_some_and(|(s, _)| self.control_protected_input(&self.shell_surface_root(&s)))
        {
            return Err(session::Refusal::Blocked);
        }
        let point = Point::from((g.rect.0 + x, g.rect.1 + y));
        if self
            .surface_under(point)
            .is_some_and(|(s, _)| self.control_protected_input(&self.shell_surface_root(&s)))
        {
            return Err(session::Refusal::Blocked);
        }
        Ok(point)
    }
    fn control_pointer_here(&self, index: usize) -> Result<(), session::Refusal> {
        let p = self.pointer.current_location();
        let g = self.control.runs[index]
            .geometry
            .as_ref()
            .ok_or(session::Refusal::NotGranted)?;
        self.control_point(index, p.x - g.rect.0, p.y - g.rect.1)
            .map(|_| ())
    }
    fn control_keyboard_here(&self, index: usize) -> Result<(), session::Refusal> {
        let output = self.control.runs[index]
            .output
            .as_ref()
            .and_then(|o| o.upgrade())
            .ok_or(session::Refusal::OutsideScope)?;
        if let Some(focus) = self.seat.get_keyboard().and_then(|k| k.current_focus()) {
            let root = self.shell_surface_root(&focus);
            if self.control_protected_input(&root) {
                return Err(session::Refusal::Blocked);
            }
            if let Some(id) = self.window_id_for_surface(&root) {
                let same = self
                    .wm
                    .window(id)
                    .and_then(|w| self.output_entry(w.output))
                    .is_some_and(|o| o.output == output);
                if same {
                    return Ok(());
                } else {
                    return Err(session::Refusal::OutsideScope);
                }
            }
            if layer_map_for_output(&output)
                .layers()
                .any(|layer| layer.wl_surface() == &root)
            {
                return Ok(());
            }
            return Err(session::Refusal::OutsideScope);
        }
        self.control_pointer_here(index)
    }
    pub(super) fn control_motion(
        &mut self,
        index: usize,
        revision: u32,
        x: f64,
        y: f64,
    ) -> Result<(), session::Refusal> {
        self.control_check_input(index, revision, Operation::Pointer)?;
        let point = self.control_point(index, x, y)?;
        self.control.dispatching = true;
        self.pointer_moved_to(point, (self.now() * 1000.0) as u32, None);
        self.control_mark_target(false);
        self.control.dispatching = false;
        Ok(())
    }
    pub(super) fn control_button(
        &mut self,
        index: usize,
        revision: u32,
        code: u32,
        pressed: WEnum<session::KeyState>,
    ) -> Result<(), session::Refusal> {
        self.control_check_input(index, revision, Operation::Pointer)?;
        self.control_pointer_here(index)?;
        if !(272..=279).contains(&code) {
            return Err(session::Refusal::InvalidInput);
        }
        let pressed = match pressed {
            WEnum::Value(session::KeyState::Pressed) => true,
            WEnum::Value(session::KeyState::Released) => false,
            _ => return Err(session::Refusal::InvalidInput),
        };
        let source = InputSource::Controller(self.control.runs[index].model.epoch());
        let transition = self
            .control
            .owners
            .set(source, Input::Button(code), pressed)
            .map_err(|_| session::Refusal::Busy)?;
        if transition.is_some() {
            self.control_mark_target(false);
            self.control.dispatching = true;
            self.pointer_button(
                code,
                if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
                (self.now() * 1000.0) as u32,
            );
            self.control.dispatching = false;
        }
        Ok(())
    }
    pub(super) fn control_key(
        &mut self,
        index: usize,
        revision: u32,
        code: u32,
        pressed: WEnum<session::KeyState>,
    ) -> Result<(), session::Refusal> {
        self.control_check_input(index, revision, Operation::Keyboard)?;
        self.control_keyboard_here(index)?;
        if code == 0 || code > MAX_KEY_CODE {
            return Err(session::Refusal::InvalidInput);
        }
        let pressed = match pressed {
            WEnum::Value(session::KeyState::Pressed) => true,
            WEnum::Value(session::KeyState::Released) => false,
            _ => return Err(session::Refusal::InvalidInput),
        };
        let source = InputSource::Controller(self.control.runs[index].model.epoch());
        let transition = self
            .control
            .owners
            .set(source, Input::Key(code + 8), pressed)
            .map_err(|_| session::Refusal::Busy)?;
        if transition.is_some() {
            self.control_mark_target(true);
            self.control.dispatching = true;
            self.keyboard_event_now(
                Keycode::new(code + 8),
                if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
                (self.now() * 1000.0) as u32,
            );
            self.control.dispatching = false;
        }
        Ok(())
    }
    pub(super) fn control_axis(
        &mut self,
        index: usize,
        revision: u32,
        horizontal: f64,
        vertical: f64,
    ) -> Result<(), session::Refusal> {
        self.control_check_input(index, revision, Operation::Pointer)?;
        self.control_pointer_here(index)?;
        if !horizontal.is_finite()
            || !vertical.is_finite()
            || horizontal.abs() > MAX_AXIS_POINTS
            || vertical.abs() > MAX_AXIS_POINTS
        {
            return Err(session::Refusal::InvalidInput);
        }
        self.control_mark_target(false);
        self.control.dispatching = true;
        self.pointer_axis(&crate::events::AxisInput {
            device: "desktop-control".into(),
            source: AxisSource::Continuous,
            usec: (self.now() * 1_000_000.0) as u64,
            amount: [Some(horizontal), Some(vertical)],
            v120: [None, None],
            relative_direction: [AxisRelativeDirection::Identical; 2],
        });
        self.control.dispatching = false;
        Ok(())
    }
    pub(super) fn control_cleanup_input(&mut self, epoch: Epoch) {
        let releases = self
            .control
            .owners
            .release_source(InputSource::Controller(epoch));
        if releases.is_empty() {
            return;
        }
        let time = (self.now() * 1000.0) as u32;
        self.control.dispatching = true;
        if releases
            .iter()
            .any(|r| matches!(r, Transition::Release(Input::Button(_))))
        {
            self.cancel_client_drag(time);
            self.pending_drag = None;
            if self.pointer_mode == crate::state::PointerMode::Model {
                self.command(cupertino_wm::Command::PointerRelease);
            }
            let pointer = self.pointer.clone();
            let serial = SERIAL_COUNTER.next_serial();
            pointer.unset_grab(self, serial, time);
            pointer.motion(
                self,
                None,
                &MotionEvent {
                    location: pointer.current_location(),
                    serial,
                    time,
                },
            );
        }
        for release in releases {
            match release {
                Transition::Release(Input::Key(code)) => {
                    self.keyboard_event_now(Keycode::new(code), KeyState::Released, time)
                }
                Transition::Release(Input::Button(code)) => {
                    self.pointer_button(code, ButtonState::Released, time)
                }
                Transition::Press(_) => unreachable!(),
            }
        }
        self.control.dispatching = false;
        self.request_redraw();
    }
}

//! `cupertino_keyboard_panels_v1` (the on-screen displays → server): the input-source switcher's and the accent
//! picker's panels, which the component that holds the `keyboard_panels` role draws (`docs/architecture/shell-host.md`,
//! *The input-source switcher and the accent picker*). The server keeps their keys, their choice and the text they change
//! ([`crate::kbd_ui`]) and sends the panels what they show; it waits for nothing they do, so a switch and an edit go on
//! without them. A pick is the only request that acts, and only on the open picker.

use cupertino_geometry::{Point as GPoint, Rect as GRect};
use cupertino_protocols::server::cupertino_keyboard_panels_v1::{self, CupertinoKeyboardPanelsV1};
use cupertino_protocols::server::cupertino_shell_v1::{CupertinoShellV1, Error as ShellError};
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, New, Resource,
};

use cupertino_wm::OutputId;

use crate::authorization::{authorized, refuse, require, role, Permit, ShellRole};
use crate::menu_bar::cut_text;
use crate::state::{ClientState, Cupertino};

/// The layer-surface namespace of the switcher's panel.
pub const SOURCES_NAMESPACE: &str = "cupertino-osd-input-sources";
/// The layer-surface namespace of the picker's panel: a press on it goes to the panel and leaves the picker open.
pub const ACCENTS_NAMESPACE: &str = "cupertino-osd-accents";
/// The most candidates a picker shows: one for each digit that picks.
const MAX_CANDIDATES: usize = 9;

/// The panels' objects and what the open picker was sent.
#[derive(Debug, Default)]
pub struct KeyboardPanels {
    /// Live objects, oldest first: the newest draws (a restarted instance's replaces the one before).
    objects: Vec<CupertinoKeyboardPanelsV1>,
    /// The serial of the last picker, which counts the pickers of the session.
    serial: u32,
    /// The open picker as the panels were last told it.
    sent: Option<SentPicker>,
}

/// One picker, from its open to its close: what identifies it and what moves.
#[derive(Clone, Debug, PartialEq)]
struct SentPicker {
    serial: u32,
    base: char,
    candidates: Vec<String>,
    anchor: GRect,
    focused: Option<usize>,
}

impl KeyboardPanels {
    /// Whether a connection holds an object (tests, diagnostics).
    pub fn bound(&self) -> bool {
        !self.objects.is_empty()
    }

    /// The serial of the open picker, as its panel knows it.
    pub fn open_serial(&self) -> Option<u32> {
        self.sent.as_ref().map(|s| s.serial)
    }
}

impl Cupertino {
    /// `cupertino_shell_v1.get_keyboard_panels`: one object for a connection that holds the role. A switch under way is
    /// sent at once, so a new instance shows it.
    pub fn create_keyboard_panels(
        &mut self,
        shell: &CupertinoShellV1,
        permit: Option<Permit<role::KeyboardPanels>>,
        id: New<CupertinoKeyboardPanelsV1>,
        init: &mut DataInit<'_, Self>,
    ) {
        if permit.is_none() {
            init.init(id, ());
            refuse(shell, ShellRole::KeyboardPanels);
            return;
        }
        let client = shell.client().map(|c| c.id());
        if self
            .overlays
            .panels
            .objects
            .iter()
            .any(|o| o.client().map(|c| c.id()) == client)
        {
            init.init(id, ());
            shell.post_error(
                ShellError::Role,
                "the connection already has the keyboard's panels",
            );
            return;
        }
        let resource = init.init(id, ());
        self.overlays.panels.objects.push(resource);
        if self.overlays.inputs.switch_open() {
            self.send_sources();
        }
    }

    /// The object that draws the panels: the newest one whose component is neither unresponsive nor lost.
    fn panels_object(&self) -> Option<CupertinoKeyboardPanelsV1> {
        self.overlays
            .panels
            .objects
            .iter()
            .rev()
            .find(|o| {
                o.client()
                    .is_some_and(|c| !self.hosts.is_client_unresponsive(&c))
            })
            .cloned()
    }

    /// Whether a held letter may arm the picker: a component draws the panels and is not busy, unresponsive or lost
    /// (a connection that is no component, a test's, is waited for).
    pub fn panels_can_open_picker(&self) -> bool {
        self.panels_object().is_some_and(|o| {
            let component = o
                .client()
                .and_then(|c| c.get_data::<ClientState>().and_then(|s| s.component));
            component.is_none_or(|c| !self.hosts.component_not_waited_for(c.name))
        })
    }

    /// The output resource of `output` the panels' client bound, `None` when it bound none.
    fn panels_output(
        &self,
        object: &CupertinoKeyboardPanelsV1,
        output: Option<OutputId>,
    ) -> Option<WlOutput> {
        let client = object.client()?;
        self.output_entry(output?)?
            .output
            .client_outputs(&client)
            .next()
    }

    /// A switch event ran: send the list while the switch is open, its close once it ends. `was_open`: a switch was
    /// open before the event, so a chord that commits at once (no switch opened) sends nothing.
    pub(crate) fn sources_changed(&mut self, was_open: bool) {
        if self.overlays.inputs.switch_open() {
            self.send_sources();
        } else if was_open {
            if let Some(object) = self.panels_object() {
                object.sources_closed();
            }
        }
    }

    /// The whole list of the open switch, naming the server's first output, where the panel shows.
    fn send_sources(&self) {
        let (Some(object), Some(open)) = (self.panels_object(), self.overlays.inputs.open.as_ref())
        else {
            return;
        };
        for (id, name) in open.order.iter().zip(&open.labels) {
            object.source_entry(cut_text(id).to_owned(), cut_text(name).to_owned());
        }
        let output = self.wm.outputs().first().map(|o| o.id);
        object.sources_changed(
            self.panels_output(&object, output).as_ref(),
            open.selection as u32,
        );
    }

    /// The output that holds the centre of `anchor` (global points): its id and its rectangle.
    fn output_of_anchor(&self, anchor: GRect) -> Option<(OutputId, GRect)> {
        let centre = GPoint::new(
            anchor.x() + anchor.width() / 2.0,
            anchor.y() + anchor.height() / 2.0,
        );
        self.outputs
            .iter()
            .map(|o| (o.id, GRect::new(o.rect.x, o.rect.y, o.rect.w, o.rect.h)))
            .find(|(_, r)| r.contains(centre))
    }

    /// Whether a picker anchored at `anchor` can show: an output holds the anchor's centre.
    pub(crate) fn picker_has_output(&self, anchor: GRect) -> bool {
        self.output_of_anchor(anchor).is_some()
    }

    /// Bring the panels up to the picker as the server holds it: a new picker gets a new serial and its list, a moved
    /// focus the list again, and a picker that closed (or was replaced) `accents_closed`.
    pub(crate) fn sync_accent_panel(&mut self) {
        let now = self.overlays.accent.popup.as_ref().map(|p| SentPicker {
            serial: 0,
            base: p.base,
            candidates: p.candidates.iter().take(MAX_CANDIDATES).cloned().collect(),
            anchor: p.anchor,
            focused: p.focused,
        });
        let sent = self.overlays.panels.sent.clone();
        let same_picker = |a: &SentPicker, b: &SentPicker| {
            a.base == b.base && a.candidates == b.candidates && a.anchor == b.anchor
        };
        let object = self.panels_object();
        match (sent, now) {
            (None, None) => {}
            (Some(sent), Some(now)) if same_picker(&sent, &now) => {
                if sent.focused != now.focused {
                    let moved = SentPicker {
                        serial: sent.serial,
                        ..now
                    };
                    if let Some(object) = &object {
                        self.send_accents(object, &moved);
                    }
                    self.overlays.panels.sent = Some(moved);
                }
            }
            (sent, now) => {
                if let (Some(sent), Some(object)) = (&sent, &object) {
                    object.accents_closed(sent.serial);
                }
                self.overlays.panels.sent = now.map(|now| {
                    self.overlays.panels.serial = self.overlays.panels.serial.wrapping_add(1);
                    let opened = SentPicker {
                        serial: self.overlays.panels.serial,
                        ..now
                    };
                    if let Some(object) = &object {
                        self.send_accents(object, &opened);
                    }
                    opened
                });
            }
        }
    }

    /// The candidates of `picker`, then where it is and which one has the focus.
    fn send_accents(&self, object: &CupertinoKeyboardPanelsV1, picker: &SentPicker) {
        let Some((output, rect)) = self.output_of_anchor(picker.anchor) else {
            return;
        };
        for text in &picker.candidates {
            object.accent_entry(cut_text(text).to_owned());
        }
        let top = if self.config.shell.show_menu_bar {
            self.config.shell.menu_bar_height
        } else {
            0.0
        };
        let a = picker.anchor;
        object.accents_changed(
            picker.serial,
            self.panels_output(object, Some(output)).as_ref(),
            (a.x() - rect.x()).round() as i32,
            (a.y() - rect.y()).round() as i32,
            a.width().round() as i32,
            a.height().round() as i32,
            top.round() as i32,
            picker
                .focused
                .and_then(|f| i32::try_from(f).ok())
                .unwrap_or(-1),
        );
    }

    /// Whether `surface` is the picker's panel: a layer surface of that namespace on a connection that holds the role.
    pub fn is_accents_panel(&self, surface: &WlSurface) -> bool {
        surface
            .client()
            .is_some_and(|c| authorized(&c, ShellRole::KeyboardPanels))
            && self.outputs.iter().any(|o| {
                layer_map_for_output(&o.output)
                    .layer_for_surface(surface, WindowSurfaceType::ALL)
                    .is_some_and(|l| l.namespace() == ACCENTS_NAMESPACE)
            })
    }

    /// The panels' component changed its liveness: unresponsive or lost, its picker is dismissed and the letter put
    /// back (its panel went with its transient surfaces); responsive again, a switch under way is sent once more.
    pub fn keyboard_panels_liveness(&mut self, client: &ClientId) {
        let Some(object) = self
            .overlays
            .panels
            .objects
            .iter()
            .find(|o| o.client().is_some_and(|c| &c.id() == client))
            .cloned()
        else {
            return;
        };
        let unresponsive = object
            .client()
            .is_none_or(|c| self.hosts.is_client_unresponsive(&c));
        if unresponsive {
            self.accent_dismiss();
        } else if self.overlays.inputs.switch_open() {
            self.send_sources();
        }
    }

    /// The session locks: a switch under way is cancelled and an open picker dismissed.
    pub fn keyboard_panels_locked(&mut self) {
        if self.overlays.inputs.switch_open() {
            let ev = self.overlays.inputs.switcher.escape();
            self.run_switch_event(ev);
        }
        self.accent_dismiss();
    }

    /// An object went (destroyed, or its connection ended): without one that draws, the open picker is dismissed and
    /// the letter put back. A switch goes on.
    fn keyboard_panels_gone(&mut self, resource: &CupertinoKeyboardPanelsV1) {
        self.overlays.panels.objects.retain(|o| o != resource);
        if self.panels_object().is_none() {
            self.accent_dismiss();
        }
    }

    /// `accent_pick`: insert candidate `index` of the open picker when `serial` is its serial.
    fn accent_pick(&mut self, serial: u32, index: u32) {
        let Some(sent) = self.overlays.panels.sent.as_ref() else {
            return;
        };
        let index = index as usize;
        if sent.serial != serial || index >= sent.candidates.len() {
            tracing::debug!(serial, index, "a pick of no open candidate: ignored");
            return;
        }
        self.overlays.accent.click(index);
        self.flush_text_ops();
        self.sync_accent_panel();
    }
}

impl Dispatch<CupertinoKeyboardPanelsV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoKeyboardPanelsV1,
        request: cupertino_keyboard_panels_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_keyboard_panels_v1::Request::Destroy => state.keyboard_panels_gone(resource),
            cupertino_keyboard_panels_v1::Request::AccentPick { serial, index }
                if require(client, ShellRole::KeyboardPanels, resource) =>
            {
                state.accent_pick(serial, index);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoKeyboardPanelsV1, _: &()) {
        state.keyboard_panels_gone(resource);
    }
}

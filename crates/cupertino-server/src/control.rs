//! Private desktop-control discovery. No grant or mutation is exposed before consent and input enforcement exist.

use cupertino_control_model::{Epoch, Session};
use cupertino_protocols::server::control::cupertino_control_session_v1::{
    self as session, CupertinoControlSessionV1,
};
use cupertino_protocols::server::control::cupertino_control_v1::{
    self as manager, CupertinoControlV1,
};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::authorization::{authorized, require, ShellRole};
use crate::state::Cupertino;

/// Discovery-only wire version. Later operations must not be advertised until their guards are implemented.
const VERSION: u32 = 1;
/// One live session per controller connection bounds state and keeps each connection's run unambiguous.
const MAX_SESSIONS_PER_CONNECTION: usize = 1;

pub struct Control {
    _global: GlobalId,
    next_epoch: u64,
    sessions: Vec<CupertinoControlSessionV1>,
}

pub struct SessionData {
    model: Option<Session>,
}

impl Control {
    pub fn new(display: &DisplayHandle) -> Self {
        Self {
            _global: display.create_global::<Cupertino, CupertinoControlV1, ()>(VERSION, ()),
            next_epoch: 1,
            sessions: Vec::new(),
        }
    }

    fn allocate_epoch(&mut self) -> Option<Epoch> {
        let epoch = Epoch::new(self.next_epoch)?;
        // Zero permanently exhausts this server's counter instead of reusing an epoch after overflow.
        self.next_epoch = self.next_epoch.checked_add(1).unwrap_or(0);
        Some(epoch)
    }
}

impl GlobalDispatch<CupertinoControlV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<CupertinoControlV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let resource = init.init(resource, ());
        require(client, ShellRole::DesktopControl, &resource);
    }

    fn can_view(client: Client, _: &()) -> bool {
        authorized(&client, ShellRole::DesktopControl)
    }
}

impl Dispatch<CupertinoControlV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoControlV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        // Destruction is harmless even after the holder exits. New objects must always be initialized, also on refusal.
        match request {
            manager::Request::Destroy => {}
            manager::Request::GetSession { id } => {
                if !require(client, ShellRole::DesktopControl, resource) {
                    init.init(id, SessionData { model: None });
                    return;
                }
                let sessions = &mut state.control.sessions;
                sessions.retain(Resource::is_alive);
                if sessions
                    .iter()
                    .filter(|s| s.client().as_ref() == Some(client))
                    .count()
                    >= MAX_SESSIONS_PER_CONNECTION
                {
                    init.init(id, SessionData { model: None });
                    resource.post_error(
                        manager::Error::SessionExists,
                        "the connection already has a control session",
                    );
                    return;
                }
                let Some(epoch) = state.control.allocate_epoch() else {
                    init.init(id, SessionData { model: None });
                    resource.post_error(manager::Error::Exhausted, "control epochs are exhausted");
                    return;
                };
                let session = init.init(
                    id,
                    SessionData {
                        model: Some(Session::new(epoch)),
                    },
                );
                session.created((epoch.get() >> 32) as u32, epoch.get() as u32, 0);
                session.status(0, session::Phase::Inactive);
                state.control.sessions.push(session);
            }
            _ => unreachable!("desktop-control manager version 1 has no other request"),
        }
    }
}

impl Dispatch<CupertinoControlSessionV1, SessionData> for Cupertino {
    fn request(
        _: &mut Self,
        client: &Client,
        resource: &CupertinoControlSessionV1,
        request: session::Request,
        data: &SessionData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            session::Request::Destroy => {}
            session::Request::Query {
                serial,
                epoch_hi,
                epoch_lo,
            } => {
                if !require(client, ShellRole::DesktopControl, resource) {
                    return;
                }
                if serial == 0 {
                    resource.post_error(session::Error::InvalidSerial, "serial zero is reserved");
                } else if data.model.as_ref().is_none_or(|model| {
                    ((u64::from(epoch_hi) << 32) | u64::from(epoch_lo)) != model.epoch().get()
                }) {
                    resource.refused(serial, session::Refusal::StaleEpoch);
                } else {
                    resource.status(serial, session::Phase::Inactive);
                }
            }
            _ => unreachable!("desktop-control session version 1 has no other request"),
        }
    }

    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &CupertinoControlSessionV1,
        _: &SessionData,
    ) {
        state.control.sessions.retain(|session| session != resource);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_exhaustion_never_reuses_a_run_identity() {
        let display = smithay::reexports::wayland_server::Display::<Cupertino>::new().unwrap();
        let mut control = Control::new(&display.handle());
        control.next_epoch = u64::MAX;
        assert_eq!(control.allocate_epoch().unwrap().get(), u64::MAX);
        assert_eq!(control.allocate_epoch(), None);
        assert_eq!(control.allocate_epoch(), None);
    }
}

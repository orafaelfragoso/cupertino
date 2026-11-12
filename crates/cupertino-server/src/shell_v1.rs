//! `cupertino_shell_v1` (shell processes → server): the private global that hands out the shell roles. Only the
//! connections the server made for a shell process see it ([`crate::authorization`]), and each request needs the role
//! it creates: `get_menu_bar` in [`crate::menu_bar`], `get_dock` in [`crate::dock`], `get_session` in
//! [`crate::session_v1`], `get_window_manager` in [`crate::window_manager`]. What any client may ask for (backdrops, title
//! bar style, appearance) is in [`crate::app_effects_v1`].

use cupertino_protocols::server::cupertino_shell_v1::{self, CupertinoShellV1};
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};

use crate::authorization::{capabilities, permit, role};
use crate::state::Cupertino;

/// Registers the `cupertino_shell_v1` global.
pub struct ShellV1Global {
    _global: GlobalId,
}

impl ShellV1Global {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            _global: dh.create_global::<Cupertino, CupertinoShellV1, ()>(20, ()),
        }
    }
}

impl GlobalDispatch<CupertinoShellV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoShellV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }

    fn can_view(client: Client, _: &()) -> bool {
        capabilities(&client).holds_shell_role()
    }
}

impl Dispatch<CupertinoShellV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        shell: &CupertinoShellV1,
        request: cupertino_shell_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        state.release_revoked_roles();
        match request {
            cupertino_shell_v1::Request::GetMenuBar {
                id,
                surface,
                output,
            } => state.create_menu_bar(
                shell,
                permit::<role::MenuBar>(client),
                id,
                surface,
                &output,
                init,
            ),
            cupertino_shell_v1::Request::GetDock {
                id,
                surface,
                output,
            } => state.create_dock(
                shell,
                permit::<role::Dock>(client),
                id,
                surface,
                &output,
                init,
            ),
            cupertino_shell_v1::Request::GetSession { id } => {
                state.create_session_observer(shell, permit::<role::Session>(client), id, init)
            }
            cupertino_shell_v1::Request::GetWindowManager { id, output } => state
                .create_window_manager(
                    shell,
                    permit::<role::WindowManager>(client),
                    id,
                    &output,
                    init,
                ),
            // `Destroy` needs nothing: objects created through the global stay valid
            _ => {}
        }
    }
}

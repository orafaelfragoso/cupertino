//! What a person chose that every window of the process follows: what a click in a scroll bar's track does, how large
//! a sidebar's icons are and what a double click on a title area does. The application's runtime on this thread holds
//! the choice; the views and widgets read it as they are built.

use cupertino_defaults::appearance::{ScrollClick, SidebarIcons, TitlebarAction};
use std::cell::Cell;

/// The choices windows follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Personal {
    pub scroll_click: ScrollClick,
    pub sidebar_icons: SidebarIcons,
    pub titlebar_double_click: TitlebarAction,
}

thread_local! {
    static PERSONAL: Cell<Personal> = const {
        Cell::new(Personal {
            scroll_click: ScrollClick::NextPage,
            sidebar_icons: SidebarIcons::Medium,
            titlebar_double_click: TitlebarAction::Zoom,
        })
    };
}

/// The choices in effect on this thread.
pub fn personal() -> Personal {
    PERSONAL.get()
}

/// Take `now` as the choices; whether they differ from what [`personal`] said before.
pub fn set_personal(now: Personal) -> bool {
    PERSONAL.replace(now) != now
}

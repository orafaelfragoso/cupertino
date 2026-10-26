//! Synthetic menus for apps that export none: the app menu (About, Hide, Hide Others, Show All,
//! Quit) and a Window menu (Minimize, Zoom, Move to Workspace, Close Window).
//!
//! Item ids are negative and stable so the compositor can map an `Activate` effect back to a
//! [`FallbackAction`] with [`action_for`]. The shortcuts shown are the default shortcut map of
//! `cupertino-input` (approximate conventions with the primary modifier mapped to Super).

use crate::item::{ItemId, MenuItem, Shortcut, ShortcutModifier};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackAction {
    About,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Minimize,
    Zoom,
    CloseWindow,
    /// 1-based workspace index.
    MoveToWorkspace(u32),
}

const ABOUT: i32 = -1000;
const HIDE: i32 = -1001;
const HIDE_OTHERS: i32 = -1002;
const SHOW_ALL: i32 = -1003;
const QUIT: i32 = -1004;
const MINIMIZE: i32 = -1010;
const ZOOM: i32 = -1011;
const CLOSE_WINDOW: i32 = -1012;
const MOVE_TO_WORKSPACE_MENU: i32 = -1013;
const MOVE_TO_WORKSPACE_BASE: i32 = -2000;
/// Identifier of the synthetic Window menu.
pub const WINDOW_MENU: ItemId = ItemId(-1020);

impl FallbackAction {
    pub fn item_id(self) -> ItemId {
        ItemId(match self {
            FallbackAction::About => ABOUT,
            FallbackAction::Hide => HIDE,
            FallbackAction::HideOthers => HIDE_OTHERS,
            FallbackAction::ShowAll => SHOW_ALL,
            FallbackAction::Quit => QUIT,
            FallbackAction::Minimize => MINIMIZE,
            FallbackAction::Zoom => ZOOM,
            FallbackAction::CloseWindow => CLOSE_WINDOW,
            FallbackAction::MoveToWorkspace(n) => MOVE_TO_WORKSPACE_BASE - n as i32,
        })
    }
}

/// The action a synthetic item stands for.
pub fn action_for(id: ItemId) -> Option<FallbackAction> {
    Some(match id.0 {
        ABOUT => FallbackAction::About,
        HIDE => FallbackAction::Hide,
        HIDE_OTHERS => FallbackAction::HideOthers,
        SHOW_ALL => FallbackAction::ShowAll,
        QUIT => FallbackAction::Quit,
        MINIMIZE => FallbackAction::Minimize,
        ZOOM => FallbackAction::Zoom,
        CLOSE_WINDOW => FallbackAction::CloseWindow,
        n if n < MOVE_TO_WORKSPACE_BASE => {
            FallbackAction::MoveToWorkspace((MOVE_TO_WORKSPACE_BASE - n) as u32)
        }
        _ => return None,
    })
}

fn shortcut(mods: &[ShortcutModifier], key: &str) -> Shortcut {
    Shortcut {
        modifiers: mods.to_vec(),
        key: key.to_string(),
    }
}

/// Items of the app menu opened by the bold app name. `about` is offered when the app has a
/// desktop entry or metadata to show.
pub fn app_menu(app_name: &str, about: bool) -> Vec<MenuItem> {
    use ShortcutModifier::*;
    let mut items = Vec::new();
    if about {
        items.push(MenuItem::new(ABOUT, &format!("About {app_name}")));
        items.push(MenuItem::separator(-1100));
    }
    items.push(
        MenuItem::new(HIDE, &format!("Hide {app_name}")).with_shortcut(shortcut(&[Primary], "h")),
    );
    items.push(
        MenuItem::new(HIDE_OTHERS, "Hide Others").with_shortcut(shortcut(&[Primary, Alt], "h")),
    );
    items.push(MenuItem::new(SHOW_ALL, "Show All"));
    items.push(MenuItem::separator(-1101));
    items.push(
        MenuItem::new(QUIT, &format!("Quit {app_name}")).with_shortcut(shortcut(&[Primary], "q")),
    );
    items
}

/// The application menu of the desktop, shown when no window is active: only the command that
/// acts without an active application, so hidden applications can be brought back.
pub fn desktop_menu() -> Vec<MenuItem> {
    vec![MenuItem::new(SHOW_ALL, "Show All")]
}

/// The Window menu. `workspaces` is the number of regular workspaces on the window's output and
/// `current` the 1-based index the window is on (its entry is checked and disabled).
pub fn window_menu(workspaces: u32, current: Option<u32>, can_zoom: bool) -> MenuItem {
    use ShortcutModifier::*;
    let mut children = vec![
        MenuItem::new(MINIMIZE, "Minimize").with_shortcut(shortcut(&[Primary], "m")),
        MenuItem {
            enabled: can_zoom,
            ..MenuItem::new(ZOOM, "Zoom")
        },
    ];
    if workspaces > 1 {
        let targets: Vec<MenuItem> = (1..=workspaces)
            .map(|n| {
                let mut it = MenuItem::new(
                    FallbackAction::MoveToWorkspace(n).item_id().0,
                    &format!("Workspace {n}"),
                );
                if current == Some(n) {
                    it = it.with_toggle(crate::item::Toggle::Check(true)).disabled();
                }
                it
            })
            .collect();
        children.push(MenuItem::separator(-1102));
        children.push(MenuItem::submenu(
            MOVE_TO_WORKSPACE_MENU,
            "Move to Workspace",
            targets,
        ));
    }
    children.push(MenuItem::separator(-1103));
    children
        .push(MenuItem::new(CLOSE_WINDOW, "Close Window").with_shortcut(shortcut(&[Primary], "w")));
    MenuItem::submenu(WINDOW_MENU.0, "Window", children)
}

/// The menus shown for an app without exported menus: just the Window menu next to the app menu.
pub fn menus(workspaces: u32, current: Option<u32>, can_zoom: bool) -> Vec<MenuItem> {
    vec![window_menu(workspaces, current, can_zoom)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_menu_has_the_expected_entries() {
        let m = app_menu("Files", true);
        let labels: Vec<&str> = m.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "About Files",
                "",
                "Hide Files",
                "Hide Others",
                "Show All",
                "",
                "Quit Files"
            ]
        );
        assert!(m[1].separator && m[5].separator);
        let quit = m.last().unwrap();
        assert_eq!(quit.shortcut.as_ref().unwrap().display(), "Super+q");
        let no_about = app_menu("Files", false);
        assert_eq!(no_about.len(), 5);
        assert_eq!(no_about[0].label, "Hide Files");
    }

    #[test]
    fn window_menu_offers_workspace_moves() {
        let w = window_menu(3, Some(2), true);
        assert_eq!(w.label, "Window");
        let labels: Vec<&str> = w.children.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Minimize",
                "Zoom",
                "",
                "Move to Workspace",
                "",
                "Close Window"
            ]
        );
        let sub = &w.children[3];
        assert!(sub.has_submenu);
        assert_eq!(sub.children.len(), 3);
        assert!(
            !sub.children[1].enabled,
            "the current workspace is checked and disabled"
        );
        assert!(sub.children[1].toggle.is_marked());
        assert!(sub.children[0].enabled);
        let single = window_menu(1, Some(1), false);
        assert!(single
            .children
            .iter()
            .all(|c| c.label != "Move to Workspace"));
        assert!(
            !single.children[1].enabled,
            "zoom disabled when the window cannot zoom"
        );
    }

    #[test]
    fn ids_round_trip_to_actions() {
        for a in [
            FallbackAction::About,
            FallbackAction::Hide,
            FallbackAction::HideOthers,
            FallbackAction::ShowAll,
            FallbackAction::Quit,
            FallbackAction::Minimize,
            FallbackAction::Zoom,
            FallbackAction::CloseWindow,
            FallbackAction::MoveToWorkspace(1),
            FallbackAction::MoveToWorkspace(7),
        ] {
            assert_eq!(action_for(a.item_id()), Some(a), "{a:?}");
        }
        assert_eq!(action_for(ItemId(5)), None);
        assert_eq!(action_for(WINDOW_MENU), None);
        assert_eq!(action_for(ItemId(-1100)), None, "separators");
    }

    #[test]
    fn synthetic_menus_work_with_the_controller_layout() {
        let ms = menus(2, Some(1), true);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].id, WINDOW_MENU);
    }
}

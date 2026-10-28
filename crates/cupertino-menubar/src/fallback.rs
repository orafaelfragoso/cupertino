//! Synthetic menus for apps that export none: the app menu (About, Settings, Hide, Hide Others, Show All, Quit), File
//! (Close Window), Edit (Undo, Redo, Cut, Copy, Paste, Select All), View (Enter Full Screen), Window (Minimize, Zoom,
//! Move to Workspace, Tile Window, Bring All to Front, the window list, Close Window) and Help (search).
//!
//! Item ids are negative and stable so the compositor can map an `Activate` effect back to a
//! [`FallbackAction`] with [`action_for`], and an open menu survives a rebuild. The items that stand for a server
//! action show the chord the server binds to it ([`Chords`]), none while it sent none; the app's own convention chords
//! are written with the primary modifier the server names.

use crate::extras::SystemRow;
use crate::item::{ItemId, MenuItem, Shortcut, ShortcutModifier, Toggle};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FallbackAction {
    About,
    Settings,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Minimize,
    Zoom,
    CloseWindow,
    ToggleFullscreen,
    TileLeft,
    TileRight,
    TileFill,
    BringAllToFront,
    Edit(EditAction),
    /// 0-based row of the window list.
    ShowWindow(usize),
    /// 1-based workspace index.
    MoveToWorkspace(u32),
}

/// An item of the Edit menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

const ABOUT: i32 = -1000;
const HIDE: i32 = -1001;
const HIDE_OTHERS: i32 = -1002;
const SHOW_ALL: i32 = -1003;
const QUIT: i32 = -1004;
const SETTINGS: i32 = -1005;
const TOGGLE_FULLSCREEN: i32 = -1006;
const MINIMIZE: i32 = -1010;
const ZOOM: i32 = -1011;
const CLOSE_WINDOW: i32 = -1012;
const MOVE_TO_WORKSPACE_MENU: i32 = -1013;
const BRING_ALL_TO_FRONT: i32 = -1014;
const TILE_LEFT: i32 = -1015;
const TILE_RIGHT: i32 = -1016;
const TILE_FILL: i32 = -1017;
const TILE_MENU: i32 = -1018;
/// Close Window of the File menu; it acts as the Window menu's.
const FILE_CLOSE_WINDOW: i32 = -1019;
/// The search field, the first row of the Help menu.
pub const HELP_SEARCH: ItemId = ItemId(-1030);
/// The row the Help menu shows when a search finds nothing.
pub const NO_SEARCH_RESULTS: ItemId = ItemId(-1031);
/// Result `n` of a Help search is `SEARCH_RESULTS_BASE - n`.
const SEARCH_RESULTS_BASE: i32 = -1050;
/// Results a Help search shows at most.
pub const MAX_SEARCH_RESULTS: usize = 12;
const UNDO: i32 = -1040;
const REDO: i32 = -1041;
const CUT: i32 = -1042;
const COPY: i32 = -1043;
const PASTE: i32 = -1044;
const SELECT_ALL: i32 = -1045;
const MOVE_TO_WORKSPACE_BASE: i32 = -2000;
/// Row `n` of the window list is `WINDOW_LIST_BASE - n`.
const WINDOW_LIST_BASE: i32 = -3000;
/// Rows the window list shows at most; further windows are not listed.
pub const MAX_WINDOW_ROWS: usize = 1000;
/// Identifier of the synthetic Window menu.
pub const WINDOW_MENU: ItemId = ItemId(-1020);
/// Identifier of the synthetic File menu.
pub const FILE_MENU: ItemId = ItemId(-1021);
/// Identifier of the synthetic Edit menu.
pub const EDIT_MENU: ItemId = ItemId(-1022);
/// Identifier of the synthetic View menu.
pub const VIEW_MENU: ItemId = ItemId(-1023);
/// Identifier of the synthetic Help menu.
pub const HELP_MENU: ItemId = ItemId(-1024);

impl FallbackAction {
    pub fn item_id(self) -> ItemId {
        ItemId(match self {
            FallbackAction::About => ABOUT,
            FallbackAction::Settings => SETTINGS,
            FallbackAction::Hide => HIDE,
            FallbackAction::HideOthers => HIDE_OTHERS,
            FallbackAction::ShowAll => SHOW_ALL,
            FallbackAction::Quit => QUIT,
            FallbackAction::Minimize => MINIMIZE,
            FallbackAction::Zoom => ZOOM,
            FallbackAction::CloseWindow => CLOSE_WINDOW,
            FallbackAction::ToggleFullscreen => TOGGLE_FULLSCREEN,
            FallbackAction::TileLeft => TILE_LEFT,
            FallbackAction::TileRight => TILE_RIGHT,
            FallbackAction::TileFill => TILE_FILL,
            FallbackAction::BringAllToFront => BRING_ALL_TO_FRONT,
            FallbackAction::Edit(EditAction::Undo) => UNDO,
            FallbackAction::Edit(EditAction::Redo) => REDO,
            FallbackAction::Edit(EditAction::Cut) => CUT,
            FallbackAction::Edit(EditAction::Copy) => COPY,
            FallbackAction::Edit(EditAction::Paste) => PASTE,
            FallbackAction::Edit(EditAction::SelectAll) => SELECT_ALL,
            FallbackAction::ShowWindow(row) => WINDOW_LIST_BASE - row as i32,
            FallbackAction::MoveToWorkspace(n) => MOVE_TO_WORKSPACE_BASE - n as i32,
        })
    }
}

/// Whether `id` is in the range of the bar's own items (every one is at most -1000: the synthetic menus, their
/// separators, the workspace targets, the window list and the system menu). The DBusMenu reader leaves out an
/// exporter's item with such an id; routing goes by [`crate::item::Origin`], not by this range.
pub fn is_system(id: ItemId) -> bool {
    id.0 <= ABOUT
}

/// The id of the row of the `n`th result of a Help search.
pub fn search_result(n: usize) -> ItemId {
    ItemId(SEARCH_RESULTS_BASE - n as i32)
}

/// The position among the results of a Help search of the row `id`; `None` for any other id.
pub fn search_result_row(id: ItemId) -> Option<usize> {
    let row = SEARCH_RESULTS_BASE.checked_sub(id.0)?;
    (0..MAX_SEARCH_RESULTS as i32)
        .contains(&row)
        .then_some(row as usize)
}

/// The action a synthetic item stands for.
pub fn action_for(id: ItemId) -> Option<FallbackAction> {
    let window_rows = WINDOW_LIST_BASE - MAX_WINDOW_ROWS as i32 + 1..=WINDOW_LIST_BASE;
    Some(match id.0 {
        ABOUT => FallbackAction::About,
        SETTINGS => FallbackAction::Settings,
        HIDE => FallbackAction::Hide,
        HIDE_OTHERS => FallbackAction::HideOthers,
        SHOW_ALL => FallbackAction::ShowAll,
        QUIT => FallbackAction::Quit,
        MINIMIZE => FallbackAction::Minimize,
        ZOOM => FallbackAction::Zoom,
        CLOSE_WINDOW | FILE_CLOSE_WINDOW => FallbackAction::CloseWindow,
        TOGGLE_FULLSCREEN => FallbackAction::ToggleFullscreen,
        TILE_LEFT => FallbackAction::TileLeft,
        TILE_RIGHT => FallbackAction::TileRight,
        TILE_FILL => FallbackAction::TileFill,
        BRING_ALL_TO_FRONT => FallbackAction::BringAllToFront,
        UNDO => FallbackAction::Edit(EditAction::Undo),
        REDO => FallbackAction::Edit(EditAction::Redo),
        CUT => FallbackAction::Edit(EditAction::Cut),
        COPY => FallbackAction::Edit(EditAction::Copy),
        PASTE => FallbackAction::Edit(EditAction::Paste),
        SELECT_ALL => FallbackAction::Edit(EditAction::SelectAll),
        n if window_rows.contains(&n) => {
            FallbackAction::ShowWindow((WINDOW_LIST_BASE - n) as usize)
        }
        n if n < MOVE_TO_WORKSPACE_BASE && n > WINDOW_LIST_BASE => {
            FallbackAction::MoveToWorkspace((MOVE_TO_WORKSPACE_BASE - n) as u32)
        }
        _ => return None,
    })
}

/// The chords the synthetic menus show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chords {
    /// The chord the server binds to the action of a system item; an item without one shows none.
    pub bound: HashMap<FallbackAction, Shortcut>,
    /// The chord the server binds to the action of a row of the system menu; a row without one shows none.
    pub system: HashMap<SystemRow, Shortcut>,
    /// What the primary modifier of the app's convention chords is shown as: [`ShortcutModifier::Primary`] when
    /// Super is the command key, [`ShortcutModifier::Ctrl`] when Ctrl is.
    pub primary: ShortcutModifier,
}

impl Default for Chords {
    fn default() -> Self {
        Self {
            bound: HashMap::new(),
            system: HashMap::new(),
            primary: ShortcutModifier::Primary,
        }
    }
}

impl Chords {
    /// An item of a system action, with the chord the server binds to it.
    fn item(&self, action: FallbackAction, label: &str) -> MenuItem {
        let item = MenuItem::new(action.item_id().0, label);
        match self.bound.get(&action) {
            Some(chord) => item.with_shortcut(chord.clone()),
            None => item,
        }
    }

    /// An app convention chord written with `Primary`, shown with the profile's primary modifier.
    fn convention(&self, modifiers: &[ShortcutModifier], key: &str) -> Shortcut {
        let mut modifiers: Vec<ShortcutModifier> = modifiers
            .iter()
            .map(|m| match m {
                ShortcutModifier::Primary => self.primary,
                m => *m,
            })
            .collect();
        modifiers.sort();
        Shortcut {
            modifiers,
            key: key.to_string(),
        }
    }

    fn settings(&self) -> MenuItem {
        MenuItem::new(SETTINGS, "Settings…")
            .with_shortcut(self.convention(&[ShortcutModifier::Primary], ","))
            .disabled()
    }
}

/// A row of the window list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowRow<'a> {
    pub title: &'a str,
    pub active: bool,
    pub minimized: bool,
}

/// Items of the app menu opened by the bold app name.
pub fn app_menu(app_name: &str, chords: &Chords) -> Vec<MenuItem> {
    vec![
        MenuItem::new(ABOUT, &format!("About {app_name}")),
        MenuItem::separator(-1100),
        chords.settings(),
        MenuItem::separator(-1104),
        chords.item(FallbackAction::Hide, &format!("Hide {app_name}")),
        chords.item(FallbackAction::HideOthers, "Hide Others"),
        MenuItem::new(SHOW_ALL, "Show All"),
        MenuItem::separator(-1101),
        chords.item(FallbackAction::Quit, &format!("Quit {app_name}")),
    ]
}

/// The application menu of the desktop, shown when no window is active: the commands that act without an active
/// application, so hidden applications can be brought back.
pub fn desktop_menu(chords: &Chords) -> Vec<MenuItem> {
    vec![
        chords.settings(),
        MenuItem::separator(-1104),
        MenuItem::new(SHOW_ALL, "Show All"),
    ]
}

/// The Window menu. `workspaces` is the number of regular workspaces on the window's output and
/// `current` the 1-based index the window is on (its entry is checked and disabled). `windows` are the application's
/// windows, the first [`MAX_WINDOW_ROWS`] listed.
pub fn window_menu(
    workspaces: u32,
    current: Option<u32>,
    can_zoom: bool,
    windows: &[WindowRow],
    chords: &Chords,
) -> MenuItem {
    let mut children = vec![
        chords.item(FallbackAction::Minimize, "Minimize"),
        MenuItem {
            enabled: can_zoom,
            ..chords.item(FallbackAction::Zoom, "Zoom")
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
                    it = it.with_toggle(Toggle::Check(true)).disabled();
                }
                it
            })
            .collect();
        children.push(MenuItem::submenu(
            MOVE_TO_WORKSPACE_MENU,
            "Move to Workspace",
            targets,
        ));
    }
    children.push(MenuItem::submenu(
        TILE_MENU,
        "Tile Window",
        vec![
            chords.item(FallbackAction::TileLeft, "Left"),
            chords.item(FallbackAction::TileRight, "Right"),
            chords.item(FallbackAction::TileFill, "Fill"),
        ],
    ));
    children.push(MenuItem::separator(-1102));
    children.push(MenuItem::new(BRING_ALL_TO_FRONT, "Bring All to Front"));
    children.push(MenuItem::separator(-1103));
    children.extend(
        windows
            .iter()
            .take(MAX_WINDOW_ROWS)
            .enumerate()
            .map(|(row, window)| {
                let label = if window.minimized {
                    format!("{} (minimized)", window.title)
                } else {
                    window.title.to_string()
                };
                MenuItem::new(FallbackAction::ShowWindow(row).item_id().0, &label)
                    .with_toggle(Toggle::Check(window.active))
            }),
    );
    children.push(MenuItem::separator(-1105));
    children.push(chords.item(FallbackAction::CloseWindow, "Close Window"));
    MenuItem::submenu(WINDOW_MENU.0, "Window", children)
}

/// The menus shown for an app without exported menus, after the app menu. `edits`: the server sends the Edit items'
/// chords to the key window (version 11); without it they are disabled.
pub fn menus(
    workspaces: u32,
    current: Option<u32>,
    can_zoom: bool,
    windows: &[WindowRow],
    chords: &Chords,
    edits: bool,
) -> Vec<MenuItem> {
    use ShortcutModifier::{Primary, Shift};
    let close = chords.bound.get(&FallbackAction::CloseWindow).cloned();
    let file_close = MenuItem {
        shortcut: close,
        ..MenuItem::new(FILE_CLOSE_WINDOW, "Close Window")
    };
    let edit = |id: i32, label: &str, modifiers: &[ShortcutModifier], key: &str| MenuItem {
        enabled: edits,
        ..MenuItem::new(id, label).with_shortcut(chords.convention(modifiers, key))
    };
    vec![
        MenuItem::submenu(FILE_MENU.0, "File", vec![file_close]),
        MenuItem::submenu(
            EDIT_MENU.0,
            "Edit",
            vec![
                edit(UNDO, "Undo", &[Primary], "z"),
                edit(REDO, "Redo", &[Shift, Primary], "z"),
                MenuItem::separator(-1106),
                edit(CUT, "Cut", &[Primary], "x"),
                edit(COPY, "Copy", &[Primary], "c"),
                edit(PASTE, "Paste", &[Primary], "v"),
                edit(SELECT_ALL, "Select All", &[Primary], "a"),
            ],
        ),
        MenuItem::submenu(
            VIEW_MENU.0,
            "View",
            vec![chords.item(FallbackAction::ToggleFullscreen, "Enter Full Screen")],
        ),
        window_menu(workspaces, current, can_zoom, windows, chords),
        MenuItem::submenu(
            HELP_MENU.0,
            "Help",
            vec![MenuItem::new(HELP_SEARCH.0, "Search").disabled()],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    fn ids(items: &[MenuItem]) -> Vec<i32> {
        items.iter().map(|i| i.id.0).collect()
    }

    fn chord(item: &MenuItem) -> Option<String> {
        item.shortcut.as_ref().map(Shortcut::display)
    }

    /// The server's default bindings under the profile where Super is the command key.
    fn super_chords() -> Chords {
        let bound = [
            (FallbackAction::Hide, "Super+h"),
            (FallbackAction::HideOthers, "Alt+Super+h"),
            (FallbackAction::Quit, "Super+q"),
            (FallbackAction::Minimize, "Super+m"),
            (FallbackAction::CloseWindow, "Super+w"),
            (FallbackAction::ToggleFullscreen, "Ctrl+Super+f"),
            (FallbackAction::TileLeft, "Ctrl+Super+Left"),
        ]
        .into_iter()
        .map(|(action, chord)| (action, Shortcut::from_chord(chord).unwrap()))
        .collect();
        Chords {
            bound,
            primary: ShortcutModifier::Primary,
            ..Chords::default()
        }
    }

    fn find<'a>(menu: &'a MenuItem, label: &str) -> &'a MenuItem {
        menu.children.iter().find(|i| i.label == label).unwrap()
    }

    #[test]
    fn app_menu_has_the_expected_entries() {
        let m = app_menu("Files", &Chords::default());
        assert_eq!(
            labels(&m),
            vec![
                "About Files",
                "",
                "Settings…",
                "",
                "Hide Files",
                "Hide Others",
                "Show All",
                "",
                "Quit Files"
            ]
        );
        assert_eq!(
            ids(&m),
            vec![-1000, -1100, -1005, -1104, -1001, -1002, -1003, -1101, -1004]
        );
        assert!(m[1].separator && m[3].separator && m[7].separator);
        assert!(!m[2].enabled, "no preferences action to run");
    }

    #[test]
    fn the_desktop_menu_offers_settings_and_show_all() {
        let m = desktop_menu(&Chords::default());
        assert_eq!(labels(&m), vec!["Settings…", "", "Show All"]);
        assert_eq!(ids(&m), vec![-1005, -1104, -1003]);
        assert!(!m[0].enabled);
    }

    #[test]
    fn the_menus_of_an_app_without_its_own() {
        let ms = menus(1, Some(1), true, &[], &Chords::default(), false);
        assert_eq!(labels(&ms), vec!["File", "Edit", "View", "Window", "Help"]);
        assert_eq!(
            ids(&ms),
            vec![
                FILE_MENU.0,
                EDIT_MENU.0,
                VIEW_MENU.0,
                WINDOW_MENU.0,
                HELP_MENU.0
            ]
        );
        assert_eq!(labels(&ms[0].children), vec!["Close Window"]);
        assert_eq!(
            labels(&ms[1].children),
            vec!["Undo", "Redo", "", "Cut", "Copy", "Paste", "Select All"]
        );
        assert!(
            ms[1].children.iter().all(|i| !i.enabled),
            "Edit is disabled while the server cannot send its chords"
        );
        let ms = menus(1, Some(1), true, &[], &Chords::default(), true);
        assert!(
            ms[1].children.iter().all(|i| i.enabled != i.separator),
            "every Edit item but the separator is enabled once it can"
        );
        assert_eq!(labels(&ms[2].children), vec!["Enter Full Screen"]);
        assert_eq!(ids(&ms[4].children), vec![-1030]);
    }

    #[test]
    fn window_menu_offers_workspace_moves_and_tiling() {
        let w = window_menu(3, Some(2), true, &[], &Chords::default());
        assert_eq!(w.label, "Window");
        assert_eq!(
            labels(&w.children),
            vec![
                "Minimize",
                "Zoom",
                "Move to Workspace",
                "Tile Window",
                "",
                "Bring All to Front",
                "",
                "",
                "Close Window"
            ]
        );
        assert_eq!(
            ids(&w.children),
            vec![-1010, -1011, -1013, -1018, -1102, -1014, -1103, -1105, -1012]
        );
        let sub = &w.children[2];
        assert!(sub.has_submenu);
        assert_eq!(sub.children.len(), 3);
        assert!(
            !sub.children[1].enabled,
            "the current workspace is checked and disabled"
        );
        assert!(sub.children[1].toggle.is_marked());
        assert!(sub.children[0].enabled);
        let tile = &w.children[3];
        assert_eq!(labels(&tile.children), vec!["Left", "Right", "Fill"]);
        assert_eq!(ids(&tile.children), vec![-1015, -1016, -1017]);
        let single = window_menu(1, Some(1), false, &[], &Chords::default());
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
    fn the_window_list_checks_the_active_window_and_marks_minimized_ones() {
        let windows = [
            WindowRow {
                title: "notes.txt",
                active: true,
                minimized: false,
            },
            WindowRow {
                title: "draft.txt",
                active: false,
                minimized: true,
            },
        ];
        let w = window_menu(1, Some(1), true, &windows, &Chords::default());
        let rows: Vec<(&str, i32, bool)> = w
            .children
            .iter()
            .filter(|i| matches!(action_for(i.id), Some(FallbackAction::ShowWindow(_))))
            .map(|i| (i.label.as_str(), i.id.0, i.toggle.is_marked()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("notes.txt", -3000, true),
                ("draft.txt (minimized)", -3001, false)
            ]
        );
        let many = vec![
            WindowRow {
                title: "w",
                active: false,
                minimized: false,
            };
            MAX_WINDOW_ROWS + 5
        ];
        let w = window_menu(1, Some(1), true, &many, &Chords::default());
        let rows: Vec<i32> = w
            .children
            .iter()
            .filter(|i| i.label == "w")
            .map(|i| i.id.0)
            .collect();
        assert_eq!(rows.len(), MAX_WINDOW_ROWS);
        assert_eq!(rows.last(), Some(&-3999));
    }

    #[test]
    fn system_items_show_the_bound_chords() {
        let chords = super_chords();
        let app = app_menu("Files", &chords);
        assert_eq!(chord(&app[4]).as_deref(), Some("Super+h"));
        assert_eq!(chord(&app[5]).as_deref(), Some("Alt+Super+h"));
        assert_eq!(chord(&app[8]).as_deref(), Some("Super+q"));
        let ms = menus(1, Some(1), true, &[], &chords, true);
        assert_eq!(chord(&ms[0].children[0]).as_deref(), Some("Super+w"));
        assert_eq!(chord(&ms[2].children[0]).as_deref(), Some("Ctrl+Super+f"));
        let window = &ms[3];
        assert_eq!(chord(find(window, "Minimize")).as_deref(), Some("Super+m"));
        assert_eq!(chord(find(window, "Zoom")), None, "unbound");
        assert_eq!(
            chord(find(window, "Close Window")).as_deref(),
            Some("Super+w")
        );
        let tile = find(window, "Tile Window");
        assert_eq!(chord(&tile.children[0]).as_deref(), Some("Ctrl+Super+Left"));
        assert_eq!(chord(&tile.children[1]), None);
    }

    #[test]
    fn without_bindings_system_items_show_no_chord() {
        let chords = Chords::default();
        let app = app_menu("Files", &chords);
        assert!(app
            .iter()
            .filter(|i| i.id.0 != SETTINGS)
            .all(|i| i.shortcut.is_none()));
        let ms = menus(2, Some(1), true, &[], &chords, true);
        assert!(ms[0].children[0].shortcut.is_none());
        assert!(ms[2].children[0].shortcut.is_none());
        assert!(ms[3].children.iter().all(|i| i.shortcut.is_none()));
    }

    #[test]
    fn app_convention_chords_use_the_profiles_primary_modifier() {
        let edit = |primary| {
            let chords = Chords {
                primary,
                ..Chords::default()
            };
            let ms = menus(1, Some(1), true, &[], &chords, true);
            let settings = desktop_menu(&chords).remove(0);
            (
                chord(&settings).unwrap(),
                ms[1].children.iter().filter_map(chord).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            edit(ShortcutModifier::Primary),
            (
                "Super+,".to_string(),
                vec![
                    "Super+z",
                    "Shift+Super+z",
                    "Super+x",
                    "Super+c",
                    "Super+v",
                    "Super+a"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            )
        );
        assert_eq!(
            edit(ShortcutModifier::Ctrl),
            (
                "Ctrl+,".to_string(),
                vec![
                    "Ctrl+z",
                    "Ctrl+Shift+z",
                    "Ctrl+x",
                    "Ctrl+c",
                    "Ctrl+v",
                    "Ctrl+a"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            )
        );
    }

    #[test]
    fn ids_round_trip_to_actions() {
        use EditAction::*;
        for a in [
            FallbackAction::About,
            FallbackAction::Settings,
            FallbackAction::Hide,
            FallbackAction::HideOthers,
            FallbackAction::ShowAll,
            FallbackAction::Quit,
            FallbackAction::Minimize,
            FallbackAction::Zoom,
            FallbackAction::CloseWindow,
            FallbackAction::ToggleFullscreen,
            FallbackAction::TileLeft,
            FallbackAction::TileRight,
            FallbackAction::TileFill,
            FallbackAction::BringAllToFront,
            FallbackAction::Edit(Undo),
            FallbackAction::Edit(Redo),
            FallbackAction::Edit(Cut),
            FallbackAction::Edit(Copy),
            FallbackAction::Edit(Paste),
            FallbackAction::Edit(SelectAll),
            FallbackAction::ShowWindow(0),
            FallbackAction::ShowWindow(MAX_WINDOW_ROWS - 1),
            FallbackAction::MoveToWorkspace(1),
            FallbackAction::MoveToWorkspace(7),
        ] {
            assert_eq!(action_for(a.item_id()), Some(a), "{a:?}");
        }
        assert_eq!(
            action_for(ItemId(FILE_CLOSE_WINDOW)),
            Some(FallbackAction::CloseWindow)
        );
        assert_eq!(action_for(ItemId(5)), None);
        for menu in [WINDOW_MENU, FILE_MENU, EDIT_MENU, VIEW_MENU, HELP_MENU] {
            assert_eq!(action_for(menu), None);
        }
        assert_eq!(action_for(ItemId(TILE_MENU)), None);
        assert_eq!(action_for(HELP_SEARCH), None);
        assert_eq!(action_for(ItemId(-1100)), None, "separators");
        assert_eq!(action_for(ItemId(-4000)), None, "past the window list");
    }
}

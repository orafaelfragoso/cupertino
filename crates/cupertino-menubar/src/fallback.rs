//! Synthetic menus for apps that export none: the app menu (About, Settings, Hide, Hide Others, Show All, Quit), File
//! (Close Window), Edit (Undo, Redo, Cut, Copy, Paste, Select All), View (Enter Full Screen), Window (Minimize, Zoom,
//! Move to Workspace, Tile Window, Bring All to Front, the window list, Close Window) and Help (search). While Alt is
//! held, Minimize, Zoom, Close Window and Bring All to Front show their alternates: Minimize All, Zoom All, Close All and
//! Bring All to Front with the windows cascaded.
//!
//! Item ids are negative and stable so the compositor can map an `Activate` effect back to a
//! [`FallbackAction`] with [`action_for`], and an open menu survives a rebuild. The items that stand for a server
//! action show the chord the server binds to it ([`Chords`]), none while it sent none; the app's own convention chords
//! are written with the primary modifier the server names.

use crate::extras::SystemRow;
use crate::item::{ItemId, MenuItem, Shortcut, ShortcutModifier, ShortcutStyle, Toggle};
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
    /// The alternates of Minimize, Zoom, Close Window and Bring All to Front, shown while Alt is held.
    MinimizeAll,
    ZoomAll,
    CloseAll,
    CascadeAll,
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
/// The alternates of Window menu items.
const MINIMIZE_ALL: i32 = -1025;
const ZOOM_ALL: i32 = -1026;
const CLOSE_ALL: i32 = -1027;
const CASCADE_ALL: i32 = -1028;
/// The search field, the first row of the Help menu.
pub const HELP_SEARCH: ItemId = ItemId(-1030);
/// The label of the search row, which its empty field shows dimmed.
pub const SEARCH_PLACEHOLDER: &str = "Search";
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
            FallbackAction::MinimizeAll => MINIMIZE_ALL,
            FallbackAction::ZoomAll => ZOOM_ALL,
            FallbackAction::CloseAll => CLOSE_ALL,
            FallbackAction::CascadeAll => CASCADE_ALL,
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
        MINIMIZE_ALL => FallbackAction::MinimizeAll,
        ZOOM_ALL => FallbackAction::ZoomAll,
        CLOSE_ALL => FallbackAction::CloseAll,
        CASCADE_ALL => FallbackAction::CascadeAll,
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
    /// The chord the server binds to the Help search shortcut, shown on the search row; none while it sent none.
    pub help_search: Option<Shortcut>,
    /// What the primary modifier of the app's convention chords is shown as: [`ShortcutModifier::Primary`] when
    /// Super is the command key, [`ShortcutModifier::Ctrl`] when Ctrl is.
    pub primary: ShortcutModifier,
}

impl Default for Chords {
    fn default() -> Self {
        Self {
            bound: HashMap::new(),
            system: HashMap::new(),
            help_search: None,
            primary: ShortcutModifier::Primary,
        }
    }
}

impl Chords {
    /// How the menus draw shortcuts: with the Mac glyphs while Super is the command key, with the PC key names while
    /// Ctrl is (the `classic-pc` profile).
    pub fn style(&self) -> ShortcutStyle {
        match self.primary {
            ShortcutModifier::Ctrl => ShortcutStyle::Names,
            _ => ShortcutStyle::Glyphs,
        }
    }

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
/// `current` the 1-based index the window is on (its entry is checked and disabled). `can_minimize` and `can_zoom`
/// enable Minimize and Zoom; their Alt alternates act on every window and stay enabled. `windows` are the
/// application's windows, the first [`MAX_WINDOW_ROWS`] listed.
pub fn window_menu(
    workspaces: u32,
    current: Option<u32>,
    can_minimize: bool,
    can_zoom: bool,
    windows: &[WindowRow],
    chords: &Chords,
) -> MenuItem {
    let mut children = vec![
        MenuItem {
            enabled: can_minimize,
            ..chords.item(FallbackAction::Minimize, "Minimize")
        }
        .with_alternate(chords.item(FallbackAction::MinimizeAll, "Minimize All")),
        MenuItem {
            enabled: can_zoom,
            ..chords.item(FallbackAction::Zoom, "Zoom")
        }
        .with_alternate(chords.item(FallbackAction::ZoomAll, "Zoom All")),
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
    // with Alt held the windows also cascade; the row reads the same
    children.push(
        MenuItem::new(BRING_ALL_TO_FRONT, "Bring All to Front")
            .with_alternate(MenuItem::new(CASCADE_ALL, "Arrange in Front")),
    );
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
    children.push(
        chords
            .item(FallbackAction::CloseWindow, "Close Window")
            .with_alternate(chords.item(FallbackAction::CloseAll, "Close All")),
    );
    MenuItem::submenu(WINDOW_MENU.0, "Window", children)
}

/// The menus shown for an app without exported menus, after the app menu. `edits`: the server sends the Edit items'
/// chords to the key window (version 11); without it they are disabled.
#[allow(clippy::too_many_arguments)]
pub fn menus(
    workspaces: u32,
    current: Option<u32>,
    can_minimize: bool,
    can_zoom: bool,
    full_screen: bool,
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
            // leaving full screen is always offered; entering only where the server would take it (it refuses the
            // person's full screen where zoom is refused); a server before shell version 16 never says full screen, so
            // there the item names entering and is enabled only where zoom is; the full-screen shortcut still leaves
            vec![MenuItem {
                enabled: can_zoom || full_screen,
                ..chords.item(
                    FallbackAction::ToggleFullscreen,
                    if full_screen {
                        "Exit Full Screen"
                    } else {
                        "Enter Full Screen"
                    },
                )
            }],
        ),
        window_menu(workspaces, current, can_minimize, can_zoom, windows, chords),
        MenuItem::submenu(
            HELP_MENU.0,
            "Help",
            vec![MenuItem {
                shortcut: chords.help_search.clone(),
                ..MenuItem::new(HELP_SEARCH.0, SEARCH_PLACEHOLDER).disabled()
            }],
        ),
    ]
}

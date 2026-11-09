//! What the context menus offer: the menu of the selected items and the menu of a folder's empty space. Each is a
//! fixed order of rows in groups; a group ends with a separator, and separators are condensed (no leading, trailing or
//! doubled one) once the rows are built.
//!
//! * Rows for places this window has no part of (volumes, the desktop, search results, the sidebar, cloud items, disc
//!   images, packages, remote servers) are left out.
//! * A row whose action the window cannot do yet is [`MenuItem::unavailable`]: drawn disabled with its title, never
//!   hidden (the backlog rows named in the README say what each needs).
//! * Rows that name the selection use the one-item form with the name in curly quotes, and the many-items form with
//!   the count.
//! * The Sort By and Group By submenus list the keys a folder can be ordered and grouped by.

use crate::context_menu::{AppChoice, Command, ItemKind, MenuItem, Mods};
use crate::sort::{SortKey, SortSpec};
use crate::tags::TagColor;
use crate::window::ViewMode;

/// The menus a row can belong to, as bits: the selected items' menu and the empty space's.
pub const CONTEXT_ITEMS: u32 = 1;
pub const CONTEXT_BACKGROUND: u32 = 2;

/// What the menu of the selection depends on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    /// Number of selected items (at least one).
    pub count: usize,
    /// Name of the first selected item, for the titles of a one-item selection.
    pub name: String,
    /// Every selected item is a folder.
    pub all_folders: bool,
    /// The only selected item is a link.
    pub single_link: bool,
    /// The folder shown is the trash's.
    pub in_trash: bool,
    /// The tag colours every selected item has.
    pub tags_on: Vec<TagColor>,
}

/// What the menu of the empty space depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Background {
    pub view: ViewMode,
    pub sort: SortSpec,
    pub in_trash: bool,
    /// The folder is on a file system mounted read-only, which takes nothing new.
    pub read_only: bool,
    /// How many items Copy put on the window's pasteboard.
    pub pasteboard: usize,
}

/// A title that names the selection: `one` with `{}` for the name, `many` with `{}` for the count.
fn named(subject: &Subject, one: &str, many: &str) -> String {
    if subject.count == 1 {
        one.replace("{}", &subject.name)
    } else {
        many.replace("{}", &subject.count.to_string())
    }
}

fn open_with(apps: &[AppChoice]) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = apps
        .iter()
        .map(|a| {
            let label = if a.is_default {
                format!("{} (default)", a.name)
            } else {
                a.name.clone()
            };
            MenuItem::command(&label, Command::OpenWith(a.id.clone()), true)
        })
        .collect();
    if !items.is_empty() {
        items.push(MenuItem::separator());
    }
    items.push(MenuItem::unavailable("Other…"));
    items
}

/// Remove the separators that lead, trail or follow another one (`_setCondensesSeparators).
pub fn condense(items: Vec<MenuItem>) -> Vec<MenuItem> {
    let mut out: Vec<MenuItem> = Vec::with_capacity(items.len());
    for item in items {
        let is_sep = item.kind == ItemKind::Separator;
        if is_sep
            && out
                .last()
                .is_none_or(|last| last.kind == ItemKind::Separator)
        {
            continue;
        }
        out.push(item);
    }
    if out
        .last()
        .is_some_and(|last| last.kind == ItemKind::Separator)
    {
        out.pop();
    }
    out
}

/// The menu of the selected items `subject`, which `apps` can open.
pub fn item_menu(subject: &Subject, apps: &[AppChoice]) -> Vec<MenuItem> {
    let one = subject.count == 1;
    let mut rows = Vec::new();
    if !subject.in_trash {
        // a new folder holding the selection; nothing is created inside the trash
        let label = if one {
            "New Folder with Selection (1 Item)".to_string()
        } else {
            format!("New Folder with Selection ({} Items)", subject.count)
        };
        rows.push(MenuItem::command(
            &label,
            Command::NewFolderWithSelection,
            true,
        ));
    }
    // Open, with Open in New Window(s) while Alt is held when every item is a folder
    let mut open = MenuItem::command("Open", Command::Open, true);
    if subject.all_folders {
        let label = if one {
            "Open in New Window"
        } else {
            "Open in New Windows"
        };
        open = open.with_alternate(
            Mods::ALT,
            MenuItem::command(label, Command::OpenInNewWindow, true),
        );
    }
    rows.push(open);
    // Open With, and Always Open With on Alt
    let enabled = !apps.is_empty();
    rows.push(
        MenuItem::submenu("Open With", open_with(apps), enabled).with_alternate(
            Mods::ALT,
            MenuItem::submenu("Always Open With", open_with(apps), enabled),
        ),
    );
    // Show Original for a link, Put Back in the trash
    if subject.single_link {
        rows.push(MenuItem::command(
            "Show Original",
            Command::ShowOriginal,
            true,
        ));
    }
    if subject.in_trash {
        rows.push(MenuItem::command("Put Back", Command::PutBack, true));
    }
    rows.push(MenuItem::separator());
    // the trash, and a permanent delete on Alt (which is to ask first)
    if subject.in_trash {
        rows.push(MenuItem::unavailable("Delete Immediately…"));
    } else {
        rows.push(
            MenuItem::command("Move to Trash", Command::MoveToTrash, true)
                .with_alternate(Mods::ALT, MenuItem::unavailable("Delete Immediately…")),
        );
    }
    rows.push(MenuItem::separator());
    // Get Info, Show Inspector on Alt, Get Summary Info on Ctrl
    rows.push(
        MenuItem::unavailable("Get Info")
            .with_alternate(Mods::ALT, MenuItem::unavailable("Show Inspector"))
            .with_alternate(Mods::CTRL, MenuItem::unavailable("Get Summary Info")),
    );
    // what changes or makes items: Rename, Compress, Duplicate, Make Alias, Quick Preview
    rows.push(MenuItem::command("Rename", Command::Rename, one));
    rows.push(
        MenuItem::unavailable(&named(subject, "Compress “{}”", "Compress {} Items"))
            .with_alternate(
                Mods::ALT,
                MenuItem::unavailable(&named(subject, "Compress “{}”…", "Compress {} Items…")),
            ),
    );
    rows.push(
        MenuItem::command("Duplicate", Command::Duplicate, true)
            .with_alternate(Mods::SHIFT_ALT, MenuItem::unavailable("Duplicate Exactly")),
    );
    rows.push(MenuItem::command("Make Alias", Command::MakeAlias, true));
    rows.push(
        MenuItem::command(
            &named(subject, "Quick Preview “{}”", "Quick Preview {} Items"),
            Command::Peek,
            true,
        )
        .with_alternate(
            Mods::ALT,
            MenuItem::unavailable(&named(subject, "Slideshow “{}”", "Slideshow {} Items")),
        ),
    );
    rows.push(MenuItem::separator());
    // Copy, Copy as Pathname on Alt; Share
    rows.push(
        MenuItem::command(
            &named(subject, "Copy “{}”", "Copy {} Items"),
            Command::Copy,
            true,
        )
        .with_alternate(
            Mods::ALT,
            MenuItem::unavailable(&named(
                subject,
                "Copy “{}” as Pathname",
                "Copy {} Items as Pathnames",
            )),
        ),
    );
    rows.push(MenuItem::submenu("Share", Vec::new(), false));
    rows.push(MenuItem::separator());
    // the tag row
    rows.push(MenuItem {
        label: "Tags…".into(),
        kind: ItemKind::TagRow {
            on: subject.tags_on.clone(),
        },
        enabled: true,
        checked: false,
        alternates: Vec::new(),
    });
    rows.push(MenuItem::separator());
    // Quick Actions
    rows.push(MenuItem::submenu("Quick Actions", Vec::new(), false));
    condense(rows)
}

/// The choices of the Sort By menu, in menu order: title, key, ascending first.
/// `None` for a key the window has no data for.
pub(crate) fn sort_choices() -> [(&'static str, Option<(SortKey, bool)>); 8] {
    [
        ("None", Some((SortKey::None, true))),
        ("Name", Some((SortKey::Name, true))),
        ("Kind", Some((SortKey::Kind, true))),
        ("Date Last Opened", Some((SortKey::DateAccessed, false))),
        ("Date Added", None),
        ("Date Modified", Some((SortKey::DateModified, false))),
        ("Date Created", Some((SortKey::DateCreated, false))),
        ("Size", Some((SortKey::Size, false))),
    ]
}

fn sort_menu(current: SortSpec) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::command("None", Command::SortBy(SortKey::None, true), true)
            .checked(current.key == SortKey::None),
        MenuItem::separator(),
        MenuItem::unavailable("Snap to Grid"),
        MenuItem::separator(),
    ];
    for (title, choice) in sort_choices().into_iter().skip(1) {
        items.push(match choice {
            Some((key, ascending)) => {
                MenuItem::command(title, Command::SortBy(key, ascending), true)
                    .checked(current.key == key)
            }
            None => MenuItem::unavailable(title),
        });
    }
    items.push(
        MenuItem::command("Tags", Command::SortBy(SortKey::Tags, true), true)
            .checked(current.key == SortKey::Tags),
    );
    items
}

/// The Group By menu: the folder view does not draw groups yet, so every choice is disabled.
fn group_menu() -> Vec<MenuItem> {
    let mut items = vec![MenuItem::unavailable("None"), MenuItem::separator()];
    items.extend(
        [
            "Name",
            "Kind",
            "Application",
            "Date Last Opened",
            "Date Added",
            "Date Modified",
            "Date Created",
            "Size",
            "Tags",
        ]
        .map(MenuItem::unavailable),
    );
    items
}

/// Paste Item, with Move Item Here on Alt: they copy or move what Copy put on the pasteboard into the folder, when
/// `can` (something was copied and the folder takes items).
fn paste_row(can: bool) -> MenuItem {
    let row = |label: &str, command: Command| match can {
        true => MenuItem::command(label, command, true),
        false => MenuItem::unavailable(label),
    };
    row("Paste Item", Command::Paste)
        .with_alternate(Mods::ALT, row("Move Item Here", Command::MoveHere))
        .with_alternate(Mods::SHIFT_ALT, MenuItem::unavailable("Paste Item Exactly"))
}

/// The menu of the empty space of a folder.
pub fn background_menu(ctx: &Background) -> Vec<MenuItem> {
    let takes_new = !ctx.in_trash && !ctx.read_only;
    let mut rows = vec![
        // New Folder, which neither the trash nor a read-only file system takes
        MenuItem::command("New Folder", Command::NewFolder, takes_new),
        // Paste Item, Move Item Here on Alt, Paste Item Exactly on Shift + Alt
        paste_row(ctx.pasteboard > 0 && takes_new),
        MenuItem::separator(),
    ];
    // Empty Trash, in the trash only
    if ctx.in_trash {
        rows.push(MenuItem::unavailable("Empty Trash"));
    }
    rows.push(MenuItem::separator());
    // Get Info, Show Inspector on Alt, Get Summary Info on Ctrl
    rows.push(
        MenuItem::unavailable("Get Info")
            .with_alternate(Mods::ALT, MenuItem::unavailable("Show Inspector"))
            .with_alternate(Mods::CTRL, MenuItem::unavailable("Get Summary Info")),
    );
    rows.push(MenuItem::separator());
    // the view modes in a submenu of their own (their keys are the menu bar's View menu's), then how the view groups
    // and orders the items (Use Groups, Group By, Sort By, Clean Up By), then Show View Options
    rows.push(MenuItem::submenu(SHOW_AS, view_menu(ctx.view), true));
    rows.push(MenuItem::unavailable("Use Groups"));
    rows.push(MenuItem::submenu("Group By", group_menu(), false));
    rows.push(MenuItem::submenu("Sort By", sort_menu(ctx.sort), true));
    rows.push(MenuItem::submenu("Clean Up By", Vec::new(), false));
    rows.push(MenuItem::unavailable("Show View Options"));
    condense(rows)
}

/// The folder menu's submenu of the view modes ([`view_menu`]).
pub const SHOW_AS: &str = "Show As";

/// The toolbar's view-mode popup, and the folder menu's [`SHOW_AS`] submenu: the four view modes with the current one
/// checked; Gallery is disabled until the window has that view.
pub fn view_menu(current: ViewMode) -> Vec<MenuItem> {
    vec![
        MenuItem::command("Icons", Command::SetView(ViewMode::Icons), true)
            .checked(current == ViewMode::Icons),
        MenuItem::command("List", Command::SetView(ViewMode::List), true)
            .checked(current == ViewMode::List),
        MenuItem::command("Columns", Command::SetView(ViewMode::Columns), true)
            .checked(current == ViewMode::Columns),
        MenuItem::unavailable("Gallery"),
    ]
}

/// The toolbar's Tags popup: a row per tag colour in the tag row's order, checked when every selected item has it, all
/// disabled without a selection; a tag editor with a name field is still to come.
pub fn tags_menu(on: &[TagColor], has_selection: bool) -> Vec<MenuItem> {
    crate::context_menu::TAG_ROW_ORDER
        .iter()
        .map(|&color| {
            MenuItem::command(color.title(), Command::ToggleTag(color), has_selection)
                .checked(on.contains(&color))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(count: usize) -> Subject {
        Subject {
            count,
            name: "notes.txt".into(),
            all_folders: false,
            single_link: false,
            in_trash: false,
            tags_on: Vec::new(),
        }
    }

    fn apps() -> Vec<AppChoice> {
        vec![
            AppChoice {
                id: "viewer.desktop".into(),
                name: "Viewer".into(),
                is_default: true,
            },
            AppChoice {
                id: "editor.desktop".into(),
                name: "Editor".into(),
                is_default: false,
            },
        ]
    }

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items
            .iter()
            .map(|i| {
                if i.kind == ItemKind::Separator {
                    "-"
                } else {
                    i.label.as_str()
                }
            })
            .collect()
    }

    #[test]
    fn the_item_menu_of_a_file_keeps_its_row_order_with_condensed_separators() {
        let items = item_menu(&subject(1), &apps());
        assert_eq!(
            labels(&items),
            [
                "New Folder with Selection (1 Item)",
                "Open",
                "Open With",
                "-",
                "Move to Trash",
                "-",
                "Get Info",
                "Rename",
                "Compress “notes.txt”",
                "Duplicate",
                "Make Alias",
                "Quick Preview “notes.txt”",
                "-",
                "Copy “notes.txt”",
                "Share",
                "-",
                "Tags…",
                "-",
                "Quick Actions"
            ]
        );
    }

    #[test]
    fn rows_without_a_backend_are_present_and_disabled() {
        let items = item_menu(&subject(2), &apps());
        for title in ["Get Info", "Compress 2 Items", "Share", "Quick Actions"] {
            let item = items
                .iter()
                .find(|i| i.label == title)
                .unwrap_or_else(|| panic!("{title} is missing"));
            assert!(
                !item.enabled && !item.is_selectable(),
                "{title} has no backend"
            );
        }
        let doing = ["Open", "Move to Trash", "Duplicate", "Make Alias"]
            .map(|t| items.iter().find(|i| i.label == t).unwrap().enabled);
        assert_eq!(doing, [true; 4]);
    }

    #[test]
    fn rename_is_for_one_item_and_open_with_needs_an_application() {
        let items = item_menu(&subject(3), &[]);
        let enabled = |title: &str| items.iter().find(|i| i.label == title).unwrap().enabled;
        assert!(!enabled("Rename") && !enabled("Open With"));
        assert!(
            item_menu(&subject(1), &apps())
                .iter()
                .find(|i| i.label == "Rename")
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn open_with_lists_the_applications_with_the_default_marked_and_other_disabled() {
        let items = item_menu(&subject(1), &apps());
        let ItemKind::Submenu(sub) = &items.iter().find(|i| i.label == "Open With").unwrap().kind
        else {
            panic!("Open With is a submenu")
        };
        assert_eq!(labels(sub), ["Viewer (default)", "Editor", "-", "Other…"]);
        assert_eq!(
            sub[1].kind,
            ItemKind::Command(Command::OpenWith("editor.desktop".into()))
        );
        assert!(!sub[3].enabled);
    }

    #[test]
    fn alt_swaps_open_with_for_always_open_with_and_open_for_a_new_window_on_folders() {
        let mut folders = subject(2);
        folders.all_folders = true;
        let items = item_menu(&folders, &apps());
        let open = items.iter().find(|i| i.label == "Open").unwrap();
        assert_eq!(open.alternates[0].0, Mods::ALT);
        assert_eq!(open.alternates[0].1.label, "Open in New Windows");
        let with = items.iter().find(|i| i.label == "Open With").unwrap();
        assert_eq!(with.alternates[0].1.label, "Always Open With");
        assert!(
            item_menu(&subject(1), &apps())
                .iter()
                .find(|i| i.label == "Open")
                .unwrap()
                .alternates
                .is_empty(),
            "files have no new window"
        );
    }

    #[test]
    fn a_link_offers_show_original_and_the_trash_offers_put_back_instead_of_move_to_trash() {
        let mut link = subject(1);
        link.single_link = true;
        assert!(labels(&item_menu(&link, &apps())).contains(&"Show Original"));
        let mut trashed = subject(1);
        trashed.in_trash = true;
        let labels = labels(&item_menu(&trashed, &apps()))
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(
            labels.contains(&"Put Back".to_string())
                && !labels.contains(&"Move to Trash".to_string())
        );
        assert!(!labels.iter().any(|l| l.starts_with("New Folder")));
    }

    #[test]
    fn the_background_menu_follows_the_table_and_marks_the_sort() {
        let ctx = Background {
            view: ViewMode::Icons,
            sort: SortSpec::default(),
            in_trash: false,
            read_only: false,
            pasteboard: 0,
        };
        let items = background_menu(&ctx);
        assert_eq!(
            labels(&items),
            [
                "New Folder",
                "Paste Item",
                "-",
                "Get Info",
                "-",
                "Show As",
                "Use Groups",
                "Group By",
                "Sort By",
                "Clean Up By",
                "Show View Options"
            ]
        );
        let ItemKind::Submenu(sort) = &items[8].kind else {
            panic!("Sort By is a submenu")
        };
        assert_eq!(
            labels(sort),
            [
                "None",
                "-",
                "Snap to Grid",
                "-",
                "Name",
                "Kind",
                "Date Last Opened",
                "Date Added",
                "Date Modified",
                "Date Created",
                "Size",
                "Tags"
            ]
        );
        let checked: Vec<&str> = sort
            .iter()
            .filter(|i| i.checked)
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(checked, ["Name"]);
        assert_eq!(
            sort[5].kind,
            ItemKind::Command(Command::SortBy(SortKey::Kind, true))
        );
        assert!(!sort[7].enabled, "no date added in the listing");
        let ItemKind::Submenu(views) = &items[5].kind else {
            panic!("Show As is a submenu")
        };
        assert_eq!(views, &view_menu(ViewMode::Icons));
        assert_eq!(labels(views), ["Icons", "List", "Columns", "Gallery"]);
        assert!(views[0].checked && !views[1].checked);
        assert!(!views[3].enabled, "no gallery view yet");
    }

    #[test]
    fn a_read_only_folder_takes_no_new_folder_and_no_paste() {
        let ctx = Background {
            view: ViewMode::List,
            sort: SortSpec::default(),
            in_trash: false,
            read_only: true,
            pasteboard: 1,
        };
        let items = background_menu(&ctx);
        let row = |label: &str| items.iter().find(|i| i.label == label).unwrap();
        assert!(!row("New Folder").enabled);
        assert!(
            !row("Paste Item").enabled,
            "nothing is pasted onto read-only media"
        );
        assert!(row("Paste Item").alternates.iter().all(|(_, a)| !a.enabled));
        let writable = background_menu(&Background {
            read_only: false,
            ..ctx
        });
        assert!(
            writable
                .iter()
                .find(|i| i.label == "Paste Item")
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn paste_alternates_and_the_trash_row_are_offered_in_the_trash() {
        let ctx = Background {
            view: ViewMode::List,
            sort: SortSpec::default(),
            in_trash: true,
            read_only: false,
            pasteboard: 2,
        };
        let items = background_menu(&ctx);
        assert!(!items[0].enabled, "nothing is created inside the trash");
        let paste = items.iter().find(|i| i.label == "Paste Item").unwrap();
        assert!(!paste.enabled, "nothing is pasted into the trash");
        assert!(labels(&items).contains(&"Empty Trash"));
        let paste = items.iter().find(|i| i.label == "Paste Item").unwrap();
        let alts: Vec<(Mods, &str)> = paste
            .alternates
            .iter()
            .map(|(m, a)| (*m, a.label.as_str()))
            .collect();
        assert_eq!(
            alts,
            [
                (Mods::ALT, "Move Item Here"),
                (Mods::SHIFT_ALT, "Paste Item Exactly")
            ]
        );
    }

    #[test]
    fn condensing_drops_leading_trailing_and_doubled_separators() {
        let s = MenuItem::separator;
        let a = || MenuItem::unavailable("a");
        let out = condense(vec![s(), a(), s(), s(), a(), s()]);
        assert_eq!(labels(&out), ["a", "-", "a"]);
    }

    #[test]
    fn the_view_popup_checks_the_mode_and_disables_the_gallery() {
        let items = view_menu(ViewMode::List);
        assert_eq!(labels(&items), ["Icons", "List", "Columns", "Gallery"]);
        assert_eq!(
            items
                .iter()
                .map(|i| (i.enabled, i.checked))
                .collect::<Vec<_>>(),
            [(true, false), (true, true), (true, false), (false, false)]
        );
        assert_eq!(
            items[0].kind,
            ItemKind::Command(Command::SetView(ViewMode::Icons))
        );
    }

    #[test]
    fn the_tags_popup_toggles_each_colour_of_the_selection() {
        let items = tags_menu(&[TagColor::Blue], true);
        assert_eq!(
            labels(&items),
            ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray"]
        );
        assert_eq!(
            items
                .iter()
                .filter(|i| i.checked)
                .map(|i| i.label.as_str())
                .collect::<Vec<_>>(),
            ["Blue"]
        );
        assert_eq!(
            items[4].kind,
            ItemKind::Command(Command::ToggleTag(TagColor::Blue))
        );
        assert!(tags_menu(&[], false).iter().all(|i| !i.enabled));
    }
}

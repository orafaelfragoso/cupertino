//! The context menus of the folder view on Kit's menus. A secondary press on an item selects it unless it is selected
//! and opens the menu of the selection; on empty space it clears the selection and opens the folder's menu. The Menu
//! key and Shift+F10 open the selection's menu, or the folder's without a selection. What each menu offers is
//! [`crate::menu_spec`]; this module turns its rows into Kit's.
//!
//! * Kit's menus do not swap rows while modifiers are held, so a row's alternates that can act are rows of their own
//!   right after it ("Move Item Here" after "Paste Item", "Open in New Window" after "Open"); the others are left
//!   out (approximate).
//! * The row of tag colours is a Tags submenu of the seven colours, checked when every selected item has the colour
//!   (approximate).
//! * No row shows keys: the keys are the menu bar's, so a context menu is as wide as its longest label and its insets.

use cupertino_kit::controls::{Menu, MenuAnchor, MenuItem as KitItem};
use cupertino_kit::core::{
    Change, Element, Event, EventCx, Key, MeasureCx, PaintCx, Phase, Propagation, SemanticsCx,
    Widget, Window, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured,
};
use cupertino_kit::platform::{MouseButton, PointerEvent};

use super::{window_key, Event as WindowEvent, Message};
use crate::context_menu::Command as MenuCommand;
use crate::context_menu::{ItemKind, MenuItem, TAG_ROW_ORDER};

/// An open context menu of a window.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OpenMenu {
    /// Where it opens: at a point of the window, or below a toolbar button.
    pub anchor: MenuAnchor,
    /// What assistive technology calls it.
    pub label: String,
    pub items: Vec<MenuItem>,
    /// The open submenus, by the index of each one's row in the menu before it.
    pub open: Vec<usize>,
}

/// The popup windows of `menu`, open in the window `id`.
pub(super) fn windows(id: u64, menu: &OpenMenu) -> Vec<Window<Message>> {
    let send = move |event: WindowEvent| Message::Window(id, event);
    Menu {
        parent: window_key(id),
        anchor: menu.anchor.clone(),
        label: menu.label.clone(),
        items: kit_items(id, &menu.items),
        current: None,
        open: menu.open.clone(),
        on_open: Box::new(move |open| send(WindowEvent::MenuOpened(open))),
        on_close: send(WindowEvent::MenuClosed),
    }
    .windows(menu_key(id))
}

fn menu_key(id: u64) -> WindowKey {
    WindowKey(Key::from(format!("browser/{id}/context-menu")))
}

/// `items` as Kit's rows of the window `id`'s menu. A context menu shows no keys: they are the menu bar's, so every
/// row is a plain label and the menu is as wide as its longest one.
pub(super) fn kit_items(id: u64, items: &[MenuItem]) -> Vec<KitItem<Message>> {
    let command =
        move |label: &str, enabled: bool, checked: bool, command: MenuCommand| KitItem::Item {
            label: label.into(),
            enabled,
            checked,
            on_select: Message::Window(id, WindowEvent::MenuCommand(command)),
        };
    let mut out = Vec::new();
    for item in items {
        match &item.kind {
            ItemKind::Command(c) => {
                out.push(command(&item.label, item.enabled, item.checked, c.clone()))
            }
            ItemKind::Unavailable => out.push(KitItem::Item {
                label: item.label.clone(),
                enabled: false,
                checked: false,
                on_select: Message::Window(id, WindowEvent::MenuClosed),
            }),
            ItemKind::Submenu(sub) => out.push(KitItem::Submenu {
                label: item.label.clone(),
                enabled: item.enabled && !sub.is_empty(),
                items: kit_items(id, sub),
            }),
            ItemKind::Separator => out.push(KitItem::Separator),
            ItemKind::TagRow { on } => out.push(KitItem::Submenu {
                label: "Tags".into(),
                enabled: item.enabled,
                items: TAG_ROW_ORDER
                    .iter()
                    .map(|&color| {
                        command(
                            color.title(),
                            true,
                            on.contains(&color),
                            MenuCommand::ToggleTag(color),
                        )
                    })
                    .collect(),
            }),
        }
        for (_, alternate) in &item.alternates {
            // a submenu alternate ("Always Open With") would only repeat its row's choices here
            if alternate.is_selectable() && matches!(alternate.kind, ItemKind::Command(_)) {
                out.extend(kit_items(id, std::slice::from_ref(alternate)));
            }
        }
    }
    out
}

/// A region whose secondary press sends `message` before the press goes on to what holds it: the context menu then
/// opens with what was pressed known.
pub(super) struct Secondary {
    pub message: Message,
    pub children: Vec<Element<Message>>,
}

impl Widget<Message> for Secondary {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() != Phase::Capture
            && matches!(
                event,
                Event::Pointer(
                    PointerEvent::Button {
                        button: MouseButton::Right,
                        pressed: true,
                        ..
                    },
                    _,
                )
            )
        {
            cx.emit(self.message.clone());
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu_spec::{background_menu, item_menu, Background, Subject};
    use crate::sort::SortSpec;
    use crate::tags::TagColor;
    use crate::window::ViewMode;

    fn labels(items: &[KitItem<Message>]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                KitItem::Item {
                    label,
                    enabled,
                    checked,
                    ..
                } => {
                    format!(
                        "{label}{}{}",
                        if *enabled { "" } else { " (off)" },
                        if *checked { " ✓" } else { "" }
                    )
                }
                KitItem::Shortcut {
                    label, shortcut, ..
                } => format!("{label} [{shortcut}]"),
                KitItem::Submenu { label, enabled, .. } => {
                    format!("{label} ›{}", if *enabled { "" } else { " (off)" })
                }
                KitItem::Separator => "-".into(),
            })
            .collect()
    }

    #[test]
    fn the_folder_menu_offers_the_views_with_the_current_one_checked() {
        let menu = background_menu(&Background {
            view: ViewMode::List,
            sort: SortSpec::default(),
            in_trash: false,
            read_only: false,
            pasteboard: 0,
        });
        let kit = kit_items(0, &menu);
        let rows = labels(&kit);
        assert!(rows.contains(&"New Folder".to_owned()), "{rows:?}");
        assert!(rows.contains(&"Show As ›".to_owned()), "{rows:?}");
        let (views, on_select) = kit
            .iter()
            .find_map(|item| match item {
                KitItem::Submenu { label, items, .. } if label == "Show As" => Some((
                    labels(items),
                    items
                        .iter()
                        .map(|item| match item {
                            KitItem::Item { on_select, .. } => on_select.clone(),
                            _ => panic!("a view is a plain row"),
                        })
                        .collect::<Vec<_>>(),
                )),
                _ => None,
            })
            .expect("a Show As submenu");
        assert_eq!(views, ["Icons", "List ✓", "Columns", "Gallery"]);
        assert_eq!(
            on_select,
            [
                ViewMode::Icons,
                ViewMode::List,
                ViewMode::Columns,
                ViewMode::Gallery
            ]
            .map(|mode| Message::Window(0, WindowEvent::MenuCommand(MenuCommand::SetView(mode))))
        );
    }

    #[test]
    fn the_tag_row_is_a_submenu_checked_where_every_item_has_the_colour() {
        let subject = Subject {
            count: 2,
            name: "a.txt".into(),
            all_folders: false,
            single_link: false,
            in_trash: false,
            tags_on: vec![TagColor::Blue],
        };
        let menu = kit_items(3, &item_menu(&subject, &[]));
        let tags = menu
            .iter()
            .find_map(|item| match item {
                KitItem::Submenu { label, items, .. } if label == "Tags" => Some(labels(items)),
                _ => None,
            })
            .expect("a Tags submenu");
        assert_eq!(tags.len(), 7);
        assert!(tags.contains(&"Blue ✓".to_owned()), "{tags:?}");
        assert!(tags.contains(&"Red".to_owned()), "{tags:?}");
        // nothing opens with no application: the submenu is off
        assert!(labels(&menu).contains(&"Open With › (off)".to_owned()));
    }

    /// A context menu shows no keys, not even for rows the menu bar has keys for: every row of every menu, submenus
    /// too, is a plain label.
    #[test]
    fn no_row_of_a_context_menu_shows_a_key() {
        let subject = Subject {
            count: 1,
            name: "a.txt".into(),
            all_folders: false,
            single_link: false,
            in_trash: false,
            tags_on: Vec::new(),
        };
        let background = Background {
            view: ViewMode::Icons,
            sort: SortSpec::default(),
            in_trash: false,
            read_only: false,
            pasteboard: 1,
        };
        let several = Subject {
            count: 3,
            ..subject.clone()
        };
        fn every(items: &[KitItem<Message>], out: &mut Vec<String>) {
            out.extend(labels(items));
            for item in items {
                if let KitItem::Submenu { items, .. } = item {
                    every(items, out);
                }
            }
        }
        // rows the menu bar has keys for, each a plain label here
        let keyed = ["Open", "Move to Trash", "Duplicate", "Make Alias"];
        for (menu, plain) in [
            (item_menu(&subject, &[]), &keyed[..]),
            (item_menu(&several, &[]), &keyed[..]),
            (
                background_menu(&background),
                &["New Folder", "Icons ✓", "List", "Columns"][..],
            ),
        ] {
            let mut rows = Vec::new();
            every(&kit_items(3, &menu), &mut rows);
            assert!(rows.iter().all(|row| !row.contains('[')), "{rows:?}");
            for row in plain {
                assert!(rows.contains(&row.to_string()), "{row}: {rows:?}");
            }
        }
    }
}

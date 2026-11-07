//! The context menus of the folder view on Kit's menus. A secondary press on an item selects it unless it is selected
//! and opens the menu of the selection; on empty space it clears the selection and opens the folder's menu. The Menu
//! key and Shift+F10 open the selection's menu, or the folder's without a selection. What each menu offers is
//! [`crate::menu_spec`]; this module turns its rows into Kit's.
//!
//! * Kit's menus do not swap rows while modifiers are held, so a row's alternates are not offered; their actions are
//!   in the menu bar (approximate).
//! * The row of tag colours is a Tags submenu of the seven colours, checked when every selected item has the colour;
//!   the row of view modes is four rows, the current one checked (approximate).

use cupertino_kit::controls::{Menu, MenuAnchor, MenuItem as KitItem};
use cupertino_kit::core::{
    Change, Element, Event, EventCx, Key, MeasureCx, PaintCx, Phase, Propagation, SemanticsCx,
    Widget, Window, WindowKey,
};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured,
};
use cupertino_kit::platform::{MouseButton, PointerEvent};

use super::{window_key, Event as WindowEvent, Message};
use crate::context_menu::{Command as MenuCommand, ViewSegment};
use crate::context_menu::{ItemKind, MenuItem, TAG_ROW_ORDER, VIEW_ROW_ORDER};

/// An open context menu of a window.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OpenMenu {
    /// Where it opens, in the window.
    pub at: Point,
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
        anchor: MenuAnchor::At(menu.at),
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

/// What the view-mode row reads as rows.
fn view_label(segment: ViewSegment) -> &'static str {
    match segment {
        ViewSegment::Icons => "as Icons",
        ViewSegment::List => "as List",
        ViewSegment::Columns => "as Columns",
        ViewSegment::Gallery => "as Gallery",
    }
}

/// `items` as Kit's rows of the window `id`'s menu.
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
            ItemKind::ViewRow { current } => {
                for segment in VIEW_ROW_ORDER {
                    let mode = segment.mode();
                    let row = match mode {
                        Some(mode) => command(
                            view_label(segment),
                            true,
                            mode == *current,
                            MenuCommand::SetView(mode),
                        ),
                        None => KitItem::Item {
                            label: view_label(segment).into(),
                            enabled: false,
                            checked: false,
                            on_select: Message::Window(id, WindowEvent::MenuClosed),
                        },
                    };
                    out.push(row);
                }
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
    use crate::window::ViewMode;

    /// The view mode each row of the view row stands for.
    fn modes() -> Vec<Option<ViewMode>> {
        VIEW_ROW_ORDER.iter().map(|s| s.mode()).collect()
    }
    use crate::menu_spec::{background_menu, item_menu, Background, Subject};
    use crate::sort::SortSpec;
    use crate::tags::TagColor;

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
                KitItem::Submenu { label, enabled, .. } => {
                    format!("{label} ›{}", if *enabled { "" } else { " (off)" })
                }
                KitItem::Separator => "-".into(),
            })
            .collect()
    }

    #[test]
    fn the_folder_menu_offers_the_two_views_with_the_current_one_checked() {
        let menu = background_menu(&Background {
            view: ViewMode::List,
            sort: SortSpec::default(),
            in_trash: false,
        });
        let rows = labels(&kit_items(0, &menu));
        assert!(rows.contains(&"New Folder".to_owned()), "{rows:?}");
        let views: Vec<&String> = rows.iter().filter(|r| r.starts_with("as ")).collect();
        assert_eq!(
            views,
            [
                "as Icons",
                "as List ✓",
                "as Columns (off)",
                "as Gallery (off)"
            ]
        );
        assert_eq!(
            modes(),
            [Some(ViewMode::Icons), Some(ViewMode::List), None, None]
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
}

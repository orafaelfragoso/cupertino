//! Menus in the `org.gtk.Menus` layout: one group of numbered menus whose items link to sections and submenus.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cupertino_menu_model::{ActionName, Command, Entry, Menu};
use cupertino_modifiers::ModifierMap;
use zbus::zvariant::{OwnedValue, Str, Value};

/// The group every menu of an object is in; a client subscribes to it once.
pub(crate) const GROUP: u32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Link {
    Section(u32),
    Submenu(u32),
}

/// One item of a menu as the protocol sends it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WireItem {
    pub label: Option<String>,
    pub action: Option<String>,
    pub accel: Option<String>,
    /// The picture before the label, a template symbol name.
    pub icon: Option<String>,
    pub link: Option<Link>,
}

impl WireItem {
    pub fn to_dict(&self) -> HashMap<String, OwnedValue> {
        let text = |s: &str| OwnedValue::from(Str::from(s.to_string()));
        let mut dict = HashMap::new();
        // a label has no mnemonic: each `_` doubled stays a literal underscore
        let label = self.label.as_ref().map(|l| l.replace('_', "__"));
        for (key, value) in [
            ("label", &label),
            ("action", &self.action),
            ("accel", &self.accel),
            ("icon", &self.icon),
        ] {
            if let Some(value) = value {
                dict.insert(key.to_string(), text(value));
            }
        }
        if let Some(link) = self.link {
            let (key, menu) = match link {
                Link::Section(m) => (":section", m),
                Link::Submenu(m) => (":submenu", m),
            };
            let pair = Value::from((GROUP, menu))
                .try_into_owned()
                .expect("a pair of numbers holds no file descriptor");
            dict.insert(key.to_string(), pair);
        }
        dict
    }
}

/// The menus of one exported object, by number; menu 0 is the root.
pub(crate) type WireMenus = BTreeMap<u32, Vec<WireItem>>;

struct Encoder<'a> {
    map: &'a dyn ModifierMap,
    menus: WireMenus,
    actions: BTreeSet<ActionName>,
}

impl Encoder<'_> {
    fn reserve(&mut self) -> u32 {
        let id = self.menus.len() as u32;
        self.menus.insert(id, Vec::new());
        id
    }

    /// Menu `id` as one section item per group of `menu`.
    fn sections(&mut self, id: u32, menu: &Menu) {
        let mut items = Vec::new();
        for section in &menu.sections {
            let sid = self.reserve();
            let entries = section.iter().map(|e| self.entry(e)).collect();
            self.menus.insert(sid, entries);
            items.push(WireItem {
                link: Some(Link::Section(sid)),
                ..WireItem::default()
            });
        }
        self.menus.insert(id, items);
    }

    fn entry(&mut self, entry: &Entry) -> WireItem {
        match entry {
            Entry::Item(item) => {
                let action = match &item.command {
                    Command::Action(a) => {
                        self.actions.insert(a.clone());
                        Some(a.full())
                    }
                    Command::System(_) => None,
                };
                WireItem {
                    label: Some(item.label.clone()),
                    action,
                    accel: item.chord.as_ref().and_then(|c| c.to_accel(self.map)),
                    icon: item.icon.clone(),
                    link: None,
                }
            }
            Entry::Submenu(menu) => WireItem {
                label: Some(menu.title.clone()),
                link: Some(Link::Submenu(self.submenu(menu))),
                ..WireItem::default()
            },
        }
    }

    fn submenu(&mut self, menu: &Menu) -> u32 {
        let id = self.reserve();
        self.sections(id, menu);
        id
    }
}

/// An encoded object: its menus and the actions they name.
pub(crate) struct Encoded {
    pub menus: WireMenus,
    pub actions: BTreeSet<ActionName>,
}

fn encoder(map: &dyn ModifierMap) -> Encoder<'_> {
    Encoder {
        map,
        menus: WireMenus::new(),
        actions: BTreeSet::new(),
    }
}

/// A menu bar: the root holds one submenu item per menu.
pub(crate) fn menu_bar(menus: &[Menu], map: &dyn ModifierMap) -> Encoded {
    let mut e = encoder(map);
    let root = e.reserve();
    let items = menus
        .iter()
        .map(|menu| WireItem {
            label: Some(menu.title.clone()),
            link: Some(Link::Submenu(e.submenu(menu))),
            ..WireItem::default()
        })
        .collect();
    e.menus.insert(root, items);
    Encoded {
        menus: e.menus,
        actions: e.actions,
    }
}

/// A single menu (the application menu): the root holds its sections.
pub(crate) fn menu(menu: &Menu, map: &dyn ModifierMap) -> Encoded {
    let mut e = encoder(map);
    let root = e.reserve();
    e.sections(root, menu);
    Encoded {
        menus: e.menus,
        actions: e.actions,
    }
}

/// `Changed` records that turn `old` into `new`: each menu that differs is replaced whole.
pub(crate) fn changes(
    old: &WireMenus,
    new: &WireMenus,
) -> Vec<(u32, u32, u32, u32, Vec<WireItem>)> {
    let ids: BTreeSet<u32> = old.keys().chain(new.keys()).copied().collect();
    ids.into_iter()
        .filter(|id| old.get(id) != new.get(id))
        .map(|id| {
            let removed = old.get(&id).map_or(0, Vec::len) as u32;
            let added = new.get(&id).cloned().unwrap_or_default();
            (GROUP, id, 0, removed, added)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_menu_model::{Chord, Item, Scope};
    use cupertino_modifiers::{Modifier, Physical};

    struct SuperIsPrimary;

    impl ModifierMap for SuperIsPrimary {
        fn physical(&self, m: Modifier) -> Option<Physical> {
            match m {
                Modifier::Primary => Some(Physical::Super),
                Modifier::Shift => Some(Physical::Shift),
                _ => None,
            }
        }

        fn logical(&self, _: Physical) -> Option<Modifier> {
            None
        }
    }

    fn item(label: &str, scope: Scope, name: &str, chord: Option<Chord>) -> Entry {
        Entry::Item(Item::new(
            label,
            ActionName::new(scope, name).unwrap(),
            chord,
        ))
    }

    fn labelled(label: &str, link: Link) -> WireItem {
        WireItem {
            label: Some(label.to_string()),
            link: Some(link),
            ..WireItem::default()
        }
    }

    fn section(id: u32) -> WireItem {
        WireItem {
            link: Some(Link::Section(id)),
            ..WireItem::default()
        }
    }

    fn action(label: &str, action: &str, accel: Option<&str>) -> WireItem {
        WireItem {
            label: Some(label.to_string()),
            action: Some(action.to_string()),
            accel: accel.map(str::to_string),
            icon: None,
            link: None,
        }
    }

    #[test]
    fn a_menu_bar_links_menus_sections_and_submenus() {
        let find = Menu::new("Find", vec![vec![item("Find…", Scope::Win, "find", None)]]);
        let edit = Menu::new(
            "Edit",
            vec![
                vec![item(
                    "Undo",
                    Scope::Win,
                    "undo",
                    Chord::new(&[Modifier::Primary], "z"),
                )],
                vec![Entry::Submenu(find)],
            ],
        );
        let file = Menu::new("File", vec![vec![item("New", Scope::App, "new", None)]]);
        let encoded = menu_bar(&[file, edit], &SuperIsPrimary);
        let expected: WireMenus = [
            (
                0,
                vec![
                    labelled("File", Link::Submenu(1)),
                    labelled("Edit", Link::Submenu(3)),
                ],
            ),
            (1, vec![section(2)]),
            (2, vec![action("New", "app.new", None)]),
            (3, vec![section(4), section(5)]),
            (4, vec![action("Undo", "win.undo", Some("<Super>z"))]),
            (5, vec![labelled("Find", Link::Submenu(6))]),
            (6, vec![section(7)]),
            (7, vec![action("Find…", "win.find", None)]),
        ]
        .into_iter()
        .collect();
        assert_eq!(encoded.menus, expected);
        let actions: Vec<String> = encoded.actions.iter().map(ActionName::full).collect();
        assert_eq!(actions, ["app.new", "win.find", "win.undo"]);
    }

    #[test]
    fn a_chord_without_keys_in_the_profile_has_no_accelerator() {
        let menu_ = Menu::new(
            "View",
            vec![vec![item(
                "Full Screen",
                Scope::Win,
                "fullscreen",
                Chord::new(&[Modifier::Primary, Modifier::Ctrl], "f"),
            )]],
        );
        let encoded = menu(&menu_, &SuperIsPrimary);
        assert_eq!(
            encoded.menus[&1],
            vec![action("Full Screen", "win.fullscreen", None)]
        );
    }

    #[test]
    fn items_become_attribute_dictionaries() {
        let dict = action("Quit", "app.quit", Some("<Super>q")).to_dict();
        let mut keys: Vec<&str> = dict.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["accel", "action", "label"]);
        assert_eq!(<&str>::try_from(&dict["accel"]).unwrap(), "<Super>q");
        let dict = action("my_notes.txt", "win.open-recent", None).to_dict();
        assert_eq!(<&str>::try_from(&dict["label"]).unwrap(), "my__notes.txt");
        let link = section(4).to_dict();
        assert_eq!(
            <(u32, u32)>::try_from(link[":section"].try_clone().unwrap()).unwrap(),
            (0, 4)
        );
    }

    #[test]
    fn only_menus_that_differ_are_sent_again() {
        let old: WireMenus = [
            (0, vec![section(1)]),
            (1, vec![action("Undo", "win.undo", None)]),
            (2, vec![]),
        ]
        .into_iter()
        .collect();
        let new: WireMenus = [
            (0, vec![section(1)]),
            (1, vec![action("Undo Typing", "win.undo", None)]),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            changes(&old, &new),
            [
                (0, 1, 0, 1, vec![action("Undo Typing", "win.undo", None)]),
                (0, 2, 0, 0, vec![]),
            ]
        );
        assert_eq!(changes(&new, &new), []);
    }
}

//! Menus in the `org.gtk.Menus` layout: one group of numbered menus whose items link to sections and submenus.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cupertino_keyboard::modifiers::ModifierMap;
use cupertino_menu_model::{ActionName, Command, Entry, Menu};
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

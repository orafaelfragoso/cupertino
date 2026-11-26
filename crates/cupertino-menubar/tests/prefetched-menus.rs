//! An application's top-level menus that its exporter leaves empty until they are about to show are loaded before the
//! bar first gets the tree, as a Chromium exporter (Electron applications) needs: a File that holds only Settings…, a
//! separator, a hidden item and Quit, which the merge moves to the App menu, never shows as a title that goes when it
//! is clicked. The exporter runs on a private session bus, with `AboutToShowGroup` and without it. A title that goes
//! under an open menu and a held press closes it cleanly.

// a test owns its thread: it blocks on the bus
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;

use cupertino_kit_shell::menu_bar::{ActiveApp, Update, WindowMenu};
use cupertino_menubar::controller::{Effect, MenuController};
use cupertino_menubar::dbus::dbusmenu::{make_node, RawNode};
use cupertino_menubar::desktop::AppName;
use cupertino_menubar::fallback::FallbackAction;
use cupertino_menubar::feed::{BarFeed, Shared};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::item::{ItemId, MenuItem};
use cupertino_menubar::layout::{BarContent, MonoMeasure, SlotKey};
use cupertino_menubar::source::MenuSource;
use cupertino_menubar::worker::{Event, Worker};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

use crate::bus::{soon, within, Bus};

const PATH: &str = "/com/canonical/menu/1";
/// The first id the exporter gives the rows of a menu; the top-level menus have 2 to 6.
const FIRST_ROW_ID: i32 = 7;

/// A row of a menu: its label, whether it is a separator, whether it is visible.
type Row = (&'static str, bool, bool);

/// The exporter's top-level menus, by id, title and rows.
const MENUS: [(i32, &str, &[Row]); 5] = [
    (
        2,
        "File",
        &[
            ("Settings...", false, true),
            ("", true, true),
            ("Close Developer Tools", false, false),
            ("Quit", false, true),
        ],
    ),
    (3, "Edit", &[("Undo", false, true), ("Copy", false, true)]),
    (4, "View", &[("Reload", false, true)]),
    (5, "Window", &[("Minimize", false, true)]),
    (6, "Help", &[("Learn More", false, true)]),
];

fn value(v: impl Into<Value<'static>>) -> OwnedValue {
    OwnedValue::try_from(v.into()).unwrap()
}

/// The menus as a Chromium exporter keeps them: a menu's rows are given once it was about to show, under fresh ids
/// each time it is.
struct Menus {
    /// The ids of each menu's rows, once given.
    rows: HashMap<i32, Vec<i32>>,
    next: i32,
    revision: u32,
    /// The events the exporter was sent: the item's id and the event's name.
    events: async_channel::Sender<(i32, String)>,
}

impl Menus {
    fn new(events: async_channel::Sender<(i32, String)>) -> Menus {
        Menus {
            rows: HashMap::new(),
            next: FIRST_ROW_ID,
            revision: 1,
            events,
        }
    }

    fn tree(&self) -> RawNode {
        let menus = MENUS
            .iter()
            .map(|(id, title, rows)| {
                let given = self.rows.get(id).map(|ids| {
                    rows.iter()
                        .zip(ids)
                        .map(|((label, separator, visible), row)| {
                            let mut props =
                                HashMap::from([("visible".to_owned(), value(*visible))]);
                            if *separator {
                                props.insert("type".into(), value("separator"));
                            } else {
                                props.insert("label".into(), value(*label));
                            }
                            make_node(*row, props, Vec::new()).unwrap()
                        })
                        .collect()
                });
                let props = HashMap::from([
                    ("label".to_owned(), value(*title)),
                    ("children-display".to_owned(), value("submenu")),
                ]);
                make_node(*id, props, given.unwrap_or_default()).unwrap()
            })
            .collect();
        make_node(0, HashMap::new(), menus).unwrap()
    }

    /// The menu `id` is about to show: its rows are made again under fresh ids.
    fn show(&mut self, id: i32) {
        let Some((_, _, rows)) = MENUS.iter().find(|(menu, ..)| *menu == id) else {
            return;
        };
        let ids = (0..rows.len()).map(|i| self.next + i as i32).collect();
        self.next += rows.len() as i32;
        self.rows.insert(id, ids);
        self.revision += 1;
    }
}

/// The exporter with `AboutToShowGroup`.
struct Grouped(Menus);
/// The exporter without it: a version 3 that answers it has no such method.
struct Single(Menus);

macro_rules! exporter {
    ($name:ident { $($group:item)* }) => {
        #[zbus::interface(name = "com.canonical.dbusmenu")]
        impl $name {
            fn get_layout(&self, _parent: i32, _depth: i32, _names: Vec<String>) -> (u32, RawNode) {
                (self.0.revision, self.0.tree())
            }

            async fn about_to_show(
                &mut self,
                id: i32,
                #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
            ) -> bool {
                self.0.show(id);
                Self::layout_updated(&emitter, self.0.revision, 0).await.unwrap();
                false
            }

            $($group)*

            fn event(&self, id: i32, event: String, _data: OwnedValue, _time: u32) {
                let _ = self.0.events.try_send((id, event));
            }

            #[zbus(signal)]
            async fn layout_updated(
                emitter: &SignalEmitter<'_>,
                revision: u32,
                parent: i32,
            ) -> zbus::Result<()>;

            #[zbus(property)]
            fn version(&self) -> u32 {
                3
            }

            #[zbus(property)]
            fn status(&self) -> String {
                "normal".into()
            }
        }
    };
}

exporter!(Grouped {
    async fn about_to_show_group(
        &mut self,
        ids: Vec<i32>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> (Vec<i32>, Vec<i32>) {
        for id in &ids {
            self.0.show(*id);
            Self::layout_updated(&emitter, self.0.revision, 0).await.unwrap();
        }
        (Vec::new(), Vec::new())
    }
});

exporter!(Single {});

/// The bar of one output, fed by the worker as the component feeds it.
struct Bar {
    feed: BarFeed,
    shared: Shared,
    controller: MenuController,
}

impl Bar {
    fn new(service: &str) -> Bar {
        let mut shared = Shared::default();
        let mut feed = BarFeed::new(0, false, false);
        let update = Update {
            active_app: Some(ActiveApp {
                app_id: "t3code".into(),
                title: "T3 Code".into(),
            }),
            menu: Some(WindowMenu::DbusMenu {
                service: service.into(),
                path: PATH.into(),
            }),
            ..Update::default()
        };
        feed.apply(&update, &mut shared, |_| AppName {
            name: "T3 Code".into(),
            has_entry: false,
        });
        let output = Rect::new(0.0, 0.0, 1600.0, 900.0);
        let controller = MenuController::new(output, Box::new(MonoMeasure::default()));
        Bar {
            feed,
            shared,
            controller,
        }
    }

    fn apply(&mut self, source: &MenuSource, root: MenuItem) {
        self.shared.apply_layout(source, root);
        let content = self.feed.content(&self.shared);
        self.controller.set_content(content);
    }

    fn titles(&self) -> Vec<String> {
        let menus = &self.controller.content().menus;
        menus.iter().map(|m| m.label.clone()).collect()
    }
}

/// The next tree the worker reports.
async fn next_tree(events: &async_channel::Receiver<Event>) -> (MenuSource, MenuItem) {
    within("a tree", async {
        loop {
            match events.recv().await.unwrap() {
                Event::Layout { source, root } => break (source, *root),
                Event::LayoutFailed { source } => panic!("the menu was given up: {source:?}"),
                _ => {}
            }
        }
    })
    .await
}

/// The first tree the bar gets from `exporter` has no File, the App menu's Settings… and Quit invoke the exporter's
/// items, and the other menus are loaded; no `opened` is sent for the menus told they are about to show.
fn first_tree_is_merged<T: zbus::object_server::Interface>(exporter: impl FnOnce(Menus) -> T) {
    let bus = Bus::start();
    async_io::block_on(async {
        let bar_conn = bus.connect().await;
        let app = bus.connect().await;
        let (told, sent) = async_channel::unbounded();
        app.object_server()
            .at(PATH, exporter(Menus::new(told)))
            .await
            .unwrap();
        let service = app.unique_name().unwrap().to_string();
        let (worker, events) = Worker::start(bar_conn).await.unwrap();
        let mut bar = Bar::new(&service);
        let fetch = bar.feed.request_layout(&mut bar.shared).expect("a load");
        worker.send(fetch).unwrap();
        let (source, root) = next_tree(&events).await;
        bar.apply(&source, root);

        let titles = bar.titles();
        assert!(!titles.iter().any(|t| t == "File"), "{titles:?}");
        for title in ["Edit", "View", "Window", "Help"] {
            assert!(titles.iter().any(|t| t == title), "{title} in {titles:?}");
        }
        let edit = bar
            .controller
            .content()
            .menus
            .iter()
            .find(|m| m.label == "Edit");
        let rows: Vec<&str> = edit
            .unwrap()
            .children
            .iter()
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(rows, ["Undo", "Copy"], "Edit was loaded");

        // File's rows were given first, from FIRST_ROW_ID: Settings… then, past the separator and the hidden item, Quit
        let settings = bar.feed.rebound(&bar.shared, FallbackAction::Settings);
        let quit = bar.feed.rebound(&bar.shared, FallbackAction::Quit);
        assert_eq!(settings, Some(ItemId(FIRST_ROW_ID)));
        assert_eq!(quit, Some(ItemId(FIRST_ROW_ID + 3)));
        let app_menu = &bar.controller.content().app_menu;
        let item = app_menu
            .iter()
            .find(|i| i.id == FallbackAction::Settings.item_id());
        assert!(
            item.is_some_and(|i| i.enabled),
            "Settings… invokes the exporter's"
        );

        assert_eq!(soon(sent.recv()).await, None, "no event for the menus told");
    });
}

#[test]
fn prefetched_menus_load_before_the_first_tree_with_group_calls() {
    first_tree_is_merged(Grouped);
}

#[test]
fn prefetched_menus_load_before_the_first_tree_without_group_calls() {
    first_tree_is_merged(Single);
}

fn content(menus: Vec<MenuItem>) -> BarContent {
    BarContent {
        app_name: "T3 Code".into(),
        menus,
        ..BarContent::default()
    }
}

#[test]
fn prefetched_menus_a_title_gone_under_a_held_press_closes_its_menu() {
    let file = MenuItem::submenu(2, "File", vec![MenuItem::new(7, "New")]);
    let edit = MenuItem::submenu(3, "Edit", vec![MenuItem::new(8, "Undo")]);
    let output = Rect::new(0.0, 0.0, 1600.0, 900.0);
    let mut c = MenuController::new(output, Box::new(MonoMeasure::default()));
    c.set_content(content(vec![file, edit.clone()]));
    let slot = c.layout().slot(&SlotKey::Menu(ItemId(2))).unwrap().rect;
    let at = Point::new(slot.x + slot.w / 2.0, slot.y + 1.0);
    c.pointer_press(at, 0.0);
    assert!(c.is_open(), "File opened under the press");

    let outcome = c.set_content(content(vec![edit]));
    assert!(outcome.effects.contains(&Effect::Closed));
    assert!(outcome.effects.iter().any(|e| matches!(
        e,
        Effect::MenuClosed {
            item: ItemId(2),
            ..
        }
    )));
    assert!(!c.is_open());
    // the release of the press, over Edit now in File's place, opens nothing and chooses nothing
    let released = c.pointer_release(at, 1.0);
    assert!(!c.is_open());
    assert!(!released
        .effects
        .iter()
        .any(|e| matches!(e, Effect::MenuOpened { .. } | Effect::Activate { .. })));
}

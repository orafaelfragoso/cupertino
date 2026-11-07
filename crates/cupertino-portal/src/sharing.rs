//! The menu-bar item of a running screen share: a StatusNotifierItem with a DBusMenu that names the application and
//! what it shares, and a Stop Sharing item that ends the share. Each share has its own item, under a bus name of its own
//! (`org.kde.StatusNotifierItem-<pid>-<n>`), so the menu bar drops it as soon as the name is released. The server's
//! privacy dot shows next to it for as long as any capture runs; this item adds who shares and the stop control.
//!
//! The item registers with `org.kde.StatusNotifierWatcher` (the menu bar), and again whenever a watcher takes the name,
//! so a menu bar that restarts shows it too. Without a watcher the item is still served, and the share runs on.

use std::collections::HashMap;

use futures_lite::StreamExt;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};
use zbus::fdo::DBusProxy;
use zbus::names::WellKnownName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, StructureBuilder, Value};
use zbus::{interface, Connection};

/// The bus name of the menu bar's item registry.
pub const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
/// The menu item that names who shares what.
pub const ITEM_SHARING: i32 = 1;
const ITEM_SEPARATOR: i32 = 2;
/// The menu item that ends the share.
pub const ITEM_STOP: i32 = 3;
/// Device pixels of the item's icon renditions.
const ICON_PX: [u32; 2] = [16, 32];
/// The colour of the item's glyph, the indigo the capture indicator shows in: red, green, blue.
const ICON_COLOUR: [u8; 3] = [84, 82, 204];

/// What the item says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The application, by its desktop entry's name, else its id.
    pub app: String,
    /// The display or window shared.
    pub source: String,
}

impl Shown {
    /// The text of the first menu item and the tooltip.
    pub fn sentence(&self) -> String {
        format!("“{}” is sharing “{}”", self.app, self.source)
    }
}

/// The objects of one share's item; [`Indicator::remove`] takes them off the bus.
pub struct Indicator {
    connection: Connection,
    name: String,
    path: OwnedObjectPath,
    gone: async_channel::Sender<()>,
}

impl Indicator {
    /// Serve the item of share `n` on `connection` and register it; Stop Sharing sends on `stop`. `None` when the item
    /// cannot be served, which leaves the share without it.
    pub async fn show(
        connection: &Connection,
        n: u64,
        shown: Shown,
        stop: async_channel::Sender<()>,
    ) -> Option<Indicator> {
        let name = format!("org.kde.StatusNotifierItem-{}-{n}", std::process::id());
        let path = OwnedObjectPath::try_from(format!("/org/cupertino/Portal/sharing/{n}")).ok()?;
        let server = connection.object_server();
        let added = server
            .at(
                &path,
                Item {
                    shown: shown.clone(),
                    path: path.clone(),
                },
            )
            .await
            .ok()?
            && server.at(&path, Menu { shown, stop }).await.ok()?;
        if !added {
            return None;
        }
        let well_known = WellKnownName::try_from(name.clone()).ok()?;
        if connection.request_name(well_known).await.is_err() {
            let _ = server.remove::<Item, _>(&path).await;
            let _ = server.remove::<Menu, _>(&path).await;
            return None;
        }
        let (gone, removed) = async_channel::bounded::<()>(1);
        let id = format!("{name}{}", path.as_str());
        let watcher = connection.clone();
        connection
            .executor()
            .spawn(
                async move {
                    futures_lite::future::or(register(&watcher, &id), async {
                        let _ = removed.recv().await;
                    })
                    .await
                },
                "cupertino-portal-sharing-item",
            )
            .detach();
        Some(Indicator {
            connection: connection.clone(),
            name,
            path,
            gone,
        })
    }

    /// The path the item and its menu are served at.
    pub fn path(&self) -> &OwnedObjectPath {
        &self.path
    }

    /// The bus name the item is registered under.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Take the item off the bus: the name is released, so the menu bar drops it.
    pub async fn remove(self) {
        let _ = self.gone.try_send(());
        if let Ok(name) = WellKnownName::try_from(self.name.as_str()) {
            let _ = self.connection.release_name(name).await;
        }
        let server = self.connection.object_server();
        let _ = server.remove::<Item, _>(&self.path).await;
        let _ = server.remove::<Menu, _>(&self.path).await;
    }
}

/// Register `id` with the watcher now and each time a watcher takes the name.
async fn register(connection: &Connection, id: &str) {
    let call = || async {
        let _ = connection
            .call_method(
                Some(WATCHER),
                WATCHER_PATH,
                Some(WATCHER),
                "RegisterStatusNotifierItem",
                &(id,),
            )
            .await;
    };
    let owners = match DBusProxy::new(connection).await {
        Ok(proxy) => proxy
            .receive_name_owner_changed_with_args(&[(0, WATCHER)])
            .await
            .ok(),
        Err(_) => None,
    };
    call().await;
    let Some(mut owners) = owners else {
        return;
    };
    while let Some(change) = owners.next().await {
        if change.args().is_ok_and(|a| a.new_owner().is_some()) {
            call().await;
        }
    }
}

/// `org.kde.StatusNotifierItem` of one share.
struct Item {
    shown: Shown,
    path: OwnedObjectPath,
}

type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    /// The menu opens on a click, so activating the item does nothing more.
    fn activate(&self, _x: i32, _y: i32) {}

    fn secondary_activate(&self, _x: i32, _y: i32) {}

    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: &str) {}

    #[zbus(property)]
    fn category(&self) -> &str {
        "SystemServices"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "cupertino-screen-sharing"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "Screen Sharing"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        ICON_PX.iter().filter_map(|&px| icon(px)).collect()
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        (
            String::new(),
            Vec::new(),
            "Screen Sharing".to_owned(),
            self.shown.sentence(),
        )
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        true
    }

    /// The menu is served next to the item.
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        self.path.clone()
    }
}

/// The glyph of the item at `px` device pixels, as ARGB32 in network byte order: a screen with a dot in it.
fn icon(px: u32) -> Option<(i32, i32, Vec<u8>)> {
    let mut pixmap = Pixmap::new(px, px)?;
    let s = px as f32 / 16.0;
    let mut paint = Paint::default();
    paint.set_color_rgba8(ICON_COLOUR[0], ICON_COLOUR[1], ICON_COLOUR[2], 255);
    paint.anti_alias = true;
    let screen = PathBuilder::from_rect(Rect::from_xywh(1.5 * s, 2.5 * s, 13.0 * s, 9.0 * s)?);
    let stroke = Stroke {
        width: 1.5 * s,
        ..Stroke::default()
    };
    pixmap.stroke_path(&screen, &paint, &stroke, Transform::identity(), None);
    let stand = PathBuilder::from_rect(Rect::from_xywh(5.0 * s, 13.0 * s, 6.0 * s, 1.5 * s)?);
    pixmap.fill_path(
        &stand,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    let dot = PathBuilder::from_circle(8.0 * s, 7.0 * s, 2.0 * s)?;
    pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
    let argb = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.alpha(), c.red(), c.green(), c.blue()]
        })
        .collect();
    Some((px as i32, px as i32, argb))
}

/// `com.canonical.dbusmenu` of one share: who shares what, and Stop Sharing.
struct Menu {
    shown: Shown,
    stop: async_channel::Sender<()>,
}

type Properties = HashMap<String, OwnedValue>;
type Node = (i32, Properties, Vec<OwnedValue>);

fn owned(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).expect("a value without file descriptors")
}

impl Menu {
    fn properties(&self, id: i32) -> Properties {
        let mut properties = Properties::new();
        match id {
            0 => {
                properties.insert("children-display".to_owned(), owned(Value::from("submenu")));
            }
            ITEM_SHARING => {
                properties.insert(
                    "label".to_owned(),
                    owned(Value::from(self.shown.sentence())),
                );
                properties.insert("enabled".to_owned(), owned(Value::from(false)));
            }
            ITEM_SEPARATOR => {
                properties.insert("type".to_owned(), owned(Value::from("separator")));
            }
            ITEM_STOP => {
                properties.insert("label".to_owned(), owned(Value::from("Stop Sharing")));
            }
            _ => {}
        }
        properties
    }

    fn node(&self, id: i32, depth: i32) -> zbus::fdo::Result<Node> {
        let children = if id == 0 && depth != 0 {
            [ITEM_SHARING, ITEM_SEPARATOR, ITEM_STOP]
                .into_iter()
                .map(|child| {
                    let (id, properties, children) = self.node(child, depth - 1)?;
                    let structure = StructureBuilder::new()
                        .add_field(id)
                        .add_field(properties)
                        .add_field(children)
                        .build()
                        .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
                    Ok(owned(Value::Structure(structure)))
                })
                .collect::<zbus::fdo::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        Ok((id, self.properties(id), children))
    }
}

#[interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Node)> {
        if !(0..=ITEM_STOP).contains(&parent_id) {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "no menu item {parent_id}"
            )));
        }
        Ok((1, self.node(parent_id, recursion_depth)?))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, Properties)> {
        ids.into_iter()
            .filter(|id| (0..=ITEM_STOP).contains(id))
            .map(|id| (id, self.properties(id)))
            .collect()
    }

    fn get_property(&self, id: i32, name: &str) -> zbus::fdo::Result<OwnedValue> {
        self.properties(id)
            .remove(name)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("item {id} has no {name}")))
    }

    fn event(&self, id: i32, event_id: &str, _data: Value<'_>, _timestamp: u32) {
        if id == ITEM_STOP && event_id == "clicked" {
            let _ = self.stop.try_send(());
        }
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event_id, _, _) in &events {
            if *id == ITEM_STOP && event_id == "clicked" {
                let _ = self.stop.try_send(());
            }
        }
        Vec::new()
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
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
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_coloured_glyph_on_a_clear_square_at_each_size() {
        for px in ICON_PX {
            let (w, h, argb) = icon(px).unwrap();
            assert_eq!((w, h), (px as i32, px as i32));
            assert_eq!(argb.len(), (px * px * 4) as usize);
            assert_eq!(argb[0], 0, "the corner is clear");
            let centre = ((px / 2 - px / 8) * px + px / 2) as usize * 4;
            assert_eq!(&argb[centre + 1..centre + 4], &ICON_COLOUR, "the dot");
        }
    }

    #[test]
    fn the_menu_names_who_shares_what_then_offers_to_stop() {
        let (stop, stopped) = async_channel::bounded(1);
        let menu = Menu {
            shown: Shown {
                app: "Meeting".to_owned(),
                source: "Studio Display".to_owned(),
            },
            stop,
        };
        let (_, root, children) = menu.node(0, -1).unwrap();
        assert_eq!(children.len(), 3);
        assert!(root.contains_key("children-display"));
        let label = |id| {
            let properties = menu.properties(id);
            properties
                .get("label")
                .map(|v| String::try_from(v.try_clone().unwrap()).unwrap())
        };
        assert_eq!(
            label(ITEM_SHARING).as_deref(),
            Some("“Meeting” is sharing “Studio Display”")
        );
        assert_eq!(label(ITEM_STOP).as_deref(), Some("Stop Sharing"));
        menu.event(ITEM_SHARING, "clicked", Value::from(0), 0);
        assert!(stopped.try_recv().is_err(), "the name is not a control");
        menu.event(ITEM_STOP, "hovered", Value::from(0), 0);
        assert!(stopped.try_recv().is_err());
        menu.event(ITEM_STOP, "clicked", Value::from(0), 0);
        assert!(stopped.try_recv().is_ok());
    }
}

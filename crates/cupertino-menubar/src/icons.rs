//! The icons of menu rows and of status items, read from the icon themes and the asset packs or decoded from what an
//! item sent. Everything here reads files or decodes images, so it runs on Kit's I/O pool (`cupertino_kit::task::io`),
//! never on the component's UI thread; the view draws what was loaded ([`crate::view::RowIcons`]).

use crate::dbus::sni::StatusItem;
use crate::extras;
use crate::feed;
use cupertino_assets::Assets;
use cupertino_icons::{SystemIcons, ThemeIcons};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub use cupertino_icons::IconImage as Pixels;

/// How long loading one icon should take: a few files of a local icon theme, read and decoded.
pub const LOAD_DEADLINE: Duration = Duration::from_secs(2);

/// One icon of a menu row to load: what the row sent inline, else the name it gives, at `px` device pixels.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Request {
    /// Identifies the icon and its size in the view's cache.
    pub key: (u64, u32),
    pub data: Option<Vec<u8>>,
    pub name: Option<String>,
    pub px: u32,
}

/// The icon themes and the asset packs, looked up on the I/O pool. One loader serves the component's tasks in turn.
#[derive(Clone, Default)]
pub struct Loader {
    packs: Arc<Mutex<Option<Packs>>>,
}

struct Packs {
    themed: ThemeIcons,
    /// The template symbols of the asset packs, which rows name as `symbol.*`.
    system: SystemIcons,
}

impl Loader {
    /// Find the packs and the themes, reporting the bar's own symbols a pack lacks. Called on the I/O pool, once at
    /// the start; an icon asked for before that finds them itself.
    pub fn prepare(&self) {
        self.with(|_| ());
    }

    fn with<T>(&self, f: impl FnOnce(&mut Packs) -> T) -> T {
        let mut packs = self.packs.lock().unwrap_or_else(PoisonError::into_inner);
        let packs = packs.get_or_insert_with(|| {
            let assets = Assets::discover();
            Packs {
                themed: ThemeIcons::new(&assets),
                // the extras' symbols are drawn by the toolkit from the same packs; a missing one is reported here
                system: SystemIcons::required(&assets, &[], &extras::SYMBOL_NAMES),
            }
        });
        f(packs)
    }

    /// The themed icon `name` at `px` device pixels.
    pub fn themed(&self, name: &str, px: u32) -> Option<Pixels> {
        self.with(|packs| packs.themed.load(name, px))
            .map(|pixmap| Pixels {
                width: pixmap.width(),
                height: pixmap.height(),
                rgba: pixmap.data().to_vec(),
            })
    }

    /// The icon of a menu row: what it sent inline, else its themed name, a template symbol drawn in black (it takes
    /// the row's colour when painted); `None` without one.
    pub fn row_icon(&self, request: &Request) -> Option<Pixels> {
        let from_data = request
            .data
            .as_deref()
            .and_then(|data| cupertino_icons::decode_png(data, request.px));
        from_data.or_else(|| {
            let name = request.name.as_deref()?;
            if name.starts_with("symbol.") {
                let black = [0.0, 0.0, 0.0, 1.0];
                return self.with(|packs| {
                    let pixmap = packs.system.symbol(name, request.px, black);
                    Some(Pixels {
                        width: pixmap.width(),
                        height: pixmap.height(),
                        rgba: pixmap.data().to_vec(),
                    })
                });
            }
            self.themed(name, request.px)
        })
    }

    /// The icon of a status item at `px` device pixels: its pixmap, else the themed icon it names.
    pub fn status_icon(&self, item: &StatusItem, px: u32) -> Option<Pixels> {
        feed::status_icon(item, px, |name, px| self.themed(name, px))
    }
}

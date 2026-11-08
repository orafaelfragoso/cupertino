//! What the window draws for each item: a picture's thumbnail once it is made ([`ThumbnailService`]), else Files' own
//! art for a folder, a text file, another document or an archive ([`crate::item_art`]) unless an asset pack ships an
//! icon of its own for that kind, else the icon of the item's type from the asset packs and the icon theme
//! ([`FileIcons`]), with the alias badge on a symbolic link.
//! Each window says which of its items are in view, so only those get thumbnails.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::next_resource_id;
use cupertino_kit::file_icons::{FileIcons, RENDITION_SCALES};
use cupertino_kit::render::{ImageResource, PixelFormat, ResourceId};

use crate::item_art::{self, Art};
use crate::thumbnail::Thumbnailer;
use crate::thumbnail_service::{
    has_thumbnail, Stamp, Thumbnail, ThumbnailService, MEMORY_BUDGET, WORKERS,
};
use crate::{mime, Entry, Kind};

/// Thumbnails are made for this many device pixels a point, so they stay sharp on a 2× display; a 1× display draws
/// them shrunk.
const THUMBNAIL_SCALE: f64 = 2.0;

/// How many different pages of lines the text files' art shows; a file always gets the same one.
const TEXT_PAGES: u64 = 8;

/// One drawing of [`item_art`]: the art, its size in points (as bits, so the key hashes) and its variant.
type ArtKey = (Art, u64, u64);

/// The icons of the items of every window.
pub(super) struct ItemIcons {
    files: RefCell<FileIcons>,
    thumbnails: Option<ThumbnailService>,
    /// The art drawn so far, kept for the life of the process so a repainted cell names the same pixels.
    art: RefCell<HashMap<ArtKey, Vec<ImageSource>>>,
    /// Whether an asset pack ships its own icon, by semantic name.
    packed: RefCell<HashMap<&'static str, bool>>,
}

/// The edge in device pixels of the thumbnails of icons `points` square.
fn thumbnail_edge(points: f64) -> u32 {
    (points * THUMBNAIL_SCALE).round().max(1.0) as u32
}

/// Whether `entry` is drawn with its thumbnail: a picture of a decoded format. A symbolic link shows its type's icon
/// with the alias badge.
fn shows_thumbnail(entry: &Entry) -> bool {
    entry.kind == Kind::File && has_thumbnail(&mime::entry_mime(entry))
}

fn source(thumbnail: &Thumbnail) -> ImageSource {
    ImageSource {
        id: ResourceId(thumbnail.id),
        image: ImageResource {
            revision: 1,
            width: thumbnail.width,
            height: thumbnail.height,
            stride: thumbnail.width * 4,
            format: PixelFormat::Rgba8,
            pixels: Arc::clone(&thumbnail.rgba),
        },
    }
}

impl ItemIcons {
    /// Icons drawn with `files`, and thumbnails kept in `thumbnails`' cache when there is one; `wake` is called when a
    /// thumbnail is made.
    pub(super) fn new(
        files: FileIcons,
        thumbnails: Option<Thumbnailer>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let ids = Arc::new(|| next_resource_id().0);
        Self {
            files: RefCell::new(files),
            thumbnails: thumbnails
                .map(|cache| ThumbnailService::new(cache, WORKERS, MEMORY_BUDGET, wake, ids)),
            art: RefCell::new(HashMap::new()),
            packed: RefCell::new(HashMap::new()),
        }
    }

    /// Whether the windows' items get thumbnails.
    pub(super) fn thumbnails(&self) -> bool {
        self.thumbnails.is_some()
    }

    /// The renditions of `entry`'s image `points` square: its thumbnail, else its type's icon.
    pub(super) fn icon(&self, entry: &Entry, points: f64) -> Vec<ImageSource> {
        let thumbnail = self
            .thumbnails
            .as_ref()
            .filter(|_| shows_thumbnail(entry))
            .and_then(|service| service.get(&entry.path, thumbnail_edge(points)));
        if let Some(thumbnail) = thumbnail {
            return vec![source(&thumbnail)];
        }
        let mime = mime::entry_mime(entry);
        let mut files = self.files.borrow_mut();
        let alias = entry.kind == Kind::Symlink;
        // a link keeps its type's icon, which carries the alias badge
        let name = files.name(&entry.path, &mime);
        let art = Art::for_icon(name).filter(|_| !alias).filter(|_| {
            !*self
                .packed
                .borrow_mut()
                .entry(name)
                .or_insert_with(|| files.pack_has(name))
        });
        match art {
            Some(art) => self.art(art, points, &entry.name),
            None => files.images(&entry.path, &mime, alias, points),
        }
    }

    /// `art` `points` square at each rendition scale; a text page's lines are picked by the item's `name`.
    fn art(&self, art: Art, points: f64, name: &str) -> Vec<ImageSource> {
        let variant = match art {
            Art::Text => {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                name.hash(&mut hasher);
                hasher.finish() % TEXT_PAGES
            }
            _ => 0,
        };
        let key = (art, points.to_bits(), variant);
        self.art
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                RENDITION_SCALES
                    .iter()
                    .map(|&scale| {
                        let px = (points * f64::from(scale)).round().max(1.0) as u32;
                        let pixels = item_art::draw(art, px, variant + 1);
                        ImageSource {
                            id: ResourceId(next_resource_id().0),
                            image: ImageResource {
                                revision: 1,
                                width: px,
                                height: px,
                                stride: px * 4,
                                format: PixelFormat::Rgba8Premultiplied,
                                pixels: Arc::from(pixels),
                            },
                        }
                    })
                    .collect()
            })
            .clone()
    }

    /// Window `window` shows `entries` with icons `points` square, the first most wanted.
    pub(super) fn want<'a>(
        &self,
        window: u64,
        entries: impl IntoIterator<Item = &'a Entry>,
        points: f64,
    ) {
        if let Some(service) = &self.thumbnails {
            let items = entries
                .into_iter()
                .filter(|e| shows_thumbnail(e))
                .map(|e| (e.path.clone(), Stamp::of(e)));
            service.want(window, thumbnail_edge(points), items);
        }
    }

    /// Window `window` closed.
    pub(super) fn forget(&self, window: u64) {
        if let Some(service) = &self.thumbnails {
            service.forget(window);
        }
    }
}

//! What the window draws for each item: a picture's thumbnail once it is made ([`ThumbnailService`]), else the icon of
//! the item's type from the asset packs and the icon theme ([`FileIcons`]), with the alias badge on a symbolic link.
//! Each window says which of its items are in view, so only those get thumbnails.

use std::cell::RefCell;
use std::sync::Arc;

use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::next_resource_id;
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::render::{ImageResource, PixelFormat, ResourceId};

use crate::thumbnail::Thumbnailer;
use crate::thumbnail_service::{
    has_thumbnail, Stamp, Thumbnail, ThumbnailService, MEMORY_BUDGET, WORKERS,
};
use crate::{mime, Entry, Kind};

/// Thumbnails are made for this many device pixels a point, so they stay sharp on a 2× display; a 1× display draws
/// them shrunk.
const THUMBNAIL_SCALE: f64 = 2.0;

/// The icons of the items of every window.
pub(super) struct ItemIcons {
    files: RefCell<FileIcons>,
    thumbnails: Option<ThumbnailService>,
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
        match thumbnail {
            Some(thumbnail) => vec![source(&thumbnail)],
            None => self.files.borrow_mut().images(
                &entry.path,
                &mime::entry_mime(entry),
                entry.kind == Kind::Symlink,
                points,
            ),
        }
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

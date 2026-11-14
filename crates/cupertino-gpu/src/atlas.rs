//! Atlases: many small images (glyph coverage, colour glyphs, path masks) packed into a few page textures, so a
//! frame draws them from one texture without uploading each.
//!
//! A page is packed in shelves (rows of one height, left to right) with a [`GAP`] around every entry so filtering
//! never reads a neighbour. An atlas has at most its kind's number of pages. When an entry fits no page and the
//! atlas is at its cap, the page used least recently is emptied and reused, unless every page was used in the
//! current frame: then [`Error::OverBudget`], and the caller draws what it has and starts a new frame. Pages and
//! entries are keyed by the caller's 64-bit keys (a glyph id, phase and size; a path's hash).
//!
//! The caps hold the atlases of a window to 16 MiB together (coverage 4, colour 8, path masks 4), half the 32 MiB
//! the design gives a GPU window for atlases and pools; [`crate::pool`] has the other half. Every page is a
//! multiple of the 2 MiB the driver rounds a texture's memory up to.

use std::collections::HashMap;

use crate::gles::{Device, Texture};
use crate::types::{Error, Region, Result, TextureDesc, TextureFormat, TextureUsage};

/// Pixels left empty around every entry.
pub const GAP: u32 = 1;

/// The kinds of atlas, each with its format, page size and cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtlasKind {
    /// 8-bit glyph coverage, tinted when drawn.
    Coverage,
    /// Premultiplied RGBA colour glyphs (emoji).
    Color,
    /// 8-bit path coverage rasterized on the CPU.
    PathMask,
}

/// An atlas's format, page size and most pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasConfig {
    pub format: TextureFormat,
    pub page_size: (u32, u32),
    pub max_pages: usize,
}

impl AtlasConfig {
    /// Bytes one page takes.
    pub fn page_bytes(&self) -> u64 {
        u64::from(self.page_size.0)
            * u64::from(self.page_size.1)
            * u64::from(self.format.bytes_per_pixel())
    }

    /// Bytes the atlas may take at most.
    pub fn cap_bytes(&self) -> u64 {
        self.page_bytes() * self.max_pages as u64
    }
}

/// Coverage atlas: 2048 x 1024 8-bit pages (2 MiB), two of them.
pub const COVERAGE: AtlasConfig = AtlasConfig {
    format: TextureFormat::R8,
    page_size: (2048, 1024),
    max_pages: 2,
};
/// Colour glyph atlas: 1024 x 1024 RGBA pages (4 MiB), two of them.
pub const COLOR: AtlasConfig = AtlasConfig {
    format: TextureFormat::Rgba8,
    page_size: (1024, 1024),
    max_pages: 2,
};
/// Path mask atlas: 2048 x 1024 8-bit pages (2 MiB), two of them.
pub const PATH_MASK: AtlasConfig = AtlasConfig {
    format: TextureFormat::R8,
    page_size: (2048, 1024),
    max_pages: 2,
};

impl AtlasKind {
    pub fn config(self) -> AtlasConfig {
        match self {
            AtlasKind::Coverage => COVERAGE,
            AtlasKind::Color => COLOR,
            AtlasKind::PathMask => PATH_MASK,
        }
    }
}

/// Where an entry lies: its page and its pixels there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub page: usize,
    pub region: Region,
}

#[derive(Clone, Debug, Default)]
struct Shelf {
    y: u32,
    height: u32,
    x: u32,
}

#[derive(Clone, Debug, Default)]
struct Page {
    shelves: Vec<Shelf>,
    next_y: u32,
    last_use: u64,
    keys: Vec<u64>,
}

/// What an insertion did to the pages, for the GPU side to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub slot: Slot,
    /// A page was added for it.
    pub new_page: bool,
    /// The page was emptied for it, and its old entries are gone.
    pub evicted: bool,
}

/// The packing and eviction of an atlas, without textures.
#[derive(Clone, Debug)]
pub struct AtlasModel {
    config: AtlasConfig,
    pages: Vec<Page>,
    entries: HashMap<u64, Slot>,
}

impl AtlasModel {
    pub fn new(config: AtlasConfig) -> Self {
        Self {
            config,
            pages: Vec::new(),
            entries: HashMap::new(),
        }
    }

    pub fn config(&self) -> AtlasConfig {
        self.config
    }

    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry `key`, marked as used in `frame`.
    pub fn get(&mut self, key: u64, frame: u64) -> Option<Slot> {
        let slot = *self.entries.get(&key)?;
        let page = &mut self.pages[slot.page];
        page.last_use = page.last_use.max(frame);
        Some(slot)
    }

    fn place(page: &mut Page, (pw, ph): (u32, u32), (w, h): (u32, u32)) -> Option<Region> {
        let (need_w, need_h) = (w + GAP, h + GAP);
        // the lowest shelf tall enough, so short entries do not take tall rows
        let best = page
            .shelves
            .iter_mut()
            .filter(|s| s.height >= need_h && s.x + need_w <= pw)
            .min_by_key(|s| s.height);
        if let Some(shelf) = best {
            let region = Region::new(shelf.x, shelf.y, w, h);
            shelf.x += need_w;
            return Some(region);
        }
        if page.next_y + need_h > ph || need_w > pw {
            return None;
        }
        let shelf = Shelf {
            y: page.next_y,
            height: need_h,
            x: need_w,
        };
        page.next_y += need_h;
        let region = Region::new(0, shelf.y, w, h);
        page.shelves.push(shelf);
        Some(region)
    }

    /// Room for a new entry `key` of `size`, used in `frame`.
    pub fn insert(&mut self, key: u64, size: (u32, u32), frame: u64) -> Result<Placement> {
        let (pw, ph) = self.config.page_size;
        if size.0 == 0 || size.1 == 0 || size.0 + GAP > pw || size.1 + GAP > ph {
            return Err(Error::Invalid(format!(
                "an atlas entry of {size:?} in pages of {pw} x {ph}"
            )));
        }
        if let Some(old) = self.entries.remove(&key) {
            self.pages[old.page].keys.retain(|k| *k != key);
        }
        let mut placed = None;
        for (i, page) in self.pages.iter_mut().enumerate() {
            if let Some(region) = Self::place(page, self.config.page_size, size) {
                placed = Some((i, region, false, false));
                break;
            }
        }
        if placed.is_none() && self.pages.len() < self.config.max_pages {
            let mut page = Page::default();
            let region = Self::place(&mut page, self.config.page_size, size)
                .expect("an empty page holds the entry");
            self.pages.push(page);
            placed = Some((self.pages.len() - 1, region, true, false));
        }
        if placed.is_none() {
            let victim = self
                .pages
                .iter()
                .enumerate()
                .filter(|(_, p)| p.last_use < frame)
                .min_by_key(|(_, p)| p.last_use)
                .map(|(i, _)| i);
            let Some(i) = victim else {
                return Err(Error::OverBudget {
                    what: "atlas",
                    need: u64::from(size.0)
                        * u64::from(size.1)
                        * u64::from(self.config.format.bytes_per_pixel()),
                    cap: self.config.cap_bytes(),
                });
            };
            for k in std::mem::take(&mut self.pages[i].keys) {
                self.entries.remove(&k);
            }
            self.pages[i] = Page::default();
            let region = Self::place(&mut self.pages[i], self.config.page_size, size)
                .expect("an empty page holds the entry");
            placed = Some((i, region, false, true));
        }
        let (page, region, new_page, evicted) = placed.expect("placed above");
        let p = &mut self.pages[page];
        p.last_use = p.last_use.max(frame);
        p.keys.push(key);
        let slot = Slot { page, region };
        self.entries.insert(key, slot);
        Ok(Placement {
            slot,
            new_page,
            evicted,
        })
    }

    /// Forget every entry and page.
    pub fn clear(&mut self) {
        self.pages.clear();
        self.entries.clear();
    }
}

/// An atlas on a device: its model and its page textures.
#[derive(Debug)]
pub struct Atlas {
    model: AtlasModel,
    pages: Vec<Texture>,
}

impl Atlas {
    pub fn new(kind: AtlasKind) -> Self {
        Self::with_config(kind.config())
    }

    pub fn with_config(config: AtlasConfig) -> Self {
        Self {
            model: AtlasModel::new(config),
            pages: Vec::new(),
        }
    }

    pub fn model(&self) -> &AtlasModel {
        &self.model
    }

    /// The entry `key`, marked as used in `frame`.
    pub fn get(&mut self, key: u64, frame: u64) -> Option<Slot> {
        self.model.get(key, frame)
    }

    /// Upload `pixels` (rows first to last, tightly packed in the atlas's format) as the entry `key`, used in
    /// `frame`.
    pub fn insert(
        &mut self,
        device: &Device,
        key: u64,
        size: (u32, u32),
        pixels: &[u8],
        frame: u64,
    ) -> Result<Slot> {
        let placement = self.model.insert(key, size, frame)?;
        if placement.new_page {
            let config = self.model.config();
            let texture = device.create_texture(&TextureDesc {
                size: config.page_size,
                format: config.format,
                usage: TextureUsage::SAMPLED,
            });
            match texture {
                Ok(t) => self.pages.push(t),
                Err(e) => {
                    self.model.clear();
                    for page in self.pages.drain(..) {
                        device.destroy_texture(page);
                    }
                    return Err(e);
                }
            }
        }
        device.write_texture(
            &self.pages[placement.slot.page],
            placement.slot.region,
            pixels,
        )?;
        Ok(placement.slot)
    }

    /// The texture of page `page`.
    pub fn page(&self, page: usize) -> Option<&Texture> {
        self.pages.get(page)
    }

    /// Bytes the atlas's pages take.
    pub fn bytes(&self) -> u64 {
        self.model.config().page_bytes() * self.pages.len() as u64
    }

    /// Free every page (a hidden window drops its atlases).
    pub fn clear(&mut self, device: &Device) {
        self.model.clear();
        for page in self.pages.drain(..) {
            device.destroy_texture(page);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: AtlasConfig = AtlasConfig {
        format: TextureFormat::R8,
        page_size: (32, 32),
        max_pages: 2,
    };

    #[test]
    fn the_caps_hold_the_atlases_to_sixteen_mebibytes_in_whole_driver_granules() {
        let mib = 1 << 20;
        assert_eq!(
            COVERAGE.cap_bytes() + COLOR.cap_bytes() + PATH_MASK.cap_bytes(),
            16 * mib
        );
        for c in [COVERAGE, COLOR, PATH_MASK] {
            assert_eq!(c.page_bytes() % (2 * mib), 0, "{c:?}");
        }
    }

    #[test]
    fn entries_pack_in_shelves_with_a_gap_and_are_found_again() {
        let mut a = AtlasModel::new(SMALL);
        let p = a.insert(1, (10, 8), 0).unwrap();
        assert_eq!(
            p.slot,
            Slot {
                page: 0,
                region: Region::new(0, 0, 10, 8)
            }
        );
        assert!(p.new_page && !p.evicted);
        let q = a.insert(2, (10, 6), 0).unwrap();
        assert_eq!(q.slot.region, Region::new(11, 0, 10, 6));
        let r = a.insert(3, (10, 12), 0).unwrap();
        assert_eq!(r.slot.region, Region::new(0, 9, 10, 12));
        assert_eq!(a.get(2, 1), Some(q.slot));
        assert_eq!(a.get(9, 1), None);
        assert!(a.insert(4, (40, 1), 0).is_err());
    }

    #[test]
    fn a_full_atlas_empties_its_least_recently_used_page_but_never_one_used_this_frame() {
        let mut a = AtlasModel::new(SMALL);
        // each 31 x 31 entry fills a page
        assert!(a.insert(1, (31, 31), 0).unwrap().new_page);
        assert!(a.insert(2, (31, 31), 1).unwrap().new_page);
        assert_eq!(a.pages(), 2);
        // page 0 was used in frame 0, page 1 in frame 1: page 0 goes
        let p = a.insert(3, (31, 31), 2).unwrap();
        assert_eq!((p.slot.page, p.evicted), (0, true));
        assert_eq!(a.get(1, 2), None);
        assert!(a.get(2, 2).is_some());
        // both pages are now used in frame 2: nothing may go
        match a.insert(4, (31, 31), 2) {
            Err(Error::OverBudget {
                what: "atlas", cap, ..
            }) => assert_eq!(cap, SMALL.cap_bytes()),
            other => panic!("{other:?}"),
        }
        // in the next frame the page used least recently goes again
        let p = a.insert(4, (31, 31), 3).unwrap();
        assert!(p.evicted);
        assert_eq!(a.len(), 2);
        assert_eq!(a.pages(), 2);
    }
}

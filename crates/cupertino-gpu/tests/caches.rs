//! Atlases and the render target pool on the device: entries uploaded where the model places them, a page reused
//! when the cap evicts it, and pooled targets handed out again.

use cupertino_gpu::atlas::{Atlas, AtlasConfig, AtlasKind};
use cupertino_gpu::pool::{TargetPool, TARGET_POOL_CAP};
use cupertino_gpu::*;

use crate::common;

/// Copy page `page` of `atlas` into an RGBA target and read the red channel of `region`.
fn read_page(device: &Device, atlas: &Atlas, page: usize, region: Region) -> Vec<u8> {
    let texture = atlas.page(page).unwrap();
    let target = common::target(device, texture.size(), TextureFormat::Rgba8);
    common::blit(device, texture, &target, Filter::Nearest);
    let rgba = device.read_bytes(&target, region).unwrap();
    device.destroy_texture(target);
    rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect()
}

#[test]
fn an_atlas_uploads_entries_where_it_places_them_and_reuses_the_evicted_page() {
    let Some(device) = common::device() else {
        return;
    };
    let mut atlas = Atlas::with_config(AtlasConfig {
        format: TextureFormat::R8,
        page_size: (32, 32),
        max_pages: 2,
    });
    let glyph = |v: u8| vec![v; 31 * 31];
    let a = atlas.insert(&device, 1, (31, 31), &glyph(10), 0).unwrap();
    let b = atlas.insert(&device, 2, (31, 31), &glyph(20), 1).unwrap();
    assert_eq!((a.page, b.page), (0, 1));
    assert_eq!(atlas.bytes(), 2 * 32 * 32);
    assert!(read_page(&device, &atlas, 0, a.region)
        .iter()
        .all(|v| *v == 10));
    assert!(read_page(&device, &atlas, 1, b.region)
        .iter()
        .all(|v| *v == 20));
    // a third entry in frame 2 evicts page 0 (used least recently) and lands in its texture
    let page0 = atlas.page(0).unwrap().id();
    let c = atlas.insert(&device, 3, (31, 31), &glyph(30), 2).unwrap();
    assert_eq!(c.page, 0);
    assert_eq!(
        atlas.page(0).unwrap().id(),
        page0,
        "the page's texture is reused, not made again"
    );
    assert!(read_page(&device, &atlas, 0, c.region)
        .iter()
        .all(|v| *v == 30));
    assert_eq!(atlas.get(1, 2), None);
    assert_eq!(atlas.get(2, 2), Some(b));
    // every page used in frame 2: the cap holds
    assert!(matches!(
        atlas.insert(&device, 4, (31, 31), &glyph(40), 2),
        Err(Error::OverBudget { .. })
    ));
    atlas.clear(&device);
    assert_eq!(atlas.bytes(), 0);
}

#[test]
fn the_real_atlases_make_their_pages_in_their_formats() {
    let Some(device) = common::device() else {
        return;
    };
    for kind in [AtlasKind::Coverage, AtlasKind::Color, AtlasKind::PathMask] {
        let config = kind.config();
        let mut atlas = Atlas::new(kind);
        let bpp = config.format.bytes_per_pixel() as usize;
        atlas
            .insert(&device, 7, (12, 9), &vec![200; 12 * 9 * bpp], 0)
            .unwrap();
        assert_eq!(atlas.page(0).unwrap().desc().format, config.format);
        assert_eq!(atlas.bytes(), config.page_bytes());
        atlas.clear(&device);
    }
}

#[test]
fn the_pool_hands_a_released_target_out_again_and_keeps_to_its_cap() {
    let Some(device) = common::device() else {
        return;
    };
    let mut pool = TargetPool::new(TARGET_POOL_CAP);
    let mut source = &device;
    let a = pool
        .acquire(&mut source, (300, 200), TextureFormat::Rgba8)
        .unwrap();
    let id = a.target.id();
    // a pooled target is a render target
    common::draw_shapes(&device, &a.target, [1.0, 0.0, 0.0, 1.0], &[]);
    pool.release(a, 0);
    let b = pool
        .acquire(&mut source, (300, 200), TextureFormat::Rgba8)
        .unwrap();
    assert_eq!(b.target.id(), id, "the same texture again");
    let c = pool
        .acquire(&mut source, (300, 200), TextureFormat::Rgba8)
        .unwrap();
    assert_ne!(c.target.id(), id, "a second one while the first is in use");
    // a 3840 x 2160 target alone passes the cap
    assert!(matches!(
        pool.acquire(&mut source, (3840, 2160), TextureFormat::Rgba8),
        Err(Error::OverBudget { .. })
    ));
    pool.release(b, 1);
    pool.release(c, 1);
    pool.trim(&mut source);
    assert_eq!(pool.bytes(), (0, 0));
}

//! Dmabufs: allocated by the device (GBM or texture export) in a layout the offer holds, drawn into, handed to a
//! second device with a sync file, imported and sampled there.

use cupertino_gpu::*;

use crate::common;

/// What a compositor would offer: the device's renderable modifiers on GBM (never linear unless the device lists
/// it), linear and the implicit layout for texture export.
fn offer(device: &Device) -> Vec<u64> {
    match device.info().allocator {
        Some(Allocator::Gbm) => device.renderable_modifiers(FOURCC_ARGB8888),
        _ => vec![MODIFIER_LINEAR, MODIFIER_INVALID],
    }
}

#[test]
fn a_dmabuf_drawn_on_one_device_is_imported_and_sampled_on_another_after_its_sync_file() {
    let Some(a) = common::device() else { return };
    if !common::has(a.info().allocator.is_some(), "dmabuf allocator") {
        return;
    }
    if common::gpu_required() {
        assert_eq!(a.info().allocator, Some(Allocator::Gbm));
    }
    let modifiers = offer(&a);
    let image = a
        .allocate_dmabuf(&DmabufDesc {
            size: (64, 32),
            fourcc: FOURCC_ARGB8888,
            modifiers: modifiers.clone(),
        })
        .unwrap();
    assert!(image.texture.is_external());
    assert!(
        modifiers.contains(&image.dmabuf.modifier) || image.dmabuf.modifier == MODIFIER_INVALID,
        "{:#x} not offered",
        image.dmabuf.modifier
    );
    if !a.info().software {
        assert_ne!(
            image.dmabuf.modifier, MODIFIER_LINEAR,
            "linear on a hardware driver"
        );
    }
    // the left half red, the right half green
    common::draw_shapes(
        &a,
        &image.texture,
        [0.0, 1.0, 0.0, 1.0],
        &[common::shape(
            [0.0, 0.0, 32.0, 32.0],
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 2.0],
        )],
    );
    // the drawing reads back on the device that drew it
    let mine = a
        .read_bytes(&image.texture, Region::new(0, 0, 64, 1))
        .unwrap();
    assert_eq!(&mine[..4], [255, 0, 0, 255]);
    assert_eq!(&mine[63 * 4..], [0, 255, 0, 255]);

    let sync_file = if a.info().native_fences {
        let (fence, fd) = a.submit_exported().unwrap();
        a.destroy_fence(fence);
        Some(fd)
    } else {
        common::has(false, "native fences");
        a.finish().unwrap();
        None
    };

    let Some(b) = common::device() else { return };
    let dmabuf = image.dmabuf.try_clone().unwrap();
    let imported = b.import_dmabuf(&dmabuf).unwrap();
    if let Some(fd) = sync_file {
        if b.info().fence_import {
            b.wait_sync_file(fd).unwrap();
        } else {
            assert!(wait_sync_file(&fd, std::time::Duration::from_secs(5)));
        }
    }
    let target = common::target(&b, (64, 32), TextureFormat::Rgba8);
    common::blit(&b, &imported, &target, Filter::Nearest);
    let theirs = b.read_bytes(&target, Region::new(0, 16, 64, 1)).unwrap();
    assert_eq!(
        &theirs[..4],
        [255, 0, 0, 255],
        "the importer sees the drawing"
    );
    assert_eq!(&theirs[40 * 4..41 * 4], [0, 255, 0, 255]);
    b.destroy_texture(target);
    b.destroy_texture(imported);
    drop(b);
    a.destroy_texture(image.texture);
}

#[test]
fn an_allocation_with_no_layout_both_sides_accept_is_refused() {
    let Some(device) = common::device() else {
        return;
    };
    if !common::has(device.info().allocator.is_some(), "dmabuf allocator") {
        return;
    }
    // a modifier no driver has
    let r = device.allocate_dmabuf(&DmabufDesc {
        size: (16, 16),
        fourcc: FOURCC_ARGB8888,
        modifiers: vec![0x00ff_0000_dead_beef],
    });
    assert!(r.is_err(), "{r:?}");
}

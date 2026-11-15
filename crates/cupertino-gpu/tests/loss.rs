//! Context loss: every context is robust (and purge-notified where the driver offers it), a reset or a purge is
//! reported once read, and a lost device refuses all work until it is replaced. The loss is injected through the
//! device's test hook, which the status check reads in place of the driver's own status.

use cupertino_gpu::*;

use crate::common;

#[test]
fn every_context_is_robust_and_a_fresh_one_is_not_lost() {
    let Some(device) = common::device() else {
        return;
    };
    assert!(device.info().robustness, "{:?}", device.info());
    if common::gpu_required() {
        assert!(device.info().purge_notification, "{:?}", device.info());
    }
    device.status().unwrap();
}

#[test]
fn a_reset_or_a_purge_is_reported_and_the_lost_device_refuses_work_until_replaced() {
    for loss in [
        Loss::Reset { guilty: Some(true) },
        Loss::Reset {
            guilty: Some(false),
        },
        Loss::Reset { guilty: None },
        Loss::Purge,
    ] {
        let Some(device) = common::device() else {
            return;
        };
        let target = common::target(&device, (4, 4), TextureFormat::Rgba8);
        device.inject_loss(loss);
        match device.status() {
            Err(Error::Lost(l)) => assert_eq!(l, loss),
            other => panic!("{loss:?}: {other:?}"),
        }
        assert_eq!(loss.counts_as_failure(), loss != Loss::Purge);
        // once lost, always lost, and every call says so
        assert!(matches!(device.status(), Err(Error::Lost(l)) if l == loss));
        assert!(matches!(
            device.begin_pass(&target, LoadOp::Load),
            Err(Error::Lost(_))
        ));
        assert!(matches!(device.submit(), Err(Error::Lost(_))));
        assert!(matches!(
            device.create_buffer(&BufferDesc {
                size: 16,
                usage: BufferUsage::Uniform
            }),
            Err(Error::Lost(_))
        ));
        drop(device);
    }
    // a new device works
    let Some(again) = common::device() else {
        return;
    };
    again.status().unwrap();
    let t = common::target(&again, (4, 4), TextureFormat::Rgba8);
    common::draw_shapes(&again, &t, [0.0; 4], &[]);
    again.destroy_texture(t);
}

/// A lost device still frees each dmabuf's image and buffer object as its texture is destroyed (only the GL deletes
/// are skipped), and going while another device's context is current leaves that one current and working.
#[test]
fn a_lost_device_frees_its_images_and_buffer_objects_and_leaves_another_context_current() {
    for loss in [Loss::Purge, Loss::Reset { guilty: None }] {
        let Some(old) = common::device() else {
            return;
        };
        if !common::has(old.info().allocator.is_some(), "dmabuf allocator") {
            return;
        }
        let modifiers = match old.info().allocator {
            Some(Allocator::Gbm) => old.renderable_modifiers(FOURCC_ARGB8888),
            _ => vec![MODIFIER_LINEAR, MODIFIER_INVALID],
        };
        let images: Vec<ExternalImage> = (0..3)
            .map(|_| {
                old.allocate_dmabuf(&DmabufDesc {
                    size: (64, 32),
                    fourcc: FOURCC_ARGB8888,
                    modifiers: modifiers.clone(),
                })
                .unwrap()
            })
            .collect();
        let staging = common::target(&old, (64, 32), TextureFormat::Rgba8);
        let gbm = old.info().allocator == Some(Allocator::Gbm);
        assert_eq!(old.live_textures().textures, 4);
        assert_eq!(
            old.live_external(),
            ExternalTally {
                images: 3,
                buffer_objects: if gbm { 3 } else { 0 }
            }
        );
        old.inject_loss(loss);
        assert!(matches!(old.status(), Err(Error::Lost(_))));
        // the replacement is made (and current) before the lost device's resources go, as a process recovering does
        let new = common::device().expect("a second device");
        let target = common::target(&new, (4, 4), TextureFormat::Rgba8);
        assert!(new.is_current() && !old.is_current());
        for image in images {
            old.destroy_texture(image.texture);
        }
        old.destroy_texture(staging);
        assert_eq!(old.live_textures(), TextureTally::default(), "{loss:?}");
        assert_eq!(old.live_external(), ExternalTally::default(), "{loss:?}");
        drop(old);
        assert!(
            new.is_current(),
            "{loss:?}: the lost device's going released the other's context"
        );
        common::draw_shapes(&new, &target, [0.0, 1.0, 0.0, 1.0], &[]);
        let pixels = new.read_bytes(&target, Region::new(0, 0, 4, 4)).unwrap();
        assert!(
            pixels.chunks(4).all(|p| p == [0, 255, 0, 255]),
            "{loss:?}: {pixels:?}"
        );
        new.destroy_texture(target);
    }
}

#[test]
fn a_render_node_that_cannot_be_opened_is_an_error_value() {
    let r = Device::new(&DeviceDesc {
        platform: Platform::RenderNode("/dev/dri/renderD-none".into()),
    });
    assert!(matches!(r, Err(Error::Unavailable(_))), "{r:?}");
}

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

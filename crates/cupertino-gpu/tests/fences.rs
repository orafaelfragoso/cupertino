//! Fences: a submission's fence signals when its work is done, on the CPU and, where the driver has native fences,
//! as a sync file.

use std::time::Duration;

use cupertino_gpu::*;

use crate::common;

#[test]
fn a_submission_s_fence_signals_once_its_drawing_is_done() {
    let Some(device) = common::device() else {
        return;
    };
    let target = common::target(&device, (256, 256), TextureFormat::Rgba8);
    common::draw_shapes(
        &device,
        &target,
        [0.0; 4],
        &[common::shape(
            [8.0, 8.0, 200.0, 200.0],
            [1.0; 4],
            [20.0, 4.0],
        )],
    );
    let fence = device.submit().unwrap();
    assert!(device.wait(&fence, Duration::from_secs(5)).unwrap());
    assert!(device.is_signalled(&fence).unwrap());
    device.destroy_fence(fence);
    // nothing submitted since: a new fence signals too
    let idle = device.submit().unwrap();
    assert!(device.wait(&idle, Duration::from_secs(5)).unwrap());
    device.destroy_fence(idle);
    device.destroy_texture(target);
}

#[test]
fn a_native_fence_is_a_sync_file_that_signals_and_that_another_wait_can_take() {
    let Some(device) = common::device() else {
        return;
    };
    if !common::has(device.info().native_fences, "native fences") {
        return;
    }
    let target = common::target(&device, (128, 128), TextureFormat::Rgba8);
    common::draw_shapes(
        &device,
        &target,
        [0.0; 4],
        &[common::shape(
            [0.0, 0.0, 128.0, 128.0],
            [1.0; 4],
            [0.0, 2.0],
        )],
    );
    let (fence, fd) = device.submit_exported().unwrap();
    assert!(
        wait_sync_file(&fd, Duration::from_secs(5)),
        "the sync file signals"
    );
    assert!(device.wait(&fence, Duration::from_secs(5)).unwrap());
    device.destroy_fence(fence);
    if common::has(device.info().fence_import, "GPU waits on sync files") {
        // the device's own work waits on it, then more work goes on
        device.wait_sync_file(fd).unwrap();
        common::draw_shapes(&device, &target, [0.0; 4], &[]);
        device.finish().unwrap();
    }
    device.destroy_texture(target);
}

//! Fences: a submission's fence signals when its work is done, on the CPU and, where the driver has native fences,
//! as a sync file; a timer query gives the GPU's own time of a piece of work.

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

/// The GPU's own time of a piece of work, by a timer query: the work's result comes back with it, and many
/// overlapping blended shapes over a large target take the GPU longer than no drawing at all.
#[test]
fn the_gpu_times_its_own_work_and_more_drawing_takes_it_longer() {
    let Some(device) = common::device() else {
        return;
    };
    if !common::has(device.times_gpu_work(), "timer query") {
        return;
    }
    let target = common::target(&device, (2048, 2048), TextureFormat::Rgba8);
    // the program made and the target cleared before anything is timed
    common::draw_shapes(&device, &target, [0.0; 4], &[]);
    device.finish().unwrap();
    let shapes: Vec<_> = (0..200)
        .map(|_| common::shape([0.0, 0.0, 2048.0, 2048.0], [0.5; 4], [64.0, 2.0]))
        .collect();
    // a disruption of the GPU's timing gives no time; a few tries get one
    let timed = |work: &dyn Fn()| {
        (0..3)
            .find_map(|_| device.gpu_time(work).unwrap().1)
            .expect("a time in three tries")
    };
    let (value, _) = device.gpu_time(|| 7).unwrap();
    assert_eq!(value, 7, "the work's result comes back");
    let nothing = timed(&|| {});
    let heavy = timed(&|| common::draw_shapes(&device, &target, [0.0; 4], &shapes));
    eprintln!("GPU time: nothing {nothing:?}, 200 shapes over 2048 x 2048 {heavy:?}");
    assert!(nothing < Duration::from_millis(1), "{nothing:?}");
    assert!(heavy > nothing, "{heavy:?} against {nothing:?}");
    device.destroy_texture(target);
}

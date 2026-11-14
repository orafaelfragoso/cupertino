//! What a process needs to move its drawing to the GPU after its first frame: programs saved by one device and loaded
//! by the next instead of compiled, a device made on one thread and used on another, and syncobj timelines that turn a
//! native fence into a point and signal an eventfd at a point.

use std::os::fd::AsFd;
use std::path::Path;
use std::time::Duration;

use cupertino_gpu::syncobj::{self, SyncNode};
use cupertino_gpu::*;

use crate::common;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cupertino-gpu-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_program_saved_by_one_device_is_loaded_by_the_next_and_draws_the_same() {
    let dir = scratch("programs");
    let Some(mut first) = common::device() else {
        return;
    };
    first.set_program_cache(Some(&dir));
    if !common::has(first.program_cache_dir().is_some(), "program binary format") {
        return;
    }
    let (pipeline, layout) = common::pipeline(&first, &shaders::SHAPE, Blend::PremultipliedOver);
    let stats = first.program_cache_stats();
    assert_eq!((stats.compiled, stats.stored, stats.loaded), (1, 1, 0));
    let cache_dir = first.program_cache_dir().unwrap().to_path_buf();
    assert!(cache_dir.starts_with(&dir));
    assert_eq!(std::fs::read_dir(&cache_dir).unwrap().count(), 1);
    let target = common::target(&first, (16, 16), TextureFormat::Rgba8);
    common::draw_shapes(&first, &target, [0.0; 4], &[]);
    let before = first
        .read_bytes(&target, Region::new(0, 0, 16, 16))
        .unwrap();
    first.destroy_pipeline(pipeline);
    drop(layout);
    first.destroy_texture(target);
    drop(first);

    let mut second = common::device().unwrap();
    second.set_program_cache(Some(&dir));
    let (pipeline, _layout) = common::pipeline(&second, &shaders::SHAPE, Blend::PremultipliedOver);
    let stats = second.program_cache_stats();
    assert_eq!((stats.loaded, stats.compiled), (1, 0), "{stats:?}");
    let target = common::target(&second, (16, 16), TextureFormat::Rgba8);
    common::draw_shapes(&second, &target, [0.0; 4], &[]);
    let after = second
        .read_bytes(&target, Region::new(0, 0, 16, 16))
        .unwrap();
    assert_eq!(before, after);
    second.destroy_pipeline(pipeline);
    second.destroy_texture(target);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_binary_the_driver_refuses_is_compiled_again_and_replaced() {
    let dir = scratch("refused");
    let Some(mut device) = common::device() else {
        return;
    };
    device.set_program_cache(Some(&dir));
    if !common::has(
        device.program_cache_dir().is_some(),
        "program binary format",
    ) {
        return;
    }
    let (p, _l) = common::pipeline(&device, &shaders::BLIT, Blend::Replace);
    device.destroy_pipeline(p);
    let cache_dir = device.program_cache_dir().unwrap().to_path_buf();
    let file = std::fs::read_dir(&cache_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    // a well-formed header around bytes no driver takes
    std::fs::write(&file, b"CPB1\x01\x00\x00\x00garbage that is not a program").unwrap();
    let (p, _l) = common::pipeline(&device, &shaders::BLIT, Blend::Replace);
    let stats = device.program_cache_stats();
    assert_eq!(stats.refused, 1, "{stats:?}");
    assert_eq!(stats.compiled, 2, "{stats:?}");
    assert_eq!(stats.stored, 2, "{stats:?}");
    assert!(std::fs::read(&file).unwrap().len() > 40, "replaced");
    device.destroy_pipeline(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The platform [`common::device`] makes its device on.
fn platform() -> Platform {
    common::render_node().map_or(Platform::Surfaceless, Platform::RenderNode)
}

/// The toolkit's programs made ahead into a cache, as the compositor makes them at a session's start, then by a toolkit
/// process's device: it makes every pipeline of them from saved binaries and compiles none.
#[test]
fn programs_made_ahead_are_loaded_by_a_later_device_and_none_is_compiled() {
    let dir = scratch("ahead");
    if common::device().is_none() {
        return;
    }
    let n = shaders::TOOLKIT.len() as u32;
    let ahead = program_cache::make_ahead(platform(), &dir, shaders::TOOLKIT, Duration::ZERO);
    let Some(made) = ahead.unwrap() else {
        common::has(false, "program binary format");
        return;
    };
    assert_eq!(
        (made.compiled, made.stored, made.loaded),
        (n, n, 0),
        "{made:?}"
    );
    // made again (the next session's start): every one loaded and checked, none compiled
    let again = program_cache::make_ahead(platform(), &dir, shaders::TOOLKIT, Duration::ZERO)
        .unwrap()
        .unwrap();
    assert_eq!(
        (again.loaded, again.compiled, again.refused),
        (n, 0, 0),
        "{again:?}"
    );

    let mut device = common::device().unwrap();
    device.set_program_cache(Some(&dir));
    for shader in shaders::TOOLKIT {
        let (p, _l) = common::pipeline(&device, shader, Blend::PremultipliedOver);
        device.destroy_pipeline(p);
    }
    let stats = device.program_cache_stats();
    assert_eq!((stats.loaded, stats.compiled), (n, 0), "{stats:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A saved program the driver refuses (written by an earlier driver of the same strings, or damaged) is compiled again
/// and replaced when the programs are made ahead; the programs another driver version saved, in that driver's own
/// directory, are never loaded: the current driver's are compiled into its directory, and the next device loads them.
#[test]
fn making_programs_ahead_replaces_a_stale_binary_and_never_loads_another_drivers() {
    let dir = scratch("stale");
    let Some(mut device) = common::device() else {
        return;
    };
    device.set_program_cache(Some(&dir));
    let Some(ours) = device.program_cache_dir().map(Path::to_path_buf) else {
        common::has(false, "program binary format");
        return;
    };
    drop(device);
    let n = shaders::TOOLKIT.len() as u32;
    let made = |what| {
        program_cache::make_ahead(platform(), &dir, shaders::TOOLKIT, Duration::ZERO)
            .unwrap()
            .unwrap_or_else(|| panic!("{what}: the driver keeps binaries"))
    };
    made("first");
    // another driver version's directory holds the same programs; ours is gone
    let theirs = dir.join(program_cache::driver_key(
        "vendor",
        "renderer",
        "an older version",
    ));
    std::fs::rename(&ours, &theirs).unwrap();
    let fresh = made("beside another driver's");
    assert_eq!((fresh.loaded, fresh.compiled), (0, n), "{fresh:?}");
    let saved = std::fs::read_dir(&theirs)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "bin")
        })
        .count() as u32;
    assert_eq!(saved, n, "left as it was");

    // a binary the driver does not take, in our directory
    let fill = shaders::UI_FILL;
    let file = ours.join(program_cache::program_key(
        fill.name,
        fill.vertex_glsl,
        fill.fragment_glsl,
    ));
    std::fs::write(
        &file,
        b"CPB1\x01\x00\x00\x00a binary from another driver build",
    )
    .unwrap();
    let replaced = made("over a stale binary");
    assert_eq!(
        (replaced.refused, replaced.compiled, replaced.loaded),
        (1, 1, n - 1),
        "{replaced:?}"
    );
    assert!(std::fs::read(&file).unwrap().len() > 64, "replaced");
    let after = made("after the replacement");
    assert_eq!((after.loaded, after.compiled), (n, 0), "{after:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_device_made_on_a_worker_thread_draws_on_the_thread_it_is_sent_to() {
    let made = std::thread::spawn(|| {
        let device = common::device()?;
        let (pipeline, _) = common::pipeline(&device, &shaders::SHAPE, Blend::PremultipliedOver);
        device.destroy_pipeline(pipeline);
        Some(device.detach().unwrap())
    })
    .join()
    .unwrap();
    let Some(detached) = made else {
        return;
    };
    let device = detached.attach().unwrap();
    let target = common::target(&device, (8, 8), TextureFormat::Rgba8);
    common::draw_shapes(&device, &target, [0.25, 0.5, 0.75, 1.0], &[]);
    let px = device.read_bytes(&target, Region::new(3, 3, 1, 1)).unwrap();
    // a clear colour reaches 8 bits within a step
    for (got, want) in px.iter().zip([64u8, 128, 191, 255]) {
        assert!(got.abs_diff(want) <= 1, "{px:?}");
    }
    device.destroy_texture(target);
}

#[test]
fn a_native_fence_becomes_a_timeline_point_and_a_signalled_point_wakes_an_eventfd() {
    let Some(node) = common::render_node() else {
        common::has(false, "render node");
        return;
    };
    let Some(device) = common::device() else {
        return;
    };
    if !common::has(device.info().native_fences, "native fences") {
        return;
    }
    let sync = SyncNode::open(&node).unwrap();
    if !common::has(sync.supports_eventfd(), "syncobj eventfd") {
        return;
    }
    let timeline = sync.timeline().unwrap();
    let exported = sync.export(&timeline).unwrap();
    drop(exported);
    let release = sync.wait_fd(&timeline, 1).unwrap();
    assert!(!syncobj::wait_readable(release.as_fd(), 0));
    assert!(!sync.signalled(&timeline, 1));
    let target = common::target(&device, (8, 8), TextureFormat::Rgba8);
    common::draw_shapes(&device, &target, [1.0; 4], &[]);
    let (fence, file) = device.submit_exported().unwrap();
    sync.signal_with(&timeline, file.as_fd(), 1).unwrap();
    assert!(
        syncobj::wait_readable(release.as_fd(), 5000),
        "the point signalled with the fence"
    );
    assert!(sync.signalled(&timeline, 1));
    assert!(!sync.signalled(&timeline, 2));
    device.destroy_fence(fence);
    device.destroy_texture(target);
    sync.destroy(timeline);
}

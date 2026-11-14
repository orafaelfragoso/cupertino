//! The renderer's programs made ahead into the program cache, as the compositor makes the toolkit's at a session's
//! start: a later device makes every one of them from the saved binaries and compiles none.

use std::rc::Rc;
use std::time::Duration;

use cupertino_gpu::{program_cache, shaders, Platform};
use cupertino_ui_gpu::GpuPrograms;

use crate::common;

#[test]
fn the_renderers_programs_made_ahead_are_all_loaded_and_none_compiled() {
    let Some(device) = common::device() else {
        return;
    };
    let mut device = Rc::try_unwrap(device).expect("the test's own device");
    let base = std::env::temp_dir().join(format!("cupertino-ui-gpu-ahead-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let platform = common::render_node().map_or(Platform::Surfaceless, Platform::RenderNode);
    let made = program_cache::make_ahead(platform, &base, shaders::TOOLKIT, Duration::ZERO)
        .expect("the programs made ahead");
    if made.is_none() {
        assert!(
            !common::gpu_required(),
            "CUPERTINO_GPU_REQUIRED=1: the device keeps no program binaries"
        );
        eprintln!("SKIPPED: the device keeps no program binaries");
        return;
    }
    device.set_program_cache(Some(&base));
    let programs = GpuPrograms::new(&device).expect("the renderer's programs");
    let stats = device.program_cache_stats();
    assert_eq!(stats.compiled, 0, "{stats:?}");
    assert!(stats.loaded > 0, "{stats:?}");
    programs.destroy(&device);
    let _ = std::fs::remove_dir_all(&base);
}

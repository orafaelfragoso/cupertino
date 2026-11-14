//! The toolkit's GPU programs made into the per-user program cache when the session starts.
//!
//! A toolkit process on the GPU makes its renderer's and its presenter's programs ([`shaders::TOOLKIT`]) from saved
//! binaries in `$XDG_CACHE_HOME/cupertino/programs/<driver>/`; with none saved (a driver update, a shader change, a new
//! user) it compiles them and keeps the driver's compiler, some 55 MiB, until it exits. The server makes them there
//! first, once, on a thread of its own with a context of its own on the render node it offers clients, so no
//! application process compiles: the thread takes the driver directory's turn the processes take
//! ([`program_cache::take_turn`]), and one that starts meanwhile waits for it and loads what it saved. Nothing of it runs
//! on the frame path, and the first frame never waits for it. `prepare_toolkit_programs: false` in `compositor.json`
//! turns it off.

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_gpu::{program_cache, shaders, Platform, ProgramCacheStats};

/// How long the thread waits for a process that is making the programs already before making them anyway.
const TURN_PATIENCE: Duration = Duration::from_secs(10);

/// What the thread did: the cache's counts, `None` when the driver keeps no program binaries.
pub type Made = Result<Option<ProgramCacheStats>, String>;

/// Starts making the toolkit's programs for the device on `node` into the cache under `base`; the thread ends when
/// they are made. `None` when the thread cannot start.
pub fn start(node: PathBuf, base: PathBuf) -> Option<JoinHandle<Made>> {
    std::thread::Builder::new()
        .name("toolkit-programs".into())
        .spawn(move || {
            let started = Instant::now();
            let made = program_cache::make_ahead(
                Platform::RenderNode(node.clone()),
                &base,
                shaders::TOOLKIT,
                TURN_PATIENCE,
            )
            .map_err(|e| e.to_string());
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            match &made {
                Ok(Some(s)) => tracing::info!(
                    node = %node.display(),
                    loaded = s.loaded,
                    compiled = s.compiled,
                    refused = s.refused,
                    ms = format!("{ms:.1}"),
                    "the toolkit's GPU programs are in the program cache"
                ),
                Ok(None) => tracing::info!(
                    node = %node.display(),
                    "the driver keeps no program binaries: the toolkit's GPU programs are not made ahead"
                ),
                Err(e) => tracing::warn!(
                    node = %node.display(),
                    error = %e,
                    "the toolkit's GPU programs could not be made ahead"
                ),
            }
            made
        })
        .map_err(|e| tracing::warn!(error = %e, "no thread for the toolkit's GPU programs"))
        .ok()
}

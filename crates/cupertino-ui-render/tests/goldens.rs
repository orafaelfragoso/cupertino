//! Pixel baselines of the CPU renderer, in `tests/fixtures/goldens`.
//!
//! Size is exact. Pixels allow at most 2/255 per RGBA channel at every pixel, with zero pixels beyond that tolerance
//! and no ignored region. Every scene lies on an opaque backdrop, so the premultiplied frame is stored as plain RGBA.
//! `CUPERTINO_UPDATE_GOLDENS=1` re-records every baseline and then fails, so the variable can never turn a run green.
//! The baselines are valid on x86_64 with the default target features and the in-repo Inter subset; other
//! architectures and `-C target-cpu=native` builds are not measured, so elsewhere the test reports a skip.

#[cfg(target_arch = "x86_64")]
#[path = "goldens/baselines.rs"]
mod baselines;
#[cfg(target_arch = "x86_64")]
#[path = "goldens/scenes.rs"]
mod scenes;

#[cfg(not(target_arch = "x86_64"))]
#[test]
fn baselines_are_measured_on_x86_64_only() {
    eprintln!("SKIPPED: the CPU renderer baselines are valid on x86_64 only");
}

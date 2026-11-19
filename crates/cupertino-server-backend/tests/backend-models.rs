//! Public-boundary tests for `cupertino-server-backend` models: frame scheduling, late latching,
//! output bit depth, GPU reset tracking, software renderer selection, display head checks, and
//! loop wait capping.

use std::time::{Duration, Instant};

use cupertino_server_backend::backend::{loop_wait, REST_WAIT};
use cupertino_server_backend::displays::{
    check_head, default_scale, fallback_modes, from_stored_transform, output_rect, state_area,
    stored_transform, Head, HeadCaps, HeadId, HeadInfo, HeadMode, HeadState,
};
use cupertino_server_backend::gpu_reset::{clean, Loss, Resets};
use cupertino_server_backend::latch::{periods_late, requested, FrameTimes, GpuDone, LateLatch};
use cupertino_server_backend::output_depth::{format_bits, Depth};
use cupertino_server_backend::schedule::FrameScheduler;
use cupertino_server_backend::software_render;
use smithay::backend::allocator::Fourcc;
use smithay::utils::Transform;

#[test]
fn loop_wait_caps_to_rest_wait_and_picks_minimum() {
    assert_eq!(loop_wait([None, None]), REST_WAIT);
    assert_eq!(loop_wait([Some(Duration::from_secs(120)), None]), REST_WAIT);
    assert_eq!(
        loop_wait([
            Some(Duration::from_millis(40)),
            None,
            Some(Duration::from_millis(8)),
        ]),
        Duration::from_millis(8)
    );
}

#[test]
fn frame_scheduler_tracks_damage_queue_and_presentation() {
    let mut sched = FrameScheduler::default();
    assert!(sched.due(Duration::ZERO, false));
    sched.called_back();
    assert!(!sched.due(Duration::ZERO, false));
    sched.damage();
    assert!(sched.due(Duration::ZERO, false));
    sched.rendered();
    let id = sched.flip_queued(Duration::from_millis(1), false);
    assert!(sched.in_flight());
    let presented = sched
        .presented(id)
        .expect("queued presentation id must match");
    assert_eq!(presented.late_periods, 0);
    assert!(!presented.locked);
    assert!(!sched.in_flight());
    // Re-reporting an already presented id is ignored.
    assert!(sched.presented(id).is_none());
}

#[test]
fn late_latch_env_and_periods_late() {
    assert!(requested(None));
    assert!(requested(Some("1")));
    assert!(!requested(Some("0")));
    assert!(!requested(Some("off")));

    let refresh = Duration::from_millis(16);
    assert_eq!(
        periods_late(
            Duration::from_millis(100),
            Duration::from_millis(100),
            refresh
        ),
        0
    );
    assert_eq!(
        periods_late(
            Duration::from_millis(100),
            Duration::from_millis(132),
            refresh
        ),
        2
    );

    let mut latch = LateLatch::default();
    let flipped = latch.flipped(
        Some(FrameTimes {
            started: Duration::from_millis(10),
            cpu: Duration::from_millis(2),
            gpu: GpuDone::Unknown,
            held: false,
        }),
        Some(Duration::from_millis(16)),
        Some(Duration::from_millis(16)),
        refresh,
        true,
    );
    assert_eq!(flipped.late, 0);
}

#[test]
fn output_depth_and_format_bits() {
    assert_eq!(format_bits(Fourcc::Xrgb8888), 8);
    assert_eq!(format_bits(Fourcc::Argb8888), 8);
    assert_eq!(format_bits(Fourcc::Xrgb2101010), 10);
    assert_eq!(format_bits(Fourcc::Argb2101010), 10);
    assert_eq!(Depth::Eight.bits(), 8);
    assert_eq!(Depth::Ten.bits(), 10);
}

#[test]
fn gpu_reset_clean_and_rate_limit() {
    assert_eq!(clean("amd\ngpu\rreset\t0"), "amd gpu reset 0");
    let mut resets = Resets::default();
    let t0 = Instant::now();
    // Purge always allowed.
    assert!(resets.allow(&Loss::Purge, t0));
    let guilty = Loss::Reset("guilty".into());
    assert!(resets.allow(&guilty, t0));
    assert!(resets.allow(&guilty, t0 + Duration::from_secs(1)));
    assert!(resets.allow(&guilty, t0 + Duration::from_secs(2)));
    // Fourth reset inside the window stops further retries.
    assert!(!resets.allow(&guilty, t0 + Duration::from_secs(3)));
}

#[test]
fn software_render_choose_prefers_hardware_then_falls_back() {
    // First is software; chooses the first working hardware card from rest.
    let chosen = software_render::choose(
        "sw",
        ["hw-bad", "hw-good"],
        |c| {
            if *c == "hw-bad" {
                anyhow::bail!("failed")
            } else {
                Ok(*c)
            }
        },
        |r| *r == "sw",
    )
    .unwrap();
    assert_eq!(chosen, ("hw-good", "hw-good"));

    // Falls back to first when no hardware card opens.
    let sw_only = software_render::choose(
        "sw",
        ["hw-bad"],
        |c| {
            if *c == "sw" {
                Ok(*c)
            } else {
                anyhow::bail!("no")
            }
        },
        |r| *r == "sw",
    )
    .unwrap();
    assert_eq!(sw_only, ("sw", "sw"));
}

#[test]
fn display_heads_rect_modes_transforms_and_validation() {
    let mode_4k = HeadMode {
        width: 3840,
        height: 2160,
        refresh_mhz: 60_000,
    };
    let mode_hd = HeadMode {
        width: 1920,
        height: 1080,
        refresh_mhz: 60_000,
    };
    assert_eq!(default_scale(mode_4k, (600, 340), false), 1.5);
    assert!(default_scale(mode_4k, (310, 174), true) > 2.0);
    assert_eq!(default_scale(mode_hd, (530, 300), false), 1.0);

    for t in [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        assert_eq!(from_stored_transform(stored_transform(t)), t);
    }

    let r = output_rect(mode_4k.output_mode(), 2.0, Transform::_90, (0, 0));
    assert_eq!((r.w, r.h), (1080.0, 1920.0));

    let state = HeadState {
        enabled: true,
        mode: mode_4k,
        scale: 2.0,
        transform: Transform::Normal,
        position: (100, 50),
    };
    let area = state_area(&state);
    assert_eq!(
        (area.x, area.y, area.w, area.h),
        (100.0, 50.0, 1920.0, 1080.0)
    );

    let head = Head {
        id: HeadId(1),
        output: None,
        key: "Test:4K:123".into(),
        powered: true,
        wanted_mode: None,
        unsupported: false,
        advertised: None,
        info: HeadInfo {
            connector: "DP-1".into(),
            make: "Test".into(),
            model: "4K".into(),
            serial: Some("123".into()),
            physical_mm: (600, 340),
            modes: vec![mode_4k, mode_hd],
            caps: HeadCaps {
                can_disable: true,
                custom_modes: false,
                transforms: true,
            },
            base_transform: Transform::Normal,
        },
        state,
    };
    assert!(check_head(&head, &state).is_ok());

    let bad_mode = HeadState {
        mode: HeadMode {
            width: 800,
            height: 600,
            refresh_mhz: 60_000,
        },
        ..state
    };
    assert!(check_head(&head, &bad_mode).is_err());

    let fallbacks = fallback_modes(mode_4k, &[mode_4k, mode_hd]);
    assert_eq!(fallbacks, vec![mode_hd]);
}

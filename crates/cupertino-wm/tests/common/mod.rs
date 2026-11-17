//! A one- or two-output window manager and a few helpers shared by the window manager tests.

#![allow(dead_code)] // each test program uses a part of this helper

use cupertino_wm::{
    AppId, Command, Effect, LifecycleState, MapParams, OutputId, Rect, WindowId, Wm, WmConfig,
};

pub const OUT: OutputId = OutputId(1);
pub const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
/// The area below a 32 point menu bar.
pub const USABLE: Rect = Rect::new(0.0, 32.0, 1920.0, 1048.0);

pub fn win(n: u64) -> WindowId {
    WindowId(n)
}

pub fn wm() -> Wm {
    let mut wm = Wm::new(WmConfig::default());
    wm.handle(
        Command::AddOutput {
            output: OUT,
            rect: SCREEN,
            usable: USABLE,
        },
        0.0,
    );
    wm
}

pub fn frame(n: u64) -> Rect {
    let n = n as f64;
    Rect::new(100.0 + 40.0 * n, 100.0 + 30.0 * n, 640.0, 480.0)
}

/// Map window `n` of application `app` and let its opening animation finish.
pub fn open(wm: &mut Wm, n: u64, app: &str, now: f64) -> Vec<Effect> {
    let fx = wm.handle(Command::Map(MapParams::new(win(n), app, frame(n))), now);
    wm.tick(now + 2.0);
    fx
}

pub fn settle(wm: &mut Wm, now: f64) {
    wm.tick(now + 5.0);
}

pub fn state(wm: &Wm, n: u64) -> LifecycleState {
    wm.window(win(n)).expect("the window exists").state()
}

pub fn app_of(wm: &Wm, n: u64) -> AppId {
    wm.window(win(n)).expect("the window exists").app
}

pub fn has(fx: &[Effect], wanted: impl Fn(&Effect) -> bool) -> bool {
    fx.iter().any(wanted)
}

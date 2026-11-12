use cupertino_wm::*;

pub const OUT: OutputId = OutputId(1);
pub const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
pub const USABLE: Rect = Rect::new(0.0, 24.0, 1920.0, 1056.0);

pub struct H {
    pub wm: Wm,
    pub now: f64,
    pub next: u64,
}

impl H {
    pub fn new() -> H {
        Self::with_config(WmConfig::default())
    }

    pub fn with_config(cfg: WmConfig) -> H {
        let mut h = H {
            wm: Wm::new(cfg),
            now: 0.0,
            next: 1,
        };
        h.cmd(Command::AddOutput {
            output: OUT,
            rect: SCREEN,
            usable: USABLE,
        });
        h
    }

    pub fn cmd(&mut self, c: Command) -> Vec<Effect> {
        self.wm.handle(c, self.now)
    }

    pub fn advance(&mut self, dt: f64) -> Vec<Effect> {
        let end = self.now + dt;
        let mut fx = Vec::new();
        while self.now < end - 1e-9 {
            self.now = (self.now + 1.0 / 120.0).min(end);
            fx.extend(self.wm.tick(self.now));
        }
        fx
    }

    pub fn settle(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        for _ in 0..2000 {
            if !self.wm.wants_tick(self.now) {
                break;
            }
            fx.extend(self.advance(1.0 / 120.0));
        }
        assert!(!self.wm.wants_tick(self.now), "did not settle");
        fx
    }

    pub fn map(&mut self, app: &str) -> WindowId {
        self.map_at(app, Rect::new(100.0, 100.0, 600.0, 400.0))
    }

    pub fn map_at(&mut self, app: &str, g: Rect) -> WindowId {
        let id = WindowId(self.next);
        self.next += 1;
        self.cmd(Command::Map(MapParams::new(id, app, g)));
        self.settle();
        id
    }

    pub fn app(&self, identity: &str) -> AppId {
        self.wm.app_for_identity(identity).unwrap()
    }

    pub fn state(&self, w: WindowId) -> LifecycleState {
        self.wm.window(w).unwrap().state()
    }

    pub fn frame(&self, w: WindowId) -> Rect {
        self.wm.presentation(w, self.now).unwrap().frame
    }

    pub fn ws(&self) -> Vec<WorkspaceId> {
        self.wm.workspaces(OUT).iter().map(|w| w.id).collect()
    }
}

pub fn has(fx: &[Effect], f: impl Fn(&Effect) -> bool) -> bool {
    fx.iter().any(f)
}

pub fn rect_eq(a: Rect, b: Rect) -> bool {
    a.to_array()
        .iter()
        .zip(b.to_array())
        .all(|(x, y)| (x - y).abs() < 1e-6)
}

pub fn motion(x: f64, y: f64) -> Command {
    Command::PointerMotion {
        pointer: Point::new(x, y),
        modifiers: Modifiers::default(),
    }
}

pub fn motion_opt(x: f64, y: f64) -> Command {
    Command::PointerMotion {
        pointer: Point::new(x, y),
        modifiers: Modifiers {
            option: true,
            ..Default::default()
        },
    }
}

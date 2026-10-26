//! Damped spring matching the design spring model.
//!
//! Progress p(t) runs from 0 to 1; `initial_velocity` is in "fractions of the total distance per
//! second", like the design spring's initial velocity.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    pub mass: f64,
    pub stiffness: f64,
    pub damping: f64,
    pub initial_velocity: f64,
    pub allows_overdamping: bool,
}

impl Default for SpringParams {
    fn default() -> Self {
        Self {
            mass: 1.0,
            stiffness: 100.0,
            damping: 10.0,
            initial_velocity: 0.0,
            allows_overdamping: false,
        }
    }
}

impl SpringParams {
    /// Reference perceptual duration/bounce constructor (formulae):
    /// mass = 1, stiffness = (2π/d)², damping = 4π(1−b)/d for b ≥ 0, 4π/((1+b)·d) for b < 0.
    pub fn from_perceptual(duration: f64, bounce: f64) -> Self {
        let inv = 1.0 / duration;
        let stiffness = if duration > 0.0 {
            (std::f64::consts::TAU * inv).powi(2)
        } else {
            f64::INFINITY
        };
        let b = if bounce < 0.0 {
            1.0 / (bounce + 1.0)
        } else {
            1.0 - bounce
        };
        Self {
            mass: 1.0,
            stiffness,
            damping: b * 4.0 * std::f64::consts::PI * inv,
            ..Default::default()
        }
    }

    /// Perceptual duration = 2π·sqrt(m/k)
    pub fn perceptual_duration(&self) -> f64 {
        std::f64::consts::TAU * (self.mass / self.stiffness).sqrt()
    }

    /// Perceptual bounce
    pub fn bounce(&self) -> f64 {
        let x =
            self.perceptual_duration() * self.damping / (self.mass * 4.0 * std::f64::consts::PI);
        let zeta = self.damping / (2.0 * (self.stiffness * self.mass).sqrt());
        if zeta > 1.0 {
            1.0 / x - 1.0
        } else {
            1.0 - x
        }
    }

    /// Damping ratio-based constructor (approximate convenience, not part of the design API).
    pub fn from_ratio(response: f64, damping_ratio: f64) -> Self {
        let w0 = std::f64::consts::TAU / response;
        Self {
            mass: 1.0,
            stiffness: w0 * w0,
            damping: 2.0 * damping_ratio * w0,
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Under { wd: f64, b: f64 },
    Critical { b: f64 },
    Over { r: f64, c1: f64, c2: f64 },
}

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub params: SpringParams,
    w0: f64,
    zeta: f64,
    mode: Mode,
}

impl Spring {
    pub fn new(params: SpringParams) -> Self {
        let w0 = (params.stiffness / params.mass).sqrt();
        let zeta = params.damping / (2.0 * (params.stiffness * params.mass).sqrt());
        let v0 = params.initial_velocity;
        let mode = if zeta < 1.0 {
            let wd = (1.0 - zeta * zeta).sqrt() * w0;
            Mode::Under {
                wd,
                b: (zeta * w0 - v0) / wd,
            }
        } else if zeta > 1.0 && params.allows_overdamping {
            // p(t) = 1 − c1·e^{(−ζω0+r)t} − c2·e^{(−ζω0−r)t}, with p(0)=0, p'(0)=v0
            let r = (zeta * zeta - 1.0).sqrt() * w0;
            let a = zeta * w0;
            let c1 = (v0 - (a + r)) / (-2.0 * r);
            let c2 = 1.0 - c1;
            Mode::Over { r, c1, c2 }
        } else {
            Mode::Critical { b: w0 - v0 }
        };
        Self {
            params,
            w0,
            zeta,
            mode,
        }
    }

    pub fn damping_ratio(&self) -> f64 {
        self.zeta
    }

    /// Progress at time `t` seconds (0 → 1, may overshoot).
    pub fn value(&self, t: f64) -> f64 {
        if t <= 0.0 {
            return 0.0;
        }
        match self.mode {
            Mode::Under { wd, b } => {
                1.0 - (-self.zeta * self.w0 * t).exp() * ((wd * t).cos() + b * (wd * t).sin())
            }
            Mode::Critical { b } => 1.0 - (-self.w0 * t).exp() * (1.0 + b * t),
            Mode::Over { r, c1, c2 } => {
                let a = self.zeta * self.w0;
                1.0 - c1 * ((-a + r) * t).exp() - c2 * ((-a - r) * t).exp()
            }
        }
    }

    /// d(progress)/dt at time `t` (fractions of distance per second).
    pub fn velocity(&self, t: f64) -> f64 {
        let t = t.max(0.0);
        match self.mode {
            Mode::Under { wd, b } => {
                let a = self.zeta * self.w0;
                let e = (-a * t).exp();
                let (s, c) = (wd * t).sin_cos();
                e * (a * (c + b * s) - (-wd * s + b * wd * c))
            }
            Mode::Critical { b } => {
                let e = (-self.w0 * t).exp();
                e * (self.w0 * (1.0 + b * t) - b)
            }
            Mode::Over { r, c1, c2 } => {
                let a = self.zeta * self.w0;
                -c1 * (-a + r) * ((-a + r) * t).exp() - c2 * (-a - r) * ((-a - r) * t).exp()
            }
        }
    }

    /// Settling duration = `duration_for_epsilon(0.001).
    pub fn settling_duration(&self) -> f64 {
        self.duration_for_epsilon(1e-3)
    }

    /// Mirrors the design duration-for-epsilon control flow:
    /// ζ<1 → analytic envelope bound; ζ≥1 → sample every 0.1 s, return the start of the first
    /// interval where |1−p| stays below ε for more than 1 s.
    pub fn duration_for_epsilon(&self, eps: f64) -> f64 {
        let eps = eps.max(1e-6);
        match self.mode {
            Mode::Under { b, .. } => {
                let amp = 1.0 + b.abs();
                ((eps / amp).ln() / (-self.zeta * self.w0)).max(0.0)
            }
            _ => {
                let mut t = 0.0;
                let mut entered: Option<f64> = None;
                while t < 600.0 {
                    let d = (1.0 - self.value(t)).abs();
                    if d < eps {
                        let s = *entered.get_or_insert(t);
                        if t - s > 1.0 {
                            return s;
                        }
                    } else {
                        entered = None;
                    }
                    t += 0.1;
                }
                entered.unwrap_or(t)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_reference() {
        let p = SpringParams::default();
        assert_eq!((p.mass, p.stiffness, p.damping), (1.0, 100.0, 10.0));
        let s = Spring::new(p);
        assert!((s.damping_ratio() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn boundary_conditions() {
        for &(k, c, v0, over) in &[
            (100.0, 10.0, 0.0, false),
            (300.0, 40.0, 2.0, false),
            (100.0, 40.0, -1.0, true),
            (100.0, 40.0, 0.0, false),
        ] {
            let s = Spring::new(SpringParams {
                stiffness: k,
                damping: c,
                initial_velocity: v0,
                allows_overdamping: over,
                ..Default::default()
            });
            assert!(s.value(0.0).abs() < 1e-12);
            let h = 1e-6;
            let num = s.value(h) / h;
            assert!((num - v0).abs() < 1e-3, "v0 {} vs {}", num, v0);
            assert!((s.velocity(0.0) - v0).abs() < 1e-9);
            assert!((s.value(20.0) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn perceptual_roundtrip() {
        for &(d, b) in &[(0.5, 0.0), (0.35, 0.3), (1.0, -0.4)] {
            let p = SpringParams::from_perceptual(d, b);
            assert!((p.perceptual_duration() - d).abs() < 1e-12);
            assert!((p.bounce() - b).abs() < 1e-9, "{} vs {}", p.bounce(), b);
        }
    }

    #[test]
    fn velocity_matches_derivative() {
        let s = Spring::new(SpringParams {
            initial_velocity: 3.0,
            ..Default::default()
        });
        for i in 1..50 {
            let t = i as f64 * 0.037;
            let h = 1e-6;
            let num = (s.value(t + h) - s.value(t - h)) / (2.0 * h);
            assert!((num - s.velocity(t)).abs() < 1e-5);
        }
    }

    #[test]
    fn settling_is_monotone_in_stiffness() {
        let a = Spring::new(SpringParams {
            stiffness: 100.0,
            ..Default::default()
        })
        .settling_duration();
        let b = Spring::new(SpringParams {
            stiffness: 400.0,
            damping: 20.0,
            ..Default::default()
        })
        .settling_duration();
        assert!(b < a);
    }
}

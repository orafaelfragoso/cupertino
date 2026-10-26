//! Pointer acceleration: the design's parametric curve family applied to relative
//! pointer motion.
//!
//! one gain ratio per event from the
//! motion magnitude; the speed the curve sees is counts per event at a nominal 67 Hz, so the
//! device's real report rate only matters when a report rate is configured. The curve is a
//! polynomial up to a linear tangent and then a square-root segment, scaled from inches per second
//! to pixels per event by 96/67. The ten curves and the setting → curve interpolation are as designed.
//! Mapping Linux device units onto "counts" is (see [`PointerAccel::new`]).
//! The same per-event logic runs a table curve ([`crate::table`]) when built through
//! [`PointerAccel::with_curve`]; a generic mouse gets the default table.
//!
//! Fresh-account settings are as designed kernel defaults: [`DEFAULT_TRACKING_SPEED`],
//! [`DEFAULT_SCROLLING_SPEED`], [`DEFAULT_DOUBLE_CLICK_TIME`], [`DEFAULT_ACCELERATION`].

use crate::table::TableCurve;

/// Nominal event rate the speed is expressed at.
pub const NOMINAL_RATE: f64 = 67.0;
/// Output pixels per inch of input at setting 0 (the `96/67` literal).
pub const PIXELS_PER_INCH: f64 = 96.0;
/// Resolution assumed when a device reports none.
pub const DEFAULT_RESOLUTION_DPI: f64 = 400.0;
/// Points scrolled per wheel line: the legacy scroll event's point delta is ten times the line
/// delta.
pub const POINTS_PER_LINE: f64 = 10.0;
/// Smallest speed fed to the curve, `1/65536`.
pub(crate) const MIN_SPEED: f64 = 1.0 / 65536.0;

/// Tracking speed of a fresh account, mouse and trackpad alike: the kernel's compiled-in
/// `0xb000`, stop 3 of [`TRACKING_SPEED_STOPS`].
pub const DEFAULT_TRACKING_SPEED: f64 = 0.6875;
/// Scrolling speed of a fresh account: the kernel's compiled-in `0x5000`.
pub const DEFAULT_SCROLLING_SPEED: f64 = 0.3125;
/// Double-click interval of a fresh account in seconds: 500 000 000 ns.
pub const DEFAULT_DOUBLE_CLICK_TIME: f64 = 0.5;
/// Pointer acceleration of a fresh account: on, because the kernel defaults never set the
/// linear-scaling flag.
pub const DEFAULT_ACCELERATION: bool = true;

/// Point delta of a legacy scroll event carrying `lines`: `trunc(10 × lines)
/// (the kernel's legacy path multiplies by the same 10.0).
pub fn scroll_point_delta(lines: f64) -> i32 {
    (lines * POINTS_PER_LINE).trunc() as i32
}

/// One curve of the family; every field is a 16.16 fixed-point value in the source data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveParams {
    /// Setting value this curve belongs to.
    pub index: f64,
    pub gain_linear: f64,
    pub gain_parabolic: f64,
    pub gain_cubic: f64,
    pub gain_quartic: f64,
    /// Input speed (inches per second) where the polynomial hands over to its tangent; 0 = none.
    pub tangent_linear: f64,
    /// Input speed where the tangent hands over to the square-root segment; 0 = none.
    pub tangent_root: f64,
}

impl CurveParams {
    /// Build from raw 16.16 fixed-point fields in source order.
    pub const fn from_fixed(raw: [i32; 7]) -> Self {
        const F: f64 = 65536.0;
        Self {
            index: raw[0] as f64 / F,
            gain_linear: raw[1] as f64 / F,
            gain_parabolic: raw[2] as f64 / F,
            gain_cubic: raw[3] as f64 / F,
            gain_quartic: raw[4] as f64 / F,
            tangent_linear: raw[5] as f64 / F,
            tangent_root: raw[6] as f64 / F,
        }
    }

    fn has_gain(&self) -> bool {
        [
            self.gain_linear,
            self.gain_parabolic,
            self.gain_cubic,
            self.gain_quartic,
        ]
        .iter()
        .any(|g| *g != 0.0)
    }

    fn lerp(&self, other: &Self, t: f64) -> Self {
        let l = |a: f64, b: f64| a + (b - a) * t;
        Self {
            index: l(self.index, other.index),
            gain_linear: l(self.gain_linear, other.gain_linear),
            gain_parabolic: l(self.gain_parabolic, other.gain_parabolic),
            gain_cubic: l(self.gain_cubic, other.gain_cubic),
            gain_quartic: l(self.gain_quartic, other.gain_quartic),
            tangent_linear: l(self.tangent_linear, other.tangent_linear),
            tangent_root: l(self.tangent_root, other.tangent_root),
        }
    }

    fn poly(&self, v: f64) -> f64 {
        self.gain_linear * v
            + (self.gain_parabolic * v).powi(2)
            + (self.gain_cubic * v).powi(3)
            + (self.gain_quartic * v).powi(4)
    }

    fn slope(&self, v: f64) -> f64 {
        self.gain_linear
            + 2.0 * v * self.gain_parabolic.powi(2)
            + 3.0 * v * v * self.gain_cubic.powi(3)
            + 4.0 * v.powi(3) * self.gain_quartic.powi(4)
    }
}

/// Curve family of the generic HID pointer driver, one curve per tracking-speed stop.
pub const MOUSE_CURVES: [CurveParams; 10] = [
    CurveParams::from_fixed([0, 65536, 0, 0, 0, 524288, 0]),
    CurveParams::from_fixed([8192, 60293, 26214, 5243, 0, 537395, 1245184]),
    CurveParams::from_fixed([32768, 60948, 36045, 6554, 0, 543949, 1179648]),
    CurveParams::from_fixed([45056, 61604, 46531, 7864, 0, 550502, 1114112]),
    CurveParams::from_fixed([57344, 62259, 57672, 9830, 0, 557056, 1048576]),
    CurveParams::from_fixed([65536, 62915, 69468, 11796, 0, 563610, 983040]),
    CurveParams::from_fixed([98304, 63570, 81920, 14418, 0, 570163, 917504]),
    CurveParams::from_fixed([131072, 64225, 95027, 17695, 0, 576717, 851968]),
    CurveParams::from_fixed([163840, 64881, 108790, 21627, 0, 583270, 786432]),
    CurveParams::from_fixed([196608, 65536, 123208, 26214, 0, 589824, 786432]),
];

/// Curve family of the built-in trackpads.
pub const TRACKPAD_CURVES: [CurveParams; 10] = [
    CurveParams::from_fixed([0, 65536, 0, 0, 0, 484966, 1376256]),
    CurveParams::from_fixed([8192, 64881, 32768, 5243, 0, 478413, 1310720]),
    CurveParams::from_fixed([32768, 64225, 43254, 6554, 0, 471859, 1245184]),
    CurveParams::from_fixed([45056, 62915, 54395, 7864, 0, 465306, 1179648]),
    CurveParams::from_fixed([57344, 61604, 65536, 9830, 0, 458752, 1114112]),
    CurveParams::from_fixed([65536, 60293, 75366, 11796, 0, 458752, 1048576]),
    CurveParams::from_fixed([98304, 58327, 85197, 13763, 0, 458752, 983040]),
    CurveParams::from_fixed([131072, 56361, 95027, 15729, 0, 458752, 917504]),
    CurveParams::from_fixed([163840, 54395, 108790, 18350, 0, 458752, 851968]),
    CurveParams::from_fixed([196608, 65536, 123208, 23593, 0, 458752, 786432]),
];

/// Scroll curve family of the built-in trackpads (`HIDScrollAccelCurves`, shared by the 37
/// built-in and the external trackpad personalities). The scroll accelerator
/// prefers these curves over the trackpads' scroll table when no algorithm is declared
/// (the scroll algorithm selection).
pub const TRACKPAD_SCROLL_CURVES: [CurveParams; 10] = [
    CurveParams::from_fixed([0, 65536, 0, 0, 0, 393216, 786432]),
    CurveParams::from_fixed([8192, 62259, 39322, 0, 0, 406323, 786432]),
    CurveParams::from_fixed([32768, 58982, 58982, 0, 0, 419430, 786432]),
    CurveParams::from_fixed([45056, 55706, 78643, 0, 0, 432538, 786432]),
    CurveParams::from_fixed([57344, 52429, 91750, 0, 0, 445645, 786432]),
    CurveParams::from_fixed([65536, 49152, 104858, 0, 0, 458752, 786432]),
    CurveParams::from_fixed([98304, 45875, 117965, 0, 0, 471859, 786432]),
    CurveParams::from_fixed([131072, 42598, 131072, 0, 0, 484966, 786432]),
    CurveParams::from_fixed([163840, 39322, 144179, 0, 0, 498074, 786432]),
    CurveParams::from_fixed([196608, 36045, 157286, 0, 0, 511181, 786432]),
];

/// Side of the square, in pixels, within which a second click still counts as a double click,
/// for a double-click interval of `interval_s` seconds: `min(6t + 2, 14) (formula of
/// the double-click slider). The 5 px it gives for the fresh 0.5 s is as the
/// fresh value: the kernel publishes the fresh click space from cursor state, not a literal.
pub fn double_click_space(interval_s: f64) -> f64 {
    (6.0 * interval_s + 2.0).min(14.0)
}

/// Tracking-speed slider stops; each selects one curve of a family.
pub const TRACKING_SPEED_STOPS: [f64; 10] =
    [0.0, 0.125, 0.5, 0.6875, 0.875, 1.0, 1.5, 2.0, 2.5, 3.0];
/// Report rate the built-in trackpads declare, in Hz.
pub const TRACKPAD_REPORT_RATE_HZ: u32 = 120;
/// Gain used by linear scaling when the setting is 0.
pub const LINEAR_MINIMUM: f64 = 0.1;

/// Which piece of the curve applies above the first tangent point.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tail {
    /// Polynomial only.
    None,
    /// Linear tangent `m1·v + b1` from `t1`, optionally followed by `√(m2·v + b2)` from `t2`.
    Tangent {
        t1: f64,
        m1: f64,
        b1: f64,
        root: Option<(f64, f64, f64)>,
    },
    /// No linear tangent: the square root starts directly at `t` (the design's handling when
    /// only the second tangent speed is set; slope `f'(t)`, not `2·f(t)·f'(t)).
    RootOnly { t: f64, m: f64, b: f64 },
}

/// A curve selected for one setting value, mapping input speed to output pixels per event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParametricCurve {
    params: CurveParams,
    tail: Tail,
}

impl ParametricCurve {
    /// Select the curve for `setting` from `curves` (ascending `index`); `None` when the setting
    /// is negative (acceleration off) or no curve has a non-zero gain. Between two stops every
    /// parameter is interpolated linearly; past the last stop the last curve is used.
    pub fn new(curves: &[CurveParams], setting: f64) -> Option<Self> {
        if setting < 0.0 {
            return None;
        }
        let valid: Vec<&CurveParams> = curves.iter().filter(|c| c.has_gain()).collect();
        let lo = valid.iter().rposition(|c| c.index <= setting).unwrap_or(0);
        let params = match valid.get(lo + 1) {
            Some(hi) if setting > valid[lo].index => valid[lo].lerp(
                hi,
                (setting - valid[lo].index) / (hi.index - valid[lo].index),
            ),
            _ => **valid.get(lo)?,
        };
        let (t1, t2) = (params.tangent_linear, params.tangent_root);
        let tail = if t1 != 0.0 {
            let m1 = params.slope(t1);
            let b1 = params.poly(t1) - m1 * t1;
            let root = (t2 != 0.0).then(|| {
                let l = m1 * t2 + b1;
                let m2 = 2.0 * l * m1;
                (t2, m2, l * l - m2 * t2)
            });
            Tail::Tangent { t1, m1, b1, root }
        } else if t2 != 0.0 {
            let m = params.slope(t2);
            Tail::RootOnly {
                t: t2,
                m,
                b: params.poly(t2).powi(2) - m * t2,
            }
        } else {
            Tail::None
        };
        Some(Self { params, tail })
    }

    /// Output speed (inches per second) for an input speed `v` (inches per second).
    pub fn speed(&self, v: f64) -> f64 {
        match self.tail {
            Tail::Tangent { t1, m1, b1, root } => match root {
                _ if v <= t1 => self.params.poly(v),
                Some((t2, m2, b2)) if v > t2 => (m2 * v + b2).sqrt(),
                _ => m1 * v + b1,
            },
            Tail::RootOnly { t, m, b } if v > t => (m * v + b).sqrt(),
            _ => self.params.poly(v),
        }
    }
}

/// The curve of one setting, from either algorithm.
#[derive(Clone, Debug, PartialEq)]
pub enum AccelCurve {
    Parametric(ParametricCurve),
    Table(TableCurve),
}

impl AccelCurve {
    /// Output pixels per event for `counts` per event of a device with `resolution` counts per
    /// inch reporting at `rate` Hz: the curve sees `counts · rate / resolution` inches per second
    /// and its output is scaled by 96/67.
    pub(crate) fn pixels(&self, counts: f64, resolution: f64, rate: f64) -> f64 {
        let v = counts * rate / resolution;
        let out = match self {
            Self::Parametric(c) => c.speed(v),
            Self::Table(c) => c.speed(v),
        };
        out * PIXELS_PER_INCH / NOMINAL_RATE
    }
}

/// How motion is scaled.
#[derive(Clone, Debug, PartialEq)]
enum Mode {
    /// Counts pass through unchanged (negative setting).
    Off,
    /// `pixels = counts × gain` (the "pointer acceleration" toggle switched off).
    Linear(f64),
    Curve(AccelCurve),
}

/// Relative-motion accelerator for one device.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerAccel {
    mode: Mode,
    resolution_dpi: f64,
    /// Report period in milliseconds when a report rate is configured.
    report_period_ms: Option<f64>,
}

impl PointerAccel {
    /// `setting` is the tracking-speed value (0…3, see [`TRACKING_SPEED_STOPS`]; negative turns
    /// acceleration off and passes counts through unchanged). `resolution_dpi` converts counts to
    /// inches (0 selects [`DEFAULT_RESOLUTION_DPI`]); the input to [`accelerate`](Self::accelerate)
    /// is in counts. `report_rate_hz` enables the time factor that slows the speed estimate when
    /// events arrive later than one report period (0 = off, as for a device that declares no rate).
    ///
    /// mapping for Linux: libinput's unaccelerated deltas are device counts normalised to
    /// 1000 dpi, which equal raw counts for a mouse without a hardware-database entry; feeding them
    /// with the 400 dpi fallback reproduces the design's treatment of a generic mouse.
    pub fn new(
        curves: &[CurveParams],
        setting: f64,
        resolution_dpi: f64,
        report_rate_hz: u32,
    ) -> Self {
        Self::with_curve(
            ParametricCurve::new(curves, setting).map(AccelCurve::Parametric),
            resolution_dpi,
            report_rate_hz,
        )
    }

    /// Like [`new`](Self::new) with an already selected curve of either algorithm; `None`
    /// passes counts through unchanged. A generic mouse uses the default table: `TableCurve::new(&AccelTable::default_table, setting).map(AccelCurve::Table).
    pub fn with_curve(curve: Option<AccelCurve>, resolution_dpi: f64, report_rate_hz: u32) -> Self {
        Self::with_mode(
            curve.map_or(Mode::Off, Mode::Curve),
            resolution_dpi,
            report_rate_hz,
        )
    }

    /// Linear scaling by `setting` (the design's "pointer acceleration" off): no curve, no
    /// speed dependence; a setting of 0 uses [`LINEAR_MINIMUM`].
    pub fn linear(setting: f64) -> Self {
        let mode = match setting {
            s if s < 0.0 => Mode::Off,
            0.0 => Mode::Linear(LINEAR_MINIMUM),
            s => Mode::Linear(s),
        };
        Self::with_mode(mode, DEFAULT_RESOLUTION_DPI, 0)
    }

    fn with_mode(mode: Mode, resolution_dpi: f64, report_rate_hz: u32) -> Self {
        Self {
            mode,
            resolution_dpi: if resolution_dpi > 0.0 {
                resolution_dpi
            } else {
                DEFAULT_RESOLUTION_DPI
            },
            report_period_ms: (report_rate_hz != 0).then(|| 1000.0 / f64::from(report_rate_hz)),
        }
    }

    /// Accelerate one event of `(dx, dy)` counts that arrived `dt_ms` after the previous one
    /// (0 when unknown). Returns pixels.
    pub fn accelerate(&self, dx: f64, dy: f64, dt_ms: f64) -> (f64, f64) {
        let curve = match &self.mode {
            Mode::Off => return (dx, dy),
            Mode::Linear(gain) => return (dx * gain, dy * gain),
            Mode::Curve(curve) => curve,
        };
        let k = match self.report_period_ms {
            Some(period) if dt_ms != 0.0 => period / period.max(dt_ms),
            _ => 1.0,
        };
        let counts = (k * dx.hypot(dy).floor()).max(MIN_SPEED);
        let r = curve.pixels(counts, self.resolution_dpi, NOMINAL_RATE) / counts;
        (dx * r, dy * r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::AccelTable;

    fn gain(curves: &[CurveParams], setting: f64, counts: f64) -> f64 {
        PointerAccel::new(curves, setting, 400.0, 0)
            .accelerate(counts, 0.0, 0.0)
            .0
            / counts
    }

    #[test]
    fn setting_zero_is_one_to_one_at_96_ppi() {
        // 400 counts = one inch → 96 px while the curve is linear (below 8 in/s = 47.8 counts).
        for counts in [1.0, 12.0, 30.0, 47.0] {
            assert!(
                (gain(&MOUSE_CURVES, 0.0, counts) - 0.24).abs() < 1e-12,
                "{counts}"
            );
        }
    }

    #[test]
    fn negative_setting_passes_counts_through() {
        let a = PointerAccel::new(&MOUSE_CURVES, -1.0, 400.0, 0);
        assert_eq!(a.accelerate(7.0, -3.0, 8.0), (7.0, -3.0));
        assert_eq!(
            PointerAccel::linear(-1.0).accelerate(7.0, -3.0, 8.0),
            (7.0, -3.0)
        );
    }

    #[test]
    fn linear_scaling_ignores_speed() {
        assert_eq!(
            PointerAccel::linear(2.0).accelerate(3.0, 50.0, 1.0),
            (6.0, 100.0)
        );
        assert_eq!(
            PointerAccel::linear(0.0).accelerate(10.0, 0.0, 1.0),
            (1.0, 0.0)
        );
    }

    #[test]
    fn matches_reference_implementation() {
        // Gains (px per count, 400 dpi, no report rate).
        let cases: [(&[CurveParams], f64, f64, f64); 8] = [
            (&MOUSE_CURVES, 1.0, 6.0, 0.5028262221579067),
            (&MOUSE_CURVES, 1.0, 60.0, 3.0174739327571514),
            (&MOUSE_CURVES, 3.0, 24.0, 3.8982192145401338),
            (&MOUSE_CURVES, 0.5, 120.0, 1.205646964942708),
            (&MOUSE_CURVES, 0.6875, 10.0, 0.4294160252705795),
            (&TRACKPAD_CURVES, 1.0, 60.0, 3.227045591846353),
            (&TRACKPAD_CURVES, 0.5, 120.0, 1.496272133801256),
            (&TRACKPAD_CURVES, 0.6875, 10.0, 0.5085040399999003),
        ];
        for (curves, setting, counts, expected) in cases {
            let g = gain(curves, setting, counts);
            assert!(
                (g - expected).abs() < 1e-9,
                "setting {setting} counts {counts}: {g}"
            );
        }
    }

    #[test]
    fn curve_is_continuous_at_both_tangents() {
        for curves in [&MOUSE_CURVES, &TRACKPAD_CURVES] {
            let c = ParametricCurve::new(curves, 1.0).unwrap();
            for t in [c.params.tangent_linear, c.params.tangent_root] {
                assert!((c.speed(t - 1e-9) - c.speed(t + 1e-9)).abs() < 1e-6, "{t}");
            }
        }
    }

    #[test]
    fn stops_select_curves_and_settings_between_stops_interpolate() {
        for (stop, curve) in TRACKING_SPEED_STOPS.iter().zip(MOUSE_CURVES) {
            assert_eq!(
                ParametricCurve::new(&MOUSE_CURVES, *stop).unwrap().params,
                curve
            );
        }
        let c = ParametricCurve::new(&MOUSE_CURVES, 0.75).unwrap().params;
        let (a, b) = (MOUSE_CURVES[3], MOUSE_CURVES[4]);
        let t = (0.75 - 0.6875) / (0.875 - 0.6875);
        assert!(
            (c.gain_parabolic - (a.gain_parabolic + (b.gain_parabolic - a.gain_parabolic) * t))
                .abs()
                < 1e-12
        );
        assert_eq!(
            ParametricCurve::new(&MOUSE_CURVES, 9.0).unwrap().params,
            MOUSE_CURVES[9]
        );
    }

    #[test]
    fn report_rate_slows_late_events() {
        let a = PointerAccel::new(&MOUSE_CURVES, 1.0, 400.0, 125);
        assert_eq!(
            a.accelerate(40.0, 0.0, 8.0).0,
            a.accelerate(40.0, 0.0, 0.0).0,
            "on time = no time factor"
        );
        // Four periods late: the curve sees the speed of 10 counts, its ratio scales all 40.
        let late = a.accelerate(40.0, 0.0, 32.0).0;
        assert!(
            (late - 40.0 * gain(&MOUSE_CURVES, 1.0, 10.0)).abs() < 1e-12,
            "{late}"
        );
    }

    #[test]
    fn default_table_matches_reference_implementation() {
        // Gains (px per count, 400 dpi, no report rate).
        let cases = [
            (0.0, 6.0, 0.23999999999999996),
            (0.6875, 1.0, 0.22858712526304895),
            (0.6875, 24.0, 0.7029787774824254),
            (0.6875, 60.0, 1.3079891292521268),
            (1.0, 6.0, 0.650114439919366),
            (1.0, 300.0, 0.7164179104477612),
            (3.0, 60.0, 3.5868774669152788),
        ];
        let table = AccelTable::default_table();
        for (setting, counts, expected) in cases {
            let curve = TableCurve::new(&table, setting).map(AccelCurve::Table);
            let g = PointerAccel::with_curve(curve, 400.0, 0)
                .accelerate(counts, 0.0, 0.0)
                .0
                / counts;
            assert!(
                (g - expected).abs() < 1e-9,
                "setting {setting} counts {counts}: {g}"
            );
        }
    }

    #[test]
    fn fresh_defaults_select_stop_three() {
        assert_eq!(TRACKING_SPEED_STOPS[3], DEFAULT_TRACKING_SPEED);
    }

    #[test]
    fn scroll_point_delta_truncates_ten_per_line() {
        assert_eq!(
            [
                scroll_point_delta(1.0),
                scroll_point_delta(0.25),
                scroll_point_delta(-1.37)
            ],
            [10, 2, -13]
        );
    }
}

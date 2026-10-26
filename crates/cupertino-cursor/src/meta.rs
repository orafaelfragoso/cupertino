//! Per-cursor metadata: hotspot, shadow and animation.
//!
//! Source: a JSON file shaped like (`{"cursors": {name: {...}}})
//! or the property list next to each cursor ([`crate::plist`]). Keys (to exist in the
//! source cursor set's property lists; the meaning of some is):
//!
//! | key | meaning |
//! |---|---|
//! | `hotx`, `hoty` | hotspot in points, origin top-left of a frame |
//! | `hotx-scaled`, `hoty-scaled` | hotspot of an alternative (scaled) variant — meaning unknown, selectable |
//! | `shadowcolor` | RGBA of the shadow, components 0…1 |
//! | `shadowoffsetx`, `shadowoffsety` | shadow offset in points; the y axis points up in the source convention, so −1 is one point *down* on screen |
//! | `blur` | Gaussian standard deviation in points (as specified for this tool) |
//! | `frames`, `delay` | number of stacked frames in the page and the frame delay in seconds |
//! | `vectoronly`, `rendershadow` | flags, effect unknown (carried, not interpreted) |

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawMeta {
    pub hotx: Option<f64>,
    pub hoty: Option<f64>,
    #[serde(rename = "hotx-scaled")]
    pub hotx_scaled: Option<f64>,
    #[serde(rename = "hoty-scaled")]
    pub hoty_scaled: Option<f64>,
    pub shadowcolor: Option<Vec<f64>>,
    pub shadowoffsetx: Option<f64>,
    pub shadowoffsety: Option<f64>,
    pub blur: Option<f64>,
    pub frames: Option<f64>,
    pub delay: Option<f64>,
    pub vectoronly: Option<bool>,
    pub rendershadow: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// Straight RGBA, 0…1.
    pub color: [f32; 4],
    /// Offset in points, screen convention (x right, y down).
    pub dx: f64,
    pub dy: f64,
    /// Gaussian standard deviation in points.
    pub sigma: f64,
}

impl Shadow {
    /// The shadow shared by most entries of the design set (colour `0,0,0,0.65`, blur 1.8, one point
    /// down). Used for cursors without metadata when requested; approximate choice.
    pub const COMMON: Shadow = Shadow {
        color: [0.0, 0.0, 0.0, 0.65],
        dx: 0.0,
        dy: 1.0,
        sigma: 1.8,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HotspotSource {
    #[default]
    Unscaled,
    Scaled,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CursorMeta {
    pub hotspot: Option<(f64, f64)>,
    pub hotspot_scaled: Option<(f64, f64)>,
    pub shadow: Option<Shadow>,
    pub frames: u32,
    pub delay_ms: u32,
    pub vector_only: bool,
    pub render_shadow: bool,
}

impl CursorMeta {
    pub fn hotspot_for(&self, source: HotspotSource) -> Option<(f64, f64)> {
        match source {
            HotspotSource::Unscaled => self.hotspot.or(self.hotspot_scaled),
            HotspotSource::Scaled => self.hotspot_scaled.or(self.hotspot),
        }
    }

    /// `flip_y`: the source's shadow y offset points up (the default interpretation).
    pub fn from_raw(r: &RawMeta, flip_y: bool) -> CursorMeta {
        let pair = |x: Option<f64>, y: Option<f64>| x.zip(y);
        let shadow = r
            .shadowcolor
            .as_ref()
            .filter(|c| c.len() >= 3)
            .zip(r.blur)
            .filter(|(_, b)| *b > 0.0)
            .map(|(c, blur)| {
                let dy = r.shadowoffsety.unwrap_or(0.0);
                Shadow {
                    color: [
                        c[0] as f32,
                        c[1] as f32,
                        c[2] as f32,
                        c.get(3).copied().unwrap_or(1.0) as f32,
                    ],
                    dx: r.shadowoffsetx.unwrap_or(0.0),
                    dy: if flip_y { -dy } else { dy },
                    sigma: blur,
                }
            });
        CursorMeta {
            hotspot: pair(r.hotx, r.hoty),
            hotspot_scaled: pair(r.hotx_scaled, r.hoty_scaled),
            shadow,
            frames: r.frames.map_or(1, |f| f.round().max(1.0) as u32),
            delay_ms: r.delay.map_or(0, |d| (d * 1000.0).round().max(0.0) as u32),
            vector_only: r.vectoronly.unwrap_or(false),
            render_shadow: r.rendershadow.unwrap_or(false),
        }
    }
}

#[derive(Deserialize)]
struct File {
    cursors: BTreeMap<String, RawMeta>,
}

/// Parse a metadata JSON document.
pub fn from_json(text: &str, flip_y: bool) -> Result<BTreeMap<String, CursorMeta>, String> {
    let f: File =
        serde_json::from_str(text).map_err(|e| format!("invalid cursor metadata: {e}"))?;
    Ok(f.cursors
        .iter()
        .map(|(k, v)| (k.clone(), CursorMeta::from_raw(v, flip_y)))
        .collect())
}

pub fn from_json_file(path: &Path, flip_y: bool) -> Result<BTreeMap<String, CursorMeta>, String> {
    from_json(
        &std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
        flip_y,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"source": "x", "cursors": {
        "a": {"hotx": 5, "hoty": 6, "hotx-scaled": 7.5, "hoty-scaled": 8, "shadowcolor": [0, 0, 0, 0.65], "shadowoffsetx": 0, "shadowoffsety": -1, "blur": 1.8, "vectoronly": true},
        "b": {"hotx": 1, "hoty": 2},
        "anim": {"hotx": 12, "hoty": 12, "frames": 6, "delay": 0.5, "shadowcolor": [1, 0, 0, 1], "blur": 0.8, "shadowoffsety": -1},
        "scaled_only": {"hotx-scaled": 3, "hoty-scaled": 4}
    }}"#;

    #[test]
    fn parses_hotspot_shadow_and_animation() {
        let m = from_json(JSON, true).unwrap();
        let a = &m["a"];
        assert_eq!(a.hotspot, Some((5.0, 6.0)));
        assert_eq!(a.hotspot_scaled, Some((7.5, 8.0)));
        let s = a.shadow.unwrap();
        assert_eq!(s.color, [0.0, 0.0, 0.0, 0.65]);
        assert_eq!(
            (s.dx, s.dy, s.sigma),
            (0.0, 1.0, 1.8),
            "source y offset −1 points up-axis → one point down"
        );
        assert!(a.vector_only && !a.render_shadow);
        assert_eq!(a.frames, 1);
        assert_eq!(m["anim"].frames, 6);
        assert_eq!(m["anim"].delay_ms, 500);
        assert!(m["b"].shadow.is_none());
    }

    #[test]
    fn y_flip_is_optional() {
        let m = from_json(JSON, false).unwrap();
        assert_eq!(m["a"].shadow.unwrap().dy, -1.0);
    }

    #[test]
    fn hotspot_source_selection_with_fallbacks() {
        let m = from_json(JSON, true).unwrap();
        assert_eq!(
            m["a"].hotspot_for(HotspotSource::Unscaled),
            Some((5.0, 6.0))
        );
        assert_eq!(m["a"].hotspot_for(HotspotSource::Scaled), Some((7.5, 8.0)));
        assert_eq!(m["b"].hotspot_for(HotspotSource::Scaled), Some((1.0, 2.0)));
        assert_eq!(
            m["scaled_only"].hotspot_for(HotspotSource::Unscaled),
            Some((3.0, 4.0))
        );
        assert_eq!(
            CursorMeta::default().hotspot_for(HotspotSource::Unscaled),
            None
        );
    }

    #[test]
    fn invalid_documents_are_errors() {
        assert!(from_json("{", true).is_err());
        assert!(from_json("{}", true).is_err());
        assert!(from_json_file(Path::new("/nonexistent/meta.json"), true).is_err());
    }

    #[test]
    fn zero_blur_means_no_shadow() {
        let m = from_json(
            r#"{"cursors": {"z": {"shadowcolor": [0,0,0,1], "blur": 0}}}"#,
            true,
        )
        .unwrap();
        assert!(m["z"].shadow.is_none());
    }
}

//! An 8-bit and a 10-bit gradient on the primary plane, the driver's dithering off where the driver lets it be set and
//! the link's depth raised to 10 bits where it can be, so the eye compares what a deep-colour plane gains.
//!
//! Three pictures, `--seconds` each: the gradients in an 8-bit plane (`XRGB8888`); the same in a 10-bit plane
//! (`ABGR2101010`); then one 10-bit plane whose every band shows the 8-bit values in its upper half and the 10-bit
//! values in its lower half, for a side-by-side look. The bands, top to bottom: grey over the darkest quarter, grey over
//! the whole range, a dark blue, a warm tone. The screen goes back to what it showed before.
//!
//! Needs the display: run it from a text console with the graphical session stopped (`tools/gpu-probes/depth.sh`).
//! `--dry-run` takes nothing: it lists the connectors, planes and properties, and makes and frees both buffers.

use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};

use drm::buffer::DrmFourcc;
use drm::control::{
    connector, crtc, property, Device as ControlDevice, FbCmd2Flags, ResourceHandle,
};
use drm::{ClientCapability, Device};

struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

/// A dumb buffer as the one-plane buffer a framebuffer of any format is made from.
struct Planar<'a>(&'a drm::control::dumbbuffer::DumbBuffer);

impl drm::buffer::PlanarBuffer for Planar<'_> {
    fn size(&self) -> (u32, u32) {
        drm::buffer::Buffer::size(self.0)
    }
    fn format(&self) -> DrmFourcc {
        drm::buffer::Buffer::format(self.0)
    }
    fn modifier(&self) -> Option<drm::buffer::DrmModifier> {
        None
    }
    fn pitches(&self) -> [u32; 4] {
        [drm::buffer::Buffer::pitch(self.0), 0, 0, 0]
    }
    fn handles(&self) -> [Option<drm::buffer::Handle>; 4] {
        [Some(drm::buffer::Buffer::handle(self.0)), None, None, None]
    }
    fn offsets(&self) -> [u32; 4] {
        [0; 4]
    }
}

impl Device for Card {}
impl ControlDevice for Card {}

/// The gradient's value at `x` of `width` in band `band`, as 10-bit red, green and blue.
pub fn band_colour(band: usize, x: u32, width: u32) -> [u32; 3] {
    let t = f64::from(x) / f64::from(width.max(2) - 1);
    let ramp = |from: f64, to: f64| ((from + (to - from) * t) * 1023.0).round() as u32;
    match band {
        0 => {
            let v = ramp(0.0, 0.25);
            [v, v, v]
        }
        1 => {
            let v = ramp(0.0, 1.0);
            [v, v, v]
        }
        2 => [ramp(0.0, 0.05), ramp(0.0, 0.1), ramp(0.05, 0.35)],
        _ => [ramp(0.3, 0.6), ramp(0.2, 0.45), ramp(0.1, 0.25)],
    }
}

/// A 10-bit value rounded to the nearest 8-bit one, back in 10 bits: what an 8-bit plane can show of it.
pub fn as_8_bit(v: u32) -> u32 {
    let e = ((f64::from(v) * 255.0 / 1023.0).round()) as u32;
    ((f64::from(e) * 1023.0 / 255.0).round()) as u32
}

/// One `XRGB8888` pixel of a 10-bit colour.
pub fn xrgb8888(c: [u32; 3]) -> u32 {
    let e = |v: u32| ((f64::from(v) * 255.0 / 1023.0).round()) as u32;
    (e(c[0]) << 16) | (e(c[1]) << 8) | e(c[2])
}

/// One opaque `ABGR2101010` pixel.
pub fn abgr2101010(c: [u32; 3]) -> u32 {
    (3 << 30) | (c[2] << 20) | (c[1] << 10) | c[0]
}

/// The three pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    Eight,
    Ten,
    Split,
}

/// Fills `pixels` (rows of `pitch` bytes) with `picture`.
pub fn fill(pixels: &mut [u8], width: u32, height: u32, pitch: u32, picture: Picture) {
    let bands = 4;
    for y in 0..height {
        let band = (y * bands / height) as usize;
        let band_top = band as u32 * height / bands;
        let band_height = height / bands;
        let upper = y < band_top + band_height / 2;
        let row = &mut pixels[(y * pitch) as usize..][..(width * 4) as usize];
        for x in 0..width {
            let c = band_colour(band, x, width);
            let px = match picture {
                Picture::Eight => xrgb8888(c),
                Picture::Ten => abgr2101010(c),
                Picture::Split if upper => abgr2101010(c.map(as_8_bit)),
                Picture::Split => abgr2101010(c),
            };
            row[(x * 4) as usize..][..4].copy_from_slice(&px.to_le_bytes());
        }
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Every property of `handle`: name, value and, for an enum, the name of its value.
fn properties<T: ResourceHandle>(
    card: &Card,
    handle: T,
) -> Vec<(property::Handle, String, u64, property::Info)> {
    let Ok(set) = card.get_properties(handle) else {
        return Vec::new();
    };
    set.iter()
        .filter_map(|(&h, &v)| {
            let info = card.get_property(h).ok()?;
            Some((h, info.name().to_string_lossy().into_owned(), v, info))
        })
        .collect()
}

fn describe(props: &[(property::Handle, String, u64, property::Info)]) -> String {
    let items: Vec<String> = props
        .iter()
        .map(|(_, name, v, info)| {
            let shown = match info.value_type() {
                property::ValueType::Enum(e) => e
                    .get_value_from_raw_value(*v)
                    .map(|e| e.name().to_string_lossy().into_owned())
                    .unwrap_or_else(|| v.to_string()),
                property::ValueType::UnsignedRange(lo, hi) => format!("{v} ({lo}..{hi})"),
                _ => v.to_string(),
            };
            format!("{}:{}", json_str(name), json_str(&shown))
        })
        .collect();
    format!("{{{}}}", items.join(","))
}

/// The properties to set on the connector before the pictures: dithering off, the link's depth up to 10 bits.
fn wanted_settings(
    props: &[(property::Handle, String, u64, property::Info)],
) -> Vec<(property::Handle, String, u64)> {
    let mut out = Vec::new();
    for (h, name, _, info) in props {
        let lower = name.to_lowercase();
        match info.value_type() {
            property::ValueType::Enum(e)
                if lower.contains("dither") && !lower.contains("depth") =>
            {
                let (_, enums) = e.values();
                if let Some(off) = enums.iter().find(|v| {
                    let n = v.name().to_string_lossy().to_lowercase();
                    n == "off" || n == "disabled" || n == "disable"
                }) {
                    out.push((*h, name.clone(), off.value()));
                }
            }
            property::ValueType::Boolean if lower.contains("dither") => {
                out.push((*h, name.clone(), 0))
            }
            property::ValueType::UnsignedRange(lo, hi) if lower == "max bpc" => {
                out.push((*h, name.clone(), 10u64.clamp(lo, hi)))
            }
            _ => {}
        }
    }
    out
}

pub fn run(args: &[String]) {
    let dry = args.iter().any(|a| a == "--dry-run");
    let seconds: u64 = arg(args, "--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let path = arg(args, "--card").unwrap_or_else(|| {
        let mut cards: Vec<String> = std::fs::read_dir("/dev/dri")
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .map(|e| e.path().to_string_lossy().into_owned())
                    .filter(|p| p.contains("/card"))
                    .collect()
            })
            .unwrap_or_default();
        cards.sort();
        cards
            .into_iter()
            .next()
            .unwrap_or_else(|| "/dev/dri/card0".into())
    });
    let file = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            println!(
                r#"{{"probe":"depth","error":{}}}"#,
                json_str(&format!("{path}: {e}"))
            );
            return;
        }
    };
    let card = Card(file);
    if dry {
        // a dry run must never hold the display: should opening have made this process its master, give it back now
        let _ = card.release_master_lock();
    }
    let _ = card.set_client_capability(ClientCapability::UniversalPlanes, true);
    let res = match card.resource_handles() {
        Ok(r) => r,
        Err(e) => {
            println!(
                r#"{{"probe":"depth","error":{}}}"#,
                json_str(&format!("resources: {e}"))
            );
            return;
        }
    };
    // the first connected connector, its CRTC (the current one, else the first its encoders allow) and preferred mode
    let mut chosen: Option<(connector::Handle, crtc::Handle, drm::control::Mode)> = None;
    for &c in res.connectors() {
        let Ok(info) = card.get_connector(c, false) else {
            continue;
        };
        let props = properties(&card, c);
        println!(
            r#"{{"probe":"depth","connector":{},"state":{},"modes":{},"properties":{}}}"#,
            json_str(&format!("{:?}-{}", info.interface(), info.interface_id())),
            json_str(&format!("{:?}", info.state())),
            info.modes().len(),
            describe(&props)
        );
        if chosen.is_some() || info.state() != connector::State::Connected {
            continue;
        }
        let crtc = info
            .current_encoder()
            .and_then(|e| card.get_encoder(e).ok())
            .and_then(|e| e.crtc())
            .or_else(|| {
                info.encoders()
                    .iter()
                    .filter_map(|&e| card.get_encoder(e).ok())
                    .flat_map(|e| res.filter_crtcs(e.possible_crtcs()))
                    .next()
            });
        let mode = info
            .modes()
            .iter()
            .find(|m| {
                m.mode_type()
                    .contains(drm::control::ModeTypeFlags::PREFERRED)
            })
            .or_else(|| info.modes().first())
            .copied();
        if let (Some(crtc), Some(mode)) = (crtc, mode) {
            chosen = Some((c, crtc, mode));
        }
    }
    let Some((conn, crtc, mode)) = chosen else {
        println!(r#"{{"probe":"depth","error":"no connected display"}}"#);
        return;
    };
    let (w, h) = mode.size();
    let (w, h) = (u32::from(w), u32::from(h));
    // the primary plane of that CRTC and whether it takes the 10-bit format
    let crtc_index = res.crtcs().iter().position(|&c| c == crtc).unwrap_or(0);
    for plane in card.plane_handles().unwrap_or_default() {
        let Ok(info) = card.get_plane(plane) else {
            continue;
        };
        if !res.filter_crtcs(info.possible_crtcs()).contains(&crtc) {
            continue;
        }
        let kind = properties(&card, plane)
            .into_iter()
            .find(|(_, n, _, _)| n == "type")
            .and_then(|(_, _, v, info)| match info.value_type() {
                property::ValueType::Enum(e) => e
                    .get_value_from_raw_value(v)
                    .map(|e| e.name().to_string_lossy().into_owned()),
                _ => None,
            })
            .unwrap_or_default();
        let formats: Vec<String> = info
            .formats()
            .iter()
            .map(|&f| DrmFourcc::try_from(f).map_or(format!("{f:#x}"), |f| f.to_string()))
            .collect();
        println!(
            r#"{{"probe":"depth","crtc_index":{crtc_index},"plane":{},"type":{},"takes_AB30":{},"takes_XR24":{},"formats":{}}}"#,
            u32::from(plane),
            json_str(&kind),
            formats.iter().any(|f| f == "AB30"),
            formats.iter().any(|f| f == "XR24"),
            json_str(&formats.join(" "))
        );
    }
    let crtc_props = properties(&card, crtc);
    println!(
        r#"{{"probe":"depth","crtc_properties":{}}}"#,
        describe(&crtc_props)
    );

    // both buffers, filled; framebuffers made, which the driver refuses for a format it cannot scan out
    let mut fbs = Vec::new();
    for (picture, format) in [
        (Picture::Eight, DrmFourcc::Xrgb8888),
        (Picture::Ten, DrmFourcc::Abgr2101010),
        (Picture::Split, DrmFourcc::Abgr2101010),
    ] {
        let mut buffer = match card.create_dumb_buffer((w, h), format, 32) {
            Ok(b) => b,
            Err(e) => {
                println!(
                    r#"{{"probe":"depth","picture":"{picture:?}","error":{}}}"#,
                    json_str(&format!("dumb buffer: {e}"))
                );
                continue;
            }
        };
        let pitch = drm::buffer::Buffer::pitch(&buffer);
        if let Ok(mut map) = card.map_dumb_buffer(&mut buffer) {
            fill(&mut map, w, h, pitch, picture);
        }
        match card.add_planar_framebuffer(&Planar(&buffer), FbCmd2Flags::empty()) {
            Ok(fb) => {
                println!(
                    r#"{{"probe":"depth","picture":"{picture:?}","format":"{format}","framebuffer":"made"}}"#
                );
                fbs.push((picture, fb, buffer));
            }
            Err(e) => {
                println!(
                    r#"{{"probe":"depth","picture":"{picture:?}","format":"{format}","error":{}}}"#,
                    json_str(&format!("framebuffer: {e}"))
                );
                let _ = card.destroy_dumb_buffer(buffer);
            }
        }
    }

    if dry {
        for (_, name, value) in wanted_settings(&properties(&card, conn)) {
            println!(
                r#"{{"probe":"depth","would_set":{},"value":{value}}}"#,
                json_str(&name)
            );
        }
    } else {
        if let Err(e) = card.acquire_master_lock() {
            println!(
                r#"{{"probe":"depth","error":{}}}"#,
                json_str(&format!("the display is held by another program ({e}): stop the graphical session first"))
            );
        } else {
            let before = card.get_crtc(crtc).ok();
            let conn_props = properties(&card, conn);
            let settings = wanted_settings(&conn_props);
            for (h, name, value) in &settings {
                let result = card.set_property(conn, *h, *value);
                println!(
                    r#"{{"probe":"depth","set":{},"value":{value},"ok":{}}}"#,
                    json_str(name),
                    result.is_ok()
                );
            }
            if settings.is_empty() {
                println!(
                    r#"{{"probe":"depth","note":"the connector has no dithering or link depth property; the driver's defaults apply"}}"#
                );
            }
            for (picture, fb, _) in &fbs {
                let shown = card.set_crtc(crtc, Some(*fb), (0, 0), &[conn], Some(mode));
                println!(
                    r#"{{"probe":"depth","showing":"{picture:?}","seconds":{seconds},"ok":{}{}}}"#,
                    shown.is_ok(),
                    shown.err().map_or(String::new(), |e| format!(
                        r#","error":{}"#,
                        json_str(&e.to_string())
                    ))
                );
                std::thread::sleep(std::time::Duration::from_secs(seconds));
            }
            println!(
                r#"{{"probe":"depth","connector_after":{}}}"#,
                describe(&properties(&card, conn))
            );
            // the console's picture back
            if let Some(before) = before {
                let _ = card.set_crtc(
                    crtc,
                    before.framebuffer(),
                    before.position(),
                    &[conn],
                    before.mode(),
                );
            }
            let _ = card.release_master_lock();
        }
    }
    for (_, fb, buffer) in fbs {
        let _ = card.destroy_framebuffer(fb);
        let _ = card.destroy_dumb_buffer(buffer);
    }
    println!(r#"{{"probe":"depth","done":true,"dry_run":{dry},"size":"{w}x{h}"}}"#);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_10_bit_pixel_packs_red_low_and_alpha_high() {
        assert_eq!(abgr2101010([1023, 0, 0]), 0xC000_03FF);
        assert_eq!(abgr2101010([0, 0, 1023]), 0xFFF0_0000);
        assert_eq!(xrgb8888([1023, 512, 0]), 0x00FF_8000);
    }

    #[test]
    fn the_8_bit_half_of_the_split_picture_has_a_quarter_of_the_steps() {
        let width = 1024;
        let ten: std::collections::BTreeSet<u32> =
            (0..width).map(|x| band_colour(1, x, width)[0]).collect();
        let eight: std::collections::BTreeSet<u32> = ten.iter().map(|&v| as_8_bit(v)).collect();
        assert_eq!(ten.len(), 1024);
        assert_eq!(eight.len(), 256);
    }

    #[test]
    fn the_split_picture_shows_8_bit_values_above_and_10_bit_below_in_each_band() {
        let (w, h) = (64, 8);
        let mut px = vec![0u8; (w * h * 4) as usize];
        fill(&mut px, w, h, w * 4, Picture::Split);
        let at = |x: u32, y: u32| {
            u32::from_le_bytes(px[((y * w + x) * 4) as usize..][..4].try_into().unwrap())
        };
        // band 1 spans rows 2 and 3: row 2 is the 8-bit half, row 3 the 10-bit half
        let x = 33;
        let c = band_colour(1, x, w);
        assert_eq!(at(x, 2), abgr2101010(c.map(as_8_bit)));
        assert_eq!(at(x, 3), abgr2101010(c));
    }
}

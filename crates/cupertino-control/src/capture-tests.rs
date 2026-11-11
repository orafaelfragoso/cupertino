//! The capture model: which device pixels a frame keeps, the image size they become and the logical rectangle the
//! image covers, and the image itself for synthetic frames.
use super::*;
use std::io::Write;

fn region(x: f64, y: f64, width: f64, height: f64) -> Region {
    Region {
        x,
        y,
        width,
        height,
    }
}

/// The logical point image pixel `(px, py)` shows under `plan`, as a caller maps it.
fn logical(plan: &Plan, px: f64, py: f64) -> (f64, f64) {
    let r = plan.region;
    (
        r.x + px * r.width / f64::from(plan.size.0),
        r.y + py * r.height / f64::from(plan.size.1),
    )
}

/// A frame of `width` × `height` device pixels whose colour at `(x, y)` is `colour(x, y)` as red, green and blue.
fn frame(width: u32, height: u32, colour: impl Fn(u32, u32) -> [u8; 3]) -> Pixels {
    let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = colour(x, y);
            // the buffer's byte order: blue, green, red, alpha
            bytes.extend_from_slice(&[b, g, r, 255]);
        }
    }
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&bytes).unwrap();
    Pixels {
        width,
        height,
        upright: true,
        file,
    }
}

/// Red on the left half, green on the bottom half: four quadrants of black, red, green and yellow.
fn quadrants(width: u32, height: u32) -> impl Fn(u32, u32) -> [u8; 3] {
    move |x, y| {
        [
            if x < width / 2 { 255 } else { 0 },
            if y < height / 2 { 0 } else { 255 },
            0,
        ]
    }
}

fn decode(png: &[u8]) -> image::RgbaImage {
    image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .unwrap()
        .to_rgba8()
}

fn rgb(image: &image::RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = image.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

#[test]
fn a_whole_output_shrinks_to_one_image_pixel_per_logical_point() {
    let plan = plan((3840, 2160), (2560, 1440), None).unwrap();
    assert_eq!(
        plan.crop,
        Crop {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160
        }
    );
    assert_eq!(plan.size, (2560, 1440));
    assert_eq!(plan.region, region(0.0, 0.0, 2560.0, 1440.0));
    assert_eq!(logical(&plan, 100.0, 50.0), (100.0, 50.0));
    // at scale 1 nothing is resampled
    let same = super::plan((800, 600), (800, 600), None).unwrap();
    assert_eq!(same.size, (800, 600));
}

#[test]
fn a_frame_with_fewer_pixels_than_points_is_never_enlarged() {
    let plan = plan((640, 480), (800, 600), None).unwrap();
    assert_eq!(plan.size, (640, 480));
    assert_eq!(plan.region, region(0.0, 0.0, 800.0, 600.0));
    // the mapping still lands on logical points
    assert_eq!(logical(&plan, 320.0, 240.0), (400.0, 300.0));
}

#[test]
fn a_region_is_cropped_at_the_device_density() {
    let plan = plan(
        (3840, 2160),
        (2560, 1440),
        Some(region(10.0, 20.0, 100.0, 50.0)),
    )
    .unwrap();
    assert_eq!(
        plan.crop,
        Crop {
            x: 15,
            y: 30,
            width: 150,
            height: 75
        }
    );
    assert_eq!(
        plan.size,
        (150, 75),
        "no resampling: one image pixel per device pixel"
    );
    assert_eq!(plan.region, region(10.0, 20.0, 100.0, 50.0));
    assert_eq!(logical(&plan, 0.0, 0.0), (10.0, 20.0));
    assert_eq!(logical(&plan, 150.0, 75.0), (110.0, 70.0));
    assert_eq!(logical(&plan, 75.0, 30.0), (60.0, 40.0));
}

#[test]
fn region_edges_between_device_pixels_move_outward_and_the_result_says_where() {
    let asked = region(10.2, 0.1, 10.0, 10.0);
    let plan = plan((3840, 2160), (2560, 1440), Some(asked)).unwrap();
    // 15.3 → 15 and 30.3 → 31 device pixels across, 0.15 → 0 and 15.15 → 16 down
    assert_eq!(
        plan.crop,
        Crop {
            x: 15,
            y: 0,
            width: 16,
            height: 16
        }
    );
    let covered = plan.region;
    assert_eq!(covered.x, 10.0);
    assert_eq!(covered.y, 0.0);
    assert!((covered.width - 16.0 / 1.5).abs() < 1e-9);
    assert!(covered.x <= asked.x && covered.y <= asked.y);
    assert!(covered.x + covered.width >= asked.x + asked.width);
    assert!(covered.y + covered.height >= asked.y + asked.height);
    // the far edge of the image is the far edge of the covered rectangle
    let (right, bottom) = logical(&plan, 16.0, 16.0);
    assert!((right - (covered.x + covered.width)).abs() < 1e-9);
    assert!((bottom - (covered.y + covered.height)).abs() < 1e-9);
    // an edge on a pixel boundary up to rounding stays there
    let third = super::plan(
        (300, 300),
        (100, 100),
        Some(region(1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0)),
    )
    .unwrap();
    assert_eq!(third.crop.x, 1);
    assert_eq!(third.crop.width, 1);
}

#[test]
fn a_region_must_be_finite_positive_and_inside_the_output() {
    let output = (800, 600);
    for bad in [
        region(f64::NAN, 0.0, 10.0, 10.0),
        region(0.0, f64::INFINITY, 10.0, 10.0),
        region(0.0, 0.0, f64::INFINITY, 10.0),
    ] {
        assert!(bad.check(output).unwrap_err().contains("finite"), "{bad:?}");
    }
    for bad in [region(0.0, 0.0, 0.0, 10.0), region(0.0, 0.0, 10.0, -1.0)] {
        assert!(
            bad.check(output).unwrap_err().contains("positive"),
            "{bad:?}"
        );
    }
    for bad in [
        region(-1.0, 0.0, 10.0, 10.0),
        region(0.0, -0.5, 10.0, 10.0),
        region(795.0, 0.0, 10.0, 10.0),
        region(0.0, 595.0, 10.0, 10.0),
        region(800.0, 0.0, 1.0, 1.0),
    ] {
        let error = bad.check(output).unwrap_err();
        assert!(
            error.contains("inside the granted output"),
            "{bad:?}: {error}"
        );
        assert!(error.contains("800x600"), "{error}");
        // refused, not clamped
        assert!(plan((800, 600), output, Some(bad)).is_err());
    }
    // the whole output and a rectangle touching its far corner are fine
    assert!(region(0.0, 0.0, 800.0, 600.0).check(output).is_ok());
    let corner = plan((1200, 900), output, Some(region(799.5, 599.5, 0.5, 0.5))).unwrap();
    assert_eq!(
        corner.crop,
        Crop {
            x: 1199,
            y: 899,
            width: 1,
            height: 1
        }
    );
}

#[test]
fn a_frame_that_is_not_upright_is_refused() {
    // a turned 2560×1440 output whose buffer kept the panel's own orientation
    for region in [None, Some(region(0.0, 0.0, 10.0, 10.0))] {
        assert_eq!(
            plan((2160, 3840), (2560, 1440), region).unwrap_err(),
            NOT_UPRIGHT
        );
    }
    // an upright buffer of a turned output maps directly
    assert_eq!(
        plan((2160, 3840), (1440, 2560), None).unwrap().size,
        (1440, 2560)
    );
    // each side rounds on its own at a fractional scale and is still upright
    assert_eq!(
        plan((1281, 720), (1025, 576), None).unwrap().size,
        (1025, 576)
    );
    assert!(plan((0, 10), (10, 10), None).is_err());
    assert!(plan((10, 10), (0, 10), None).is_err());
}

#[test]
fn the_image_is_the_frame_at_logical_size_with_the_colours_in_place() {
    // a 800×600 output at scale 1.5
    let pixels = frame(1200, 900, quadrants(1200, 900));
    let plan = plan((1200, 900), (800, 600), None).unwrap();
    let image = decode(&pixels.png(&plan).unwrap());
    assert_eq!(image.dimensions(), (800, 600));
    assert_eq!(rgb(&image, 100, 100), [255, 0, 0]);
    assert_eq!(rgb(&image, 700, 100), [0, 0, 0]);
    assert_eq!(rgb(&image, 100, 500), [255, 255, 0]);
    assert_eq!(rgb(&image, 700, 500), [0, 255, 0]);
    // the quadrants meet at the logical centre
    assert_eq!(rgb(&image, 397, 297), [255, 0, 0]);
    assert_eq!(rgb(&image, 402, 302), [0, 255, 0]);
}

#[test]
fn a_region_image_holds_the_device_pixels_of_that_rectangle() {
    let pixels = frame(1200, 900, quadrants(1200, 900));
    // 40 points around the centre: device pixels 570..630 by 420..480
    let plan = plan(
        (1200, 900),
        (800, 600),
        Some(region(380.0, 280.0, 40.0, 40.0)),
    )
    .unwrap();
    let image = decode(&pixels.png(&plan).unwrap());
    assert_eq!(image.dimensions(), (60, 60));
    // a crop is exact: the edge between the quadrants is sharp at image pixel 30
    assert_eq!(rgb(&image, 29, 29), [255, 0, 0]);
    assert_eq!(rgb(&image, 30, 29), [0, 0, 0]);
    assert_eq!(rgb(&image, 29, 30), [255, 255, 0]);
    assert_eq!(rgb(&image, 30, 30), [0, 255, 0]);
    // and placed where the region is: a crop of the top-left corner is all red
    let corner = super::plan((1200, 900), (800, 600), Some(region(0.0, 0.0, 10.0, 10.0))).unwrap();
    let pixels = frame(1200, 900, quadrants(1200, 900));
    let image = decode(&pixels.png(&corner).unwrap());
    assert_eq!(image.dimensions(), (15, 15));
    assert!(image.pixels().all(|p| p.0 == [255, 0, 0, 255]));
}

#[test]
fn one_pixel_strokes_stay_dark_at_two_thirds() {
    // white with a one-device-pixel black column every six pixels, like the stems of small text at scale 1.5
    let pixels = frame(600, 30, |x, _| if x % 6 == 2 { [0; 3] } else { [255; 3] });
    let plan = plan((600, 30), (400, 20), None).unwrap();
    let image = decode(&pixels.png(&plan).unwrap());
    assert_eq!(image.dimensions(), (400, 20));
    // every stroke still has an image pixel well below mid-grey, and the paper between strokes stays light
    for period in 1..99 {
        let row: Vec<u8> = (period * 4..period * 4 + 4)
            .map(|x| rgb(&image, x, 10)[0])
            .collect();
        let darkest = *row.iter().min().unwrap();
        let lightest = *row.iter().max().unwrap();
        assert!(darkest < 100, "stroke {period} faded to {darkest}: {row:?}");
        assert!(
            lightest > 200,
            "paper {period} greyed to {lightest}: {row:?}"
        );
    }
}

#[test]
fn a_crop_outside_the_frame_is_refused() {
    let pixels = frame(10, 10, |_, _| [0; 3]);
    let plan = Plan {
        crop: Crop {
            x: 5,
            y: 0,
            width: 6,
            height: 10,
        },
        size: (6, 10),
        region: region(5.0, 0.0, 6.0, 10.0),
    };
    assert_eq!(
        pixels.png(&plan).unwrap_err(),
        "capture crop lies outside the frame"
    );
}

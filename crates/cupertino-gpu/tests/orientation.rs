//! Orientation on this backend: clip y = -1 and fragment row 0 are a target's first row, v = 0 is a texture's
//! first row, and pixels are written and read first row first.

use cupertino_gpu::*;
use cupertino_raster_math::dither;

use crate::common;

#[test]
fn a_shape_at_the_top_left_lands_in_the_first_rows_and_columns() {
    let Some(device) = common::device() else {
        return;
    };
    let target = common::target(&device, (16, 8), TextureFormat::Rgba8);
    common::draw_shapes(
        &device,
        &target,
        [0.0; 4],
        &[common::shape(
            [0.0, 0.0, 4.0, 2.0],
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 2.0],
        )],
    );
    let pixels = device
        .read_bytes(&target, Region::new(0, 0, 16, 8))
        .unwrap();
    let red = |x: usize, y: usize| pixels[(y * 16 + x) * 4];
    assert_eq!(red(0, 0), 255);
    assert_eq!(red(3, 1), 255);
    assert_eq!(red(4, 0), 0);
    assert_eq!(red(0, 2), 0);
    assert_eq!(red(15, 7), 0);
    device.destroy_texture(target);
}

#[test]
fn a_texture_s_first_row_is_drawn_on_the_target_s_first_row() {
    let Some(device) = common::device() else {
        return;
    };
    let source = device
        .create_texture(&TextureDesc {
            size: (2, 2),
            format: TextureFormat::Rgba8,
            usage: TextureUsage::SAMPLED,
        })
        .unwrap();
    // first row red then green, second row blue then white
    let texels = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    device
        .write_texture(&source, Region::new(0, 0, 2, 2), &texels)
        .unwrap();
    let target = common::target(&device, (4, 4), TextureFormat::Rgba8);
    common::blit(&device, &source, &target, Filter::Nearest);
    let pixels = device.read_bytes(&target, Region::new(0, 0, 4, 4)).unwrap();
    let at = |x: usize, y: usize| &pixels[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
    assert_eq!(at(0, 0), [255, 0, 0, 255]);
    assert_eq!(at(3, 0), [0, 255, 0, 255]);
    assert_eq!(at(0, 3), [0, 0, 255, 255]);
    assert_eq!(at(3, 3), [255, 255, 255, 255]);
    // a part read back is the same part
    assert_eq!(
        device.read_bytes(&target, Region::new(2, 3, 1, 1)).unwrap(),
        [255, 255, 255, 255]
    );
    device.destroy_texture(target);
    device.destroy_texture(source);
}

#[test]
fn dither_is_keyed_to_the_pixel_it_is_stored_in_counted_from_the_first_row() {
    let Some(device) = common::device() else {
        return;
    };
    let size = 16u32;
    let target = common::target(&device, (size, size), TextureFormat::Rgba8);
    let v = 100.3 / 255.0;
    let (pipeline, layout) = common::pipeline(&device, &shaders::DITHER_FILL, Blend::Replace);
    let fill = common::uniform(&device, &common::bytes(&[v, 0.0, 0.0, 0.0]), 16);
    let group = device
        .create_bind_group(
            &layout,
            &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer {
                    buffer: &fill,
                    offset: 0,
                    size: 16,
                },
            }],
        )
        .unwrap();
    let mut pass = device.begin_pass(&target, LoadOp::Clear([0.0; 4])).unwrap();
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group).unwrap();
    pass.draw(0..3, 1).unwrap();
    pass.end();
    let pixels = device
        .read_bytes(&target, Region::new(0, 0, size, size))
        .unwrap();
    let (mut decisive, mut flipped_differs) = (0, false);
    for y in 0..size {
        for x in 0..size {
            let got = pixels[((y * size + x) * 4) as usize];
            let want = dither::store(v, x, y);
            let exact = v * 255.0 + dither::offset(x, y);
            // a store far from the half step decides the position; one near it may round either way
            if (exact.fract() - 0.5).abs() > 0.1 {
                assert_eq!(got, want, "pixel {x} {y}");
                decisive += 1;
                flipped_differs |= dither::store(v, x, size - 1 - y) != want;
            } else {
                assert!(got.abs_diff(want) <= 1, "pixel {x} {y}: {got} {want}");
            }
        }
    }
    assert!(
        decisive > 100 && flipped_differs,
        "the pattern tells the rows apart"
    );
    device.destroy_buffer(fill);
    device.destroy_pipeline(pipeline);
    device.destroy_texture(target);
}

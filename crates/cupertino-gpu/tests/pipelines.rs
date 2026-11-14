//! Every generated program compiles and links on the device, and the shape program draws the shared coverage
//! through instanced vertex buffers, blended premultiplied, clipped by the scissor.

use cupertino_gpu::*;
use cupertino_raster_math::coverage::{coverage, rounded_distance};

use crate::common;

#[test]
fn every_generated_program_makes_a_pipeline() {
    let Some(device) = common::device() else {
        return;
    };
    for shader in shaders::ALL {
        let layout = common::layout(&device, shader);
        let vertex_buffers = if shader.name == "shape" {
            vec![common::shape_instances()]
        } else {
            vec![]
        };
        let pipeline = device
            .create_pipeline(&PipelineDesc {
                shader,
                layouts: &[&layout],
                vertex_buffers: &vertex_buffers,
                blend: Blend::Replace,
                topology: Topology::TriangleList,
            })
            .unwrap_or_else(|e| panic!("{}: {e}", shader.name));
        device.destroy_pipeline(pipeline);
    }
}

#[test]
fn shapes_draw_the_shared_coverage_of_rectangles_rounded_and_continuous_corners() {
    let Some(device) = common::device() else {
        return;
    };
    let size = (96, 40);
    let target = common::target(&device, size, TextureFormat::Rgba8);
    // a sharp rectangle, a circular corner and a continuous one, white on transparent
    let shapes = [
        ([2.0f32, 3.0, 26.0, 30.5], [0.0f32, 2.0]),
        ([33.0, 2.25, 28.0, 33.0], [9.0, 2.0]),
        ([65.5, 4.0, 29.0, 32.0], [12.0, 4.0]),
    ];
    let instances: Vec<Vec<f32>> = shapes
        .iter()
        .map(|(r, c)| common::shape(*r, [1.0; 4], *c))
        .collect();
    common::draw_shapes(&device, &target, [0.0; 4], &instances);
    let pixels = device
        .read_bytes(&target, Region::new(0, 0, size.0, size.1))
        .unwrap();
    let mut edge_pixels = 0;
    for y in 0..size.1 {
        for x in 0..size.0 {
            let centre = [x as f32 + 0.5, y as f32 + 0.5];
            let want: f32 = shapes
                .iter()
                .map(|(r, c)| {
                    let half = [r[2] * 0.5, r[3] * 0.5];
                    let p = [centre[0] - (r[0] + half[0]), centre[1] - (r[1] + half[1])];
                    let extent = c[0].min(half[0].min(half[1]));
                    coverage(rounded_distance(p, half, extent, c[1]), 1.0)
                })
                .fold(0.0, |acc, a| a + acc * (1.0 - a));
            let got = f32::from(pixels[((y * size.0 + x) * 4 + 3) as usize]) / 255.0;
            assert!(
                (got - want).abs() <= 2.0 / 255.0,
                "pixel {x} {y}: drawn {got}, defined {want}"
            );
            if want > 0.02 && want < 0.98 {
                edge_pixels += 1;
            }
        }
    }
    assert!(
        edge_pixels > 100,
        "the shapes have anti-aliased edges ({edge_pixels})"
    );
    device.destroy_texture(target);
}

#[test]
fn a_premultiplied_colour_blends_over_and_the_scissor_clips() {
    let Some(device) = common::device() else {
        return;
    };
    let target = common::target(&device, (8, 8), TextureFormat::Rgba8);
    // half-transparent red over opaque blue
    common::draw_shapes(
        &device,
        &target,
        [0.0, 0.0, 1.0, 1.0],
        &[common::shape(
            [0.0, 0.0, 8.0, 8.0],
            [0.5, 0.0, 0.0, 0.5],
            [0.0, 2.0],
        )],
    );
    let px = device.read_bytes(&target, Region::new(3, 3, 1, 1)).unwrap();
    assert!(
        (i32::from(px[0]) - 128).abs() <= 1
            && px[1] == 0
            && (i32::from(px[2]) - 128).abs() <= 1
            && px[3] == 255,
        "{px:?}"
    );

    // a clear-free pass whose scissor keeps one column
    let (pipeline, layout) = common::pipeline(&device, &shaders::DITHER_FILL, Blend::Replace);
    let fill = common::uniform(&device, &common::bytes(&[1.0, 0.0, 0.0, 0.0]), 16);
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
    let mut pass = device.begin_pass(&target, LoadOp::Load).unwrap();
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group).unwrap();
    pass.set_scissor(Some(Region::new(5, 0, 1, 8)));
    pass.draw(0..3, 1).unwrap();
    pass.end();
    let row = device.read_bytes(&target, Region::new(0, 2, 8, 1)).unwrap();
    for x in 0..8 {
        let p = &row[x * 4..x * 4 + 4];
        if x == 5 {
            assert_eq!(p, [255, 255, 255, 255], "inside the scissor");
        } else {
            assert!(
                (i32::from(p[2]) - 128).abs() <= 1,
                "outside the scissor, kept: {p:?}"
            );
        }
    }
    device.destroy_buffer(fill);
    device.destroy_pipeline(pipeline);
    device.destroy_texture(target);
}

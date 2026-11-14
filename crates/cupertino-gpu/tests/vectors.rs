//! The generated programs compute the shared definitions: each is run on the device over the committed vectors of
//! `cupertino-raster-math` and compared with them.

use cupertino_gpu::*;
use cupertino_raster_math::blur::plan_blur;
use cupertino_raster_math::{dither, vectors};

use crate::common;

/// The size of the test programs' `Rows` block: 256 four-float rows.
const ROWS_BYTES: u64 = 256 * 16;
/// Float maths on the GPU (`pow`, `exp`, divisions) may differ from the CPU's in the last bits.
const CLOSE: f32 = 2e-3;

fn close(got: f32, want: f64, tolerance: f32, what: &str) {
    let want = want as f32;
    assert!(
        (got - want).abs() <= tolerance * want.abs().max(1.0),
        "{what}: the GPU gives {got}, the definition {want}"
    );
}

#[test]
fn coverage_matches_the_distance_and_coverage_vectors() {
    let Some(device) = common::device() else {
        return;
    };
    let rows = vectors::rows(vectors::COVERAGE);
    // two rows of the block per vector: 128 vectors a draw
    for chunk in rows.chunks(128) {
        let data: Vec<f32> = chunk
            .iter()
            .flat_map(|r| [r[0], r[1], r[2], r[3], r[4], r[5], r[6], 0.0].map(|v| v as f32))
            .collect();
        let out = common::probe(
            &device,
            &shaders::PROBE_COVERAGE,
            &data,
            ROWS_BYTES,
            chunk.len() as u32,
        );
        for (r, px) in chunk.iter().zip(&out) {
            close(px[0], r[7], CLOSE, &format!("distance of {r:?}"));
            close(px[1], r[8], CLOSE, &format!("coverage of {r:?}"));
        }
    }
}

#[test]
fn shadows_match_the_drop_and_inner_shadow_vectors() {
    let Some(device) = common::device() else {
        return;
    };
    let rows = vectors::rows(vectors::SHADOW);
    for chunk in rows.chunks(85) {
        let data: Vec<f32> = chunk
            .iter()
            .flat_map(|r| r[..12].iter().map(|v| *v as f32))
            .collect();
        let out = common::probe(
            &device,
            &shaders::PROBE_SHADOW,
            &data,
            ROWS_BYTES,
            chunk.len() as u32,
        );
        for (r, px) in chunk.iter().zip(&out) {
            close(px[0], r[12], CLOSE, &format!("drop shadow of {r:?}"));
            close(
                px[1],
                r[13],
                CLOSE,
                &format!("cut-out drop shadow of {r:?}"),
            );
            close(px[2], r[14], CLOSE, &format!("inner shadow of {r:?}"));
        }
    }
}

#[test]
fn gradients_match_the_gradient_vectors() {
    let Some(device) = common::device() else {
        return;
    };
    let rows = vectors::rows(vectors::GRADIENT);
    for (g, stops) in vectors::gradient_stops().iter().enumerate() {
        let mine: Vec<&Vec<f64>> = rows.iter().filter(|r| r[0] as usize == g).collect();
        assert!(!mine.is_empty() && mine.len() <= 256);
        let s = stops.as_slice();
        let mut data = vec![0.0f32; 300];
        for (i, stop) in s.iter().enumerate() {
            data[i] = stop.offset;
            data[8 + 4 * i..12 + 4 * i].copy_from_slice(&stop.color);
        }
        data[40] = f32::from_bits(s.len() as u32);
        for (i, r) in mine.iter().enumerate() {
            data[44 + i] = r[1] as f32;
        }
        let out = common::probe(
            &device,
            &shaders::PROBE_GRADIENT,
            &data,
            1200,
            mine.len() as u32,
        );
        for (r, px) in mine.iter().zip(&out) {
            for c in 0..4 {
                close(px[c], r[2 + c], 1e-5, &format!("channel {c} of {r:?}"));
            }
        }
    }
}

#[test]
fn dither_matches_the_matrix_exactly_and_the_stored_values_within_a_step() {
    let Some(device) = common::device() else {
        return;
    };
    let rows = vectors::rows(vectors::DITHER);
    let data: Vec<f32> = rows
        .iter()
        .flat_map(|r| [r[0] as f32, r[1] as f32, r[2] as f32, 0.0])
        .collect();
    let out = common::probe(
        &device,
        &shaders::PROBE_DITHER,
        &data,
        ROWS_BYTES,
        rows.len() as u32,
    );
    for (r, px) in rows.iter().zip(&out) {
        assert_eq!(px[0], r[3] as f32, "the offset of pixel {} {}", r[0], r[1]);
        let stored = f32::from(dither::quantize(px[1]));
        assert!(
            (stored - r[4] as f32).abs() <= 1.0,
            "stored {stored} for {r:?}"
        );
    }
}

/// The blur vectors' input, uploaded as 32-bit floats so every precision starts from the same values.
fn pattern(device: &Device) -> Texture {
    let image = vectors::pattern();
    let texture = device
        .create_texture(&TextureDesc {
            size: (image.width as u32, image.height as u32),
            format: TextureFormat::Rgba32Float,
            usage: TextureUsage::SAMPLED,
        })
        .unwrap();
    let data: Vec<f32> = image.pixels.iter().flatten().copied().collect();
    device
        .write_texture(
            &texture,
            Region::new(0, 0, image.width as u32, image.height as u32),
            &common::bytes(&data),
        )
        .unwrap();
    texture
}

fn pass_into(device: &Device, pipeline: &Pipeline, group: &BindGroup, target: &Texture) {
    let mut pass = device.begin_pass(target, LoadOp::Clear([0.0; 4])).unwrap();
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group).unwrap();
    pass.draw(0..3, 1).unwrap();
    pass.end();
}

#[test]
fn the_blur_pyramid_and_kernel_match_the_blur_vectors_at_both_edges_and_precisions() {
    let Some(device) = common::device() else {
        return;
    };
    let plan = plan_blur(16, 16, 0.5, 6.0, 1.0);
    let rows = vectors::rows(vectors::BLUR);
    let source = pattern(&device);
    let (halve, halve_layout) = common::pipeline(&device, &shaders::BLUR_HALVE, Blend::Replace);
    let (gauss, gauss_layout) = common::pipeline(&device, &shaders::BLUR_GAUSS, Blend::Replace);
    for (edge, precision) in [(0u32, 0u32), (0, 1), (1, 0), (1, 1)] {
        let format = if precision == 0 {
            if !common::has(device.info().half_float_targets, "16-bit float targets") {
                continue;
            }
            TextureFormat::Rgba16Float
        } else {
            TextureFormat::Rgba8
        };
        let mut current: Option<Texture> = None;
        let level = common::uniform(
            &device,
            &[edge.to_le_bytes(), [0; 4], [0; 4], [0; 4]].concat(),
            16,
        );
        for &(w, h) in &plan.levels {
            let out = common::target(&device, (w, h), format);
            let group = device
                .create_bind_group(
                    &halve_layout,
                    &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::Buffer {
                                buffer: &level,
                                offset: 0,
                                size: 16,
                            },
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Texture(current.as_ref().unwrap_or(&source)),
                        },
                    ],
                )
                .unwrap();
            pass_into(&device, &halve, &group, &out);
            if let Some(old) = current.replace(out) {
                device.destroy_texture(old);
            }
        }
        let mut weights = [0.0f32; 36];
        weights[..plan.weights.len()].copy_from_slice(&plan.weights);
        for direction in [(1i32, 0i32), (0, 1)] {
            let mut block = common::bytes(&weights);
            block.extend(direction.0.to_le_bytes());
            block.extend(direction.1.to_le_bytes());
            block.extend((plan.weights.len() as i32 - 1).to_le_bytes());
            block.extend(edge.to_le_bytes());
            let kernel = common::uniform(&device, &block, 160);
            let input = current.take().unwrap();
            let out = common::target(&device, input.size(), format);
            let group = device
                .create_bind_group(
                    &gauss_layout,
                    &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::Buffer {
                                buffer: &kernel,
                                offset: 0,
                                size: 160,
                            },
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Texture(&input),
                        },
                    ],
                )
                .unwrap();
            pass_into(&device, &gauss, &group, &out);
            device.destroy_texture(input);
            device.destroy_buffer(kernel);
            current = Some(out);
        }
        let result = current.take().unwrap();
        let (w, h) = result.size();
        let region = Region::new(0, 0, w, h);
        let pixels: Vec<f32> = if precision == 0 {
            device.read_floats(&result, region).unwrap()
        } else {
            device
                .read_bytes(&result, region)
                .unwrap()
                .iter()
                .map(|b| f32::from(*b) / 255.0)
                .collect()
        };
        // half floats hold about three decimal digits; an 8-bit store may round either way near a half step
        let tolerance = if precision == 0 {
            2e-3
        } else {
            1.0 / 255.0 + 1e-4
        };
        let mine: Vec<&Vec<f64>> = rows
            .iter()
            .filter(|r| r[0] as u32 == edge && r[1] as u32 == precision)
            .collect();
        assert_eq!(mine.len() as u32, w * h);
        for r in mine {
            let (x, y) = (r[2] as u32, r[3] as u32);
            for c in 0..4 {
                let got = pixels[((y * w + x) * 4 + c) as usize];
                let want = r[4 + c as usize] as f32;
                assert!(
                    (got - want).abs() <= tolerance,
                    "edge {edge} precision {precision} pixel {x} {y} channel {c}: GPU {got}, definition {want}"
                );
            }
        }
        device.destroy_texture(result);
        device.destroy_buffer(level);
    }
    device.destroy_texture(source);
}

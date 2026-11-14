//! A device for the tests, and the small pieces every test file draws with.
//!
//! The tests run on the first render node through GBM, or on the surfaceless platform without one or with
//! `LIBGL_ALWAYS_SOFTWARE` set (llvmpipe, as CI runs them). Without any EGL device a test is skipped with a message,
//! unless `CUPERTINO_EGL_REQUIRED=1` (CI) or `CUPERTINO_GPU_REQUIRED=1` (the local GPU tier) makes that a failure;
//! the latter also fails a test whose hardware feature (a GBM device, texture export, native fences) is missing,
//! and a software device.

use std::path::PathBuf;

use cupertino_gpu::shaders::{NamedBinding, ShaderSource};
use cupertino_gpu::*;

pub fn gpu_required() -> bool {
    std::env::var("CUPERTINO_GPU_REQUIRED").as_deref() == Ok("1")
}

fn egl_required() -> bool {
    gpu_required() || std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() == Ok("1")
}

/// The first render node, unless a software device is asked for.
pub fn render_node() -> Option<PathBuf> {
    if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_some() {
        return None;
    }
    let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().next()
}

/// A device for a test, or None (with a message) when the machine has none and none is required.
pub fn device() -> Option<Device> {
    let mut why = String::new();
    if let Some(node) = render_node() {
        match Device::new(&DeviceDesc {
            platform: Platform::RenderNode(node.clone()),
        }) {
            Ok(d) => return Some(checked(d)),
            Err(e) => {
                assert!(
                    !gpu_required(),
                    "CUPERTINO_GPU_REQUIRED=1: no device on {}: {e}",
                    node.display()
                );
                why = format!("{}: {e}; ", node.display());
            }
        }
    }
    match Device::new(&DeviceDesc {
        platform: Platform::Surfaceless,
    }) {
        Ok(d) => Some(checked(d)),
        Err(e) => {
            assert!(
                !egl_required(),
                "no EGL device and CUPERTINO_EGL_REQUIRED=1: {why}{e}"
            );
            eprintln!(
                "SKIPPED: no EGL device ({why}{e}); set CUPERTINO_EGL_REQUIRED=1 to fail instead"
            );
            None
        }
    }
}

fn checked(d: Device) -> Device {
    eprintln!("device: {} ({:?})", d.info().renderer, d.info().allocator);
    if gpu_required() {
        assert!(
            !d.info().software,
            "CUPERTINO_GPU_REQUIRED=1 but the device is {}",
            d.info().renderer
        );
    }
    d
}

/// Whether a feature the test needs is there; without it the test is skipped, or fails under
/// `CUPERTINO_GPU_REQUIRED=1`.
pub fn has(present: bool, what: &str) -> bool {
    if !present {
        assert!(
            !gpu_required(),
            "CUPERTINO_GPU_REQUIRED=1: the device has no {what}"
        );
        eprintln!(
            "SKIPPED: the device has no {what}; set CUPERTINO_GPU_REQUIRED=1 to fail instead"
        );
    }
    present
}

/// The layout of group 0 of `shader`, from its generated bindings.
pub fn layout(device: &Device, shader: &ShaderSource) -> BindGroupLayout {
    let mut entries = Vec::new();
    for b in shader.bindings() {
        match b {
            NamedBinding::Uniform { binding, .. } => entries.push(BindingLayout {
                binding,
                kind: BindingKind::UniformBuffer,
            }),
            NamedBinding::Texture {
                binding, sampler, ..
            } => {
                entries.push(BindingLayout {
                    binding,
                    kind: BindingKind::Texture,
                });
                if let Some((_, s)) = sampler {
                    entries.push(BindingLayout {
                        binding: s,
                        kind: BindingKind::Sampler,
                    });
                }
            }
        }
    }
    entries.sort_by_key(|e| e.binding);
    entries.dedup();
    device.create_bind_group_layout(&entries).unwrap()
}

/// A pipeline of `shader` with its own group 0 layout and no vertex buffers.
pub fn pipeline(
    device: &Device,
    shader: &ShaderSource,
    blend: Blend,
) -> (Pipeline, BindGroupLayout) {
    let layout = layout(device, shader);
    let pipeline = device
        .create_pipeline(&PipelineDesc {
            shader,
            layouts: &[&layout],
            vertex_buffers: &[],
            blend,
            topology: Topology::TriangleList,
        })
        .unwrap();
    (pipeline, layout)
}

/// The shape program's instance layout: rectangle, colour, corner.
pub fn shape_instances() -> VertexLayout {
    VertexLayout {
        stride: 40,
        step: StepMode::Instance,
        attributes: vec![
            VertexAttribute {
                location: 0,
                format: VertexFormat::Float32x4,
                offset: 0,
            },
            VertexAttribute {
                location: 1,
                format: VertexFormat::Float32x4,
                offset: 16,
            },
            VertexAttribute {
                location: 2,
                format: VertexFormat::Float32x2,
                offset: 32,
            },
        ],
    }
}

/// One shape instance: rectangle (x, y, w, h), premultiplied colour, corner (extent, exponent).
pub fn shape(rect: [f32; 4], color: [f32; 4], corner: [f32; 2]) -> Vec<f32> {
    let mut v = rect.to_vec();
    v.extend(color);
    v.extend(corner);
    v
}

pub fn bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A uniform buffer holding `data`, padded to `size` bytes.
pub fn uniform(device: &Device, data: &[u8], size: u64) -> Buffer {
    let buffer = device
        .create_buffer(&BufferDesc {
            size,
            usage: BufferUsage::Uniform,
        })
        .unwrap();
    let mut padded = data.to_vec();
    padded.resize(size as usize, 0);
    device.write_buffer(&buffer, 0, &padded).unwrap();
    buffer
}

pub fn target(device: &Device, size: (u32, u32), format: TextureFormat) -> Texture {
    device
        .create_texture(&TextureDesc {
            size,
            format,
            usage: TextureUsage::TARGET,
        })
        .unwrap()
}

/// Draw `shapes` with the shape program into `target` (cleared to `clear` first).
pub fn draw_shapes(device: &Device, target: &Texture, clear: [f32; 4], shapes: &[Vec<f32>]) {
    let layout = layout(device, &shaders::SHAPE);
    let pipeline = device
        .create_pipeline(&PipelineDesc {
            shader: &shaders::SHAPE,
            layouts: &[&layout],
            vertex_buffers: &[shape_instances()],
            blend: Blend::PremultipliedOver,
            topology: Topology::TriangleStrip,
        })
        .unwrap();
    let (w, h) = target.size();
    let viewport = uniform(device, &bytes(&[w as f32, h as f32, 0.0, 0.0]), 16);
    let data: Vec<f32> = shapes.iter().flatten().copied().collect();
    let instances = device
        .create_buffer(&BufferDesc {
            size: (data.len() * 4) as u64,
            usage: BufferUsage::Vertex,
        })
        .unwrap();
    device.write_buffer(&instances, 0, &bytes(&data)).unwrap();
    let group = device
        .create_bind_group(
            &layout,
            &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer {
                    buffer: &viewport,
                    offset: 0,
                    size: 16,
                },
            }],
        )
        .unwrap();
    let mut pass = device.begin_pass(target, LoadOp::Clear(clear)).unwrap();
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group).unwrap();
    pass.set_vertex_buffer(0, &instances, 0).unwrap();
    pass.draw(0..4, shapes.len() as u32).unwrap();
    pass.end();
    device.destroy_buffer(instances);
    device.destroy_buffer(viewport);
    device.destroy_pipeline(pipeline);
}

/// Draw `source` over the whole of `target` with the blit program.
pub fn blit(device: &Device, source: &Texture, target: &Texture, filter: Filter) {
    let (pipeline, layout) = pipeline(device, &shaders::BLIT, Blend::Replace);
    let sampler = device
        .create_sampler(&SamplerDesc {
            filter,
            address: AddressMode::ClampToEdge,
        })
        .unwrap();
    let group = device
        .create_bind_group(
            &layout,
            &[
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Texture(source),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&sampler),
                },
            ],
        )
        .unwrap();
    let mut pass = device.begin_pass(target, LoadOp::Clear([0.0; 4])).unwrap();
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group).unwrap();
    pass.draw(0..3, 1).unwrap();
    pass.end();
    device.destroy_sampler(sampler);
    device.destroy_pipeline(pipeline);
}

/// Run a one-row test program over `width` pixels with its uniform block `data` (of `size` bytes): the pixels.
pub fn probe(
    device: &Device,
    shader: &ShaderSource,
    data: &[f32],
    size: u64,
    width: u32,
) -> Vec<[f32; 4]> {
    let (pipeline, layout) = pipeline(device, shader, Blend::Replace);
    let buffer = uniform(device, &bytes(data), size);
    let group = device
        .create_bind_group(
            &layout,
            &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer {
                    buffer: &buffer,
                    offset: 0,
                    size,
                },
            }],
        )
        .unwrap();
    let out = target(device, (width, 1), TextureFormat::Rgba32Float);
    let mut pass = device.begin_pass(&out, LoadOp::Clear([0.0; 4])).unwrap();
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group).unwrap();
    pass.draw(0..3, 1).unwrap();
    pass.end();
    let floats = device
        .read_floats(&out, Region::new(0, 0, width, 1))
        .unwrap();
    device.destroy_texture(out);
    device.destroy_buffer(buffer);
    device.destroy_pipeline(pipeline);
    floats.as_chunks::<4>().0.to_vec()
}

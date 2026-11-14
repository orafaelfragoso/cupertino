//! The borrowed-context contract: a device borrowed from another context's owner draws between `enter` and
//! `leave`, and leaves the owner's state as it found it.

use super::*;

/// A context to be the owner, as the crate's integration tests choose it.
fn owner() -> Option<Device> {
    let required = |v: &str| std::env::var(v).as_deref() == Ok("1");
    let node = std::fs::read_dir("/dev/dri").ok().and_then(|d| {
        let mut nodes: Vec<_> = d
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("renderD"))
            })
            .collect();
        nodes.sort();
        nodes.into_iter().next()
    });
    let node = node.filter(|_| std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_none());
    if let Some(node) = node {
        match Device::new(&DeviceDesc {
            platform: Platform::RenderNode(node),
        }) {
            Ok(d) => return Some(d),
            Err(e) => assert!(!required("CUPERTINO_GPU_REQUIRED"), "{e}"),
        }
    }
    match Device::new(&DeviceDesc {
        platform: Platform::Surfaceless,
    }) {
        Ok(d) => Some(d),
        Err(e) => {
            assert!(
                !required("CUPERTINO_EGL_REQUIRED") && !required("CUPERTINO_GPU_REQUIRED"),
                "no EGL device: {e}"
            );
            eprintln!("SKIPPED: no EGL device ({e}); set CUPERTINO_EGL_REQUIRED=1 to fail instead");
            None
        }
    }
}

/// Owner state unlike anything a pass sets.
fn unusual_state(
    gl: &glow::Context,
) -> (
    glow::Texture,
    glow::Buffer,
    glow::Program,
    glow::Framebuffer,
) {
    // Safety: the owner's context is current; its own objects.
    unsafe {
        let texture = gl.create_texture().unwrap();
        gl.active_texture(glow::TEXTURE3);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let buffer = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::UNIFORM_BUFFER, Some(buffer));
        gl.buffer_data_size(glow::UNIFORM_BUFFER, 512, glow::DYNAMIC_DRAW);
        gl.bind_buffer_range(glow::UNIFORM_BUFFER, 0, Some(buffer), 256, 64);
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
        let program = gl.create_program().unwrap();
        let vs = gl.create_shader(glow::VERTEX_SHADER).unwrap();
        gl.shader_source(
            vs,
            "#version 300 es\nvoid main() { gl_Position = vec4(0.0); }\n",
        );
        gl.compile_shader(vs);
        let fs = gl.create_shader(glow::FRAGMENT_SHADER).unwrap();
        gl.shader_source(
            fs,
            "#version 300 es\nprecision mediump float;\nout vec4 c;\nvoid main() { c = vec4(1.0); }\n",
        );
        gl.compile_shader(fs);
        gl.attach_shader(program, vs);
        gl.attach_shader(program, fs);
        gl.link_program(program);
        assert!(gl.get_program_link_status(program));
        gl.use_program(Some(program));
        let fb_texture = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(fb_texture));
        gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, 8, 8);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let fb = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(fb_texture),
            0,
        );
        gl.viewport(1, 2, 3, 4);
        gl.scissor(2, 3, 4, 5);
        gl.enable(glow::SCISSOR_TEST);
        gl.enable(glow::BLEND);
        gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE, glow::ZERO, glow::DST_ALPHA);
        gl.blend_equation_separate(glow::FUNC_SUBTRACT, glow::FUNC_ADD);
        gl.color_mask(true, false, true, false);
        gl.clear_color(0.25, 0.5, 0.75, 1.0);
        gl.enable(glow::CULL_FACE);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 2);
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 8);
        gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 17);
        (texture, buffer, program, fb)
    }
}

#[test]
fn a_borrowed_device_draws_between_enter_and_leave_and_leaves_the_owner_s_state_as_it_found_it() {
    let Some(owner) = owner() else { return };
    unusual_state(&owner.gl);
    let before = owner.capture_state();
    // Safety: the owner's context is current and outlives the borrowed device.
    let device =
        unsafe { Device::borrow(owner.info.robustness, owner.info.purge_notification) }.unwrap();
    assert!(device.is_borrowed());
    // outside enter and leave it refuses work
    assert!(matches!(
        device.create_buffer(&BufferDesc {
            size: 16,
            usage: BufferUsage::Uniform
        }),
        Err(Error::Invalid(_))
    ));
    let saved = device.enter().unwrap();
    assert_eq!(saved, before);
    // a pass that touches every kind of state: a uniform block, a texture with a sampler, instances, blending,
    // a scissor, an upload and a read-back
    let layout = device
        .create_bind_group_layout(&[
            BindingLayout {
                binding: 0,
                kind: BindingKind::UniformBuffer,
            },
            BindingLayout {
                binding: 1,
                kind: BindingKind::Texture,
            },
            BindingLayout {
                binding: 2,
                kind: BindingKind::Sampler,
            },
        ])
        .unwrap();
    let target = device
        .create_texture(&TextureDesc {
            size: (16, 16),
            format: TextureFormat::Rgba8,
            usage: TextureUsage::TARGET,
        })
        .unwrap();
    let source = device
        .create_texture(&TextureDesc {
            size: (1, 1),
            format: TextureFormat::Rgba8,
            usage: TextureUsage::SAMPLED,
        })
        .unwrap();
    device
        .write_texture(&source, Region::new(0, 0, 1, 1), &[0, 0, 255, 255])
        .unwrap();
    let sampler = device
        .create_sampler(&SamplerDesc {
            filter: Filter::Linear,
            address: AddressMode::ClampToEdge,
        })
        .unwrap();
    let viewport = device
        .create_buffer(&BufferDesc {
            size: 16,
            usage: BufferUsage::Uniform,
        })
        .unwrap();
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    device
        .write_buffer(&viewport, 0, &floats(&[16.0, 16.0, 0.0, 0.0]))
        .unwrap();
    let instances = device
        .create_buffer(&BufferDesc {
            size: 40,
            usage: BufferUsage::Vertex,
        })
        .unwrap();
    device
        .write_buffer(
            &instances,
            0,
            &floats(&[0.0, 0.0, 8.0, 16.0, 1.0, 0.0, 0.0, 1.0, 0.0, 2.0]),
        )
        .unwrap();
    let blit = device
        .create_pipeline(&PipelineDesc {
            shader: &crate::shaders::BLIT,
            layouts: &[&layout],
            vertex_buffers: &[],
            blend: Blend::Replace,
            topology: Topology::TriangleList,
        })
        .unwrap();
    let shape = device
        .create_pipeline(&PipelineDesc {
            shader: &crate::shaders::SHAPE,
            layouts: &[&layout],
            vertex_buffers: &[VertexLayout {
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
            }],
            blend: Blend::PremultipliedOver,
            topology: Topology::TriangleStrip,
        })
        .unwrap();
    let group = device
        .create_bind_group(
            &layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer {
                        buffer: &viewport,
                        offset: 0,
                        size: 16,
                    },
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Texture(&source),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&sampler),
                },
            ],
        )
        .unwrap();
    {
        let mut pass = device.begin_pass(&target, LoadOp::Clear([0.0; 4])).unwrap();
        pass.set_pipeline(&blit);
        pass.set_bind_group(0, &group).unwrap();
        pass.draw(0..3, 1).unwrap();
        pass.set_pipeline(&shape);
        pass.set_bind_group(0, &group).unwrap();
        pass.set_vertex_buffer(0, &instances, 0).unwrap();
        pass.set_scissor(Some(Region::new(0, 0, 16, 8)));
        pass.draw(0..4, 1).unwrap();
        pass.end();
    }
    let pixels = device
        .read_bytes(&target, Region::new(0, 0, 16, 16))
        .unwrap();
    let at = |x: usize, y: usize| &pixels[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
    assert_eq!(at(2, 2), [255, 0, 0, 255], "the shape, inside the scissor");
    assert_eq!(at(2, 12), [0, 0, 255, 255], "the blit, outside the scissor");
    assert_eq!(at(12, 2), [0, 0, 255, 255], "the blit, beside the shape");
    let fence = device.submit().unwrap();
    assert!(device
        .wait(&fence, std::time::Duration::from_secs(5))
        .unwrap());
    device.destroy_fence(fence);
    device.leave(saved);
    assert_eq!(owner.capture_state(), before, "the owner's state is back");
    // the resources go between enter and leave too
    let saved = device.enter().unwrap();
    for p in [blit, shape] {
        device.destroy_pipeline(p);
    }
    device.destroy_buffer(viewport);
    device.destroy_buffer(instances);
    device.destroy_sampler(sampler);
    device.destroy_texture(source);
    device.destroy_texture(target);
    device.leave(saved);
    assert_eq!(owner.capture_state(), before);
    // dropping it touches nothing of the owner's either
    drop(device);
    assert_eq!(owner.capture_state(), before);
}

#[test]
fn a_borrowed_device_s_pass_draws_into_the_framebuffer_its_owner_bound_at_the_owner_s_viewport() {
    let Some(owner) = owner() else { return };
    let size = 16u32;
    let target = owner
        .create_texture(&TextureDesc {
            size: (size, size),
            format: TextureFormat::Rgba8,
            usage: TextureUsage::TARGET,
        })
        .unwrap();
    owner
        .write_texture(
            &target,
            Region::new(0, 0, size, size),
            &[0, 0, 255, 255].repeat((size * size) as usize),
        )
        .unwrap();
    let (texture, buffer, program, fb) = unusual_state(&owner.gl);
    // the owner's own framebuffer and viewport, with the scissor test on and blending as it likes it
    // Safety: the owner's context is current; its own framebuffer.
    unsafe {
        owner
            .gl
            .bind_framebuffer(glow::FRAMEBUFFER, target.framebuffer);
        owner.gl.viewport(4, 2, 8, 10);
    }
    let before = owner.capture_state();
    // Safety: the owner's context is current and outlives the borrowed device.
    let device =
        unsafe { Device::borrow(owner.info.robustness, owner.info.purge_notification) }.unwrap();
    assert!(matches!(owner.begin_owner_pass(), Err(Error::Invalid(_))));
    let saved = device.enter().unwrap();
    let layout = device
        .create_bind_group_layout(&[BindingLayout {
            binding: 0,
            kind: BindingKind::UniformBuffer,
        }])
        .unwrap();
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let viewport = device
        .create_buffer(&BufferDesc {
            size: 16,
            usage: BufferUsage::Uniform,
        })
        .unwrap();
    device
        .write_buffer(&viewport, 0, &floats(&[8.0, 10.0, 0.0, 0.0]))
        .unwrap();
    // a red square over the viewport's first four columns and rows, and a green one over all of it
    let instances = device
        .create_buffer(&BufferDesc {
            size: 80,
            usage: BufferUsage::Vertex,
        })
        .unwrap();
    device
        .write_buffer(
            &instances,
            0,
            &floats(&[
                0.0, 0.0, 4.0, 4.0, 1.0, 0.0, 0.0, 1.0, 0.0, 2.0, //
                0.0, 0.0, 8.0, 10.0, 0.0, 1.0, 0.0, 1.0, 0.0, 2.0,
            ]),
        )
        .unwrap();
    let shape = device
        .create_pipeline(&PipelineDesc {
            shader: &crate::shaders::SHAPE,
            layouts: &[&layout],
            vertex_buffers: &[VertexLayout {
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
            }],
            blend: Blend::Replace,
            topology: Topology::TriangleStrip,
        })
        .unwrap();
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
    {
        let mut pass = device.begin_owner_pass().unwrap();
        assert_eq!(pass.size(), (8, 10), "the owner's viewport");
        pass.set_pipeline(&shape);
        pass.set_bind_group(0, &group).unwrap();
        pass.set_vertex_buffer(0, &instances, 0).unwrap();
        pass.draw(0..4, 1).unwrap();
        // the second square only inside a scissor of the framebuffer's pixels: its rows 9 to 11
        pass.set_vertex_buffer(0, &instances, 40).unwrap();
        pass.set_scissor(Some(Region::new(0, 9, size, 3)));
        pass.draw(0..4, 1).unwrap();
        pass.end();
    }
    device.leave(saved);
    assert_eq!(owner.capture_state(), before, "the owner's state is back");
    let saved = device.enter().unwrap();
    device.destroy_pipeline(shape);
    device.destroy_buffer(viewport);
    device.destroy_buffer(instances);
    device.leave(saved);
    drop(device);
    assert_eq!(owner.capture_state(), before);
    // Safety: the owner's context; its objects, gone before the target is read.
    unsafe {
        owner.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        owner.gl.disable(glow::SCISSOR_TEST);
        owner.gl.delete_framebuffer(fb);
        owner.gl.delete_program(program);
        owner.gl.delete_buffer(buffer);
        owner.gl.delete_texture(texture);
    }
    let pixels = owner
        .read_bytes(&target, Region::new(0, 0, size, size))
        .unwrap();
    let at = |x: u32, y: u32| {
        let i = ((y * size + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    let (red, green, blue) = ([255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]);
    // the viewport starts at the framebuffer's pixel (4, 2)
    assert_eq!(at(5, 3), red, "the first square, at the viewport's origin");
    assert_eq!(at(9, 4), blue, "the framebuffer kept beside it");
    assert_eq!(at(3, 3), blue, "nothing left of the viewport");
    assert_eq!(at(6, 10), green, "the second square inside the scissor");
    assert_eq!(at(6, 8), blue, "and nothing of it outside");
    assert_eq!(at(6, 12), blue, "nothing below the viewport");
    owner.destroy_texture(target);
}

#[test]
fn driver_reset_statuses_are_read_as_their_losses() {
    for loss in [
        Loss::Reset { guilty: Some(true) },
        Loss::Reset {
            guilty: Some(false),
        },
        Loss::Reset { guilty: None },
        Loss::Purge,
    ] {
        assert_eq!(loss_of(status_of(loss)), Some(loss));
    }
    assert_eq!(loss_of(0), None);
    assert_eq!(loss_of(GL_PURGED_CONTEXT_RESET_NV), Some(Loss::Purge));
    assert_eq!(loss_of(0x1234), Some(Loss::Reset { guilty: None }));
}

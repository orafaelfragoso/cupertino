use super::*;

impl Device {
    pub fn create_buffer(&self, desc: &BufferDesc) -> Result<Buffer> {
        self.ready()?;
        let target = buffer_target(desc.usage);
        self.touch(Touched::BUFFERS);
        // Safety: a current context; the buffer is bound and sized.
        unsafe {
            let raw = self.gl.create_buffer().map_err(Error::Driver)?;
            self.gl.bind_buffer(target, Some(raw));
            self.gl
                .buffer_data_size(target, desc.size as i32, glow::DYNAMIC_DRAW);
            self.gl.bind_buffer(target, None);
            Ok(Buffer {
                raw,
                size: desc.size,
                usage: desc.usage,
            })
        }
    }

    pub fn write_buffer(&self, buffer: &Buffer, offset: u64, data: &[u8]) -> Result<()> {
        self.ready()?;
        if offset + data.len() as u64 > buffer.size {
            return Err(Error::Invalid(format!(
                "{} bytes at {offset} into a buffer of {}",
                data.len(),
                buffer.size
            )));
        }
        let target = buffer_target(buffer.usage);
        self.touch(Touched::BUFFERS);
        // Safety: a buffer of this device, written inside its size.
        unsafe {
            self.gl.bind_buffer(target, Some(buffer.raw));
            self.gl
                .buffer_sub_data_u8_slice(target, offset as i32, data);
            self.gl.bind_buffer(target, None);
        }
        Ok(())
    }

    pub fn destroy_buffer(&self, buffer: Buffer) {
        if self.ready().is_ok() {
            // Safety: a buffer of this device, destroyed once.
            unsafe { self.gl.delete_buffer(buffer.raw) };
        }
    }

    // ---- textures ----

    pub fn create_texture(&self, desc: &TextureDesc) -> Result<Texture> {
        self.ready()?;
        let (w, h) = desc.size;
        if w == 0 || h == 0 || w > self.info.max_texture_size || h > self.info.max_texture_size {
            return Err(Error::Invalid(format!("a texture of {w} x {h}")));
        }
        if desc.usage.render_target {
            let renders = match desc.format {
                TextureFormat::Rgba16Float => self.info.half_float_targets,
                TextureFormat::Rgba32Float => self.info.float_targets,
                _ => true,
            };
            if !renders {
                return Err(Error::Unsupported(format!(
                    "rendering to {:?}",
                    desc.format
                )));
            }
        }
        self.touch(Touched::TEXTURES.with(Touched::FRAMEBUFFERS));
        // Safety: a current context; the texture is made, bound and given storage.
        unsafe {
            let raw = self.gl.create_texture().map_err(Error::Driver)?;
            self.gl.bind_texture(glow::TEXTURE_2D, Some(raw));
            self.gl.tex_storage_2d(
                glow::TEXTURE_2D,
                1,
                internal_format(desc.format),
                w as i32,
                h as i32,
            );
            self.texture_defaults();
            let mut texture = Texture {
                raw,
                desc: *desc,
                framebuffer: None,
                image: None,
                bo: None,
                id: next_id(),
                foreign: false,
                counted: 0,
            };
            if desc.usage.render_target {
                match self.framebuffer_of(raw) {
                    Ok(fb) => texture.framebuffer = Some(fb),
                    Err(e) => {
                        self.gl.delete_texture(raw);
                        return Err(e);
                    }
                }
            }
            self.count_made(&mut texture);
            Ok(texture)
        }
    }

    /// A render target over `name`, a 2D texture of `size` texels in 8-bit RGBA the device's owner made in the
    /// context it lent: drawn into, sampled and read like the device's own textures (its filtering set to theirs,
    /// one level, nearest). Destroying the result
    /// ([`Device::destroy_texture`]) releases only the framebuffer the device made for it; the texture stays the
    /// owner's. Only a borrowed device wraps.
    ///
    /// # Safety
    ///
    /// `name` is a texture of the current context with storage of `size` in a colour-renderable 8-bit RGBA format,
    /// and it outlives the result.
    pub unsafe fn wrap_texture(&self, name: u32, size: (u32, u32)) -> Result<Texture> {
        self.ready()?;
        if !self.is_borrowed() {
            return Err(Error::Invalid(
                "only a borrowed device wraps its owner's textures".into(),
            ));
        }
        let raw = std::num::NonZeroU32::new(name)
            .map(glow::NativeTexture)
            .ok_or_else(|| Error::Invalid("texture name 0".into()))?;
        let (w, h) = size;
        if w == 0 || h == 0 {
            return Err(Error::Invalid(format!("a texture of {w} x {h}")));
        }
        self.touch(Touched::FRAMEBUFFERS.with(Touched::TEXTURES));
        // Safety: the caller's texture of the current context; it reads its one level, as the device's own do (its
        // owner sets its own filtering when it samples it)
        let framebuffer = unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(raw));
            self.texture_defaults();
            self.framebuffer_of(raw)
        }?;
        let mut texture = Texture {
            raw,
            desc: TextureDesc {
                size,
                format: TextureFormat::Rgba8,
                usage: TextureUsage {
                    sampled: true,
                    render_target: true,
                },
            },
            framebuffer: Some(framebuffer),
            image: None,
            bo: None,
            id: next_id(),
            foreign: true,
            counted: 0,
        };
        self.count_made(&mut texture);
        Ok(texture)
    }

    /// Without a sampler bound, a texture reads its single level, nearest, clamped: complete without mipmaps.
    unsafe fn texture_defaults(&self) {
        let gl = &self.gl;
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, 0);
    }

    unsafe fn framebuffer_of(&self, texture: glow::Texture) -> Result<glow::Framebuffer> {
        let gl = &self.gl;
        let fb = gl.create_framebuffer().map_err(Error::Driver)?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fb);
            return Err(Error::Unsupported(format!(
                "the texture cannot be rendered to ({status:#x})"
            )));
        }
        Ok(fb)
    }

    /// Write `data` (rows first to last, tightly packed, in the texture's format; 16-bit floats for
    /// [`TextureFormat::Rgba16Float`]) into `region`.
    pub fn write_texture(&self, texture: &Texture, region: Region, data: &[u8]) -> Result<()> {
        self.ready()?;
        let bpp = texture.desc.format.bytes_per_pixel() as usize;
        let need = region.width as usize * region.height as usize * bpp;
        let (w, h) = texture.desc.size;
        if data.len() != need || region.x + region.width > w || region.y + region.height > h {
            return Err(Error::Invalid(format!(
                "{} bytes for {region:?} of a {w} x {h} texture",
                data.len()
            )));
        }
        if texture.is_external() {
            return Err(Error::Invalid(
                "dmabuf textures are drawn into, not written".into(),
            ));
        }
        let (format, ty) = upload_format(texture.desc.format);
        self.touch(Touched::TEXTURES.with(Touched::PIXELS));
        // Safety: a texture of this device; the data is as large as the region.
        unsafe {
            self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture.raw));
            self.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                region.x as i32,
                region.y as i32,
                region.width as i32,
                region.height as i32,
                format,
                ty,
                glow::PixelUnpackData::Slice(Some(data)),
            );
        }
        Ok(())
    }

    pub(super) fn read(
        &self,
        texture: &Texture,
        region: Region,
        format: u32,
        ty: u32,
        out: &mut [u8],
    ) -> Result<()> {
        self.ready()?;
        let fb = texture
            .framebuffer
            .ok_or_else(|| Error::Invalid("only render targets are read back".into()))?;
        let (w, h) = texture.desc.size;
        if region.x + region.width > w || region.y + region.height > h {
            return Err(Error::Invalid(format!("{region:?} of a {w} x {h} texture")));
        }
        self.touch(Touched::FRAMEBUFFERS.with(Touched::PIXELS));
        // Safety: a complete framebuffer of this device; `out` holds the region in the format asked for.
        unsafe {
            self.gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fb));
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            self.gl.read_pixels(
                region.x as i32,
                region.y as i32,
                region.width as i32,
                region.height as i32,
                format,
                ty,
                glow::PixelPackData::Slice(Some(out)),
            );
            self.gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            match self.gl.get_error() {
                glow::NO_ERROR => Ok(()),
                e => Err(Error::Driver(format!("reading back: {e:#x}"))),
            }
        }
    }

    /// The pixels of `region` of an 8-bit render target, rows first to last, in its format's channels.
    pub fn read_bytes(&self, texture: &Texture, region: Region) -> Result<Vec<u8>> {
        let n = region.width as usize * region.height as usize;
        let mut rgba = vec![0u8; n * 4];
        match texture.desc.format {
            TextureFormat::Rgba8 => {
                self.read(texture, region, glow::RGBA, glow::UNSIGNED_BYTE, &mut rgba)?;
                Ok(rgba)
            }
            TextureFormat::R8 => {
                self.read(texture, region, glow::RGBA, glow::UNSIGNED_BYTE, &mut rgba)?;
                Ok(rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect())
            }
            f => Err(Error::Invalid(format!("{f:?} is read with read_floats"))),
        }
    }

    /// The pixels of `region` of a float render target, rows first to last, four `f32` a pixel.
    pub fn read_floats(&self, texture: &Texture, region: Region) -> Result<Vec<f32>> {
        match texture.desc.format {
            TextureFormat::Rgba16Float | TextureFormat::Rgba32Float => {
                let n = region.width as usize * region.height as usize * 4;
                let mut bytes = vec![0u8; n * 4];
                self.read(texture, region, glow::RGBA, glow::FLOAT, &mut bytes)?;
                Ok(bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_ne_bytes(*b))
                    .collect())
            }
            f => Err(Error::Invalid(format!("{f:?} is read with read_bytes"))),
        }
    }

    pub fn destroy_texture(&self, texture: Texture) {
        let t = self.live.get();
        self.live.set(TextureTally {
            textures: t.textures.saturating_sub(1),
            bytes: t.bytes.saturating_sub(texture.counted),
        });
        // the GL names go with a lost context; the image and the buffer object are the display's and GBM's, which a
        // loss leaves, so they are freed whatever the context's state
        if self.ready().is_ok() {
            // Safety: a texture of this device, its context current (`ready`), destroyed once.
            unsafe {
                if let Some(fb) = texture.framebuffer {
                    self.gl.delete_framebuffer(fb);
                }
                if !texture.foreign {
                    self.gl.delete_texture(texture.raw);
                }
            }
        }
        // Safety: the image and buffer object this texture owns, made by this device's display and GBM device,
        // destroyed once; neither call needs a current context.
        unsafe {
            if let Some(image) = texture.image {
                self.ctx.destroy_image(image);
            }
            if let Some(bo) = texture.bo {
                self.ctx.gbm_free(bo);
            }
        }
    }

    /// The EGL images and GBM buffer objects the device made and has not destroyed: those of its dmabufs and
    /// imports, freed with their textures, a lost device's too.
    pub fn live_external(&self) -> ExternalTally {
        self.ctx.live()
    }

    /// Whether the device's own context is current on this thread (always false for a borrowed device, whose context
    /// is its owner's).
    pub fn is_current(&self) -> bool {
        !self.borrowed && self.ctx.is_current()
    }

    // ---- external memory ----

    /// The modifiers of `fourcc` the device renders to.
    pub fn renderable_modifiers(&self, fourcc: u32) -> Vec<u64> {
        self.ctx.renderable_modifiers(fourcc)
    }

    /// A render target whose memory is a dmabuf: a GBM buffer object with one of the modifiers offered, or,
    /// without GBM, a texture exported in a layout the offer must hold.
    pub fn allocate_dmabuf(&self, desc: &DmabufDesc) -> Result<ExternalImage> {
        self.ready()?;
        match self.info.allocator {
            Some(Allocator::Gbm) => {
                let (planes, modifier, bo) =
                    self.ctx
                        .gbm_allocate(desc.size, desc.fourcc, &desc.modifiers)?;
                let raw: Vec<(i32, u32, u32)> = planes
                    .iter()
                    .map(|p| (p.fd.as_raw_fd(), p.offset, p.stride))
                    .collect();
                let dmabuf = Dmabuf {
                    size: desc.size,
                    fourcc: desc.fourcc,
                    modifier,
                    planes: planes
                        .into_iter()
                        .map(|p| DmabufPlane {
                            fd: p.fd,
                            offset: p.offset,
                            stride: p.stride,
                        })
                        .collect(),
                };
                let texture = match self.ctx.import(desc.size, desc.fourcc, modifier, &raw) {
                    Ok(image) => self.texture_of_image(image, desc.size, Some(bo)),
                    Err(e) => {
                        // Safety: the object made above, not yet handed anywhere.
                        unsafe { self.ctx.gbm_free(bo) };
                        Err(e)
                    }
                }?;
                Ok(ExternalImage { texture, dmabuf })
            }
            Some(Allocator::Export) => {
                let texture = self.create_texture(&TextureDesc {
                    size: desc.size,
                    format: TextureFormat::Rgba8,
                    usage: TextureUsage::TARGET,
                })?;
                let (image, plane, fourcc, modifier) = match self.ctx.export(raw_name(texture.raw))
                {
                    Ok(x) => x,
                    Err(e) => {
                        self.destroy_texture(texture);
                        return Err(e);
                    }
                };
                if !desc.modifiers.contains(&modifier) {
                    // Safety: the image made by the export above.
                    unsafe { self.ctx.destroy_image(image) };
                    self.destroy_texture(texture);
                    return Err(Error::Unsupported(format!(
                        "the exported layout {modifier:#x} is not offered"
                    )));
                }
                let mut texture = texture;
                texture.image = Some(image);
                Ok(ExternalImage {
                    texture,
                    dmabuf: Dmabuf {
                        size: desc.size,
                        fourcc,
                        modifier,
                        planes: vec![DmabufPlane {
                            fd: plane.fd,
                            offset: plane.offset,
                            stride: plane.stride,
                        }],
                    },
                })
            }
            None => Err(Error::Unsupported("the device allocates no dmabufs".into())),
        }
    }

    /// A texture (sampled and drawn into) whose memory is the dmabuf `dmabuf`, made by another device or process.
    pub fn import_dmabuf(&self, dmabuf: &Dmabuf) -> Result<Texture> {
        self.ready()?;
        if !self.ctx.can_import() {
            return Err(Error::Unsupported("no dmabuf import".into()));
        }
        let raw: Vec<(i32, u32, u32)> = dmabuf
            .planes
            .iter()
            .map(|p| (p.fd.as_raw_fd(), p.offset, p.stride))
            .collect();
        let image = self
            .ctx
            .import(dmabuf.size, dmabuf.fourcc, dmabuf.modifier, &raw)?;
        self.texture_of_image(image, dmabuf.size, None)
    }

    pub(super) fn texture_of_image(
        &self,
        image: egl::EGLImage,
        size: (u32, u32),
        bo: Option<*mut c_void>,
    ) -> Result<Texture> {
        let target = self
            .image_target_texture
            .ok_or_else(|| Error::Unsupported("no glEGLImageTargetTexture2DOES".into()))?;
        self.touch(Touched::TEXTURES.with(Touched::FRAMEBUFFERS));
        // Safety: a current context; the image is valid and becomes the texture's storage.
        unsafe {
            let raw = self.gl.create_texture().map_err(Error::Driver)?;
            self.gl.bind_texture(glow::TEXTURE_2D, Some(raw));
            target(glow::TEXTURE_2D, image);
            self.texture_defaults();
            let mut texture = Texture {
                raw,
                desc: TextureDesc {
                    size,
                    format: TextureFormat::Rgba8,
                    usage: TextureUsage::TARGET,
                },
                framebuffer: None,
                image: Some(image),
                bo,
                id: next_id(),
                foreign: false,
                counted: 0,
            };
            match self.framebuffer_of(raw) {
                Ok(fb) => texture.framebuffer = Some(fb),
                // an import-only layout is still sampled
                Err(_) => texture.desc.usage = TextureUsage::SAMPLED,
            }
            self.count_made(&mut texture);
            Ok(texture)
        }
    }

    // ---- samplers ----

    pub fn create_sampler(&self, desc: &SamplerDesc) -> Result<Sampler> {
        self.ready()?;
        let filter = match desc.filter {
            Filter::Nearest => glow::NEAREST,
            Filter::Linear => glow::LINEAR,
        } as i32;
        let wrap = match desc.address {
            AddressMode::ClampToEdge => glow::CLAMP_TO_EDGE,
            AddressMode::Repeat => glow::REPEAT,
        } as i32;
        // Safety: a current context; a new sampler object's parameters.
        unsafe {
            let raw = self.gl.create_sampler().map_err(Error::Driver)?;
            self.gl
                .sampler_parameter_i32(raw, glow::TEXTURE_MIN_FILTER, filter);
            self.gl
                .sampler_parameter_i32(raw, glow::TEXTURE_MAG_FILTER, filter);
            self.gl
                .sampler_parameter_i32(raw, glow::TEXTURE_WRAP_S, wrap);
            self.gl
                .sampler_parameter_i32(raw, glow::TEXTURE_WRAP_T, wrap);
            Ok(Sampler { raw })
        }
    }

    pub fn destroy_sampler(&self, sampler: Sampler) {
        if self.ready().is_ok() {
            // Safety: a sampler of this device, destroyed once.
            unsafe { self.gl.delete_sampler(sampler.raw) };
        }
    }

    // ---- bind groups and pipelines ----

    pub fn create_bind_group_layout(&self, entries: &[BindingLayout]) -> Result<BindGroupLayout> {
        for e in entries {
            binding_index(0, e.binding)?;
        }
        Ok(BindGroupLayout {
            entries: entries.to_vec(),
        })
    }

    pub fn create_bind_group(
        &self,
        layout: &BindGroupLayout,
        entries: &[BindGroupEntry<'_>],
    ) -> Result<BindGroup> {
        self.ready()?;
        let mut out = Vec::new();
        for l in &layout.entries {
            let e = entries
                .iter()
                .find(|e| e.binding == l.binding)
                .ok_or_else(|| Error::Invalid(format!("binding {} has no resource", l.binding)))?;
            let bound = match (l.kind, e.resource) {
                (
                    BindingKind::UniformBuffer,
                    BindingResource::Buffer {
                        buffer,
                        offset,
                        size,
                    },
                ) => {
                    if buffer.usage != BufferUsage::Uniform || offset + size > buffer.size {
                        return Err(Error::Invalid(format!(
                            "binding {}: not a uniform range",
                            l.binding
                        )));
                    }
                    Bound::Buffer {
                        raw: buffer.raw,
                        offset,
                        size,
                    }
                }
                (BindingKind::Texture, BindingResource::Texture(t)) => {
                    if !t.desc.usage.sampled {
                        return Err(Error::Invalid(format!(
                            "binding {}: the texture is not sampled",
                            l.binding
                        )));
                    }
                    Bound::Texture(t.raw)
                }
                (BindingKind::Sampler, BindingResource::Sampler(s)) => Bound::Sampler(s.raw),
                (kind, _) => {
                    return Err(Error::Invalid(format!(
                        "binding {} holds a {kind:?}",
                        l.binding
                    )));
                }
            };
            out.push((l.binding, bound));
        }
        Ok(BindGroup { entries: out })
    }

    pub fn create_pipeline(&self, desc: &PipelineDesc<'_>) -> Result<Pipeline> {
        self.ready()?;
        let gl = &self.gl;
        self.touch(Touched::PROGRAM);
        // Safety: a current context; shaders compiled and linked, names looked up in the linked program.
        unsafe {
            let program = self.link(desc.shader)?;
            gl.use_program(Some(program));
            let mut textures = Vec::new();
            let mut result = Ok(());
            for b in desc.shader.bindings() {
                match b {
                    NamedBinding::Uniform {
                        group,
                        binding,
                        block,
                    } => {
                        let point = binding_index(group, binding);
                        match (point, gl.get_uniform_block_index(program, &block)) {
                            (Ok(point), Some(index)) => {
                                gl.uniform_block_binding(program, index, point)
                            }
                            (Err(e), _) => result = Err(e),
                            (_, None) => {
                                result = Err(Error::Driver(format!(
                                    "{}: no block {block}",
                                    desc.shader.name
                                )))
                            }
                        }
                    }
                    NamedBinding::Texture {
                        group,
                        binding,
                        sampler,
                        name,
                    } => match binding_index(group, binding) {
                        Ok(unit) => {
                            if let Some(location) = gl.get_uniform_location(program, &name) {
                                gl.uniform_1_i32(Some(&location), unit as i32);
                            }
                            textures.push((group, binding, sampler));
                        }
                        Err(e) => result = Err(e),
                    },
                }
            }
            // the program stays current: a pass sets its own, and a borrowed device's leave restores the owner's
            // (an earlier program may be one deleted since, which cannot be made current again)
            if let Err(e) = result {
                gl.delete_program(program);
                return Err(e);
            }
            Ok(Pipeline {
                program,
                layouts: desc.layouts.iter().map(|l| (*l).clone()).collect(),
                vertex_buffers: desc.vertex_buffers.to_vec(),
                blend: desc.blend,
                topology: desc.topology,
                textures,
            })
        }
    }

    /// Makes `shaders` into the program cache for a later device of the same driver, without keeping them: each is
    /// loaded where a saved binary the driver takes is there, else compiled, linked and saved (a binary the driver
    /// refuses is replaced), then deleted. Without a cache it does nothing. The counts are in
    /// [`Device::program_cache_stats`].
    pub fn prepare_programs(&self, shaders: &[ShaderSource]) -> Result<()> {
        if self.program_cache.is_none() {
            return Ok(());
        }
        self.ready()?;
        self.touch(Touched::PROGRAM);
        for shader in shaders {
            let program = self.link(shader)?;
            // Safety: a program of this device's current context, deleted once.
            unsafe { self.gl.delete_program(program) };
        }
        Ok(())
    }

    /// The linked program of `shader`: loaded from a saved binary where the cache has one the driver takes, else
    /// compiled and linked (and saved).
    pub(super) fn link(&self, shader: &ShaderSource) -> Result<glow::Program> {
        let gl = &self.gl;
        let file = program_key(shader.name, shader.vertex_glsl, shader.fragment_glsl);
        let cache = self.program_cache.as_ref();
        if let Some((format, buffer)) = cache.and_then(|c| c.load(&file)) {
            // Safety: a current context; the program is deleted when the driver refuses the binary.
            unsafe {
                let program = gl.create_program().map_err(Error::Driver)?;
                gl.program_binary(program, &glow::ProgramBinary { buffer, format });
                if gl.get_program_link_status(program) {
                    if let Some(c) = cache {
                        c.note_loaded();
                    }
                    return Ok(program);
                }
                gl.delete_program(program);
                // a format the driver does not know leaves an error, which would fail the next query that checks
                while gl.get_error() != glow::NO_ERROR {}
            }
            if let Some(c) = cache {
                c.note_refused(&file);
            }
        }
        // Safety: a current context; shaders and the program are deleted on every failure.
        unsafe {
            let compile = |kind: u32, source: &str| -> Result<glow::Shader> {
                let s = gl.create_shader(kind).map_err(Error::Driver)?;
                gl.shader_source(s, source);
                gl.compile_shader(s);
                if !gl.get_shader_compile_status(s) {
                    let log = gl.get_shader_info_log(s);
                    gl.delete_shader(s);
                    return Err(Error::Driver(format!("{}: {log}", shader.name)));
                }
                Ok(s)
            };
            let vs = compile(glow::VERTEX_SHADER, shader.vertex_glsl)?;
            let fs = match compile(glow::FRAGMENT_SHADER, shader.fragment_glsl) {
                Ok(fs) => fs,
                Err(e) => {
                    gl.delete_shader(vs);
                    return Err(e);
                }
            };
            let program = gl.create_program().map_err(Error::Driver)?;
            if cache.is_some() {
                gl.program_binary_retrievable_hint(program, true);
            }
            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.link_program(program);
            gl.detach_shader(program, vs);
            gl.detach_shader(program, fs);
            gl.delete_shader(vs);
            gl.delete_shader(fs);
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(Error::Driver(format!("{}: {log}", shader.name)));
            }
            if let Some(c) = cache {
                c.note_compiled();
                if let Some(binary) = gl.get_program_binary(program) {
                    c.store(&file, binary.format, &binary.buffer);
                }
            }
            Ok(program)
        }
    }

    pub fn destroy_pipeline(&self, pipeline: Pipeline) {
        if self.ready().is_ok() {
            // Safety: a program of this device, destroyed once.
            unsafe { self.gl.delete_program(pipeline.program) };
        }
    }

    // ---- passes and submission ----
}

pub(super) fn raw_name(texture: glow::Texture) -> u32 {
    texture.0.get()
}

pub(super) fn buffer_target(usage: BufferUsage) -> u32 {
    match usage {
        BufferUsage::Uniform => glow::UNIFORM_BUFFER,
        BufferUsage::Vertex => glow::ARRAY_BUFFER,
        BufferUsage::Index => glow::ELEMENT_ARRAY_BUFFER,
    }
}

pub(super) fn internal_format(format: TextureFormat) -> u32 {
    match format {
        TextureFormat::R8 => glow::R8,
        TextureFormat::Rgba8 => glow::RGBA8,
        TextureFormat::Rgba16Float => glow::RGBA16F,
        TextureFormat::Rgba32Float => glow::RGBA32F,
    }
}

pub(super) fn upload_format(format: TextureFormat) -> (u32, u32) {
    match format {
        TextureFormat::R8 => (glow::RED, glow::UNSIGNED_BYTE),
        TextureFormat::Rgba8 => (glow::RGBA, glow::UNSIGNED_BYTE),
        TextureFormat::Rgba16Float => (glow::RGBA, glow::HALF_FLOAT),
        TextureFormat::Rgba32Float => (glow::RGBA, glow::FLOAT),
    }
}

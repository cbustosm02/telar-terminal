//! Render por GPU: todo (celdas, glifos, barra lateral, tabs) es una lista de
//! quads instanciados que se dibuja con un solo draw call por frame.

use std::sync::Arc;

use winit::window::Window;

use crate::font::{Fonts, ATLAS_SIZE};
use crate::theme::Rgba;

pub const KIND_RECT: f32 = 0.0;
pub const KIND_MASK: f32 = 1.0;
pub const KIND_COLOR: f32 = 2.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Quad {
    /// x, y, ancho, alto en píxeles físicos.
    pub rect: [f32; 4],
    /// Región del atlas en píxeles (solo glifos).
    pub uv: [f32; 4],
    pub color: Rgba,
    /// tipo (KIND_*), radio de esquinas, sin uso, sin uso.
    pub params: [f32; 4],
}

impl Quad {
    pub fn rect(x: f32, y: f32, w: f32, h: f32, color: Rgba) -> Self {
        Self { rect: [x, y, w, h], uv: [0.0; 4], color, params: [KIND_RECT, 0.0, 0.0, 0.0] }
    }

    pub fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgba) -> Self {
        Self { rect: [x, y, w, h], uv: [0.0; 4], color, params: [KIND_RECT, radius, 0.0, 0.0] }
    }
}

const SHADER: &str = r#"
struct Globals { screen: vec2<f32>, atlas: vec2<f32> };
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local: vec2<f32>,
    @location(3) half_size: vec2<f32>,
    @location(4) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32,
      @location(0) rect: vec4<f32>,
      @location(1) uv: vec4<f32>,
      @location(2) color: vec4<f32>,
      @location(3) params: vec4<f32>) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let c = corners[vi];
    let p = rect.xy + c * rect.zw;
    var o: VsOut;
    o.pos = vec4(p.x / g.screen.x * 2.0 - 1.0, 1.0 - p.y / g.screen.y * 2.0, 0.0, 1.0);
    o.uv = (uv.xy + c * uv.zw) / g.atlas;
    o.color = color;
    o.half_size = rect.zw * 0.5;
    o.local = (c - vec2(0.5)) * rect.zw;
    o.params = params;
    return o;
}

@fragment
fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let t = textureSampleLevel(atlas_tex, atlas_samp, i.uv, 0.0);
    let kind = i.params.x;
    if (kind < 0.5) {
        let r = i.params.y;
        if (r > 0.0) {
            let q = abs(i.local) - i.half_size + vec2(r);
            let d = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
            return vec4(i.color.rgb, i.color.a * clamp(0.5 - d, 0.0, 1.0));
        }
        return i.color;
    }
    if (kind < 1.5) {
        return vec4(i.color.rgb, i.color.a * t.a);
    }
    return t;
}
"#;

pub struct Renderer {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: usize,
    atlas: wgpu::Texture,
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone()).expect("superficie");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .expect("adaptador GPU");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("telar"),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            })
            .await
            .expect("dispositivo GPU");

        let caps = surface.get_capabilities(&adapter);
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("configuración de superficie");
        // Formato sin sRGB: los colores del tema se escriben tal cual.
        config.format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(config.format);
        config.present_mode = wgpu::PresentMode::Fifo;
        config.desired_maximum_frame_latency = 2;
        if caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&device, &config);

        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quads"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quads"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Quad>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let capacity = 4096;
        let instances = Self::instance_buffer(&device, capacity);

        Self { window, surface, device, queue, config, pipeline, bind_group, uniforms, instances, capacity, atlas }
    }

    fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quads"),
            size: (capacity * std::mem::size_of::<Quad>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Dibuja el frame. Con `capture`, además guarda el frame en un BMP (modo snapshot).
    pub fn render(&mut self, quads: &[Quad], clear: Rgba, fonts: &mut Fonts, capture: Option<&std::path::Path>) {
        for up in fonts.take_uploads() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: up.x, y: up.y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &up.data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(up.w * 4), rows_per_image: Some(up.h) },
                wgpu::Extent3d { width: up.w, height: up.h, depth_or_array_layers: 1 },
            );
        }

        let globals = [self.config.width as f32, self.config.height as f32, ATLAS_SIZE as f32, ATLAS_SIZE as f32];
        self.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&globals));

        if quads.len() > self.capacity {
            self.capacity = quads.len().next_power_of_two();
            self.instances = Self::instance_buffer(&self.device, self.capacity);
        }
        self.queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(quads));

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64,
                            g: clear[1] as f64,
                            b: clear[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if !quads.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..6, 0..quads.len() as u32);
            }
        }
        let readback = capture.map(|_| self.copy_frame(&mut encoder, &frame.texture));
        self.queue.submit([encoder.finish()]);
        if let (Some(path), Some((buf, padded))) = (capture, readback) {
            self.save_bmp(path, &buf, padded);
        }
        self.window.pre_present_notify();
        self.queue.present(frame);
    }

    fn copy_frame(&self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) -> (wgpu::Buffer, u32) {
        let (w, h) = (self.config.width, self.config.height);
        let padded = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("captura"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        (buf, padded)
    }

    /// BMP de 32 bits de arriba hacia abajo (BMP ya guarda los píxeles en orden BGRA).
    fn save_bmp(&self, path: &std::path::Path, buf: &wgpu::Buffer, padded: u32) {
        let (w, h) = (self.config.width, self.config.height);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(data) = buf.slice(..).get_mapped_range() else { return };
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut out = Vec::with_capacity((54 + w * h * 4) as usize);
        let size = 54 + w * h * 4;
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(-(h as i32)).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&[0u8; 24]);
        for row in 0..h {
            let start = (row * padded) as usize;
            for px in data[start..start + (w * 4) as usize].chunks(4) {
                if bgra {
                    out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                } else {
                    out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
        }
        if let Err(e) = std::fs::write(path, out) {
            log::error!("no se pudo guardar la captura: {e}");
        }
    }
}

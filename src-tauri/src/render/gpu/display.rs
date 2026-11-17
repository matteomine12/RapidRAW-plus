use crate::app::state::{AppState, PreviewTarget};
use crate::render::gpu::context::current_gpu_context;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DisplayTransform {
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub window: [f32; 2],
    pub image_size: [f32; 2],
    pub texture_size: [f32; 2],
    pub pixelated: f32,
    /// scRGB scale (SDR white nits / 80) when the surface is HDR, 0 for an SDR surface.
    pub hdr_scale: f32,
    pub bg_primary: [f32; 4],
    pub bg_secondary: [f32; 4],
    pub original_size: [f32; 2],
    pub split_origin: [f32; 2],
    pub split_normal: [f32; 2],
    pub split_offset: [f32; 2],
    /// `[source_is_linear, 0, 0, 0]`: the shown texture holds linear light (HDR render) rather than 8-bit sRGB.
    pub extra: [f32; 4],
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DisplayLayer {
    Edited,
    Original,
}

impl Default for DisplayTransform {
    fn default() -> Self {
        Self {
            rect: [0.0, 0.0, 100.0, 100.0],
            clip: [0.0, 0.0, 10000.0, 10000.0],
            window: [1280.0, 720.0],
            image_size: [100.0, 100.0],
            texture_size: [100.0, 100.0],
            pixelated: 0.0,
            hdr_scale: 0.0,
            bg_primary: [24.0 / 255.0, 24.0 / 255.0, 24.0 / 255.0, 1.0],
            bg_secondary: [35.0 / 255.0, 35.0 / 255.0, 35.0 / 255.0, 1.0],
            original_size: [100.0, 100.0],
            split_origin: [0.0, 0.0],
            split_normal: [0.0, 0.0],
            split_offset: [0.0, 0.0],
            extra: [0.0; 4],
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
pub struct DisplayView {
    pub zoom: f32,
    pub center: [f32; 2],
}

impl Default for DisplayView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            center: [0.5, 0.5],
        }
    }
}

pub struct DisplayFrame {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    capacity: [u32; 2],
}

#[derive(Clone, Copy)]
pub struct DisplayOutput {
    pub target: PreviewTarget,
    pub layer: DisplayLayer,
    pub to_viewer: bool,
    pub content: u64,
}

pub fn display_content_key(path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

pub fn align_texture_dimension(value: u32) -> u32 {
    (value + 255) & !255
}

pub struct WgpuDisplay {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    pub transform_buffer: wgpu::Buffer,
    pub latest_transform: DisplayTransform,
    pub current_bind_group: Option<wgpu::BindGroup>,
    pub edited_view: Option<wgpu::TextureView>,
    pub original_texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    pub standalone: bool,
    /// The surface is a float scRGB one and the shader scales into it.
    pub hdr_output: bool,
    /// The surface has no fp16 scRGB format but offers HDR10 (PQ, Rec.2020) on `Rgb10a2Unorm`, so HDR output
    /// uses that instead of `Rgba16Float`.
    pub hdr10: bool,
    pub view: DisplayView,
    pub frame: Option<DisplayFrame>,
    pub content_key: Option<u64>,
    pub rect_key: Option<u64>,
    pub pending_rect: Option<(u64, [f32; 4])>,
    #[cfg(target_os = "linux")]
    pub wayland: Option<crate::platform::wayland_subsurface::WaylandSubsurface>,
}

impl WgpuDisplay {
    pub fn original_texture(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        linear: bool,
    ) -> &wgpu::Texture {
        let format = if linear {
            wgpu::TextureFormat::Rgba16Float
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        if self.original_texture.as_ref().is_none_or(|(t, _)| {
            t.width() != width || t.height() != height || t.format() != format
        }) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Original Display Texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.original_texture = Some((texture, view));
        }
        &self.original_texture.as_ref().unwrap().0
    }

    pub fn rebind(&mut self, device: &wgpu::Device) {
        let Some(edited_view) = &self.edited_view else {
            return;
        };
        let original_view = self
            .original_texture
            .as_ref()
            .map_or(edited_view, |(_, view)| view);
        self.current_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.transform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(edited_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(original_view),
                },
            ],
            label: None,
        }));
    }

    pub fn show_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        image_size: [f32; 2],
        texture_size: [f32; 2],
        content: Option<u64>,
        linear: bool,
    ) {
        self.set_source_linear(linear);
        self.latest_transform.image_size = image_size;
        self.latest_transform.texture_size = texture_size;
        self.edited_view = Some(view.clone());
        if let Some(key) = content {
            self.content_key = Some(key);
            if let Some((pending_key, rect)) = self.pending_rect
                && pending_key == key
            {
                self.latest_transform.rect = rect;
                self.rect_key = Some(key);
                self.pending_rect = None;
            }
        }
        self.place_image();
        self.rebind(device);
        self.flush(device, queue);
    }

    /// Tells the shader whether the textures it samples hold linear light (HDR render) or 8-bit sRGB.
    pub fn set_source_linear(&mut self, linear: bool) {
        self.latest_transform.extra[0] = if linear { 1.0 } else { 0.0 };
    }

    pub fn show_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        size: [u32; 2],
        max_dimension: u32,
    ) {
        let area = |dims: [u32; 2]| dims[0] as u64 * dims[1] as u64;
        let format = source.format();
        let reusable = self.frame.as_ref().is_some_and(|frame| {
            frame.texture.format() == format
                && frame.capacity[0] >= size[0]
                && frame.capacity[1] >= size[1]
                && area(frame.capacity) <= 4 * area(size)
        });
        if !reusable {
            let capacity = size.map(|dim| align_texture_dimension(dim).min(max_dimension));
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Viewer Frame Texture"),
                size: wgpu::Extent3d {
                    width: capacity[0],
                    height: capacity[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.frame = Some(DisplayFrame {
                texture,
                view,
                capacity,
            });
        }
        let Some(frame) = &self.frame else {
            return;
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Viewer Frame Copy"),
        });
        encoder.copy_texture_to_texture(
            source.as_image_copy(),
            frame.texture.as_image_copy(),
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        let view = frame.view.clone();
        let capacity = frame.capacity;
        self.show_texture(
            device,
            queue,
            &view,
            size.map(|dim| dim as f32),
            capacity.map(|dim| dim as f32),
            None,
            format == wgpu::TextureFormat::Rgba16Float,
        );
    }

    pub fn set_view(&mut self, view: DisplayView) {
        self.view = view;
        self.place_image();
    }

    pub fn set_background(&mut self, color: [f32; 4]) {
        self.latest_transform.bg_primary = color;
        self.latest_transform.bg_secondary = color;
    }

    pub fn resize(&mut self, device: &wgpu::Device, size: tauri::PhysicalSize<u32>) {
        let width = size.width.max(1);
        let height = size.height.max(1);
        if width != self.config.width || height != self.config.height {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(device, &self.config);
        }
        self.place_image();
    }

    pub fn place_editor_rect(&mut self, content: Option<u64>, rect: [f32; 4]) -> bool {
        match content {
            Some(key) if self.content_key != Some(key) => {
                self.pending_rect = Some((key, rect));
                false
            }
            _ => {
                self.latest_transform.rect = rect;
                self.rect_key = self.content_key;
                self.pending_rect = None;
                true
            }
        }
    }

    pub fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        self.latest_transform.hdr_scale = if self.hdr_output {
            crate::render::hdr::scrgb_scale()
        } else {
            0.0
        };
        // 1.0: the surface is HDR10, so the shader writes PQ-encoded Rec.2020 instead of scRGB.
        self.latest_transform.extra[1] = if self.hdr_output && self.hdr10 { 1.0 } else { 0.0 };
        queue.write_buffer(
            &self.transform_buffer,
            0,
            bytemuck::bytes_of(&self.latest_transform),
        );
        self.render(device, queue)
    }

    fn place_image(&mut self) {
        if !self.standalone {
            return;
        }
        let surface_w = self.config.width as f32;
        let surface_h = self.config.height as f32;
        let [image_w, image_h] = self.latest_transform.image_size;
        let scale = if image_w > 0.0 && image_h > 0.0 {
            (surface_w / image_w).min(surface_h / image_h) * self.view.zoom
        } else {
            0.0
        };
        let placed_w = image_w * scale;
        let placed_h = image_h * scale;
        self.latest_transform.rect = [
            (surface_w / 2.0 - self.view.center[0] * placed_w).round(),
            (surface_h / 2.0 - self.view.center[1] * placed_h).round(),
            placed_w,
            placed_h,
        ];
        self.latest_transform.clip = [0.0, 0.0, surface_w, surface_h];
        self.latest_transform.window = [surface_w, surface_h];
    }

    /// The background in the surface's own encoding (sRGB values for an SDR surface, scaled linear for scRGB).
    fn clear_color(&self) -> wgpu::Color {
        let [r, g, b, a] = self.latest_transform.bg_primary;
        if !self.hdr_output {
            return wgpu::Color {
                r: r as f64,
                g: g as f64,
                b: b as f64,
                a: a as f64,
            };
        }
        let scale = crate::render::hdr::scrgb_scale() as f64;
        let linear = |v: f32| {
            let v = v.clamp(0.0, 1.0) as f64;
            let l = if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            };
            l * scale
        };
        if self.hdr10 {
            // HDR10 surface: PQ-encoded Rec.2020, with 1.0 (SDR white) shown at the SDR white level in nits.
            let [pr, pg, pb] = hdr10_pq_encode([linear(r), linear(g), linear(b)]);
            return wgpu::Color {
                r: pr,
                g: pg,
                b: pb,
                a: a as f64,
            };
        }
        wgpu::Color {
            r: linear(r),
            g: linear(g),
            b: linear(b),
            a: a as f64,
        }
    }

    /// Switches the surface between SDR and scRGB (float) output, e.g. when HDR is turned on in the OS or in
    /// the settings. Returns whether the mode changed; the shown image is dropped and must be rendered again.
    pub fn set_hdr_output(
        &mut self,
        device: &wgpu::Device,
        adapter: &wgpu::Adapter,
        want_hdr: bool,
    ) -> bool {
        let caps = self.surface.get_capabilities(adapter);
        let can_hdr = surface_supports_hdr(&caps, device) || self.hdr10;
        let want_hdr = want_hdr && can_hdr;
        if want_hdr == self.hdr_output {
            return false;
        }
        let format = if want_hdr && self.hdr10 {
            Some(wgpu::TextureFormat::Rgb10a2Unorm)
        } else if want_hdr {
            Some(wgpu::TextureFormat::Rgba16Float)
        } else {
            sdr_swapchain_format(&caps, device)
        };
        let Some(format) = format else {
            return false;
        };
        set_hdr10_swapchain(want_hdr && self.hdr10);
        self.config.format = format;
        self.surface.configure(device, &self.config);
        self.pipeline = create_display_pipeline(device, &self.bind_group_layout, format);
        self.hdr_output = want_hdr;
        self.frame = None;
        self.original_texture = None;
        self.edited_view = None;
        self.current_bind_group = None;
        log::info!("WGPU display surface switched to {:?}", format);
        true
    }

    fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        if self.current_bind_group.is_none() && !self.standalone {
            return false;
        }
        if !self.standalone && self.rect_key != self.content_key {
            return false;
        }
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex)
            | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(tex)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
                    _ => return false,
                }
            }
            _ => return false,
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color()),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: NonZero::new(0),
            });
            if let Some(bind_group) = &self.current_bind_group {
                let clip_x1 = self.latest_transform.clip[0].max(0.0);
                let clip_y1 = self.latest_transform.clip[1].max(0.0);
                let clip_x2 =
                    (self.latest_transform.clip[0] + self.latest_transform.clip[2]).max(0.0);
                let clip_y2 =
                    (self.latest_transform.clip[1] + self.latest_transform.clip[3]).max(0.0);

                let final_clip_x = clip_x1.floor() as u32;
                let final_clip_y = clip_y1.floor() as u32;
                let final_clip_w = (clip_x2.ceil() as u32).saturating_sub(final_clip_x);
                let final_clip_h = (clip_y2.ceil() as u32).saturating_sub(final_clip_y);

                let max_x = self.config.width;
                let max_y = self.config.height;

                if final_clip_x < max_x && final_clip_y < max_y {
                    let clamped_width = final_clip_w.min(max_x - final_clip_x);
                    let clamped_height = final_clip_h.min(max_y - final_clip_y);

                    if clamped_width > 0 && clamped_height > 0 {
                        rpass.set_scissor_rect(
                            final_clip_x,
                            final_clip_y,
                            clamped_width,
                            clamped_height,
                        );

                        rpass.set_pipeline(&self.pipeline);
                        rpass.set_bind_group(0, bind_group, &[]);
                        rpass.draw(0..4, 0..1);
                    }
                }
            }
        }
        queue.submit(Some(encoder.finish()));
        output.present();
        true
    }
}

/// Whether the surface can present half-float scRGB (HDR) on this adapter.
fn surface_supports_hdr(caps: &wgpu::SurfaceCapabilities, device: &wgpu::Device) -> bool {
    let format = wgpu::TextureFormat::Rgba16Float;
    caps.formats.contains(&format) && device.features().contains(format.required_features())
}

/// The 8-bit (or 10-bit on Linux) surface format used for SDR output.
fn sdr_swapchain_format(
    caps: &wgpu::SurfaceCapabilities,
    device: &wgpu::Device,
) -> Option<wgpu::TextureFormat> {
    let usable = |f: &wgpu::TextureFormat| device.features().contains(f.required_features());
    #[cfg(target_os = "linux")]
    let preferred = [
        wgpu::TextureFormat::Rgb10a2Unorm,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
    ]
    .into_iter()
    .find(|f| caps.formats.contains(f) && usable(f));
    #[cfg(not(target_os = "linux"))]
    let preferred: Option<wgpu::TextureFormat> = None;
    preferred
        .or_else(|| {
            caps.formats
                .iter()
                .copied()
                .find(|f| !f.is_srgb() && *f != wgpu::TextureFormat::Rgba16Float && usable(f))
        })
        .or_else(|| caps.formats.iter().copied().find(usable))
}

fn create_display_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    swapchain_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Display Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/display.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Display Pipeline Layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Display Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: swapchain_format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: NonZero::new(0),
        cache: None,
    })
}

/// Every (format, colour space) pair the Vulkan driver offers for the surface, before wgpu filters them.
/// wgpu only keeps fp16 when it comes with `EXTENDED_SRGB_LINEAR`, so this is how HDR offered in another
/// form (HDR10 PQ on a 10-bit format, as NVIDIA does on Wayland) is found.
#[cfg(target_os = "linux")]
fn raw_vulkan_surface_formats(
    surface: &wgpu::Surface<'_>,
    adapter: &wgpu::Adapter,
) -> Vec<(ash::vk::Format, ash::vk::ColorSpaceKHR)> {
    use wgpu::hal::api::Vulkan;
    unsafe {
        let Some(hal_surface) = surface.as_hal::<Vulkan>() else {
            return Vec::new();
        };
        let Some(raw_surface) = hal_surface.raw_native_handle() else {
            return Vec::new();
        };
        let Some(hal_adapter) = adapter.as_hal::<Vulkan>() else {
            return Vec::new();
        };
        let shared = hal_adapter.shared_instance();
        let loader = ash::khr::surface::Instance::new(shared.entry(), shared.raw_instance());
        match loader.get_physical_device_surface_formats(hal_adapter.raw_physical_device(), raw_surface) {
            Ok(formats) => formats.into_iter().map(|f| (f.format, f.color_space)).collect(),
            Err(e) => {
                log::warn!("Could not list Vulkan surface formats: {e}");
                Vec::new()
            }
        }
    }
}

/// Whether HDR10 (PQ, Rec.2020) can be presented on a 10-bit surface: the driver offers it and the 10-bit
/// format is one wgpu can use.
#[cfg(target_os = "linux")]
fn surface_offers_hdr10(
    surface: &wgpu::Surface<'_>,
    adapter: &wgpu::Adapter,
    caps: &wgpu::SurfaceCapabilities,
    device: &wgpu::Device,
) -> bool {
    use ash::vk::{ColorSpaceKHR, Format};
    let format = wgpu::TextureFormat::Rgb10a2Unorm;
    if !caps.formats.contains(&format) || !device.features().contains(format.required_features()) {
        return false;
    }
    let raw = raw_vulkan_surface_formats(surface, adapter);
    for (f, cs) in &raw {
        log::info!("Vulkan surface format: {f:?} / {cs:?}");
    }
    raw.iter()
        .any(|(f, cs)| *f == Format::A2B10G10R10_UNORM_PACK32 && *cs == ColorSpaceKHR::HDR10_ST2084_EXT)
}

#[cfg(not(target_os = "linux"))]
fn surface_offers_hdr10(
    _surface: &wgpu::Surface<'_>,
    _adapter: &wgpu::Adapter,
    _caps: &wgpu::SurfaceCapabilities,
    _device: &wgpu::Device,
) -> bool {
    false
}

/// Makes `Rgb10a2Unorm` swapchains use the HDR10 colour space (or the default sRGB one again).
fn set_hdr10_swapchain(enabled: bool) {
    #[cfg(target_os = "linux")]
    wgpu::hal::vulkan::set_hdr10_pq(enabled);
    #[cfg(not(target_os = "linux"))]
    let _ = enabled;
}

/// Encodes linear Rec.709 light given in scRGB units (1.0 = 80 nits) as PQ (SMPTE ST 2084) in Rec.2020,
/// the signal an HDR10 surface expects. Mirrors `to_surface` in `display.wgsl`.
fn hdr10_pq_encode(scrgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = scrgb.map(|v| v.max(0.0) * crate::render::hdr::SCRGB_REFERENCE_NITS as f64);
    let rec2020 = [
        0.627_404 * r + 0.329_283 * g + 0.043_313 * b,
        0.069_097 * r + 0.919_540 * g + 0.011_362 * b,
        0.016_391 * r + 0.088_013 * g + 0.895_595 * b,
    ];
    rec2020.map(|nits| {
        let y = (nits / 10_000.0).clamp(0.0, 1.0).powf(0.159_301_757_812_5);
        ((0.835_937_5 + 18.851_562_5 * y) / (1.0 + 18.687_5 * y)).powf(78.843_75)
    })
}

#[cfg(not(target_os = "android"))]
pub fn build_wgpu_display(
    surface: wgpu::Surface<'static>,
    adapter: &wgpu::Adapter,
    device: &wgpu::Device,
    size: tauri::PhysicalSize<u32>,
    standalone: bool,
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))] platform: PlatformSurface,
) -> Result<WgpuDisplay, String> {
    let swapchain_caps = surface.get_capabilities(adapter);
    log::info!("WGPU surface formats offered: {:?}", swapchain_caps.formats);
    let hdr_fp16 = surface_supports_hdr(&swapchain_caps, device);
    let hdr10 = !hdr_fp16 && surface_offers_hdr10(&surface, adapter, &swapchain_caps, device);
    let hdr_capable = hdr_fp16 || hdr10;
    crate::render::hdr::set_surface_capable(hdr_capable);
    crate::render::hdr::refresh_global();
    let hdr_output = hdr_capable && crate::render::hdr::display_active();
    set_hdr10_swapchain(hdr_output && hdr10);
    let swapchain_format = if hdr_output && hdr10 {
        wgpu::TextureFormat::Rgb10a2Unorm
    } else if hdr_output {
        wgpu::TextureFormat::Rgba16Float
    } else {
        sdr_swapchain_format(&swapchain_caps, device)
            .ok_or("Surface is not supported by the GPU adapter")?
    };

    let alpha_mode = if cfg!(target_os = "windows")
        && swapchain_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::Opaque)
    {
        wgpu::CompositeAlphaMode::Opaque
    } else if swapchain_caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
    {
        wgpu::CompositeAlphaMode::PreMultiplied
    } else if swapchain_caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
    {
        wgpu::CompositeAlphaMode::PostMultiplied
    } else {
        swapchain_caps.alpha_modes[0]
    };

    let present_mode = if (standalone || cfg!(target_os = "linux"))
        && swapchain_caps
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
    {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::Fifo
    };

    let config = wgpu::SurfaceConfiguration {
        width: size.width.max(1),
        height: size.height.max(1),
        format: swapchain_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        present_mode,
        alpha_mode,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(device, &config);
    log::info!(
        "WGPU display surface: {:?}, {:?} (HDR capable: {}, HDR10 PQ: {}, HDR output: {})",
        swapchain_format,
        present_mode,
        hdr_capable,
        hdr10,
        hdr_output
    );

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Display BGL"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                count: None,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                count: None,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                count: None,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                count: None,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            },
        ],
    });

    let pipeline = create_display_pipeline(device, &bind_group_layout, swapchain_format);

    let transform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Transform Buffer"),
        size: std::mem::size_of::<DisplayTransform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Display Sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    Ok(WgpuDisplay {
        surface,
        config,
        pipeline,
        bind_group_layout,
        transform_buffer,
        latest_transform: DisplayTransform::default(),
        sampler,
        current_bind_group: None,
        edited_view: None,
        original_texture: None,
        standalone,
        hdr_output,
        hdr10,
        view: DisplayView::default(),
        frame: None,
        content_key: None,
        rect_key: None,
        pending_rect: None,
        #[cfg(target_os = "linux")]
        wayland: platform,
    })
}

#[cfg(target_os = "linux")]
pub type PlatformSurface = Option<crate::platform::wayland_subsurface::WaylandSubsurface>;

#[cfg(not(target_os = "linux"))]
pub type PlatformSurface = ();

#[cfg(target_os = "linux")]
pub fn surface_for_window(
    instance: &wgpu::Instance,
    window: &tauri::WebviewWindow,
) -> Result<(wgpu::Surface<'static>, PlatformSurface), String> {
    use gtk::prelude::WidgetExt;

    if !gtk::is_initialized_main_thread() {
        return Err("Wayland subsurface must be created on the main thread".to_string());
    }

    if let Ok(gtk_window) = window.gtk_window() {
        gtk_window.realize();
    }
    let wayland = crate::platform::wayland_subsurface::create(window)?;
    let surface = wayland.create_surface(instance)?;
    Ok((surface, Some(wayland)))
}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
pub fn surface_for_window(
    instance: &wgpu::Instance,
    window: &tauri::WebviewWindow,
) -> Result<(wgpu::Surface<'static>, PlatformSurface), String> {
    let surface = instance
        .create_surface(window.clone())
        .map_err(|e| e.to_string())?;
    Ok((surface, ()))
}

#[cfg(not(target_os = "android"))]
pub fn create_window_surface(
    instance: &wgpu::Instance,
    window: &tauri::WebviewWindow,
) -> Result<(wgpu::Surface<'static>, PlatformSurface), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let instance = instance.clone();
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            let _ = tx.send(surface_for_window(&instance, &target));
        })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}

pub fn resize_display(window: &tauri::Window, size: tauri::PhysicalSize<u32>) {
    update_display_size(window, size, false);
}

pub fn redraw_display(window: &tauri::Window, size: tauri::PhysicalSize<u32>) {
    update_display_size(window, size, true);
}

fn update_display_size(window: &tauri::Window, size: tauri::PhysicalSize<u32>, wait: bool) {
    let Some(ctx) = current_gpu_context(&window.state::<AppState>()) else {
        return;
    };
    let Some(display) = ctx.display_for_label(window.label()) else {
        return;
    };
    let mut lock = if wait {
        display.lock().unwrap_or_else(|e| e.into_inner())
    } else {
        match display.try_lock() {
            Ok(lock) => lock,
            Err(_) => return,
        }
    };
    if let Some(display) = lock.as_mut() {
        display.resize(&ctx.device, size);
        display.flush(&ctx.device, &ctx.queue);
    }
}

pub static WGPU_DISPLAY_FAILED: AtomicBool = AtomicBool::new(false);

pub fn wgpu_display_failed() -> bool {
    WGPU_DISPLAY_FAILED.load(Ordering::Acquire)
}

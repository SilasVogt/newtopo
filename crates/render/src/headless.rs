//! Headless rendering: offscreen texture → RGBA image / PNG. Used by
//! `newtopo-cli render` and the visual tests (SPEC §10, §11).

use crate::camera::OrbitCamera;
use crate::gpu::{GpuScene, Renderer};
use crate::scene::{RenderOptions, RenderScene};
use anyhow::{Context, anyhow};
use std::path::Path;

/// A device + renderer without any window.
pub struct Headless {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    pub adapter_info: String,
}

impl Headless {
    pub fn new() -> anyhow::Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|e| anyhow!("no GPU adapter for headless rendering: {e}"))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("newtopo headless"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: Default::default(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: Default::default(),
        }))
        .context("request headless device")?;
        let renderer = Renderer::new(&device);
        Ok(Headless {
            device,
            queue,
            renderer,
            adapter_info: format!("{} ({:?})", info.name, info.backend),
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Render the scene layers with `camera` into an RGBA (sRGB) image.
    pub fn render(
        &mut self,
        layers: &[&RenderScene],
        camera: &OrbitCamera,
        size: [u32; 2],
        opts: &RenderOptions,
    ) -> anyhow::Result<image::RgbaImage> {
        let size = [size[0].clamp(1, 8192), size[1].clamp(1, 8192)];
        self.renderer.ensure_target(&self.device, size);
        let gpu: Vec<GpuScene> = layers
            .iter()
            .map(|s| GpuScene::upload(&self.device, s))
            .collect();
        let (center, radius) = bounds_sphere(layers);
        let cam = camera.matrices([size[0] as f32, size[1] as f32], center, radius as f64);

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("headless"),
            });
        let refs: Vec<&GpuScene> = gpu.iter().collect();
        self.renderer
            .render(&self.queue, &mut enc, &refs, &cam, radius, opts);

        let target = self.renderer.target.as_ref().expect("target");
        let bpr_unpadded = size[0] * 4;
        let bpr = bpr_unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * size[1]) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.resolve,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(size[1]),
                },
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);

        let (tx, rx) = std::sync::mpsc::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| anyhow!("device poll: {e}"))?;
        rx.recv()
            .context("map callback")?
            .map_err(|e| anyhow!("map readback: {e}"))?;
        let mut pixels = Vec::with_capacity((size[0] * size[1] * 4) as usize);
        {
            let data = readback
                .get_mapped_range(..)
                .map_err(|e| anyhow!("mapped range: {e:?}"))?;
            for row in 0..size[1] {
                let start = (row * bpr) as usize;
                pixels.extend_from_slice(&data[start..start + bpr_unpadded as usize]);
            }
        }
        readback.unmap();
        image::RgbaImage::from_raw(size[0], size[1], pixels)
            .ok_or_else(|| anyhow!("bad image size"))
    }
}

/// Bounding sphere of all layers (for clip planes and ortho bias).
pub fn bounds_sphere(layers: &[&RenderScene]) -> (glam::DVec3, f32) {
    let mut min = glam::Vec3::splat(f32::INFINITY);
    let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
    for l in layers {
        if let Some((a, b)) = l.bounds() {
            min = min.min(a);
            max = max.max(b);
        }
    }
    if min.x > max.x {
        return (glam::DVec3::ZERO, 1.0);
    }
    let c = (min + max) * 0.5;
    (c.as_dvec3(), ((max - min).length() * 0.5).max(1e-3))
}

/// Render a scene to a PNG file. This is the entry point for
/// `newtopo-cli render`: build the [`RenderScene`] (world space) from the
/// core scene, choose a camera (e.g. [`OrbitCamera::frame_aabb`] +
/// [`crate::camera::NamedView`]) and call this.
pub fn render_scene_to_png(
    scene: &RenderScene,
    camera: &OrbitCamera,
    size: [u32; 2],
    opts: &RenderOptions,
    path: &Path,
) -> anyhow::Result<()> {
    let mut h = Headless::new()?;
    let img = h.render(&[scene], camera, size, opts)?;
    img.save(path)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

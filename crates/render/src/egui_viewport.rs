//! egui integration: an `egui_wgpu` paint callback that renders the scene
//! into the shared offscreen target during `prepare` and blits it into
//! egui's render pass during `paint`.

use crate::camera::CameraMatrices;
use crate::gpu::{GpuScene, Renderer};
use crate::scene::{RenderOptions, RenderScene};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use std::sync::Arc;

const BLIT_WGSL: &str = r#"
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@vertex fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: VsOut;
    o.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}
fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}
@fragment fn fs_linear(i: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(tex, samp, i.uv).rgb, 1.0);
}
@fragment fn fs_gamma(i: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(to_gamma(textureSample(tex, samp, i.uv).rgb), 1.0);
}
"#;

/// GPU state kept in egui-wgpu's callback resources.
pub struct ViewportResources {
    /// One render target and layer cache per viewport slot, so several
    /// views (e.g. a before/after compare) can draw in the same frame.
    slots: Vec<Slot>,
    blit: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

struct Slot {
    renderer: Renderer,
    layers: Vec<(u64, GpuScene)>,
    blit_bg: Option<wgpu::BindGroup>,
}

/// Create the viewport resources. Call once after eframe created its
/// wgpu render state.
pub fn install(rs: &egui_wgpu::RenderState) {
    install_with(
        &rs.device,
        rs.target_format,
        &mut rs.renderer.write().callback_resources,
    );
}

/// Like [`install`], for a caller-owned `egui_wgpu::Renderer` (headless UI
/// screenshots).
pub fn install_with(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    resources: &mut CallbackResources,
) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewport blit"),
        source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
    });
    let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewport blit"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewport blit"),
        bind_group_layouts: &[Some(&blit_layout)],
        immediate_size: 0,
    });
    let fs = if target_format.is_srgb() {
        "fs_linear"
    } else {
        "fs_gamma"
    };
    let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewport blit"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(fs),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("viewport blit"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let res = ViewportResources {
        slots: Vec::new(),
        blit,
        blit_layout,
        sampler,
    };
    resources.insert(res);
}

/// A process-wide unique revision for [`ViewportCallback`] layers. Use a
/// fresh one whenever a layer's content changes.
pub fn next_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One frame of the viewport. Layers are re-uploaded only when their
/// revision changes, so pass a stable revision for unchanged content and a
/// new [`next_revision`] when it changes.
pub struct ViewportCallback {
    pub layers: Vec<(u64, Arc<RenderScene>)>,
    pub camera: CameraMatrices,
    pub scene_radius: f32,
    pub options: RenderOptions,
    /// Physical pixel size of the viewport rect.
    pub size_px: [u32; 2],
    /// Which view this is (0 = the main viewport); each slot keeps its own
    /// render target.
    pub slot: usize,
}

impl CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(all) = resources.get_mut::<ViewportResources>() else {
            return Vec::new();
        };
        while all.slots.len() <= self.slot {
            all.slots.push(Slot {
                renderer: Renderer::new(device),
                layers: Vec::new(),
                blit_bg: None,
            });
        }
        let (blit_layout, sampler) = (&all.blit_layout, &all.sampler);
        let res = &mut all.slots[self.slot];
        if res.renderer.ensure_target(device, self.size_px) || res.blit_bg.is_none() {
            let view = &res.renderer.target.as_ref().expect("target").resolve_view;
            res.blit_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("viewport blit"),
                layout: blit_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            }));
        }
        res.layers.truncate(self.layers.len());
        for (i, (rev, scene)) in self.layers.iter().enumerate() {
            match res.layers.get_mut(i) {
                Some((r, _)) if *r == *rev => {}
                Some(slot) => *slot = (*rev, GpuScene::upload(device, scene)),
                None => res.layers.push((*rev, GpuScene::upload(device, scene))),
            }
        }
        let refs: Vec<&GpuScene> = res.layers.iter().map(|(_, g)| g).collect();
        if std::env::var("NEWTOPO_RENDER_DEBUG").is_ok() {
            eprintln!(
                "viewport prepare: size {:?} layers {:?} cam eye {:?} near {} far {}",
                self.size_px,
                self.layers
                    .iter()
                    .map(|(r, s)| (*r, s.mesh.indices.len(), s.lines.len()))
                    .collect::<Vec<_>>(),
                self.camera.eye,
                self.camera.near,
                self.camera.far
            );
        }
        res.renderer.render(
            queue,
            encoder,
            &refs,
            &self.camera,
            self.scene_radius,
            &self.options,
        );
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(all) = resources.get::<ViewportResources>() else {
            return;
        };
        let Some(bg) = all.slots.get(self.slot).and_then(|s| s.blit_bg.as_ref()) else {
            return;
        };
        pass.set_pipeline(&all.blit);
        pass.set_bind_group(0, bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Convenience: wrap the callback as an egui shape for `rect`.
pub fn paint_callback(rect: egui::Rect, cb: ViewportCallback) -> egui::Shape {
    egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(rect, cb))
}

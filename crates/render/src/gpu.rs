//! GPU side: pipelines, uploaded scenes and the offscreen target. Shared by
//! the egui viewport callback and the headless renderer.

use crate::camera::CameraMatrices;
use crate::scene::{LineSeg, MeshVertex, PointMark, RenderOptions, RenderScene, ShadeMode};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Offscreen colour format. sRGB so blending and MSAA resolve happen in
/// linear space and the readback bytes are ready for PNG.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLE_COUNT: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Globals {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    viewport: [f32; 4],
    params: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear4(c: [f32; 4]) -> [f32; 4] {
    [
        srgb_to_linear(c[0]),
        srgb_to_linear(c[1]),
        srgb_to_linear(c[2]),
        c[3],
    ]
}

pub struct Pipelines {
    globals_layout: wgpu::BindGroupLayout,
    bg: wgpu::RenderPipeline,
    mesh: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    points: wgpu::RenderPipeline,
}

const MESH_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
const LINE_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32, 2 => Float32x3, 3 => Float32, 4 => Float32x4];
const POINT_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32, 2 => Float32x4];

impl Pipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("newtopo shaders"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport layout"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });

        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let make = |label: &str,
                    vs: &str,
                    fs: &str,
                    buffers: &[Option<wgpu::VertexBufferLayout>],
                    depth_write: bool,
                    depth_cmp: wgpu::CompareFunction,
                    blend: Option<wgpu::BlendState>,
                    cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(depth_cmp),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLE_COUNT,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let mesh_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &MESH_ATTRS,
        };
        let line_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineSeg>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &LINE_ATTRS,
        };
        let point_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PointMark>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &POINT_ATTRS,
        };
        use wgpu::CompareFunction as C;
        Pipelines {
            bg: make(
                "background",
                "vs_bg",
                "fs_bg",
                &[],
                false,
                C::Always,
                None,
                None,
            ),
            mesh: make(
                "mesh",
                "vs_mesh",
                "fs_mesh",
                &[Some(mesh_layout.clone())],
                true,
                C::Less,
                None,
                None,
            ),
            translucent: make(
                "translucent",
                "vs_translucent",
                "fs_translucent",
                &[Some(mesh_layout.clone())],
                false,
                C::LessEqual,
                blend,
                None,
            ),
            lines: make(
                "lines",
                "vs_line",
                "fs_line",
                &[Some(line_layout)],
                false,
                C::LessEqual,
                blend,
                None,
            ),
            points: make(
                "points",
                "vs_point",
                "fs_point",
                &[Some(point_layout)],
                false,
                C::LessEqual,
                blend,
                None,
            ),
            globals_layout,
        }
    }
}

/// A [`RenderScene`] uploaded to GPU buffers.
pub struct GpuScene {
    mesh_vb: Option<wgpu::Buffer>,
    mesh_ib: Option<wgpu::Buffer>,
    mesh_count: u32,
    tr_vb: Option<wgpu::Buffer>,
    tr_ib: Option<wgpu::Buffer>,
    tr_count: u32,
    lines: Option<wgpu::Buffer>,
    line_count: u32,
    points: Option<wgpu::Buffer>,
    point_count: u32,
}

fn buf(
    device: &wgpu::Device,
    label: &str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> Option<wgpu::Buffer> {
    if bytes.is_empty() {
        return None;
    }
    Some(
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage,
        }),
    )
}

impl GpuScene {
    pub fn empty() -> Self {
        GpuScene {
            mesh_vb: None,
            mesh_ib: None,
            mesh_count: 0,
            tr_vb: None,
            tr_ib: None,
            tr_count: 0,
            lines: None,
            line_count: 0,
            points: None,
            point_count: 0,
        }
    }

    pub fn upload(device: &wgpu::Device, s: &RenderScene) -> Self {
        let v = wgpu::BufferUsages::VERTEX;
        let i = wgpu::BufferUsages::INDEX;
        GpuScene {
            mesh_vb: buf(device, "mesh vb", bytemuck::cast_slice(&s.mesh.vertices), v),
            mesh_ib: buf(device, "mesh ib", bytemuck::cast_slice(&s.mesh.indices), i),
            mesh_count: s.mesh.indices.len() as u32,
            tr_vb: buf(
                device,
                "translucent vb",
                bytemuck::cast_slice(&s.translucent.vertices),
                v,
            ),
            tr_ib: buf(
                device,
                "translucent ib",
                bytemuck::cast_slice(&s.translucent.indices),
                i,
            ),
            tr_count: s.translucent.indices.len() as u32,
            lines: buf(device, "lines", bytemuck::cast_slice(&s.lines), v),
            line_count: s.lines.len() as u32,
            points: buf(device, "points", bytemuck::cast_slice(&s.points), v),
            point_count: s.points.len() as u32,
        }
    }
}

/// Offscreen MSAA colour + depth, resolved into a single-sample texture.
pub struct Target {
    pub size: [u32; 2],
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    pub resolve: wgpu::Texture,
    pub resolve_view: wgpu::TextureView,
}

impl Target {
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let tex = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let att = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let msaa = tex("viewport msaa", COLOR_FORMAT, SAMPLE_COUNT, att);
        let depth = tex("viewport depth", DEPTH_FORMAT, SAMPLE_COUNT, att);
        let resolve = tex(
            "viewport resolve",
            COLOR_FORMAT,
            1,
            att | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        Target {
            size,
            msaa_view: msaa.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            resolve_view: resolve.create_view(&Default::default()),
            resolve,
        }
    }
}

/// The renderer: pipelines + per-frame uniforms + the offscreen target.
pub struct Renderer {
    pipelines: Pipelines,
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    pub target: Option<Target>,
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let pipelines = Pipelines::new(device);
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &pipelines.globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        Renderer {
            pipelines,
            globals,
            globals_bg,
            target: None,
        }
    }

    /// (Re)create the target if the size changed. Returns true if it did.
    pub fn ensure_target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        let size = [size[0].max(1), size[1].max(1)];
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        self.target = Some(Target::new(device, size));
        true
    }

    /// Record a full viewport draw of `layers` (drawn in order: all opaque
    /// meshes, then translucent, lines, points) into the target.
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layers: &[&GpuScene],
        cam: &CameraMatrices,
        scene_radius: f32,
        opts: &RenderOptions,
    ) {
        let Some(target) = &self.target else { return };
        let g = Globals {
            view: cam.view_f32().to_cols_array_2d(),
            proj: cam.proj_f32().to_cols_array_2d(),
            viewport: [
                target.size[0] as f32,
                target.size[1] as f32,
                opts.pixels_per_point,
                cam.near as f32,
            ],
            params: [
                if cam.ortho { 1.0 } else { 0.0 },
                opts.overlay_bias,
                if opts.shade == ShadeMode::Flat {
                    1.0
                } else {
                    0.0
                },
                scene_radius.max(1e-3),
            ],
            bg_top: linear4(opts.background_top),
            bg_bottom: linear4(opts.background_bottom),
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.msaa_view,
                depth_slice: None,
                resolve_target: Some(&target.resolve_view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.globals_bg, &[]);
        pass.set_pipeline(&self.pipelines.bg);
        pass.draw(0..3, 0..1);

        pass.set_pipeline(&self.pipelines.mesh);
        for l in layers {
            if let (Some(vb), Some(ib)) = (&l.mesh_vb, &l.mesh_ib) {
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..l.mesh_count, 0, 0..1);
            }
        }
        pass.set_pipeline(&self.pipelines.translucent);
        for l in layers {
            if let (Some(vb), Some(ib)) = (&l.tr_vb, &l.tr_ib) {
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..l.tr_count, 0, 0..1);
            }
        }
        pass.set_pipeline(&self.pipelines.lines);
        for l in layers {
            if let Some(b) = &l.lines {
                pass.set_vertex_buffer(0, b.slice(..));
                pass.draw(0..6, 0..l.line_count);
            }
        }
        pass.set_pipeline(&self.pipelines.points);
        for l in layers {
            if let Some(b) = &l.points {
                pass.set_vertex_buffer(0, b.slice(..));
                pass.draw(0..6, 0..l.point_count);
            }
        }
    }
}

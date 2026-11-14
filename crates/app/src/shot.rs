//! Headless UI screenshots: runs the real app UI against an offscreen
//! wgpu target (egui_wgpu renderer + the viewport paint callback) and
//! writes PNGs. No window, no user settings read or written.
//!
//! `newtopo --shot <dir>`

use crate::app::App;
use crate::prefs::Paths;
use crate::selection::{EdgeSel, Mode};
use crate::tools::Tool;
use anyhow::{Context as _, anyhow};
use egui::{Event, Pos2, RawInput, Rect, ViewportId, ViewportInfo, pos2, vec2};
use newtopo_kernel::{BodyId, FaceId};
use std::path::Path;

const SIZE: [f32; 2] = [1440.0, 900.0];
const PPP: f32 = 1.25;

struct Shooter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    _headless: newtopo_render::Headless,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Shooter {
    fn new() -> anyhow::Result<Self> {
        let headless = newtopo_render::Headless::new()?;
        let device = headless.device().clone();
        let queue = headless.queue().clone();
        let mut renderer = egui_wgpu::Renderer::new(
            &device,
            FORMAT,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: false,
                ..Default::default()
            },
        );
        newtopo_render::egui_viewport::install_with(
            &device,
            FORMAT,
            &mut renderer.callback_resources,
        );
        Ok(Shooter {
            device,
            queue,
            renderer,
            _headless: headless,
        })
    }

    fn raw(&self, t: f64, events: Vec<Event>) -> RawInput {
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(SIZE[0], SIZE[1]))),
            time: Some(t),
            predicted_dt: 1.0 / 60.0,
            events,
            focused: true,
            ..Default::default()
        };
        raw.viewports.insert(
            ViewportId::ROOT,
            ViewportInfo {
                native_pixels_per_point: Some(PPP),
                focused: Some(true),
                ..Default::default()
            },
        );
        raw
    }

    /// Run `frames` frames with per-frame events, then render the last one.
    fn shoot(
        &mut self,
        ctx: &egui::Context,
        app: &mut App,
        frames: Vec<Vec<Event>>,
        out: &Path,
    ) -> anyhow::Result<()> {
        let mut last = None;
        let mut pending_free = Default::default();
        let n = frames.len();
        for (i, ev) in frames.into_iter().enumerate() {
            let mut full = ctx.run_ui(self.raw(i as f64 * 0.06 + 0.5, ev), |ui| app.frame(ui));
            for (id, deltas) in &full.textures_delta.set {
                for delta in deltas {
                    self.renderer
                        .update_texture(&self.device, &self.queue, *id, delta);
                }
            }
            let free = full.textures_delta.free.clone();
            full.textures_delta.clear();
            if i + 1 < n {
                for id in &free {
                    self.renderer.free_texture(id);
                }
            } else {
                pending_free = free;
            }
            last = Some(full);
        }
        let full = last.ok_or_else(|| anyhow!("no frames"))?;
        let ppp = full.pixels_per_point;
        let jobs = ctx.tessellate(full.shapes, ppp);
        let size = [
            (SIZE[0] * PPP).round() as u32,
            (SIZE[1] * PPP).round() as u32,
        ];
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: ppp,
        };
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui shot"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        let extra =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut enc, &jobs, &screen);
        {
            let bg = app.st.pal.bg.to_normalized_gamma_f32();
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui shot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0] as f64,
                            g: bg[1] as f64,
                            b: bg[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        let bpr = (size[0] * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui readback"),
            size: (bpr * size[1]) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
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
        self.queue.submit(extra.into_iter().chain([enc.finish()]));
        let (tx, rx) = std::sync::mpsc::channel();
        buf.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| anyhow!("poll: {e}"))?;
        rx.recv()?.map_err(|e| anyhow!("map: {e}"))?;
        let mut px = Vec::with_capacity((size[0] * size[1] * 4) as usize);
        {
            let data = buf.get_mapped_range(..).map_err(|e| anyhow!("{e:?}"))?;
            for row in 0..size[1] {
                let s = (row * bpr) as usize;
                px.extend_from_slice(&data[s..s + (size[0] * 4) as usize]);
            }
        }
        buf.unmap();
        for id in &pending_free {
            self.renderer.free_texture(id);
        }
        let img = image::RgbaImage::from_raw(size[0], size[1], px)
            .ok_or_else(|| anyhow!("image size"))?;
        img.save(out)
            .with_context(|| format!("write {}", out.display()))?;
        eprintln!("wrote {}", out.display());
        Ok(())
    }
}

fn frames(n: usize) -> Vec<Vec<Event>> {
    vec![vec![]; n]
}

fn pointer_frames(n: usize, at: Pos2) -> Vec<Vec<Event>> {
    (0..n).map(|_| vec![Event::PointerMoved(at)]).collect()
}

/// Every scenario: (file name, setup, frames).
type Setup = Box<dyn Fn(&mut App)>;

pub fn run(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut s = Shooter::new()?;
    let viewport_centre = pos2(SIZE[0] * 0.5, SIZE[1] * 0.5);
    let scenarios: Vec<(&str, Setup, Vec<Vec<Event>>)> = vec![
        ("01-empty.png", Box::new(|_a: &mut App| {}), frames(6)),
        (
            "02-sample-connect.png",
            Box::new(|a: &mut App| {
                a.st.load_sample();
                a.st.mode = Mode::Face;
                for f in [3u32, 4, 5] {
                    a.st.sel.faces.insert((BodyId(0), FaceId(f)));
                }
                a.st.tool.active = Tool::Connect;
                a.st.tool.seg.a = 16;
            }),
            pointer_frames(8, pos2(720.0, 380.0)),
        ),
        (
            "03-palette.png",
            Box::new(|a: &mut App| {
                a.st.load_sample();
                a.st.ui.palette.open_fresh();
                a.st.ui.palette.query = "sel".into();
            }),
            frames(8),
        ),
        (
            "04-radial.png",
            Box::new(move |a: &mut App| {
                a.st.load_sample();
                a.st.ui.radial.open_at(viewport_centre, 0.0, true);
            }),
            pointer_frames(6, pos2(SIZE[0] * 0.5 + 60.0, SIZE[1] * 0.5 - 100.0)),
        ),
        (
            "05-preferences.png",
            Box::new(|a: &mut App| {
                a.st.load_sample();
                a.st.ui.prefs_open = true;
                a.st.ui.prefs_tab = crate::prefs_ui::PrefsTab::Appearance;
            }),
            frames(8),
        ),
        (
            "06-preview-wire.png",
            Box::new(|a: &mut App| {
                a.st.load_sample();
                a.st.view.preview = true;
                a.st.view.wireframe = true;
                a.st.sel.objects.insert(BodyId(1));
            }),
            frames(6),
        ),
        (
            "07-light-edges.png",
            Box::new(|a: &mut App| {
                a.st.set_theme("Paper (light)");
                a.st.load_sample();
                a.st.mode = Mode::Edge;
                // A loop around the boss: select one edge, then walk the loop.
                a.st.ensure_view_data();
                // A horizontal edge half way up the boss.
                let pick = a.st.vd.edges.iter().position(|e| {
                    let (p, q) = (e.path[0], e.path[e.path.len() - 1]);
                    e.body == BodyId(0) && (p.z - q.z).abs() < 1e-3 && p.z > 20.0 && p.z < 40.0
                });
                if let Some(e) = pick {
                    if let Some(sel) = a.st.vd.edges[e].sel.clone() {
                        a.st.sel.edges.insert(sel);
                    }
                    a.st.select_loop(false, false);
                }
                let _ = EdgeSel::Index(BodyId(0), 0);
            }),
            frames(6),
        ),
        (
            "08-keymap.png",
            Box::new(|a: &mut App| {
                a.st.load_sample();
                a.st.ui.prefs_open = true;
                a.st.ui.prefs_tab = crate::prefs_ui::PrefsTab::Keymap;
            }),
            frames(6),
        ),
        (
            "09-studio-rename.png",
            Box::new(|a: &mut App| {
                a.st.load_sample();
                a.st.sel.objects.insert(BodyId(0));
                a.st.sel.objects.insert(BodyId(1));
                a.st.ui.rename.kind = crate::studio::RenameKind::Number;
                a.st.ui
                    .dock_actions
                    .push(crate::app::DockAction::Focus(crate::app::Tab::Rename));
            }),
            frames(8),
        ),
    ];
    for (name, setup, fr) in scenarios {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, Paths::default(), true, true);
        setup(&mut app);
        s.shoot(&ctx, &mut app, fr, &dir.join(name))?;
        if std::env::var("NEWTOPO_SHOT_DEBUG").is_ok() && app.st.has_content() {
            let cam = app.st.view.camera;
            eprintln!(
                "camera {cam:?} bounds {:?} rect {:?} base tris {}",
                app.st.vd.bounds,
                app.st.view.rect,
                app.st.vd.base.mesh.indices.len() / 3
            );
            let mut h = newtopo_render::Headless::new()?;
            let img = h.render(
                &[&app.st.vd.base, &app.st.overlay],
                &cam,
                [800, 600],
                &Default::default(),
            )?;
            img.save(dir.join(format!("debug-{name}")))?;
        }
    }
    Ok(())
}

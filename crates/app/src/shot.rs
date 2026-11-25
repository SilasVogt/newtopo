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
    /// Monotonic UI clock shared by every frame this shooter runs.
    clock: f64,
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
            clock: 0.5,
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
            let _ = i;
            self.clock += 0.06;
            let mut full = ctx.run_ui(self.raw(self.clock, ev), |ui| app.frame(ui));
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
        (
            "19-update-ask.png",
            Box::new(|a: &mut App| {
                a.st.load_sample();
                a.st.updates.ask_open = true;
            }),
            frames(6),
        ),
        (
            "20-update-available.png",
            Box::new(|a: &mut App| {
                a.st.load_sample();
                a.st.updates.available = Some(crate::update::Available::for_version(
                    crate::update::Version(0, 2, 0),
                ));
                a.st.updates.dialog_open = true;
            }),
            frames(6),
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

// ---------------------------------------------------------------- real models

/// Run frames (no rendering) until `done` or a timeout; background jobs
/// (import/export threads) need real time, so this sleeps between frames.
fn settle(
    s: &mut Shooter,
    ctx: &egui::Context,
    app: &mut App,
    t: &mut f64,
    done: impl Fn(&App) -> bool,
    max_secs: f64,
) -> bool {
    settle_ex(s, ctx, app, t, done, max_secs, true)
}

fn settle_ex(
    s: &mut Shooter,
    ctx: &egui::Context,
    app: &mut App,
    t: &mut f64,
    done: impl Fn(&App) -> bool,
    max_secs: f64,
    upload: bool,
) -> bool {
    let start = std::time::Instant::now();
    loop {
        s.clock += 0.05;
        *t = s.clock;
        let raw = s.raw(s.clock, vec![]);
        let mut out = ctx.run_ui(raw, |ui| app.frame(ui));
        // Textures (the font atlas) must reach the renderer even when this
        // frame isn't drawn — but only for the context being rendered: the
        // renderer keys textures by id, which every context reuses.
        for (id, deltas) in out.textures_delta.set.iter().filter(|_| upload) {
            for d in deltas {
                s.renderer.update_texture(&s.device, &s.queue, *id, d);
            }
        }
        for id in out.textures_delta.free.iter().filter(|_| upload) {
            s.renderer.free_texture(id);
        }
        out.textures_delta.clear();
        if done(app) {
            return true;
        }
        if start.elapsed().as_secs_f64() > max_secs {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Import a model, waiting for the helper; accepts the import list as is.
fn import(
    s: &mut Shooter,
    ctx: &egui::Context,
    app: &mut App,
    t: &mut f64,
    path: &Path,
) -> anyhow::Result<()> {
    app.st.start_import(path.to_path_buf());
    let ok = settle(s, ctx, app, t, |a| !a.st.busy(), 120.0);
    anyhow::ensure!(ok, "import of {} timed out", path.display());
    if app.st.ui.import_list.is_some() {
        app.st.accept_import_list();
        settle(s, ctx, app, t, |_| true, 1.0);
    }
    if let Some(r) = &app.st.ui.import_report {
        anyhow::ensure!(
            r.errors.is_empty(),
            "import of {} failed: {:?}",
            path.display(),
            r.errors
        );
        app.st.ui.import_report = None;
    }
    anyhow::ensure!(
        app.st.doc.has_scene(),
        "no scene after importing {}",
        path.display()
    );
    settle(s, ctx, app, t, |_| true, 1.0);
    Ok(())
}

/// Faces of `body` whose surface is of `kind`.
fn faces_of_kind(app: &App, body: BodyId, kind: newtopo_kernel::SurfaceKind) -> Vec<FaceId> {
    let Some(scene) = app.st.doc.scene.as_ref() else {
        return vec![];
    };
    scene
        .body_view(body)
        .map(|bv| {
            bv.topology
                .faces
                .iter()
                .enumerate()
                .filter(|(_, f)| f.surface.kind == kind)
                .map(|(i, _)| FaceId(i as u32))
                .collect()
        })
        .unwrap_or_default()
}

fn first_body(app: &App) -> BodyId {
    app.st
        .doc
        .scene
        .as_ref()
        .and_then(|s| s.doc().bodies.first().map(|b| b.id))
        .unwrap_or_default()
}

/// Screenshots and an end-to-end check on the reference STEP models
/// (`newtopo-testmodels <dir>`): import, tools, preview, export, save/open.
pub fn run_real(dir: &Path, models: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut s = Shooter::new()?;
    let tmp = dir.join("e2e");
    std::fs::create_dir_all(&tmp)?;
    let new_app = || {
        let ctx = egui::Context::default();
        let app = App::new(&ctx, Paths::default(), true, true);
        (ctx, app, 1.0f64)
    };

    // 1. Filleted box: object mode, Slice ×10.
    {
        let (ctx, mut app, mut t) = new_app();
        import(
            &mut s,
            &ctx,
            &mut app,
            &mut t,
            &models.join("filleted_box.step"),
        )?;
        let b = first_body(&app);
        app.st.mode = Mode::Object;
        app.st.sel.objects.insert(b);
        app.st.tool.seg.slice = 10;
        app.st.activate_tool(Tool::Slice);
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 1.0);
        let stats = app.st.doc.scene.as_mut().map(|s| s.stats());
        eprintln!(
            "filleted_box after slice: {stats:?}, lines {:?}",
            app.st.tool.last_lines_added
        );
        s.shoot(
            &ctx,
            &mut app,
            frames(6),
            &dir.join("10-filleted-box-slice.png"),
        )?;
        // Apply the previewed Slice (nothing is written before Apply), then
        // save and reopen: same output.
        app.st.apply_preview();
        let p = tmp.join("filleted_box.ntopo");
        app.st.doc.save(&p).map_err(anyhow::Error::msg)?;
        let before = app.st.doc.scene.as_mut().map(|s| s.stats());
        let (ctx2, mut app2, mut t2) = new_app();
        app2.st.open_scene(Some(p.clone()));
        settle_ex(&mut s, &ctx2, &mut app2, &mut t2, |_| true, 1.0, false);
        let after = app2.st.doc.scene.as_mut().map(|s| s.stats());
        anyhow::ensure!(
            before == after,
            "save/open changed the output: {before:?} vs {after:?}"
        );
        // Export OBJ and FBX through the app's export path.
        for (fmt, ext) in [
            (newtopo_core::export::Format::Obj, "obj"),
            (newtopo_core::export::Format::Fbx, "fbx"),
            (newtopo_core::export::Format::Glb, "glb"),
        ] {
            let out = tmp.join(format!("filleted_box.{ext}"));
            app.st.export_to(
                out.clone(),
                crate::jobs::ExportDialog {
                    format: fmt,
                    opts: Default::default(),
                    units: "mm".into(),
                },
            );
            settle(&mut s, &ctx, &mut app, &mut t, |a| !a.st.busy(), 60.0);
            anyhow::ensure!(
                out.metadata().map(|m| m.len() > 100).unwrap_or(false),
                "export {ext} missing"
            );
        }
        // Palette over the real scene.
        app.st.ui.palette.open_fresh();
        app.st.ui.palette.query = "slice".into();
        s.shoot(&ctx, &mut app, frames(8), &dir.join("12-palette-real.png"))?;
        app.st.ui.palette.open = false;
        // Export dialog.
        app.st.open_export_dialog();
        s.shoot(&ctx, &mut app, frames(8), &dir.join("13-export-dialog.png"))?;
        app.st.ui.export = None;
    }

    // 2. Plate with holes: Connect on the hole walls, preview with auto-fill.
    {
        let (ctx, mut app, mut t) = new_app();
        import(
            &mut s,
            &ctx,
            &mut app,
            &mut t,
            &models.join("plate_holes.step"),
        )?;
        let b = first_body(&app);
        let walls = faces_of_kind(&app, b, newtopo_kernel::SurfaceKind::Cylinder);
        eprintln!("plate_holes: {} cylinder faces", walls.len());
        app.st.mode = Mode::Face;
        for f in &walls {
            app.st.sel.faces.insert((b, *f));
        }
        app.st.tool.seg.a = 8;
        app.st.activate_tool(Tool::Connect);
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 1.0);
        // Get closer: frame a few hole walls, look in at a lower angle.
        let keep = app.st.sel.faces.clone();
        app.st.sel.faces = keep.iter().take(6).cloned().collect();
        app.st.frame_selection();
        app.st.sel.faces = keep;
        app.st.view.camera.pitch = 0.62;
        app.st.view.camera.yaw += 0.25;
        s.shoot(
            &ctx,
            &mut app,
            frames(6),
            &dir.join("11a-plate-connect.png"),
        )?;
        app.st.view.preview = true;
        app.st.view.wireframe = true;
        app.st.tool.active = Tool::Select;
        app.st.sel.faces.clear();
        let stats = app.st.doc.scene.as_mut().map(|s| s.stats());
        eprintln!("plate_holes preview: {stats:?}");
        s.shoot(
            &ctx,
            &mut app,
            frames(6),
            &dir.join("11-plate-connect-preview.png"),
        )?;
        // Keep the Connect, then switch auto-fill on: it previews as
        // Before | After with the same camera (SPEC §5.10); keep it on for
        // the steps below.
        app.st.apply_preview();
        app.st.run(crate::commands::Cmd::AutoFill);
        anyhow::ensure!(
            app.st.doc.in_preview() && app.st.view.compare,
            "auto-fill opens the compare"
        );
        s.shoot(
            &ctx,
            &mut app,
            frames(10),
            &dir.join("19-autofill-compare.png"),
        )?;
        app.st.apply_preview();
        // Edge mode: loop on a Connect line, then remove it as one step.
        app.st.view.preview = false;
        app.st.mode = Mode::Edge;
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        if let Some(e) = app
            .st
            .vd
            .edges
            .iter()
            .find(|e| e.kind == crate::viewdata::EdgeKind::Line)
            .and_then(|e| e.sel.clone())
        {
            app.st.sel.edges.insert(e);
            app.st.select_loop(false, false);
            let n = app.st.sel.edges.len();
            let rev = app.st.doc.revision;
            app.st.remove_selected();
            eprintln!(
                "plate_holes: removed a loop of {n} edges (revision {rev} -> {})",
                app.st.doc.revision
            );
            app.st.undo();
        }
    }

    // 2b. Single-line tools and edits on the filleted box.
    {
        let (ctx, mut app, mut t) = new_app();
        import(
            &mut s,
            &ctx,
            &mut app,
            &mut t,
            &models.join("filleted_box.step"),
        )?;
        let b = first_body(&app);
        let planes = faces_of_kind(&app, b, newtopo_kernel::SurfaceKind::Plane);
        // Pick the planar faces the default iso camera sees: top, +X, -Y.
        let normal_of = |app: &App, f: FaceId| -> glam::DVec3 {
            app.st
                .doc
                .scene
                .as_ref()
                .and_then(|s| {
                    s.body_view(b).and_then(|bv| {
                        bv.face_meshes
                            .get(f.idx())
                            .and_then(|m| m.normals.first().copied())
                    })
                })
                .unwrap_or_default()
        };
        let best = |app: &App, dir: glam::DVec3| {
            *planes
                .iter()
                .max_by(|x, y| {
                    normal_of(app, **x)
                        .dot(dir)
                        .total_cmp(&normal_of(app, **y).dot(dir))
                })
                .unwrap()
        };
        let top = best(&app, glam::DVec3::Z);
        let side_x = best(&app, glam::DVec3::X);
        let side_y = best(&app, glam::DVec3::NEG_Y);
        let planes = [top, side_x, side_y];
        let report = |app: &mut App, what: &str, rev0: u64| {
            let last = app
                .st
                .toasts
                .items
                .last()
                .map(|t| t.text.clone())
                .unwrap_or_default();
            let ok = app.st.doc.revision != rev0;
            eprintln!(
                "{what}: {} {}",
                if ok { "applied" } else { "NOT applied" },
                if ok { String::new() } else { last }
            );
            ok
        };
        // Grid on a planar face.
        app.st.mode = Mode::Face;
        app.st.sel.faces.insert((b, top));
        app.st.tool.seg.u = 4;
        app.st.tool.seg.v = 3;
        let r0 = app.st.doc.revision;
        app.st.activate_tool(Tool::Grid);
        report(&mut app, "grid 4x3", r0);
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        // Iso through the centre of another planar face.
        let f2 = planes[1];
        let c = app.st.vd.face_centers[&newtopo_render::pick::FaceRef {
            body: b.0,
            face: f2.0,
        }];
        app.st.tool.active = Tool::Iso;
        app.st.hover = Some(crate::viewport::Hover::Face(
            newtopo_render::pick::FaceRef {
                body: b.0,
                face: f2.0,
            },
            c.as_dvec3(),
        ));
        app.st.on_hover_changed();
        let r0 = app.st.doc.revision;
        app.st.iso_click();
        report(&mut app, "iso", r0);
        app.st.tool.iso_split = true;
        app.st.tool.iso_dir = crate::tools::IsoDirSel::V;
        app.st.on_hover_changed();
        let r0 = app.st.doc.revision;
        app.st.iso_click();
        report(&mut app, "iso split", r0);
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        // Cut across a third planar face (two face points).
        let f3 = planes[2];
        let fr = newtopo_render::pick::FaceRef {
            body: b.0,
            face: f3.0,
        };
        let centre = app.st.vd.face_centers[&fr];
        let tris = &app.st.vd.face_tris[&fr];
        let corner = app.st.vd.pick.positions[app.st.vd.pick.tris[tris[0] as usize][0] as usize];
        let pts = [centre.lerp(corner, 0.6), centre.lerp(corner, -0.6)];
        app.st.tool.active = Tool::Cut;
        app.st.tool.cut.clear();
        for p in pts {
            if let Some(uv) = app.st.face_uv(b, f3, p.as_dvec3()) {
                app.st.tool.cut.push(crate::tools::CutPick {
                    point: newtopo_core::doc::CutPoint::Face {
                        body: b,
                        face: f3,
                        uv,
                    },
                    pos: p,
                });
            }
        }
        let r0 = app.st.doc.revision;
        app.st.cut_finish(false);
        report(&mut app, "cut", r0);
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        // Keep the setup: the undos below take back one edit each.
        app.st.apply_preview();
        // Collapse and remove a generated edge; weld two generated vertices.
        app.st.tool.active = Tool::Select;
        app.st.mode = Mode::Edge;
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        let lines: Vec<_> = app
            .st
            .vd
            .edges
            .iter()
            .filter(|e| e.kind == crate::viewdata::EdgeKind::Line)
            .filter_map(|e| e.sel.clone())
            .collect();
        eprintln!("generated edges: {}", lines.len());
        if let Some(e) = lines.first() {
            app.st.sel.edges = [e.clone()].into();
            let r0 = app.st.doc.revision;
            app.st.collapse_selected();
            report(&mut app, "collapse", r0);
            app.st.apply_preview();
            app.st.undo();
        }
        if lines.len() > 3 {
            app.st.sel.edges = lines[1..4].iter().cloned().collect();
            let r0 = app.st.doc.revision;
            app.st.remove_selected();
            report(&mut app, "remove 3 edges (one step)", r0);
            app.st.apply_preview();
            let r1 = app.st.doc.revision;
            app.st.undo();
            eprintln!(
                "undo after remove: revision {r1} -> {}",
                app.st.doc.revision
            );
        }
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        app.st.mode = Mode::Vertex;
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        let gv: Vec<usize> = (0..app.st.vd.verts.len())
            .filter(|&i| matches!(&app.st.vd.verts[i].sel, crate::selection::VertSel::Key(_, k) if !k.is_cad()))
            .collect();
        anyhow::ensure!(gv.len() >= 2, "the applied setup survives the undos");
        if gv.len() >= 2 {
            // The two closest generated vertices.
            let mut best = (f32::MAX, 0, 0);
            for (x, &i) in gv.iter().enumerate() {
                for &j in &gv[x + 1..] {
                    let d = app.st.vd.verts[i].pos.distance(app.st.vd.verts[j].pos);
                    if d < best.0 {
                        best = (d, i, j);
                    }
                }
            }
            let r0 = app.st.doc.revision;
            app.st.weld(best.1, best.2);
            if let Some(c) = app.st.ui.confirm.take() {
                eprintln!("weld asks for confirmation: {}", c.body);
                app.st
                    .apply_group(c.ops.into_iter().map(crate::jobs::with_confirmed).collect());
            }
            report(&mut app, "weld", r0);
        }
        // Preview of everything.
        app.st.mode = Mode::Object;
        app.st.view.preview = true;
        app.st.view.wireframe = true;
        let issues = app
            .st
            .doc
            .scene
            .as_mut()
            .map(|sc| sc.validate_all())
            .unwrap_or_default();
        eprintln!(
            "filleted box after tools: {} validation issues",
            issues.len()
        );
        for (_, i) in issues.iter().take(5) {
            eprintln!("  issue: {}", newtopo_core::scene::issue_reason(i));
        }
        s.shoot(
            &ctx,
            &mut app,
            frames(6),
            &dir.join("16-filleted-box-tools-preview.png"),
        )?;
    }

    // 3. Assembly: the import list.
    {
        let (ctx, mut app, mut t) = new_app();
        app.st.start_import(models.join("assembly.step"));
        let ok = settle(&mut s, &ctx, &mut app, &mut t, |a| !a.st.busy(), 120.0);
        anyhow::ensure!(ok, "assembly import timed out");
        anyhow::ensure!(
            app.st.ui.import_list.is_some(),
            "assembly should open the import list"
        );
        if let Some(l) = &mut app.st.ui.import_list {
            if let Some(r) = l.rows.get_mut(1) {
                r.choice = crate::jobs::ImportChoice::Exclude;
            }
            if let Some(r) = l.rows.get_mut(0) {
                r.selected = true;
            }
        }
        s.shoot(&ctx, &mut app, frames(8), &dir.join("14-import-list.png"))?;
        app.st.accept_import_list();
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 1.0);
        app.st.ui.import_report = None;
        app.st.set_mode(Mode::Object);
        s.shoot(&ctx, &mut app, frames(6), &dir.join("15-assembly.png"))?;
    }

    // 4. Link on the plate: Connect on one hole wall reaches all 16.
    {
        let (ctx, mut app, mut t) = new_app();
        import(
            &mut s,
            &ctx,
            &mut app,
            &mut t,
            &models.join("plate_holes.step"),
        )?;
        let b = first_body(&app);
        let walls = faces_of_kind(&app, b, newtopo_kernel::SurfaceKind::Cylinder);
        app.st.apply(newtopo_core::ops::Op::SetLink { on: true });
        app.st.mode = Mode::Face;
        app.st.sel.faces.insert((b, walls[0]));
        app.st.tool.seg.a = 8;
        app.st.activate_tool(Tool::Connect);
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 1.0);
        let lines = app
            .st
            .vd
            .edges
            .iter()
            .filter(|e| e.kind == crate::viewdata::EdgeKind::Line)
            .count();
        let partners = app.st.partner_faces().len();
        eprintln!(
            "link: Connect on 1 wall → {lines} generated edges; {partners} linked walls tinted"
        );
        anyhow::ensure!(
            partners == walls.len() - 1,
            "link set should cover every hole wall"
        );
        app.st.view.camera.pitch = 0.62;
        app.st.view.camera.yaw += 0.25;
        s.shoot(&ctx, &mut app, frames(6), &dir.join("17-plate-link.png"))?;
        // Tab flips A/B on the hovered patch (FlipConnect).
        let c = app.st.vd.face_centers[&newtopo_render::pick::FaceRef {
            body: b.0,
            face: walls[0].0,
        }];
        app.st.hover = Some(crate::viewport::Hover::Face(
            newtopo_render::pick::FaceRef {
                body: b.0,
                face: walls[0].0,
            },
            c.as_dvec3(),
        ));
        let r0 = app.st.doc.revision;
        app.st.tool_key(egui::Key::Tab, egui::Modifiers::NONE);
        eprintln!(
            "flip A/B: {}",
            if app.st.doc.revision != r0 {
                "applied"
            } else {
                "not applied"
            }
        );
        // Layer ops: create, move, hide, then undo them.
        let r0 = app.st.doc.revision;
        let id = app
            .st
            .apply(newtopo_core::ops::Op::CreateLayer {
                name: "Holes".into(),
            })
            .and_then(|o| o.created);
        if let Some(id) = id {
            let l = newtopo_core::doc::LayerId(id);
            app.st
                .apply(newtopo_core::ops::Op::MoveBodyToLayer { body: b, layer: l });
            app.st.apply(newtopo_core::ops::Op::SetLayerVisible {
                layer: l,
                visible: false,
            });
            app.st.ensure_view_data();
            let hidden = app.st.effective_hidden().contains(&b);
            eprintln!("layers: created {id}, moved, hidden → body hidden: {hidden}");
            for _ in 0..3 {
                app.st.undo();
            }
        }
        eprintln!("layers: revision {r0} → {} after undo", app.st.doc.revision);
    }

    // 5. Copy/paste rules between the two posts of the assembly; Replace.
    {
        let (ctx, mut app, mut t) = new_app();
        import(
            &mut s,
            &ctx,
            &mut app,
            &mut t,
            &models.join("assembly.step"),
        )?;
        let bodies: Vec<(String, BodyId)> = app
            .st
            .doc
            .scene
            .as_ref()
            .map(|s| {
                s.doc()
                    .bodies
                    .iter()
                    .map(|b| (b.name.clone(), b.id))
                    .collect()
            })
            .unwrap_or_default();
        let id = |n: &str| bodies.iter().find(|b| b.0 == n).map(|b| b.1);
        let (Some(left), Some(right)) = (id("Post left"), id("Post right")) else {
            anyhow::bail!("assembly posts not found: {bodies:?}");
        };
        let walls = faces_of_kind(&app, left, newtopo_kernel::SurfaceKind::Cylinder);
        app.st.mode = Mode::Face;
        app.st.sel.faces = walls.iter().map(|f| (left, *f)).collect();
        app.st.tool.seg.a = 12;
        app.st.activate_tool(Tool::Connect);
        app.st.tool.active = Tool::Select;
        app.st.sel.faces.clear();
        app.st.mode = Mode::Object;
        app.st.sel.objects = [left].into();
        app.st.copy_rules();
        app.st.sel.objects = [right].into();
        let r0 = app.st.doc.revision;
        app.st.paste_rules();
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 1.0);
        let lines_on = |app: &App, b: BodyId| {
            app.st
                .vd
                .edges
                .iter()
                .filter(|e| e.body == b && e.kind == crate::viewdata::EdgeKind::Line)
                .count()
        };
        eprintln!(
            "paste: revision {r0} → {}; generated edges left {} right {}; last toast: {:?}",
            app.st.doc.revision,
            lines_on(&app, left),
            lines_on(&app, right),
            app.st.toasts.items.last().map(|t| t.text.clone())
        );
        anyhow::ensure!(
            lines_on(&app, right) > 0 && lines_on(&app, right) == lines_on(&app, left),
            "paste should copy the posts' lines"
        );
        app.st.sel.objects = [left, right].into();
        s.shoot(
            &ctx,
            &mut app,
            frames(6),
            &dir.join("18-assembly-paste.png"),
        )?;
        // Re-import with Replace (the default for same-named objects).
        app.st.start_import(models.join("assembly.step"));
        settle(&mut s, &ctx, &mut app, &mut t, |a| !a.st.busy(), 120.0);
        let all_replace = app.st.ui.import_list.as_ref().is_some_and(|l| {
            l.rows
                .iter()
                .all(|r| r.choice == crate::jobs::ImportChoice::Replace)
        });
        app.st.accept_import_list();
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 1.0);
        // The paste preview is still open, so the import is held: nothing
        // is added to a scene that Discard could roll back.
        anyhow::ensure!(
            app.st.doc.in_preview() && app.st.ui.import_report.is_none(),
            "an import waits for the open preview"
        );
        app.st.apply_preview();
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 1.0);
        let report = app.st.ui.import_report.take();
        anyhow::ensure!(
            report
                .as_ref()
                .is_some_and(|r| r.bodies > 0 && r.errors.is_empty()),
            "the held import runs once the preview is applied"
        );
        eprintln!(
            "replace: list defaulted to Replace: {all_replace}; report: {:?}",
            report.map(|r| (r.bodies, r.errors, r.warnings))
        );
        let n = app
            .st
            .doc
            .scene
            .as_ref()
            .map(|s| s.doc().bodies.len())
            .unwrap_or(0);
        eprintln!(
            "replace: {n} bodies after re-import; right post lines {}",
            lines_on(
                &app,
                app.st
                    .doc
                    .scene
                    .as_ref()
                    .and_then(|s| s
                        .doc()
                        .bodies
                        .iter()
                        .find(|b| b.name == "Post right")
                        .map(|b| b.id))
                    .unwrap_or_default()
            )
        );
    }
    // 6. A face selection across two bodies: one Connect per body, one step.
    {
        let (ctx, mut app, mut t) = new_app();
        import(
            &mut s,
            &ctx,
            &mut app,
            &mut t,
            &models.join("assembly.step"),
        )?;
        let id = |app: &App, n: &str| {
            app.st
                .doc
                .scene
                .as_ref()
                .and_then(|s| s.doc().bodies.iter().find(|b| b.name == n).map(|b| b.id))
        };
        let (Some(left), Some(right)) = (id(&app, "Post left"), id(&app, "Post right")) else {
            anyhow::bail!("assembly posts not found");
        };
        app.st.mode = Mode::Face;
        for b in [left, right] {
            for f in faces_of_kind(&app, b, newtopo_kernel::SurfaceKind::Cylinder) {
                app.st.sel.faces.insert((b, f));
            }
        }
        app.st.tool.seg.a = 6;
        let r0 = app.st.doc.revision;
        app.st.activate_tool(Tool::Connect);
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        let lines_on = |app: &App, b: BodyId| {
            app.st
                .vd
                .edges
                .iter()
                .filter(|e| e.body == b && e.kind == crate::viewdata::EdgeKind::Line)
                .count()
        };
        let (l, r) = (lines_on(&app, left), lines_on(&app, right));
        eprintln!(
            "two-body connect: revision {r0} → {}; generated edges left {l} right {r}",
            app.st.doc.revision
        );
        anyhow::ensure!(l > 0 && l == r, "Connect should reach both bodies");
        // Apply, so Connect is its own undo step.
        app.st.apply_preview();
        // A wheel tick changes both in the same gesture.
        app.st.wheel_count(1.0, true);
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.6);
        let (l2, r2) = (lines_on(&app, left), lines_on(&app, right));
        eprintln!("two-body wheel tick: left {l2} right {r2}");
        anyhow::ensure!(l2 > l && l2 == r2, "wheel tick should change both bodies");
        app.st.apply_preview();
        // Two undos bring both back to nothing: the gesture, then Connect.
        app.st.undo();
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        anyhow::ensure!(
            lines_on(&app, left) == l && lines_on(&app, right) == l,
            "the first undo takes back only the wheel tick"
        );
        app.st.undo();
        settle(&mut s, &ctx, &mut app, &mut t, |_| true, 0.5);
        anyhow::ensure!(
            app.st.vd.edges.iter().any(|e| e.body == left)
                && app.st.vd.edges.iter().any(|e| e.body == right),
            "the imported bodies are still there"
        );
        anyhow::ensure!(
            lines_on(&app, left) == 0 && lines_on(&app, right) == 0,
            "each step covered both bodies"
        );
    }
    Ok(())
}

// Release builds on Windows are GUI apps without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! newtopo desktop app (SPEC §5): eframe + wgpu, docked panels.
//!
//! Flags:
//! * `--shot <dir>`: render a set of UI screenshots headless (no window) and exit.
//! * `--sample`: start with the synthetic sample model loaded.
//! * a path to a `.ntopo` or CAD file opens/imports it.

mod app;
mod commands;
mod docstate;
mod icons;
mod jobs;
mod keymap;
mod kit;
mod launch;
mod palette;
mod panels;
mod prefs;
mod prefs_ui;
mod report;
mod selection;
mod shot;
mod strings;
mod studio;
mod theme;
mod toasts;
mod tools;
#[cfg(test)]
mod uitest;
mod update;
mod viewdata;
mod viewport;

use std::path::PathBuf;

struct EframeApp {
    app: app::App,
    /// `--window-shot <png>`: capture the real window after a few frames and quit.
    window_shot: Option<PathBuf>,
    frames: u32,
}

impl eframe::App for EframeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.frame(ui);
        if let Some(path) = self.window_shot.clone() {
            self.frames += 1;
            let ctx = ui.ctx().clone();
            ctx.request_repaint();
            if self.frames == 40 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            let shot = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                let [w, h] = img.size;
                let px: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, px) {
                    let _ = buf.save(&path);
                    eprintln!("wrote {}", path.display());
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if self.frames > 400 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn on_exit(&mut self) {
        self.app.on_exit();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.app.st.pal.bg.to_normalized_gamma_f32()
    }
}

fn icon() -> Option<egui::IconData> {
    let img = image::load_from_memory_with_format(
        include_bytes!("../../../assets/icon.png"),
        image::ImageFormat::Png,
    )
    .ok()?
    .to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = launch::parse_args(&args);
    // Interactive runs log to the data directory and keep a crash file
    // there; screenshot runs touch nothing on disk (SPEC §5.11).
    let paths = prefs::Paths::user();
    let data = paths.data.clone().filter(|_| launch.shot.is_none());
    report::log::init(data.as_deref());
    report::crash::install_hook();
    if let Some(d) = &data {
        report::crash::set_crash_dir(d);
    }
    if let Some(dir) = &launch.shot {
        let r = match &launch.models {
            Some(m) => shot::run_real(dir, m),
            None => shot::run(dir),
        };
        if let Err(e) = r {
            eprintln!("screenshots failed: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let sample = launch.sample;
    let window_shot = launch.window_shot.clone();
    let open = launch.open.clone();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(strings::APP_NAME)
        .with_app_id("newtopo")
        .with_inner_size([1480.0, 920.0])
        .with_min_inner_size([960.0, 600.0])
        .with_drag_and_drop(true);
    if let Some(i) = icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(i));
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        multisampling: 1,
        depth_buffer: 0,
        ..Default::default()
    };
    eframe::run_native(
        strings::APP_NAME,
        options,
        Box::new(move |cc| {
            let gpu = cc
                .wgpu_render_state
                .as_ref()
                .map(|rs| report::system::GpuInfo::from_wgpu(&rs.adapter.get_info()));
            let gpu_ok = match &cc.wgpu_render_state {
                Some(rs) => {
                    newtopo_render::egui_viewport::install(rs);
                    true
                }
                None => false,
            };
            log::info!(
                "{} starting on {} {}; graphics: {}",
                report::version_line(),
                report::system::platform_short(),
                std::env::consts::ARCH,
                gpu.as_ref()
                    .map(|g| format!("{} ({})", g.name, g.backend))
                    .unwrap_or_else(|| "none".into())
            );
            let mut app = app::App::new(&cc.egui_ctx, paths, gpu_ok, false);
            app.st.gpu_info = gpu;
            if sample {
                app.st.load_sample();
            }
            if let Some(p) = open {
                app.st.open_from_launch(p);
            }
            // Interactive launches only: never during a --window-shot capture.
            if window_shot.is_none() {
                app.st.startup_update_check();
            }
            Ok(Box::new(EframeApp {
                app,
                window_shot,
                frames: 0,
            }))
        }),
    )
}

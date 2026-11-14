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
mod palette;
mod panels;
mod prefs;
mod prefs_ui;
mod selection;
mod shot;
mod strings;
mod studio;
mod theme;
mod toasts;
mod tools;
#[cfg(test)]
mod uitest;
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--shot") {
        let dir = args
            .get(i + 1)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("screenshots"));
        if let Err(e) = shot::run(&dir) {
            eprintln!("screenshots failed: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let sample = args.iter().any(|a| a == "--sample");
    let window_shot = args
        .iter()
        .position(|a| a == "--window-shot")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let open: Option<PathBuf> = args
        .iter()
        .enumerate()
        .find(|(i, a)| !a.starts_with("--") && (*i == 0 || args[i - 1] != "--window-shot"))
        .map(|(_, a)| PathBuf::from(a));

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
            let gpu_ok = match &cc.wgpu_render_state {
                Some(rs) => {
                    newtopo_render::egui_viewport::install(rs);
                    true
                }
                None => false,
            };
            let mut app = app::App::new(&cc.egui_ctx, prefs::Paths::user(), gpu_ok, false);
            if sample {
                app.st.load_sample();
            }
            if let Some(p) = open {
                if jobs::is_scene(&p) {
                    app.st.open_scene(Some(p));
                } else if jobs::is_cad(&p) {
                    app.st.start_import(p);
                }
            }
            Ok(Box::new(EframeApp {
                app,
                window_shot,
                frames: 0,
            }))
        }),
    )
}

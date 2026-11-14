//! Preferences window (SPEC §5.7): general, appearance, statistics, keymap.

use crate::app::AppState;
use crate::commands::{ALL, Cmd};
use crate::palette::{KeyCapture, fuzzy_score};
use crate::prefs::{NavPreset, RadialStyle, StatsShow};
use crate::strings as S;
use egui::{Color32, RichText, Ui, vec2};
use newtopo_render::{OverlayColors, UpAxis};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrefsTab {
    #[default]
    General,
    Appearance,
    Stats,
    Keymap,
}

fn to_c32(c: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (c[3] * 255.0) as u8,
    )
}

fn from_c32(c: Color32) -> [f32; 4] {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    [
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ]
}

fn overlay_fields(o: &mut OverlayColors) -> Vec<(&'static str, &mut [f32; 4])> {
    vec![
        ("CAD edges and split lines", &mut o.cad_edge),
        ("Generated lines", &mut o.line),
        ("Fill edges", &mut o.fill_edge),
        ("Selection", &mut o.selection),
        ("Hover", &mut o.hover),
        ("Vertices", &mut o.vertex),
        ("Connect arrow A", &mut o.arrow_a),
        ("Connect arrow B", &mut o.arrow_b),
        ("Linked faces", &mut o.linked_tint),
        ("Symmetry plane", &mut o.symmetry_plane),
        ("Split preview", &mut o.split_preview),
        ("Warning tint", &mut o.warning),
        ("Weld source", &mut o.weld_from),
        ("Weld target", &mut o.weld_to),
        ("Preview surface", &mut o.preview_surface),
        ("Preview wireframe", &mut o.preview_wire),
        ("Background top", &mut o.background_top),
        ("Background bottom", &mut o.background_bottom),
        ("Ground grid", &mut o.grid),
    ]
}

impl AppState {
    pub fn prefs_window(&mut self, ctx: &egui::Context) {
        if !self.ui.prefs_open {
            return;
        }
        let mut open = true;
        egui::Window::new(S::PREFS_TITLE)
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(vec2(640.0, 600.0))
            .min_width(520.0)
            .default_pos(ctx.content_rect().center() - vec2(310.0, 280.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (t, l) in [
                        (PrefsTab::General, S::PREFS_GENERAL),
                        (PrefsTab::Appearance, S::PREFS_APPEARANCE),
                        (PrefsTab::Stats, S::PREFS_STATS),
                        (PrefsTab::Keymap, S::PREFS_KEYMAP),
                    ] {
                        let on = self.ui.prefs_tab == t;
                        let text = RichText::new(l).color(if on {
                            self.pal.text_strong
                        } else {
                            self.pal.text_dim
                        });
                        if ui
                            .add(egui::Button::selectable(on, text).min_size(vec2(0.0, 28.0)))
                            .clicked()
                        {
                            self.ui.prefs_tab = t;
                        }
                    }
                });
                ui.separator();
                ui.add_space(4.0);
                let before = (self.prefs.clone(), self.keymap.clone());
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.ui.prefs_tab {
                        PrefsTab::General => self.prefs_general(ui),
                        PrefsTab::Appearance => self.prefs_appearance(ui),
                        PrefsTab::Stats => self.prefs_stats(ui),
                        PrefsTab::Keymap => self.prefs_keymap(ui),
                    });
                if (self.prefs.clone(), self.keymap.clone()) != before {
                    self.dirty_settings = true;
                }
            });
        if !open {
            self.ui.prefs_open = false;
        }
    }

    fn prefs_general(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        heading(ui, S::PREFS_NAV);
        for (p, l, help) in [
            (NavPreset::Blender, S::NAV_BLENDER, S::NAV_BLENDER_HELP),
            (NavPreset::Maya, S::NAV_MAYA, S::NAV_MAYA_HELP),
            (NavPreset::Max, S::NAV_MAX, S::NAV_MAX_HELP),
        ] {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.prefs.nav, p, l);
                ui.label(RichText::new(help).small().color(pal.text_dim));
            });
        }
        ui.label(
            RichText::new("Right-drag draws a lasso, left-drag a box, in every preset.")
                .small()
                .color(pal.text_dim),
        );
        ui.add_space(10.0);
        egui::Grid::new("prefs-general")
            .num_columns(2)
            .spacing(vec2(16.0, 8.0))
            .show(ui, |ui| {
                ui.label(S::PREFS_UNITS);
                egui::ComboBox::from_id_salt("units")
                    .selected_text(self.prefs.units.clone())
                    .show_ui(ui, |ui| {
                        for u in ["mm", "cm", "m", "in"] {
                            ui.selectable_value(&mut self.prefs.units, u.to_string(), u);
                        }
                    });
                ui.end_row();
                ui.label(S::PREFS_UP);
                ui.horizontal(|ui| {
                    let before = self.prefs.up;
                    ui.selectable_value(&mut self.prefs.up, UpAxis::Z, "Z up");
                    ui.selectable_value(&mut self.prefs.up, UpAxis::Y, "Y up");
                    if before != self.prefs.up {
                        self.view.camera.up = self.prefs.up;
                    }
                });
                ui.end_row();
                ui.label(S::PREFS_AUTOSAVE);
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.prefs.autosave_minutes).range(0..=120));
                    ui.label(RichText::new(S::PREFS_MINUTES).color(pal.text_dim));
                    if self.prefs.autosave_minutes == 0 {
                        ui.label(RichText::new("off").small().color(pal.warn));
                    }
                });
                ui.end_row();
                ui.label(S::PREFS_UI_SCALE);
                ui.add(egui::Slider::new(&mut self.prefs.ui_scale, 0.8..=1.6).step_by(0.05));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.prefs.select_fill_edges, S::PREFS_SELECT_FILL);
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.prefs.selection_undo, S::PREFS_SEL_UNDO);
                ui.end_row();
            });
        ui.add_space(10.0);
        heading(ui, S::IMPORT_OPTIONS);
        egui::Grid::new("prefs-import")
            .num_columns(2)
            .spacing(vec2(16.0, 8.0))
            .show(ui, |ui| {
                ui.label("");
                ui.checkbox(&mut self.prefs.import.heal, S::IMPORT_HEAL);
                ui.end_row();
                ui.label(S::IMPORT_SEWING);
                ui.add_enabled(
                    self.prefs.import.heal,
                    egui::DragValue::new(&mut self.prefs.import.sewing_tolerance)
                        .speed(0.0001)
                        .range(1e-6..=10.0)
                        .max_decimals(5),
                );
                ui.end_row();
                ui.label(S::IMPORT_UNITS);
                egui::ComboBox::from_id_salt("import-units")
                    .selected_text(self.prefs.import.units.clone())
                    .show_ui(ui, |ui| {
                        for u in ["mm", "cm", "m", "in"] {
                            ui.selectable_value(&mut self.prefs.import.units, u.to_string(), u);
                        }
                    });
                ui.end_row();
            });
    }

    fn prefs_appearance(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        egui::Grid::new("prefs-appearance")
            .num_columns(2)
            .spacing(vec2(16.0, 8.0))
            .show(ui, |ui| {
                ui.label(S::PREFS_THEME);
                let mut chosen = self.theme.name.clone();
                egui::ComboBox::from_id_salt("theme")
                    .selected_text(chosen.clone())
                    .show_ui(ui, |ui| {
                        for t in &self.themes {
                            ui.selectable_value(&mut chosen, t.name.clone(), &t.name);
                        }
                    });
                if chosen != self.theme.name {
                    self.set_theme(&chosen);
                }
                ui.end_row();
                ui.label(S::PREFS_RADIAL_THEME);
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.prefs.radial_style,
                        RadialStyle::FollowTheme,
                        "Follow theme",
                    );
                    ui.selectable_value(
                        &mut self.prefs.radial_style,
                        RadialStyle::HighContrast,
                        "High contrast",
                    );
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.prefs.show_grid, "Ground grid");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.prefs.show_symmetry_planes, "Show symmetry planes");
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            heading(ui, S::PREFS_OVERLAY);
            if self.prefs.overlay.is_some() && ui.small_button(S::PREFS_RESET_COLOURS).clicked() {
                self.prefs.overlay = None;
                self.invalidate_view();
            }
        });
        ui.label(
            RichText::new(
                "Themes are JSON files; drop your own into the themes folder next to prefs.json.",
            )
            .small()
            .color(pal.text_dim),
        );
        ui.add_space(4.0);
        let mut colors = self.colors();
        let before = colors.clone();
        egui::Grid::new("overlay-colours")
            .num_columns(4)
            .spacing(vec2(12.0, 6.0))
            .show(ui, |ui| {
                for (i, (label, c)) in overlay_fields(&mut colors).into_iter().enumerate() {
                    let mut col = to_c32(*c);
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        *c = from_c32(col);
                    }
                    ui.label(label);
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        if colors != before {
            self.prefs.overlay = Some(colors);
            self.invalidate_view();
        }
    }

    fn prefs_stats(&mut self, ui: &mut Ui) {
        heading(ui, S::PREFS_STATS_SHOW);
        ui.horizontal(|ui| {
            ui.radio_value(
                &mut self.prefs.stats.show,
                StatsShow::Totals,
                S::STATS_TOTALS,
            );
            ui.radio_value(
                &mut self.prefs.stats.show,
                StatsShow::Selection,
                S::STATS_SELECTION,
            );
            ui.radio_value(&mut self.prefs.stats.show, StatsShow::Both, S::STATS_BOTH);
        });
        ui.add_space(8.0);
        heading(ui, S::PREFS_STATS_ROWS);
        let s = &mut self.prefs.stats;
        for (l, v) in [
            (S::STAT_VERTS, &mut s.verts),
            (S::STAT_EDGES, &mut s.edges),
            (S::STAT_FACES, &mut s.faces),
            (S::STAT_TRIS, &mut s.tris),
            (S::STAT_QUADS, &mut s.quads),
            (S::STAT_NGONS, &mut s.ngons),
            (S::STAT_DORMANT, &mut s.dormant),
        ] {
            ui.checkbox(v, l);
        }
    }

    fn prefs_keymap(&mut self, ui: &mut Ui) {
        let pal = self.pal;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.ui.keymap_filter)
                    .hint_text(S::PREFS_KEYMAP_SEARCH)
                    .desired_width(240.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(S::PREFS_RESET_ALL_KEYS).clicked() {
                    self.keymap.reset_all();
                }
            });
        });
        ui.label(RichText::new("Click a shortcut to change it. Double-press bindings fire when the same key is pressed twice within 300 ms.").small().color(pal.text_dim));
        ui.add_space(6.0);
        let filter = self.ui.keymap_filter.clone();
        let mut capture: Option<Cmd> = None;
        let mut reset: Option<Cmd> = None;
        let mut cmds: Vec<Cmd> = ALL
            .iter()
            .copied()
            .filter(|c| {
                filter.is_empty()
                    || fuzzy_score(&filter, c.label()).is_some()
                    || self
                        .keymap
                        .label(*c)
                        .to_lowercase()
                        .contains(&filter.to_lowercase())
            })
            .collect();
        cmds.sort_by_key(|c| (c.category(), c.label()));
        egui::Grid::new("keymap")
            .num_columns(4)
            .striped(true)
            .spacing(vec2(12.0, 4.0))
            .show(ui, |ui| {
                for c in cmds {
                    ui.label(c.label());
                    ui.label(
                        RichText::new(c.category().label())
                            .small()
                            .color(pal.text_dim),
                    );
                    let label = self.keymap.label(c);
                    let text = if label.is_empty() {
                        S::KEY_NONE.to_string()
                    } else {
                        label
                    };
                    if ui
                        .add(
                            egui::Button::new(RichText::new(text).monospace())
                                .min_size(vec2(110.0, 0.0)),
                        )
                        .clicked()
                    {
                        capture = Some(c);
                    }
                    if !self.keymap.is_default(c) {
                        if ui.small_button("Reset").clicked() {
                            reset = Some(c);
                        }
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
        if let Some(c) = capture {
            self.ui.key_capture = Some(KeyCapture {
                cmd: c,
                captured: None,
                double: false,
            });
        }
        if let Some(c) = reset {
            self.keymap.reset(c);
        }
    }
}

fn heading(ui: &mut Ui, s: &str) {
    ui.label(RichText::new(s).font(crate::theme::medium(14.0)));
}

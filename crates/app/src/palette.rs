//! Command palette and shortcut capture (SPEC §5.4).

use crate::app::AppState;
use crate::commands::{ALL, Cmd};
use crate::icons;
use crate::keymap::Binding;
use crate::strings as S;
use egui::{Align2, Color32, Key, Rect, RichText, Sense, Stroke, vec2};

#[derive(Default)]
pub struct PaletteState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    focus: bool,
}

impl PaletteState {
    pub fn open_fresh(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.focus = true;
    }
}

pub struct KeyCapture {
    pub cmd: Cmd,
    pub captured: Option<Binding>,
    pub double: bool,
}

/// Subsequence fuzzy match. Higher is better; `None` = no match.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let q: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let mut score = 0;
    let mut ti = 0;
    let mut prev_match: Option<usize> = None;
    for qc in q {
        let mut found = None;
        while ti < t.len() {
            if t[ti] == qc {
                found = Some(ti);
                ti += 1;
                break;
            }
            ti += 1;
        }
        let i = found?;
        score += 1;
        if prev_match.is_some_and(|p| p + 1 == i) {
            score += 5;
        }
        if i == 0 || !t[i - 1].is_alphanumeric() {
            score += 8;
        }
        prev_match = Some(i);
    }
    // Prefer shorter texts and earlier first matches.
    Some(score * 10 - t.len() as i32)
}

impl AppState {
    fn palette_matches(&self) -> Vec<Cmd> {
        let q = self.ui.palette.query.trim();
        if q.is_empty() {
            let mut favs: Vec<Cmd> = self
                .prefs
                .favourites
                .iter()
                .filter_map(|id| Cmd::from_id(id))
                .collect();
            let mut rest: Vec<Cmd> = ALL.iter().copied().filter(|c| !favs.contains(c)).collect();
            rest.sort_by_key(|c| (c.category(), c.label()));
            favs.extend(rest);
            return favs;
        }
        let mut scored: Vec<(i32, Cmd)> = ALL
            .iter()
            .filter_map(|c| {
                let label = fuzzy_score(q, c.label());
                let cat = fuzzy_score(q, &format!("{} {}", c.category().label(), c.label()))
                    .map(|s| s - 20);
                let s = label.max(cat)?;
                let fav = if self.prefs.favourites.iter().any(|f| f == c.id()) {
                    15
                } else {
                    0
                };
                Some((s + fav, *c))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.label().cmp(b.1.label())));
        scored.into_iter().map(|x| x.1).collect()
    }

    pub fn toggle_favourite(&mut self, c: Cmd) {
        if let Some(i) = self.prefs.favourites.iter().position(|f| f == c.id()) {
            self.prefs.favourites.remove(i);
        } else {
            self.prefs.favourites.push(c.id().to_string());
        }
        self.dirty_settings = true;
    }

    fn pick_from_palette(&mut self, c: Cmd) {
        self.ui.palette.open = false;
        self.run(c);
    }

    pub fn palette_ui(&mut self, ctx: &egui::Context) {
        if !self.ui.palette.open {
            return;
        }
        let pal = self.pal;
        let screen = ctx.content_rect();
        // Dim + click-outside-to-close.
        let dim = egui::Area::new(egui::Id::new("palette-dim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let r = ui.allocate_rect(screen, Sense::click());
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    Color32::from_black_alpha(if pal.dark { 90 } else { 40 }),
                );
                r.clicked()
            });
        if dim.inner {
            self.ui.palette.open = false;
            return;
        }
        let matches = self.palette_matches();
        let n = matches.len();
        let (up, down, enter) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
            )
        });
        if down && n > 0 {
            self.ui.palette.selected = (self.ui.palette.selected + 1) % n;
        }
        if up && n > 0 {
            self.ui.palette.selected = (self.ui.palette.selected + n - 1) % n;
        }
        self.ui.palette.selected = self.ui.palette.selected.min(n.saturating_sub(1));
        if enter && let Some(c) = matches.get(self.ui.palette.selected).copied() {
            self.pick_from_palette(c);
            return;
        }

        let width = 580.0f32.min(screen.width() - 40.0);
        let pos = egui::pos2(screen.center().x - width * 0.5, screen.top() + 70.0);
        let mut run: Option<Cmd> = None;
        let mut fav_toggle: Option<Cmd> = None;
        let mut capture: Option<Cmd> = None;
        let mut reset: Option<Cmd> = None;
        egui::Area::new(egui::Id::new("palette")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::new()
                .fill(pal.panel_raised)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(12)
                .shadow(ctx.global_style().visuals.window_shadow)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(width - 20.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        icons::paint(ui.painter(), r, icons::Icon::Search, pal.text_dim);
                        let te = egui::TextEdit::singleline(&mut self.ui.palette.query)
                            .hint_text(S::PALETTE_HINT)
                            .frame(egui::Frame::NONE)
                            .font(egui::FontId::proportional(16.0))
                            .desired_width(f32::INFINITY);
                        let r = ui.add(te);
                        if self.ui.palette.focus {
                            r.request_focus();
                            self.ui.palette.focus = false;
                        }
                        if r.changed() {
                            self.ui.palette.selected = 0;
                        }
                    });
                    ui.add_space(6.0);
                    ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, pal.separator));
                    ui.add_space(6.0);
                    if matches.is_empty() {
                        ui.label(RichText::new(S::PALETTE_NO_MATCH).color(pal.text_dim));
                        return;
                    }
                    let nfav = if self.ui.palette.query.trim().is_empty() {
                        self.prefs.favourites.iter().filter(|f| Cmd::from_id(f).is_some()).count()
                    } else {
                        0
                    };
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        for (i, c) in matches.iter().enumerate() {
                            if self.ui.palette.query.trim().is_empty() && (i == 0 || i == nfav) {
                                let title = if i < nfav { S::PALETTE_FAVOURITES } else { S::PALETTE_ALL };
                                ui.add_space(if i == 0 { 0.0 } else { 6.0 });
                                ui.label(RichText::new(title.to_uppercase()).small().color(pal.text_dim));
                            }
                            let is_fav = self.prefs.favourites.iter().any(|f| f == c.id());
                            let selected = i == self.ui.palette.selected;
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                            if selected || resp.hovered() {
                                ui.painter().rect_filled(rect, 6.0, if selected { pal.selection_bg } else { pal.widget });
                            }
                            if selected {
                                resp.scroll_to_me(None);
                            }
                            let ir = Rect::from_min_size(rect.min + vec2(8.0, 7.0), vec2(16.0, 16.0));
                            if let Some(icon) = c.icon() {
                                icons::paint(ui.painter(), ir, icon, if selected { pal.accent } else { pal.text_dim });
                            }
                            ui.painter().text(rect.min + vec2(34.0, 15.0), Align2::LEFT_CENTER, c.label(), egui::FontId::proportional(13.5), pal.text_strong);
                            let cat_x = rect.min.x + 34.0 + ui.fonts_mut(|f| f.layout_no_wrap(c.label().into(), egui::FontId::proportional(13.5), Color32::WHITE).size().x) + 10.0;
                            ui.painter().text(egui::pos2(cat_x, rect.center().y), Align2::LEFT_CENTER, c.category().label(), egui::FontId::proportional(11.5), pal.text_dim);
                            // Right side: shortcut, star, menu.
                            let menu_r = Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 22.0));
                            let star_r = Rect::from_center_size(egui::pos2(rect.right() - 42.0, rect.center().y), vec2(22.0, 22.0));
                            let sc = self.keymap.label(*c);
                            if !sc.is_empty() {
                                let g = ui.painter().layout_no_wrap(sc, egui::FontId::monospace(11.5), pal.text);
                                let kr = Rect::from_min_size(egui::pos2(star_r.left() - 10.0 - g.size().x - 12.0, rect.center().y - 10.0), vec2(g.size().x + 12.0, 20.0));
                                ui.painter().rect_filled(kr, 4.0, pal.inset);
                                ui.painter().rect_stroke(kr, 4.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
                                ui.painter().galley(kr.min + vec2(6.0, (20.0 - g.size().y) * 0.5), g, pal.text);
                            }
                            let star = ui.interact(star_r, ui.id().with(("star", c.id())), Sense::click());
                            if is_fav || resp.hovered() || star.hovered() || selected {
                                icons::paint(
                                    ui.painter(),
                                    star_r.shrink(4.0),
                                    if is_fav { icons::Icon::Star } else { icons::Icon::StarOutline },
                                    if is_fav { pal.warn } else { pal.text_dim },
                                );
                            }
                            if star.on_hover_text(if is_fav { S::PALETTE_REMOVE_FAV } else { S::PALETTE_ADD_FAV }).clicked() {
                                fav_toggle = Some(*c);
                            }
                            let menu = ui.interact(menu_r, ui.id().with(("menu", c.id())), Sense::click());
                            if menu.hovered() {
                                ui.painter().rect_filled(menu_r, 4.0, pal.widget_hover);
                            }
                            icons::paint(ui.painter(), menu_r.shrink(4.0), icons::Icon::Dots, pal.text_dim);
                            let cmd_menu = |ui: &mut egui::Ui, capture: &mut Option<Cmd>, fav: &mut Option<Cmd>, reset: &mut Option<Cmd>| {
                                if ui.button(S::PALETTE_CHANGE_SHORTCUT).clicked() {
                                    *capture = Some(*c);
                                    ui.close();
                                }
                                if ui.button(if is_fav { S::PALETTE_REMOVE_FAV } else { S::PALETTE_ADD_FAV }).clicked() {
                                    *fav = Some(*c);
                                    ui.close();
                                }
                                if ui.button(S::PALETTE_RESET_SHORTCUT).clicked() {
                                    *reset = Some(*c);
                                    ui.close();
                                }
                            };
                            egui::Popup::menu(&menu).show(|ui| cmd_menu(ui, &mut capture, &mut fav_toggle, &mut reset));
                            resp.context_menu(|ui| cmd_menu(ui, &mut capture, &mut fav_toggle, &mut reset));
                            if resp.clicked() {
                                run = Some(*c);
                            }
                        }
                    });
                });
        });
        if let Some(c) = fav_toggle {
            self.toggle_favourite(c);
        }
        if let Some(c) = reset {
            self.keymap.reset(c);
            self.dirty_settings = true;
        }
        if let Some(c) = capture {
            self.ui.key_capture = Some(KeyCapture {
                cmd: c,
                captured: None,
                double: false,
            });
        }
        if let Some(c) = run {
            self.pick_from_palette(c);
        }
    }

    // ------------------------------------------------------------ key capture

    pub fn capture_key(&mut self, key: Key, mods: egui::Modifiers) {
        let Some(cap) = &mut self.ui.key_capture else {
            return;
        };
        if key == Key::Escape && mods.is_none() && cap.captured.is_none() {
            self.ui.key_capture = None;
            return;
        }
        cap.captured = Some(Binding {
            double: cap.double,
            ..Binding::new(key, mods)
        });
    }

    pub fn key_capture_ui(&mut self, ctx: &egui::Context) {
        let Some(cap) = &mut self.ui.key_capture else {
            return;
        };
        let pal = self.pal;
        let cmd = cap.cmd;
        let mut save = false;
        let mut cancel = false;
        let mut clear = false;
        let current = self.keymap.label(cmd);
        let conflicts: Vec<Cmd> = cap
            .captured
            .map(|b| {
                self.keymap.conflicts(
                    &Binding {
                        double: cap.double,
                        ..b
                    },
                    cmd,
                )
            })
            .unwrap_or_default();
        egui::Modal::new(egui::Id::new("key-capture")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(
                RichText::new(S::KEY_CAPTURE_TITLE)
                    .font(crate::theme::medium(16.0))
                    .color(pal.text_strong),
            );
            ui.label(RichText::new(cmd.label()).color(pal.text_dim));
            ui.add_space(10.0);
            let shown = cap
                .captured
                .map(|b| {
                    Binding {
                        double: cap.double,
                        ..b
                    }
                    .display()
                })
                .unwrap_or_else(|| "…".into());
            egui::Frame::new()
                .fill(pal.inset)
                .corner_radius(8)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(shown)
                                .font(egui::FontId::monospace(20.0))
                                .color(pal.accent),
                        )
                    });
                });
            ui.add_space(6.0);
            if ui
                .checkbox(&mut cap.double, S::KEY_CAPTURE_DOUBLE)
                .changed()
                && let Some(b) = &mut cap.captured
            {
                b.double = cap.double;
            }
            ui.label(
                RichText::new(format!(
                    "Current: {}",
                    if current.is_empty() {
                        S::KEY_NONE.to_string()
                    } else {
                        current
                    }
                ))
                .small()
                .color(pal.text_dim),
            );
            if !conflicts.is_empty() {
                let names: Vec<&str> = conflicts.iter().map(|c| c.label()).collect();
                ui.label(
                    RichText::new(format!(
                        "{}: {} (saving moves it here)",
                        S::KEY_CAPTURE_CONFLICT,
                        names.join(", ")
                    ))
                    .small()
                    .color(pal.warn),
                );
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(cap.captured.is_some(), egui::Button::new(S::DLG_SAVE))
                    .clicked()
                {
                    save = true;
                }
                if ui.button("No shortcut").clicked() {
                    clear = true;
                }
                if ui.button(S::DLG_CANCEL).clicked() {
                    cancel = true;
                }
            });
        });
        if save && let Some(b) = cap.captured {
            let b = Binding {
                double: cap.double,
                ..b
            };
            for other in self.keymap.conflicts(&b, cmd) {
                let rest: Vec<Binding> = self
                    .keymap
                    .bindings(other)
                    .into_iter()
                    .filter(|x| !(x.same_combo(&b) && x.double == b.double))
                    .collect();
                self.keymap.set(other, &rest);
            }
            self.keymap.set(cmd, &[b]);
            self.dirty_settings = true;
            self.ui.key_capture = None;
        } else if clear {
            self.keymap.set(cmd, &[]);
            self.dirty_settings = true;
            self.ui.key_capture = None;
        } else if cancel {
            self.ui.key_capture = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy() {
        assert!(fuzzy_score("con", "Connect").is_some());
        assert!(fuzzy_score("xyz", "Connect").is_none());
        // Word starts and consecutive runs rank higher.
        let a = fuzzy_score("sg", "Surface Grid").unwrap();
        let b = fuzzy_score("sg", "Visible-only selection toggle").unwrap_or(i32::MIN);
        assert!(a > b);
        let exact = fuzzy_score("undo", "Undo").unwrap();
        let loose = fuzzy_score("undo", "Unhide all dormant").unwrap_or(i32::MIN);
        assert!(exact > loose);
    }
}

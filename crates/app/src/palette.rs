//! Command palette, radial menu and shortcut capture (SPEC §5.4).

use crate::app::AppState;
use crate::commands::{ALL, Cmd};
use crate::icons;
use crate::keymap::Binding;
use crate::prefs::{RADIAL_SLOTS, RadialStyle};
use crate::strings as S;
use crate::toasts::ToastKind;
use egui::{Align2, Color32, Key, Pos2, Rect, RichText, Sense, Stroke, vec2};

/// Hold shorter than this keeps the radial menu open (SPEC §5.4).
pub const RADIAL_TAP_SECS: f64 = 0.2;
const RADIAL_INNER: f32 = 66.0;
const RADIAL_OUTER: f32 = 168.0;

/// An annular sector (angles clockwise from 12 o'clock) as a filled mesh.
fn wedge(c: Pos2, r0: f32, r1: f32, a0: f32, a1: f32, color: Color32) -> egui::Shape {
    let mut m = egui::Mesh::default();
    let n = 10;
    for k in 0..=n {
        let a = a0 + (a1 - a0) * k as f32 / n as f32;
        let d = vec2(a.sin(), -a.cos());
        m.colored_vertex(c + d * r0, color);
        m.colored_vertex(c + d * r1, color);
    }
    for k in 0..n as u32 {
        let i = k * 2;
        m.add_triangle(i, i + 1, i + 3);
        m.add_triangle(i, i + 3, i + 2);
    }
    egui::Shape::mesh(m)
}

#[derive(Default)]
pub struct PaletteState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    /// When set, picking a command assigns it to this radial slot.
    pub for_radial_slot: Option<usize>,
    focus: bool,
}

impl PaletteState {
    pub fn open_fresh(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.for_radial_slot = None;
        self.focus = true;
    }
}

#[derive(Default)]
pub struct RadialState {
    pub open: bool,
    pub center: Pos2,
    pub opened_at: f64,
    /// Key still held (release decides tap vs. hold).
    pub held: bool,
    pub sticky: bool,
    pub hovered: Option<usize>,
    pub moving: Option<usize>,
    pub menu_slot: Option<usize>,
}

impl RadialState {
    pub fn open_at(&mut self, c: Pos2, t: f64, sticky: bool) {
        *self = RadialState { open: true, center: c, opened_at: t, held: !sticky, sticky, hovered: None, moving: None, menu_slot: None };
    }
    pub fn close(&mut self) {
        self.open = false;
        self.held = false;
        self.moving = None;
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
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
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
            let mut favs: Vec<Cmd> = self.prefs.favourites.iter().filter_map(|id| Cmd::from_id(id)).collect();
            let mut rest: Vec<Cmd> = ALL.iter().copied().filter(|c| !favs.contains(c)).collect();
            rest.sort_by_key(|c| (c.category(), c.label()));
            favs.extend(rest);
            return favs;
        }
        let mut scored: Vec<(i32, Cmd)> = ALL
            .iter()
            .filter_map(|c| {
                let label = fuzzy_score(q, c.label());
                let cat = fuzzy_score(q, &format!("{} {}", c.category().label(), c.label())).map(|s| s - 20);
                let s = label.max(cat)?;
                let fav = if self.prefs.favourites.iter().any(|f| f == c.id()) { 15 } else { 0 };
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

    pub fn add_to_radial(&mut self, c: Cmd, slot: Option<usize>) {
        let slot = slot.or_else(|| self.prefs.radial.iter().position(|s| s.is_none()));
        match slot {
            Some(i) => {
                self.prefs.radial[i] = Some(c.id().to_string());
                self.dirty_settings = true;
                self.toast(ToastKind::Info, format!("Added \"{}\" to the radial menu", c.label()));
            }
            None => self.toast(ToastKind::Warn, S::RADIAL_FULL),
        }
    }

    fn pick_from_palette(&mut self, c: Cmd) {
        self.ui.palette.open = false;
        if let Some(slot) = self.ui.palette.for_radial_slot.take() {
            self.add_to_radial(c, Some(slot));
        } else {
            self.run(c);
        }
    }

    pub fn palette_ui(&mut self, ctx: &egui::Context) {
        if !self.ui.palette.open {
            return;
        }
        let pal = self.pal;
        let screen = ctx.content_rect();
        // Dim + click-outside-to-close.
        let dim = egui::Area::new(egui::Id::new("palette-dim")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let r = ui.allocate_rect(screen, Sense::click());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(if pal.dark { 90 } else { 40 }));
            r.clicked()
        });
        if dim.inner {
            self.ui.palette.open = false;
            return;
        }
        let matches = self.palette_matches();
        let n = matches.len();
        let (up, down, enter) = ctx.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.key_pressed(Key::Enter)));
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
        let mut radial_add: Option<Cmd> = None;
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
                        let hint = if self.ui.palette.for_radial_slot.is_some() { "Pick a command for the radial slot…" } else { S::PALETTE_HINT };
                        let te = egui::TextEdit::singleline(&mut self.ui.palette.query)
                            .hint_text(hint)
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
                            let cmd_menu = |ui: &mut egui::Ui, capture: &mut Option<Cmd>, fav: &mut Option<Cmd>, radial: &mut Option<Cmd>, reset: &mut Option<Cmd>| {
                                if ui.button(S::PALETTE_CHANGE_SHORTCUT).clicked() {
                                    *capture = Some(*c);
                                    ui.close();
                                }
                                if ui.button(if is_fav { S::PALETTE_REMOVE_FAV } else { S::PALETTE_ADD_FAV }).clicked() {
                                    *fav = Some(*c);
                                    ui.close();
                                }
                                if ui.button(S::PALETTE_ADD_RADIAL).clicked() {
                                    *radial = Some(*c);
                                    ui.close();
                                }
                                if ui.button(S::PALETTE_RESET_SHORTCUT).clicked() {
                                    *reset = Some(*c);
                                    ui.close();
                                }
                            };
                            egui::Popup::menu(&menu).show(|ui| cmd_menu(ui, &mut capture, &mut fav_toggle, &mut radial_add, &mut reset));
                            resp.context_menu(|ui| cmd_menu(ui, &mut capture, &mut fav_toggle, &mut radial_add, &mut reset));
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
        if let Some(c) = radial_add {
            self.add_to_radial(c, None);
        }
        if let Some(c) = reset {
            self.keymap.reset(c);
            self.dirty_settings = true;
        }
        if let Some(c) = capture {
            self.ui.key_capture = Some(KeyCapture { cmd: c, captured: None, double: false });
        }
        if let Some(c) = run {
            self.pick_from_palette(c);
        }
    }

    // ------------------------------------------------------------ radial

    fn radial_slot_at(&self, p: Pos2) -> Option<usize> {
        let d = p - self.ui.radial.center;
        if d.length() < RADIAL_INNER * 0.6 || d.length() > RADIAL_OUTER + 40.0 {
            return None;
        }
        // Slot 0 at the top, clockwise.
        let a = d.x.atan2(-d.y).rem_euclid(std::f32::consts::TAU);
        let step = std::f32::consts::TAU / RADIAL_SLOTS as f32;
        Some(((a + step * 0.5) / step) as usize % RADIAL_SLOTS)
    }

    pub fn radial_release(&mut self) {
        let held_for = self.time - self.ui.radial.opened_at;
        self.ui.radial.held = false;
        if held_for < RADIAL_TAP_SECS {
            // Tap: stay open until a click.
            self.ui.radial.sticky = true;
            return;
        }
        let slot = self.ui.radial.hovered;
        self.ui.radial.close();
        if let Some(c) = slot.and_then(|i| self.prefs.radial[i].clone()).and_then(|id| Cmd::from_id(&id)) {
            self.run(c);
        }
    }

    pub fn radial_ui(&mut self, ctx: &egui::Context) {
        if !self.ui.radial.open {
            return;
        }
        let pal = self.pal;
        let hc = self.prefs.radial_style == RadialStyle::HighContrast;
        let (bg, slot_bg, hover_bg, text, dim, hover_text) = if hc {
            (Color32::from_black_alpha(235), Color32::from_gray(28), Color32::WHITE, Color32::WHITE, Color32::from_gray(150), Color32::BLACK)
        } else {
            (pal.radial_bg, pal.radial_slot, pal.radial_hover, pal.text_strong, pal.text_dim, pal.accent_text)
        };
        let screen = ctx.content_rect();
        let margin = vec2(RADIAL_OUTER + 12.0, RADIAL_OUTER + 12.0);
        let c = self.ui.radial.center.clamp(screen.min + margin, (screen.max - margin).max(screen.min + margin));
        self.ui.radial.center = c;
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        self.ui.radial.hovered = pointer.and_then(|p| self.radial_slot_at(p));
        let mut run: Option<Cmd> = None;
        let mut close = false;
        let mut replace: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut start_move: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        egui::Area::new(egui::Id::new("radial")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let resp = ui.allocate_rect(screen, Sense::click());
            let p = ui.painter();
            p.circle_filled(c + vec2(0.0, 6.0), RADIAL_OUTER + 10.0, Color32::from_black_alpha(if pal.dark { 60 } else { 25 }));
            p.circle_filled(c, RADIAL_OUTER + 6.0, bg);
            let step = std::f32::consts::TAU / RADIAL_SLOTS as f32;
            let gap = 0.022;
            for i in 0..RADIAL_SLOTS {
                let a0 = i as f32 * step - step * 0.5 + gap;
                let a1 = i as f32 * step + step * 0.5 - gap;
                let cmd = self.prefs.radial[i].as_deref().and_then(Cmd::from_id);
                let hovered = self.ui.radial.hovered == Some(i);
                let fill = if hovered { hover_bg } else if cmd.is_some() { slot_bg } else { slot_bg.gamma_multiply(0.5) };
                p.add(wedge(c, RADIAL_INNER, RADIAL_OUTER, a0, a1, fill));
                if self.ui.radial.moving == Some(i) {
                    p.add(wedge(c, RADIAL_OUTER - 4.0, RADIAL_OUTER, a0, a1, pal.warn));
                }
                let am = i as f32 * step;
                let mid = c + vec2(am.sin(), -am.cos()) * (RADIAL_INNER + RADIAL_OUTER) * 0.5;
                let fg = if hovered { hover_text } else { text };
                match cmd {
                    Some(cmd) => {
                        if let Some(icon) = cmd.icon() {
                            icons::paint(p, Rect::from_center_size(mid - vec2(0.0, 9.0), vec2(18.0, 18.0)), icon, fg);
                        }
                        p.text(mid + vec2(0.0, 10.0), Align2::CENTER_CENTER, short_label(cmd), egui::FontId::proportional(11.5), fg);
                    }
                    None => {
                        p.text(mid, Align2::CENTER_CENTER, "+", egui::FontId::proportional(16.0), if hovered { fg } else { dim });
                    }
                }
            }
            // Centre: the hovered command's full name, or a hint.
            p.circle_filled(c, RADIAL_INNER - 6.0, slot_bg.gamma_multiply(0.6));
            let centre_text = if self.ui.radial.moving.is_some() {
                S::RADIAL_MOVE_HINT.to_string()
            } else {
                self.ui.radial.hovered.and_then(|i| self.prefs.radial[i].as_deref().and_then(Cmd::from_id)).map(|c| c.label().to_string()).unwrap_or_default()
            };
            if !centre_text.is_empty() {
                let g = p.layout(centre_text, egui::FontId::proportional(12.0), text, (RADIAL_INNER - 12.0) * 2.0);
                p.galley(c - g.size() * 0.5, g, text);
            } else {
                p.text(c, Align2::CENTER_CENTER, &self.keymap.label(Cmd::Radial), egui::FontId::monospace(12.0), dim);
            }
            // Clicks: primary runs (or completes a move), secondary opens the slot menu.
            if resp.secondary_clicked() {
                self.ui.radial.menu_slot = self.ui.radial.hovered;
            }
            if resp.clicked() {
                match (self.ui.radial.hovered, self.ui.radial.moving) {
                    (Some(i), Some(src)) => swap = Some((src, i)),
                    (Some(i), None) => run = self.prefs.radial[i].as_deref().and_then(Cmd::from_id),
                    (None, _) if self.ui.radial.sticky => close = true,
                    _ => {}
                }
            }
            if let Some(i) = self.ui.radial.menu_slot {
                let has = self.prefs.radial[i].is_some();
                resp.context_menu(|ui| {
                    if ui.button(S::RADIAL_REPLACE).clicked() {
                        replace = Some(i);
                        ui.close();
                    }
                    if ui.add_enabled(has, egui::Button::new(S::RADIAL_REMOVE)).clicked() {
                        remove = Some(i);
                        ui.close();
                    }
                    if ui.add_enabled(has, egui::Button::new(S::RADIAL_MOVE)).clicked() {
                        start_move = Some(i);
                        ui.close();
                    }
                });
            }
        });
        if let Some(i) = replace {
            self.ui.radial.close();
            self.ui.palette.open_fresh();
            self.ui.palette.for_radial_slot = Some(i);
            return;
        }
        if let Some(i) = remove {
            self.prefs.radial[i] = None;
            self.dirty_settings = true;
        }
        if let Some(i) = start_move {
            self.ui.radial.moving = Some(i);
            self.ui.radial.sticky = true;
        }
        if let Some((a, b)) = swap {
            self.prefs.radial.swap(a, b);
            self.ui.radial.moving = None;
            self.dirty_settings = true;
            return;
        }
        if let Some(cmd) = run {
            self.ui.radial.close();
            self.run(cmd);
        } else if close {
            self.ui.radial.close();
        }
    }

    // ------------------------------------------------------------ key capture

    pub fn capture_key(&mut self, key: Key, mods: egui::Modifiers) {
        let Some(cap) = &mut self.ui.key_capture else { return };
        if key == Key::Escape && mods.is_none() && cap.captured.is_none() {
            self.ui.key_capture = None;
            return;
        }
        cap.captured = Some(Binding { double: cap.double, ..Binding::new(key, mods) });
    }

    pub fn key_capture_ui(&mut self, ctx: &egui::Context) {
        let Some(cap) = &mut self.ui.key_capture else { return };
        let pal = self.pal;
        let cmd = cap.cmd;
        let mut save = false;
        let mut cancel = false;
        let mut clear = false;
        let current = self.keymap.label(cmd);
        let conflicts: Vec<Cmd> = cap.captured.map(|b| self.keymap.conflicts(&Binding { double: cap.double, ..b }, cmd)).unwrap_or_default();
        egui::Modal::new(egui::Id::new("key-capture")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(S::KEY_CAPTURE_TITLE).font(crate::theme::medium(16.0)).color(pal.text_strong));
            ui.label(RichText::new(cmd.label()).color(pal.text_dim));
            ui.add_space(10.0);
            let shown = cap.captured.map(|b| Binding { double: cap.double, ..b }.display()).unwrap_or_else(|| "…".into());
            egui::Frame::new().fill(pal.inset).corner_radius(8).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| ui.label(RichText::new(shown).font(egui::FontId::monospace(20.0)).color(pal.accent)));
            });
            ui.add_space(6.0);
            if ui.checkbox(&mut cap.double, S::KEY_CAPTURE_DOUBLE).changed()
                && let Some(b) = &mut cap.captured
            {
                b.double = cap.double;
            }
            ui.label(RichText::new(format!("Current: {}", if current.is_empty() { S::KEY_NONE.to_string() } else { current })).small().color(pal.text_dim));
            if !conflicts.is_empty() {
                let names: Vec<&str> = conflicts.iter().map(|c| c.label()).collect();
                ui.label(RichText::new(format!("{}: {} (saving moves it here)", S::KEY_CAPTURE_CONFLICT, names.join(", "))).small().color(pal.warn));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(cap.captured.is_some(), egui::Button::new(S::DLG_SAVE)).clicked() {
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
            let b = Binding { double: cap.double, ..b };
            for other in self.keymap.conflicts(&b, cmd) {
                let rest: Vec<Binding> = self.keymap.bindings(other).into_iter().filter(|x| !(x.same_combo(&b) && x.double == b.double)).collect();
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

/// Compact label for radial slots.
fn short_label(c: Cmd) -> &'static str {
    match c {
        Cmd::Remove => "Remove",
        Cmd::Loop => "Loop",
        Cmd::Ring => "Ring",
        Cmd::LoopAlternate => "Loop ½",
        Cmd::RingAlternate => "Ring ½",
        Cmd::Preview => "Preview",
        Cmd::AutoFill => "Auto-fill",
        Cmd::Link => "Link",
        Cmd::Symmetry => "Symmetry",
        Cmd::Frame => "Frame",
        Cmd::Ortho => "Ortho",
        Cmd::Weld => "Weld",
        Cmd::Grid => "Grid",
        Cmd::Open => "Open",
        Cmd::SaveAs => "Save as",
        Cmd::Import => "Import",
        Cmd::Export => "Export",
        c => {
            let l = c.label();
            if l.len() <= 12 { l } else { l.split_whitespace().next().unwrap_or(l) }
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

    #[test]
    fn radial_tap_constant() {
        assert!((RADIAL_TAP_SECS - 0.2).abs() < 1e-9);
    }
}

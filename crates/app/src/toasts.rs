//! Toasts: short messages at the bottom of the window (SPEC §5.9:
//! refusals are toasts with a reason; nothing fails silently).

use crate::icons::{self, Icon};
use crate::theme::Palette;
use egui::{Align2, RichText, Stroke, vec2};

const LIFETIME: f64 = 4.5;
const MAX_SHOWN: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub born: f64,
}

#[derive(Default)]
pub struct Toasts {
    pub items: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, kind: ToastKind, text: impl Into<String>, now: f64) {
        let text = text.into();
        // Repeating the same message refreshes it instead of stacking.
        self.items.retain(|t| t.text != text);
        self.items.push(Toast { kind, text, born: now });
        log::info!("toast: {}", self.items.last().map(|t| t.text.as_str()).unwrap_or(""));
    }

    pub fn show(&mut self, ctx: &egui::Context, pal: &Palette, now: f64) {
        self.items.retain(|t| now - t.born < LIFETIME + if t.kind == ToastKind::Error { 3.0 } else { 0.0 });
        if self.items.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        let screen = ctx.content_rect();
        let start = self.items.len().saturating_sub(MAX_SHOWN);
        egui::Area::new(egui::Id::new("toasts"))
            .order(egui::Order::Tooltip)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -40.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_max_width((screen.width() - 80.0).min(560.0));
                for t in &self.items[start..] {
                    let age = now - t.born;
                    let fade = (1.0 - ((age - LIFETIME + 0.6) / 0.6).clamp(0.0, 1.0)) as f32;
                    let (icon, col) = match t.kind {
                        ToastKind::Info => (Icon::Info, pal.accent),
                        ToastKind::Warn => (Icon::Warning, pal.warn),
                        ToastKind::Error => (Icon::Warning, pal.error),
                    };
                    egui::Frame::new()
                        .fill(pal.panel_raised.gamma_multiply(fade.max(0.05)))
                        .stroke(Stroke::new(1.0, pal.border.gamma_multiply(fade)))
                        .corner_radius(10)
                        .inner_margin(egui::Margin { left: 12, right: 14, top: 8, bottom: 8 })
                        .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: egui::Color32::from_black_alpha((70.0 * fade) as u8) })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                                icons::paint(ui.painter(), r, icon, col.gamma_multiply(fade));
                                ui.add(egui::Label::new(RichText::new(&t.text).color(pal.text_strong.gamma_multiply(fade))).wrap());
                            });
                        });
                    ui.add_space(6.0);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes() {
        let mut t = Toasts::default();
        t.push(ToastKind::Info, "a", 0.0);
        t.push(ToastKind::Info, "a", 1.0);
        t.push(ToastKind::Warn, "b", 1.0);
        assert_eq!(t.items.len(), 2);
        assert_eq!(t.items[0].born, 1.0);
    }
}

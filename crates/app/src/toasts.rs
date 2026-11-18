//! Toasts: short messages at the bottom of the window (SPEC §5.9:
//! refusals are toasts with a reason; nothing fails silently).

use crate::icons::{self, Icon};
use crate::theme::Palette;
use egui::{Align2, RichText, Stroke, vec2};

const LIFETIME: f64 = 4.5;
const MAX_SHOWN: usize = 3;

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
        self.items.push(Toast {
            kind,
            text,
            born: now,
        });
        log::info!(
            "toast: {}",
            self.items.last().map(|t| t.text.as_str()).unwrap_or("")
        );
    }

    /// Toasts stack in the bottom-right corner of the viewport (`area`, when
    /// it was drawn this frame), no wider than it, and above the tool status
    /// pill (`avoid`) when they would overlap it. With no viewport they use
    /// the window corner.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        pal: &Palette,
        now: f64,
        area: Option<egui::Rect>,
        avoid: Option<egui::Rect>,
    ) {
        self.items.retain(|t| {
            now - t.born < LIFETIME + if t.kind == ToastKind::Error { 3.0 } else { 0.0 }
        });
        if self.items.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        let screen = ctx.content_rect();
        let area = match area {
            Some(a) if a.is_positive() && screen.contains_rect(a) => a,
            _ => screen.shrink2(vec2(0.0, 24.0)),
        };
        let max_w = toast_width(area.width());
        let corner = toast_corner(area, max_w, avoid);
        let start = self.items.len().saturating_sub(MAX_SHOWN);
        egui::Area::new(egui::Id::new("toasts"))
            .order(egui::Order::Tooltip)
            .pivot(Align2::RIGHT_BOTTOM)
            .fixed_pos(corner)
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_max_width(max_w);
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
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
                            .inner_margin(egui::Margin {
                                left: 12,
                                right: 14,
                                top: 8,
                                bottom: 8,
                            })
                            .shadow(egui::Shadow {
                                offset: [0, 4],
                                blur: 14,
                                spread: 0,
                                color: egui::Color32::from_black_alpha((70.0 * fade) as u8),
                            })
                            .show(ui, |ui| {
                                // Size each toast to its text, up to the area width.
                                let text_w = ui
                                    .painter()
                                    .layout_no_wrap(
                                        t.text.clone(),
                                        egui::TextStyle::Body.resolve(ui.style()),
                                        pal.text_strong,
                                    )
                                    .size()
                                    .x;
                                let w = (16.0 + ui.spacing().item_spacing.x + text_w + 2.0)
                                    .min(ui.available_width());
                                ui.allocate_ui_with_layout(
                                    vec2(w, 0.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        let (r, _) = ui.allocate_exact_size(
                                            vec2(16.0, 16.0),
                                            egui::Sense::hover(),
                                        );
                                        icons::paint(
                                            ui.painter(),
                                            r,
                                            icon,
                                            col.gamma_multiply(fade),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&t.text)
                                                    .color(pal.text_strong.gamma_multiply(fade)),
                                            )
                                            .wrap(),
                                        );
                                    },
                                );
                            });
                        ui.add_space(6.0);
                    }
                });
            });
    }
}

const INSET: f32 = 14.0;

/// Widest a toast may be: a share of the viewport, but never wider than
/// the viewport minus both insets.
fn toast_width(area_w: f32) -> f32 {
    (area_w * 0.45)
        .clamp(220.0, 420.0)
        .min(area_w - 2.0 * INSET)
        .max(1.0)
}

/// Bottom-right corner of the toast stack: the viewport corner, raised
/// above the status pill when a stack this wide would reach it.
fn toast_corner(area: egui::Rect, max_w: f32, avoid: Option<egui::Rect>) -> egui::Pos2 {
    let mut corner = area.right_bottom() - vec2(INSET, INSET);
    if let Some(pill) = avoid
        && corner.x - max_w < pill.right() + 8.0
    {
        corner.y = corner.y.min(pill.top() - 8.0);
    }
    corner
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

    #[test]
    fn width_is_capped_by_the_viewport() {
        assert_eq!(toast_width(1200.0), 420.0);
        assert_eq!(toast_width(600.0), 270.0);
        assert_eq!(toast_width(300.0), 220.0);
        // Narrow viewports: never wider than the viewport minus insets.
        assert_eq!(toast_width(200.0), 172.0);
    }

    #[test]
    fn stack_rises_above_the_pill_only_when_it_would_overlap() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(440.0, 600.0));
        let pill = egui::Rect::from_center_size(egui::pos2(220.0, 576.0), vec2(150.0, 28.0));
        let w = toast_width(area.width());
        let c = toast_corner(area, w, Some(pill));
        assert!(c.y <= pill.top() - 8.0, "raised above the pill");
        let wide = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1400.0, 600.0));
        let pill = egui::Rect::from_center_size(egui::pos2(700.0, 576.0), vec2(150.0, 28.0));
        let c = toast_corner(wide, toast_width(wide.width()), Some(pill));
        assert_eq!(c.y, 600.0 - INSET, "wide viewport: stays in the corner");
        assert_eq!(toast_corner(wide, 420.0, None).y, 600.0 - INSET);
    }
}

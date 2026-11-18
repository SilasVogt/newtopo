//! Small shared widgets so dialogs and option groups look the same
//! everywhere: one modal frame, one dialog header (title, optional
//! subtitle, close button), one footer, a segmented option group and a
//! checkbox whose empty box stays visible on dark panels.

use crate::theme::Palette;
use egui::{Align, Color32, Layout, Margin, Response, RichText, Sense, Stroke, Ui, vec2};

/// Frame for every modal dialog.
pub fn modal_frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.panel_raised)
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius((pal.rounding * 1.6).round() as u8)
        .inner_margin(Margin::same(18))
        .shadow(egui::Shadow {
            offset: [0, 10],
            blur: 32,
            spread: 0,
            color: Color32::from_black_alpha(if pal.dark { 120 } else { 45 }),
        })
}

/// A modal with the shared frame.
pub fn modal(id: &str, pal: &Palette) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id)).frame(modal_frame(pal))
}

/// Dialog title row with a close button on the right. Returns true when
/// the close button was clicked.
pub fn dialog_header(ui: &mut Ui, pal: &Palette, title: &str, subtitle: Option<&str>) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                RichText::new(title)
                    .font(crate::theme::medium(16.0))
                    .color(pal.text_strong),
            );
            if let Some(s) = subtitle {
                ui.label(RichText::new(s).small().color(pal.text_dim));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if close_button(ui, pal).on_hover_text("Close (Esc)").clicked() {
                close = true;
            }
        });
    });
    ui.add_space(10.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, pal.separator);
    ui.add_space(12.0);
    close
}

/// The small "x" used by dialog headers.
pub fn close_button(ui: &mut Ui, pal: &Palette) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Close")
    });
    if ui.is_rect_visible(r) {
        if resp.hovered() {
            ui.painter()
                .rect_filled(r, pal.rounding * 0.7, pal.widget_hover);
        }
        let c = if resp.hovered() {
            pal.text_strong
        } else {
            pal.text_dim
        };
        let s = Stroke::new(1.4, c);
        let k = r.shrink(7.5);
        ui.painter()
            .line_segment([k.left_top(), k.right_bottom()], s);
        ui.painter()
            .line_segment([k.right_top(), k.left_bottom()], s);
    }
    resp
}

/// Footer separator plus right-aligned buttons: the primary action last
/// (rightmost). Returns `Some(0)` for the primary, `Some(i)` for
/// `secondary[i - 1]`.
pub fn dialog_footer(
    ui: &mut Ui,
    pal: &Palette,
    primary: &str,
    primary_enabled: bool,
    secondary: &[&str],
) -> Option<usize> {
    let mut out = None;
    ui.add_space(14.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, pal.separator);
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui
            .add_enabled(
                primary_enabled,
                egui::Button::new(RichText::new(primary).color(pal.accent_text))
                    .fill(pal.accent)
                    .min_size(vec2(84.0, 28.0)),
            )
            .clicked()
        {
            out = Some(0);
        }
        for (i, s) in secondary.iter().enumerate() {
            if ui
                .add(egui::Button::new(*s).min_size(vec2(72.0, 28.0)))
                .clicked()
            {
                out = Some(i + 1);
            }
        }
    });
    out
}

/// A connected group of options (wrap `selectable_value`s in it).
pub fn segmented<R>(ui: &mut Ui, pal: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(pal.inset)
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(pal.rounding.round() as u8)
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let w = &mut ui.style_mut().visuals.widgets;
            // Unselected options sit flat on the inset; the selected one
            // gets the selection fill.
            w.inactive.weak_bg_fill = Color32::TRANSPARENT;
            w.inactive.bg_fill = Color32::TRANSPARENT;
            ui.horizontal(add).inner
        })
        .inner
}

/// A checkbox with a visible outline on its empty box.
pub fn checkbox(ui: &mut Ui, value: &mut bool, text: impl Into<egui::WidgetText>) -> Response {
    ui.scope(|ui| {
        let v = ui.visuals_mut();
        let border = v.widgets.inactive.fg_stroke.color.gamma_multiply(0.45);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, border.gamma_multiply(1.6));
        ui.checkbox(value, text)
    })
    .inner
}

/// Right-aligned fixed-width cell (for numeric grid columns).
pub fn right_cell(ui: &mut Ui, width: f32, text: RichText) -> Response {
    ui.allocate_ui_with_layout(
        vec2(width, ui.spacing().interact_size.y),
        Layout::right_to_left(Align::Center),
        |ui| ui.label(text),
    )
    .inner
}

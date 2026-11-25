//! UI themes (SPEC §5.7): JSON files with the UI palette and default
//! viewport overlay colours. Two ship built in; users can drop more JSON
//! files into `<config>/themes/`.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    TextStyle,
};
use newtopo_render::OverlayColors;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

pub const BUILTIN_DARK: &str = include_str!("../../../assets/themes/graphite-dark.json");
pub const BUILTIN_LIGHT: &str = include_str!("../../../assets/themes/paper-light.json");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub bg: String,
    pub panel: String,
    pub panel_raised: String,
    pub inset: String,
    pub widget: String,
    pub widget_hover: String,
    pub widget_active: String,
    pub border: String,
    pub separator: String,
    pub text: String,
    pub text_dim: String,
    pub text_strong: String,
    pub accent: String,
    pub accent_text: String,
    pub selection_bg: String,
    pub warn: String,
    pub error: String,
    pub ok: String,
    pub rounding: f32,
    pub overlay: OverlayColors,
}

/// Parsed colours for drawing.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    pub panel_raised: Color32,
    pub inset: Color32,
    pub widget: Color32,
    pub widget_hover: Color32,
    pub widget_active: Color32,
    pub border: Color32,
    pub separator: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_strong: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub selection_bg: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub ok: Color32,
    pub rounding: f32,
}

pub fn parse_color(s: &str) -> Color32 {
    let h = s.trim().trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255);
    match h.len() {
        6 => Color32::from_rgb(b(0), b(2), b(4)),
        8 => Color32::from_rgba_unmultiplied(b(0), b(2), b(4), b(6)),
        _ => Color32::from_rgb(255, 0, 255),
    }
}

#[cfg(test)]
pub fn color_hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

impl Theme {
    pub fn builtin_dark() -> Theme {
        serde_json::from_str(BUILTIN_DARK).expect("built-in dark theme parses")
    }
    pub fn builtin_light() -> Theme {
        serde_json::from_str(BUILTIN_LIGHT).expect("built-in light theme parses")
    }

    /// Built-in themes plus any valid JSON themes in `dir`.
    pub fn available(dir: Option<&Path>) -> Vec<Theme> {
        let mut out = vec![Theme::builtin_dark(), Theme::builtin_light()];
        if let Some(dir) = dir
            && let Ok(rd) = std::fs::read_dir(dir)
        {
            let mut extra: Vec<Theme> = rd
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|s| serde_json::from_str::<Theme>(&s).ok())
                .filter(|t| !out.iter().any(|o: &Theme| o.name == t.name))
                .collect();
            extra.sort_by(|a, b| a.name.cmp(&b.name));
            out.extend(extra);
        }
        out
    }

    pub fn palette(&self) -> Palette {
        Palette {
            dark: self.dark,
            bg: parse_color(&self.bg),
            panel: parse_color(&self.panel),
            panel_raised: parse_color(&self.panel_raised),
            inset: parse_color(&self.inset),
            widget: parse_color(&self.widget),
            widget_hover: parse_color(&self.widget_hover),
            widget_active: parse_color(&self.widget_active),
            border: parse_color(&self.border),
            separator: parse_color(&self.separator),
            text: parse_color(&self.text),
            text_dim: parse_color(&self.text_dim),
            text_strong: parse_color(&self.text_strong),
            accent: parse_color(&self.accent),
            accent_text: parse_color(&self.accent_text),
            selection_bg: parse_color(&self.selection_bg),
            warn: parse_color(&self.warn),
            error: parse_color(&self.error),
            ok: parse_color(&self.ok),
            rounding: self.rounding,
        }
    }
}

/// Fonts: Noto Sans (OFL, vendored in `assets/fonts`) for the UI, egui's
/// built-in Hack for monospace.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "noto-sans".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/fonts/NotoSans-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "noto-sans-medium".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/fonts/NotoSans-Medium.ttf"
        ))),
    );
    if let Some(p) = fonts.families.get_mut(&FontFamily::Proportional) {
        p.insert(0, "noto-sans".into());
    }
    let mut strong = vec!["noto-sans-medium".to_string()];
    strong.extend(
        fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts
        .families
        .insert(FontFamily::Name("medium".into()), strong);
    ctx.set_fonts(fonts);
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

/// Apply a theme to egui's style.
pub fn apply(ctx: &egui::Context, theme: &Theme, scale: f32) {
    let p = theme.palette();
    let r = CornerRadius::same(p.rounding.round() as u8);
    let r_small = CornerRadius::same((p.rounding * 0.7).round() as u8);
    ctx.set_theme(if p.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(13.5)),
            (TextStyle::Button, FontId::proportional(13.5)),
            (TextStyle::Heading, medium(17.0)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 6.0);
        s.button_padding = egui::vec2(9.0, 4.0);
        s.interact_size = egui::vec2(36.0, 24.0);
        s.window_margin = Margin::same(14);
        s.menu_margin = Margin::same(6);
        s.indent = 16.0;
        s.slider_width = 150.0;
        s.combo_width = 150.0;
        s.icon_width = 15.0;
        s.icon_width_inner = 9.0;
        s.icon_spacing = 6.0;
        s.menu_spacing = 2.0;

        let v = &mut style.visuals;
        v.dark_mode = p.dark;
        v.override_text_color = None;
        v.panel_fill = p.panel;
        v.window_fill = p.panel_raised;
        v.window_stroke = Stroke::new(1.0, p.border);
        v.window_corner_radius = CornerRadius::same((p.rounding * 1.6).round() as u8);
        v.menu_corner_radius = r;
        v.window_shadow = Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }),
        };
        v.popup_shadow = Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 90 } else { 30 }),
        };
        v.extreme_bg_color = p.inset;
        v.text_edit_bg_color = Some(p.inset);
        v.faint_bg_color = p.panel_raised;
        v.code_bg_color = p.inset;
        v.warn_fg_color = p.warn;
        v.error_fg_color = p.error;
        v.hyperlink_color = p.accent;
        v.selection.bg_fill = p.selection_bg;
        v.selection.stroke = Stroke::new(1.0, p.accent);
        v.striped = false;
        v.indent_has_left_vline = false;
        v.slider_trailing_fill = true;
        v.collapsing_header_frame = false;
        v.window_highlight_topmost = false;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = p.panel;
        w.noninteractive.weak_bg_fill = p.panel;
        w.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
        w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
        w.noninteractive.corner_radius = r_small;

        w.inactive.bg_fill = p.widget;
        w.inactive.weak_bg_fill = p.widget;
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(1.0, p.text);
        w.inactive.corner_radius = r_small;

        w.hovered.bg_fill = p.widget_hover;
        w.hovered.weak_bg_fill = p.widget_hover;
        w.hovered.bg_stroke = Stroke::new(1.0, p.border);
        w.hovered.fg_stroke = Stroke::new(1.5, p.text_strong);
        w.hovered.corner_radius = r_small;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = p.widget_active;
        w.active.weak_bg_fill = p.widget_active;
        w.active.bg_stroke = Stroke::new(1.0, p.accent);
        w.active.fg_stroke = Stroke::new(1.5, p.text_strong);
        w.active.corner_radius = r_small;
        w.active.expansion = 0.0;

        w.open.bg_fill = p.widget_active;
        w.open.weak_bg_fill = p.widget_hover;
        w.open.bg_stroke = Stroke::new(1.0, p.border);
        w.open.fg_stroke = Stroke::new(1.0, p.text_strong);
        w.open.corner_radius = r_small;
    });
    ctx.set_zoom_factor(scale.clamp(0.75, 2.0));
}

/// egui_dock style matching the theme.
pub fn dock_style(style: &egui::Style, p: &Palette) -> egui_dock::Style {
    let mut ds = egui_dock::Style::from_egui(style);
    ds.main_surface_border_stroke = Stroke::NONE;
    ds.main_surface_border_rounding = CornerRadius::ZERO;
    ds.separator.width = 3.0;
    ds.separator.color_idle = p.bg;
    ds.separator.color_hovered = p.accent.gamma_multiply(0.6);
    ds.separator.color_dragged = p.accent;
    ds.tab_bar.bg_fill = p.bg;
    ds.tab_bar.height = 30.0;
    ds.tab_bar.hline_color = p.bg;
    ds.tab_bar.corner_radius = CornerRadius::ZERO;
    ds.tab.tab_body.bg_fill = p.panel;
    ds.tab.tab_body.stroke = Stroke::NONE;
    ds.tab.tab_body.corner_radius = CornerRadius::ZERO;
    ds.tab.tab_body.inner_margin = Margin::same(10);
    let rr = CornerRadius {
        nw: p.rounding as u8,
        ne: p.rounding as u8,
        sw: 0,
        se: 0,
    };
    ds.tab.active.bg_fill = p.panel;
    ds.tab.active.text_color = p.text_strong;
    ds.tab.active.outline_color = p.panel;
    ds.tab.active.corner_radius = rr;
    ds.tab.focused = ds.tab.active.clone();
    ds.tab.focused.text_color = p.text_strong;
    ds.tab.inactive.bg_fill = p.bg;
    ds.tab.inactive.text_color = p.text_dim;
    ds.tab.inactive.outline_color = p.bg;
    ds.tab.inactive.corner_radius = rr;
    ds.tab.hovered.bg_fill = p.widget;
    ds.tab.hovered.text_color = p.text;
    ds.tab.hovered.outline_color = p.widget;
    ds.tab.hovered.corner_radius = rr;
    ds.tab.active_with_kb_focus = ds.tab.active.clone();
    ds.tab.inactive_with_kb_focus = ds.tab.inactive.clone();
    ds.tab.focused_with_kb_focus = ds.tab.focused.clone();
    ds.tab.hline_below_active_tab_name = false;
    ds.buttons.close_tab_color = p.text_dim;
    ds.buttons.close_tab_active_color = p.text_strong;
    ds.buttons.close_tab_bg_fill = p.widget_hover;
    ds.overlay.selection_color = p.accent.gamma_multiply(0.35);
    ds.overlay.button_color = p.widget_hover;
    ds.overlay.button_border_stroke = Stroke::new(1.0, p.border);
    ds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_parse() {
        let d = Theme::builtin_dark();
        let l = Theme::builtin_light();
        assert!(d.dark && !l.dark);
        assert_eq!(parse_color("#3ec7a5"), Color32::from_rgb(0x3e, 0xc7, 0xa5));
        assert_eq!(color_hex(parse_color("#14171bf0")), "#14171bf0");
    }

    /// User themes written before the radial menu was removed still parse.
    #[test]
    fn theme_with_old_radial_colours_parses() {
        let old = BUILTIN_DARK.replacen(
            "\"rounding\"",
            "\"radial_bg\": \"#14171bf0\", \"radial_slot\": \"#232930\", \"radial_hover\": \"#3ec7a5\", \"rounding\"",
            1,
        );
        assert!(old.contains("radial_bg"));
        let t: Theme = serde_json::from_str(&old).unwrap();
        assert_eq!(t, Theme::builtin_dark());
    }
}

//! Icons drawn with egui's painter, designed for newtopo (no icon font, no
//! third-party art). Each icon is drawn into a square in a 16-unit grid.

use egui::{Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Vertex,
    Edge,
    Face,
    Object,
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Preview,
    AutoFill,
    Link,
    Symmetry,
    Search,
    Plus,
    Trash,
    ChevronRight,
    ChevronDown,
    Star,
    StarOutline,
    Layer,
    Body,
    Connect,
    Grid,
    Slice,
    Iso,
    Cut,
    Weld,
    Collapse,
    Remove,
    Undo,
    Redo,
    Import,
    Export,
    Save,
    Open,
    Gear,
    Command,
    Radial,
    Rename,
    Bridge,
    Uv,
    Frame,
    Cube,
    Warning,
    Info,
    Check,
    Close,
    Dots,
    Keyboard,
}

/// Map a 16-unit design coordinate into `rect`.
struct Grid16 {
    o: Pos2,
    s: f32,
}

impl Grid16 {
    fn new(rect: Rect) -> Self {
        let side = rect.width().min(rect.height());
        let o = rect.center() - vec2(side, side) * 0.5;
        Grid16 { o, s: side / 16.0 }
    }
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.o + vec2(x, y) * self.s
    }
    fn r(&self, v: f32) -> f32 {
        v * self.s
    }
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let g = Grid16::new(rect);
    let w = (g.s * 1.35).max(1.0);
    let st = Stroke::new(w, color);
    let thin = Stroke::new((w * 0.75).max(1.0), color);
    let line = |a: (f32, f32), b: (f32, f32)| painter.line_segment([g.p(a.0, a.1), g.p(b.0, b.1)], st);
    let poly = |pts: &[(f32, f32)], closed: bool, s: Stroke| {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| g.p(x, y)).collect();
        if closed {
            painter.add(Shape::closed_line(pts, s));
        } else {
            painter.add(Shape::line(pts, s));
        }
    };
    let fill = |pts: &[(f32, f32)], c: Color32| {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| g.p(x, y)).collect();
        painter.add(Shape::convex_polygon(pts, c, Stroke::NONE));
    };
    let dot = |x: f32, y: f32, r: f32| painter.circle_filled(g.p(x, y), g.r(r), color);
    let ring = |x: f32, y: f32, r: f32| painter.circle_stroke(g.p(x, y), g.r(r), st);
    let faint = color.gamma_multiply(0.35);
    match icon {
        Icon::Vertex => {
            poly(&[(3.0, 12.5), (8.0, 3.5), (13.0, 12.5)], true, Stroke::new(w * 0.7, faint));
            dot(3.0, 12.5, 1.6);
            dot(8.0, 3.5, 2.3);
            dot(13.0, 12.5, 1.6);
        }
        Icon::Edge => {
            poly(&[(3.0, 12.5), (8.0, 3.5), (13.0, 12.5)], true, Stroke::new(w * 0.7, faint));
            painter.line_segment([g.p(3.0, 12.5), g.p(8.0, 3.5)], Stroke::new(w * 1.6, color));
        }
        Icon::Face => {
            fill(&[(3.0, 12.5), (8.0, 3.5), (13.0, 12.5)], color.gamma_multiply(0.75));
            poly(&[(3.0, 12.5), (8.0, 3.5), (13.0, 12.5)], true, thin);
        }
        Icon::Object | Icon::Cube | Icon::Body => {
            poly(&[(8.0, 2.5), (13.5, 5.3), (13.5, 11.0), (8.0, 13.8), (2.5, 11.0), (2.5, 5.3)], true, st);
            poly(&[(2.5, 5.3), (8.0, 8.1), (13.5, 5.3)], false, thin);
            line((8.0, 8.1), (8.0, 13.8));
            if icon == Icon::Body {
                fill(&[(8.0, 8.1), (13.5, 5.3), (13.5, 11.0), (8.0, 13.8)], faint);
            }
        }
        Icon::Eye | Icon::EyeOff => {
            let pts: Vec<(f32, f32)> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0 * std::f32::consts::PI;
                    (8.0 - 6.0 * t.cos(), 8.0 - 3.6 * t.sin())
                })
                .chain((0..=16).map(|i| {
                    let t = i as f32 / 16.0 * std::f32::consts::PI;
                    (8.0 + 6.0 * t.cos(), 8.0 + 3.6 * t.sin())
                }))
                .collect();
            poly(&pts, true, thin);
            dot(8.0, 8.0, 1.9);
            if icon == Icon::EyeOff {
                line((3.0, 13.0), (13.0, 3.0));
            }
        }
        Icon::Lock | Icon::Unlock => {
            painter.rect_filled(Rect::from_min_max(g.p(3.5, 7.5), g.p(12.5, 13.5)), g.r(1.5), color);
            let arc: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0 * std::f32::consts::PI;
                    (8.0 - 3.0 * t.cos(), 5.5 - 2.5 * t.sin())
                })
                .collect();
            if icon == Icon::Lock {
                let mut a = vec![(5.0, 7.5)];
                a.extend(arc);
                a.push((11.0, 7.5));
                poly(&a, false, st);
            } else {
                let mut a = vec![(5.0, 5.5)];
                a.extend(arc);
                a.insert(0, (5.0, 7.5));
                poly(&a, false, st);
            }
        }
        Icon::Preview => {
            fill(&[(2.5, 4.5), (8.0, 2.0), (13.5, 4.5), (8.0, 7.0)], faint);
            poly(&[(2.5, 4.5), (8.0, 2.0), (13.5, 4.5), (8.0, 7.0)], true, thin);
            poly(&[(2.5, 4.5), (2.5, 11.0), (8.0, 14.0), (13.5, 11.0), (13.5, 4.5)], false, st);
            line((8.0, 7.0), (8.0, 14.0));
            line((5.25, 5.75), (5.25, 12.5));
            line((10.75, 5.75), (10.75, 12.5));
        }
        Icon::AutoFill => {
            poly(&[(2.5, 3.0), (13.5, 3.0), (13.5, 13.0), (2.5, 13.0)], true, st);
            line((8.0, 3.0), (8.0, 13.0));
            line((2.5, 8.0), (13.5, 8.0));
            painter.line_segment([g.p(8.0, 8.0), g.p(13.5, 13.0)], Stroke::new(w * 0.8, color));
        }
        Icon::Link => {
            painter.rect_stroke(
                Rect::from_min_max(g.p(1.5, 5.5), g.p(9.0, 10.5)),
                g.r(2.5),
                st,
                egui::StrokeKind::Middle,
            );
            painter.rect_stroke(
                Rect::from_min_max(g.p(7.0, 5.5), g.p(14.5, 10.5)),
                g.r(2.5),
                st,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Symmetry => {
            painter.add(Shape::dashed_line(&[g.p(8.0, 1.5), g.p(8.0, 14.5)], thin, g.r(1.6), g.r(1.4)));
            fill(&[(6.5, 3.5), (6.5, 12.5), (2.0, 12.5)], color);
            poly(&[(9.5, 3.5), (9.5, 12.5), (14.0, 12.5)], true, thin);
        }
        Icon::Search => {
            ring(7.0, 7.0, 4.2);
            line((10.2, 10.2), (13.8, 13.8));
        }
        Icon::Plus => {
            line((8.0, 3.0), (8.0, 13.0));
            line((3.0, 8.0), (13.0, 8.0));
        }
        Icon::Trash => {
            line((3.0, 4.5), (13.0, 4.5));
            poly(&[(6.0, 4.5), (6.5, 2.5), (9.5, 2.5), (10.0, 4.5)], false, thin);
            poly(&[(4.3, 4.5), (5.0, 13.5), (11.0, 13.5), (11.7, 4.5)], false, st);
        }
        Icon::ChevronRight => poly(&[(6.0, 4.0), (10.0, 8.0), (6.0, 12.0)], false, st),
        Icon::ChevronDown => poly(&[(4.0, 6.0), (8.0, 10.0), (12.0, 6.0)], false, st),
        Icon::Star | Icon::StarOutline => {
            let pts: Vec<(f32, f32)> = (0..10)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    let r = if i % 2 == 0 { 6.3 } else { 2.7 };
                    (8.0 + r * a.cos(), 8.6 + r * a.sin())
                })
                .collect();
            if icon == Icon::Star {
                // Non-convex: fill as a fan of triangles around the centre.
                for i in 0..10 {
                    let a = pts[i];
                    let b = pts[(i + 1) % 10];
                    fill(&[(8.0, 8.6), a, b], color);
                }
            } else {
                poly(&pts, true, thin);
            }
        }
        Icon::Layer => {
            fill(&[(8.0, 2.5), (14.0, 5.5), (8.0, 8.5), (2.0, 5.5)], faint);
            poly(&[(8.0, 2.5), (14.0, 5.5), (8.0, 8.5), (2.0, 5.5)], true, thin);
            poly(&[(2.0, 8.3), (8.0, 11.3), (14.0, 8.3)], false, thin);
            poly(&[(2.0, 11.0), (8.0, 14.0), (14.0, 11.0)], false, thin);
        }
        Icon::Connect => {
            // A curved band with lines following the bend.
            let top: Vec<(f32, f32)> = (0..=10).map(|i| {
                let t = i as f32 / 10.0;
                (2.5 + 11.0 * t, 5.0 - 2.5 * (t * std::f32::consts::PI).sin())
            }).collect();
            let bot: Vec<(f32, f32)> = top.iter().map(|&(x, y)| (x, y + 8.0)).collect();
            poly(&top, false, thin);
            poly(&bot, false, thin);
            for t in [0.25f32, 0.5, 0.75] {
                let x = 2.5 + 11.0 * t;
                let y = 5.0 - 2.5 * (t * std::f32::consts::PI).sin();
                line((x, y), (x, y + 8.0));
            }
        }
        Icon::Grid => {
            poly(&[(2.5, 2.5), (13.5, 2.5), (13.5, 13.5), (2.5, 13.5)], true, thin);
            for k in [6.17f32, 9.83] {
                line((k, 2.5), (k, 13.5));
                line((2.5, k), (13.5, k));
            }
        }
        Icon::Slice => {
            for k in [4.0f32, 8.0, 12.0] {
                poly(&[(k - 1.5, 2.5), (k + 1.5, 4.0), (k + 1.5, 13.5), (k - 1.5, 12.0)], true, thin);
            }
            line((1.5, 8.0), (14.5, 8.0));
        }
        Icon::Iso => {
            let c: Vec<(f32, f32)> = (0..=10).map(|i| {
                let t = i as f32 / 10.0;
                (2.5 + 11.0 * t, 11.5 - 3.0 * (t * std::f32::consts::PI).sin())
            }).collect();
            poly(&c, false, thin);
            line((8.0, 2.5), (8.0, 8.5));
            dot(8.0, 8.5, 1.6);
        }
        Icon::Cut => {
            poly(&[(2.5, 12.5), (6.5, 6.0), (10.0, 9.5), (13.5, 3.5)], false, st);
            dot(2.5, 12.5, 1.5);
            dot(6.5, 6.0, 1.5);
            dot(10.0, 9.5, 1.5);
            dot(13.5, 3.5, 1.5);
        }
        Icon::Weld => {
            dot(3.5, 12.0, 1.8);
            ring(12.0, 4.0, 2.4);
            dot(12.0, 4.0, 1.0);
            painter.add(Shape::dashed_line(&[g.p(4.5, 11.0), g.p(10.0, 5.8)], thin, g.r(1.4), g.r(1.2)));
        }
        Icon::Collapse => {
            line((2.5, 8.0), (6.0, 8.0));
            line((10.0, 8.0), (13.5, 8.0));
            poly(&[(4.5, 6.0), (6.5, 8.0), (4.5, 10.0)], false, thin);
            poly(&[(11.5, 6.0), (9.5, 8.0), (11.5, 10.0)], false, thin);
            dot(8.0, 8.0, 1.6);
        }
        Icon::Remove => {
            poly(&[(2.5, 2.5), (13.5, 2.5), (13.5, 13.5), (2.5, 13.5)], true, thin);
            painter.add(Shape::dashed_line(&[g.p(8.0, 2.5), g.p(8.0, 13.5)], st, g.r(1.5), g.r(1.3)));
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            let fx = |x: f32| if flip < 0.0 { 16.0 - x } else { x };
            let arc: Vec<(f32, f32)> = (0..=12).map(|i| {
                let t = i as f32 / 12.0 * std::f32::consts::PI;
                (fx(8.5 + 4.5 * t.sin() * 0.95), 9.0 - 4.0 * t.cos())
            }).collect();
            let mut a = vec![(fx(4.0), 5.0)];
            a.extend(arc);
            a.push((fx(5.0), 13.0));
            poly(&a, false, st);
            poly(&[(fx(6.5), 2.5), (fx(4.0), 5.0), (fx(6.5), 7.5)], false, st);
        }
        Icon::Import | Icon::Export => {
            poly(&[(2.5, 9.5), (2.5, 13.5), (13.5, 13.5), (13.5, 9.5)], false, st);
            if icon == Icon::Import {
                line((8.0, 2.0), (8.0, 10.0));
                poly(&[(5.0, 7.0), (8.0, 10.0), (11.0, 7.0)], false, st);
            } else {
                line((8.0, 10.0), (8.0, 2.0));
                poly(&[(5.0, 5.0), (8.0, 2.0), (11.0, 5.0)], false, st);
            }
        }
        Icon::Save => {
            poly(&[(2.5, 2.5), (11.0, 2.5), (13.5, 5.0), (13.5, 13.5), (2.5, 13.5)], true, st);
            poly(&[(5.0, 2.5), (5.0, 6.0), (10.5, 6.0), (10.5, 2.5)], false, thin);
            painter.rect_filled(Rect::from_min_max(g.p(5.0, 9.0), g.p(11.0, 13.5)), 0.0, faint);
        }
        Icon::Open => {
            poly(&[(2.0, 13.0), (2.0, 3.5), (6.0, 3.5), (7.5, 5.0), (12.5, 5.0), (12.5, 7.0)], false, st);
            poly(&[(2.0, 13.0), (4.0, 7.5), (14.5, 7.5), (12.5, 13.0)], true, st);
        }
        Icon::Gear => {
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                line((8.0 + 4.0 * a.cos(), 8.0 + 4.0 * a.sin()), (8.0 + 6.2 * a.cos(), 8.0 + 6.2 * a.sin()));
            }
            ring(8.0, 8.0, 4.0);
            ring(8.0, 8.0, 1.6);
        }
        Icon::Command => {
            poly(&[(2.0, 4.0), (6.0, 8.0), (2.0, 12.0)], false, st);
            line((8.0, 12.0), (14.0, 12.0));
        }
        Icon::Radial => {
            ring(8.0, 8.0, 5.5);
            for i in 0..6 {
                let a = i as f32 * std::f32::consts::TAU / 6.0;
                dot(8.0 + 5.5 * a.cos(), 8.0 + 5.5 * a.sin(), 1.3);
            }
            dot(8.0, 8.0, 1.2);
        }
        Icon::Rename => {
            painter.rect_stroke(Rect::from_min_max(g.p(1.5, 4.5), g.p(14.5, 11.5)), g.r(1.5), thin, egui::StrokeKind::Middle);
            line((10.5, 2.5), (10.5, 13.5));
            line((9.0, 2.5), (12.0, 2.5));
            line((9.0, 13.5), (12.0, 13.5));
            line((4.0, 8.0), (7.5, 8.0));
        }
        Icon::Bridge => {
            ring(3.5, 8.0, 2.0);
            ring(12.5, 8.0, 2.0);
            painter.add(Shape::dashed_line(&[g.p(5.5, 8.0), g.p(10.5, 8.0)], st, g.r(1.3), g.r(1.0)));
        }
        Icon::Uv => {
            poly(&[(2.5, 2.5), (13.5, 2.5), (13.5, 13.5), (2.5, 13.5)], true, thin);
            fill(&[(4.5, 4.5), (8.0, 4.5), (8.0, 8.0), (4.5, 8.0)], color);
            fill(&[(8.0, 8.0), (11.5, 8.0), (11.5, 11.5), (8.0, 11.5)], color);
        }
        Icon::Frame => {
            for (x, y, dx, dy) in [(2.5, 2.5, 1.0, 1.0), (13.5, 2.5, -1.0, 1.0), (2.5, 13.5, 1.0, -1.0), (13.5, 13.5, -1.0, -1.0)] {
                poly(&[(x + dx * 3.5, y), (x, y), (x, y + dy * 3.5)], false, st);
            }
            dot(8.0, 8.0, 1.8);
        }
        Icon::Warning => {
            poly(&[(8.0, 2.0), (14.5, 13.5), (1.5, 13.5)], true, st);
            line((8.0, 6.0), (8.0, 9.5));
            dot(8.0, 11.6, 0.9);
        }
        Icon::Info => {
            ring(8.0, 8.0, 6.0);
            line((8.0, 7.2), (8.0, 11.5));
            dot(8.0, 4.9, 0.9);
        }
        Icon::Check => poly(&[(3.0, 8.5), (6.5, 12.0), (13.0, 4.5)], false, st),
        Icon::Close => {
            line((4.0, 4.0), (12.0, 12.0));
            line((12.0, 4.0), (4.0, 12.0));
        }
        Icon::Dots => {
            dot(3.5, 8.0, 1.3);
            dot(8.0, 8.0, 1.3);
            dot(12.5, 8.0, 1.3);
        }
        Icon::Keyboard => {
            painter.rect_stroke(Rect::from_min_max(g.p(1.5, 4.0), g.p(14.5, 12.5)), g.r(1.5), thin, egui::StrokeKind::Middle);
            for x in [4.0f32, 6.7, 9.4, 12.0] {
                dot(x, 6.8, 0.7);
            }
            line((5.0, 9.8), (11.0, 9.8));
        }
    }
}

/// A square, frameless icon button. `on` draws it as active (accent).
pub fn icon_button(ui: &mut Ui, icon: Icon, on: bool, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let accent = v.selection.stroke.color;
        let (bg, fg) = if on {
            (Some(accent.gamma_multiply(0.22)), accent)
        } else if resp.hovered() {
            (Some(v.widgets.hovered.weak_bg_fill), v.widgets.hovered.fg_stroke.color)
        } else {
            (None, v.widgets.inactive.fg_stroke.color)
        };
        if let Some(bg) = bg {
            ui.painter().rect_filled(rect, v.widgets.inactive.corner_radius, bg);
        }
        let inner = rect.shrink(size * 0.2);
        paint(ui.painter(), inner, icon, fg);
    }
    resp
}

/// Icon + label button used in toolbars (keeps the label short).
pub fn icon_text_button(ui: &mut Ui, icon: Icon, text: &str, on: bool) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let h = ui.spacing().interact_size.y;
    let w = h * 0.72 + 6.0 + galley.size().x + 14.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let accent = v.selection.stroke.color;
        let (bg, fg) = if on {
            (accent.gamma_multiply(0.2), accent)
        } else if resp.hovered() {
            (v.widgets.hovered.weak_bg_fill, v.widgets.hovered.fg_stroke.color)
        } else {
            (Color32::TRANSPARENT, v.widgets.inactive.fg_stroke.color)
        };
        ui.painter().rect_filled(rect, v.widgets.inactive.corner_radius, bg);
        let ir = Rect::from_center_size(pos2(rect.left() + 7.0 + h * 0.36, rect.center().y), Vec2::splat(h * 0.6));
        paint(ui.painter(), ir, icon, fg);
        ui.painter().galley(pos2(ir.right() + 6.0, rect.center().y - galley.size().y * 0.5), galley, fg);
    }
    resp
}

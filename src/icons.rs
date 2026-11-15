//! Small vector icon set drawn with the painter (the bundled fonts lack these glyphs),
//! plus a few styled buttons built on top of it.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Undo,
    Redo,
    Plus,
    Export,
    Keyboard,
    Theme,
    Back,
    Cursor,
    Pencil,
    Folder,
    Trash,
    More,
    Speaker,
    SpeakerOff,
    Headphones,
    Follow,
    Music,
}

/// Draw `icon` centred in `rect` (designed on a 16x16 grid).
pub fn paint(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 16.0;
    let o = rect.center() - vec2(8.0 * s, 8.0 * s);
    let pt = |x: f32, y: f32| -> Pos2 { o + vec2(x * s, y * s) };
    let st = Stroke::new(1.6 * s.max(0.75), color);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(pts.iter().map(|&(x, y)| pt(x, y)).collect(), st));
    };
    match icon {
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![pt(4.5, 2.5), pt(13.5, 8.0), pt(4.5, 13.5)], color, Stroke::NONE));
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(pt(4.0, 3.0), pt(7.0, 13.0)), CornerRadius::same(1), color);
            p.rect_filled(Rect::from_min_max(pt(9.0, 3.0), pt(12.0, 13.0)), CornerRadius::same(1), color);
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_min_max(pt(3.5, 3.5), pt(12.5, 12.5)), CornerRadius::same(2), color);
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            let fx = |x: f32| if flip < 0.0 { 16.0 - x } else { x };
            let arc: Vec<(f32, f32)> = (0..=10)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 10.0);
                    (fx(8.5 - 4.5 * a.cos()), 9.5 + 4.5 * a.sin())
                })
                .collect();
            line(&arc);
            line(&[(fx(4.0), 9.5), (fx(4.0), 13.0)]);
            line(&[(fx(1.8), 7.3), (fx(4.0), 9.8), (fx(6.4), 7.6)]);
        }
        Icon::Plus => {
            line(&[(8.0, 3.0), (8.0, 13.0)]);
            line(&[(3.0, 8.0), (13.0, 8.0)]);
        }
        Icon::Export => {
            line(&[(8.0, 2.5), (8.0, 10.0)]);
            line(&[(5.0, 5.5), (8.0, 2.5), (11.0, 5.5)]);
            line(&[(3.0, 9.0), (3.0, 13.5), (13.0, 13.5), (13.0, 9.0)]);
        }
        Icon::Keyboard => {
            p.rect_stroke(Rect::from_min_max(pt(1.5, 4.0), pt(14.5, 12.0)), CornerRadius::same(2), st, StrokeKind::Middle);
            for x in [4.0, 6.5, 9.0, 11.5] {
                p.circle_filled(pt(x, 6.8), 0.8 * s, color);
            }
            line(&[(5.0, 9.6), (11.0, 9.6)]);
        }
        Icon::Theme => {
            let c = pt(8.0, 8.0);
            p.circle_stroke(c, 5.5 * s, st);
            let half: Vec<Pos2> = (0..=16).map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 16.0;
                c + vec2(a.cos(), a.sin()) * 5.5 * s
            }).collect();
            p.add(Shape::convex_polygon(half, color, Stroke::NONE));
        }
        Icon::Back => line(&[(10.0, 3.0), (5.0, 8.0), (10.0, 13.0)]),
        Icon::Cursor => {
            p.add(Shape::convex_polygon(vec![pt(4.0, 2.0), pt(12.5, 9.5), pt(8.5, 10.0), pt(6.2, 14.0)], color, Stroke::NONE));
        }
        Icon::Pencil => {
            line(&[(3.0, 13.0), (4.0, 10.0), (11.0, 3.0), (13.0, 5.0), (6.0, 12.0), (3.0, 13.0)]);
            line(&[(9.5, 4.5), (11.5, 6.5)]);
        }
        Icon::Folder => {
            line(&[(2.0, 4.0), (6.0, 4.0), (7.5, 5.5), (14.0, 5.5), (14.0, 12.5), (2.0, 12.5), (2.0, 4.0)]);
        }
        Icon::Trash => {
            line(&[(3.0, 4.5), (13.0, 4.5)]);
            line(&[(6.0, 4.5), (6.5, 2.5), (9.5, 2.5), (10.0, 4.5)]);
            line(&[(4.5, 4.5), (5.3, 13.5), (10.7, 13.5), (11.5, 4.5)]);
        }
        Icon::More => {
            for x in [3.5, 8.0, 12.5] {
                p.circle_filled(pt(x, 8.0), 1.3 * s, color);
            }
        }
        Icon::Speaker | Icon::SpeakerOff => {
            p.add(Shape::convex_polygon(vec![pt(2.0, 6.0), pt(5.0, 6.0), pt(8.5, 3.0), pt(8.5, 13.0), pt(5.0, 10.0), pt(2.0, 10.0)], color, Stroke::NONE));
            if icon == Icon::Speaker {
                line(&[(10.5, 6.0), (11.3, 8.0), (10.5, 10.0)]);
                line(&[(12.5, 4.5), (13.8, 8.0), (12.5, 11.5)]);
            } else {
                line(&[(10.5, 5.5), (14.5, 10.5)]);
                line(&[(14.5, 5.5), (10.5, 10.5)]);
            }
        }
        Icon::Headphones => {
            let arc: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
                    (8.0 + 5.5 * a.cos(), 9.0 + 5.5 * a.sin())
                })
                .collect();
            line(&arc);
            p.rect_filled(Rect::from_min_max(pt(2.0, 9.0), pt(4.8, 13.5)), CornerRadius::same(1), color);
            p.rect_filled(Rect::from_min_max(pt(11.2, 9.0), pt(14.0, 13.5)), CornerRadius::same(1), color);
        }
        Icon::Follow => {
            line(&[(3.0, 3.0), (3.0, 13.0)]);
            line(&[(6.0, 8.0), (13.0, 8.0)]);
            line(&[(10.0, 5.0), (13.0, 8.0), (10.0, 11.0)]);
        }
        Icon::Music => {
            p.circle_filled(pt(5.5, 12.0), 2.3 * s, color);
            line(&[(7.6, 12.0), (7.6, 2.5), (12.5, 4.0)]);
        }
    }
}

/// Square icon button. `on` draws it in the accent colour (for toggles).
pub fn button(ui: &mut Ui, pal: &Palette, icon: Icon, tip: &str, on: bool, enabled: bool) -> Response {
    let size = 30.0;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let hovered = enabled && resp.hovered();
    let bg = if on {
        pal.accent.gamma_multiply(if hovered { 0.32 } else { 0.22 })
    } else if hovered {
        pal.text.gamma_multiply(0.10)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
    let fg = if !enabled {
        pal.text_weak.gamma_multiply(0.5)
    } else if on {
        pal.accent
    } else {
        pal.text
    };
    paint(ui.painter(), rect.shrink(7.0), icon, fg);
    let resp = resp.on_hover_text(tip);
    if hovered { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// The big round play/pause button.
pub fn play_button(ui: &mut Ui, pal: &Palette, playing: bool, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(40.0), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && resp.hovered();
    let fill = if !enabled {
        pal.text_weak.gamma_multiply(0.4)
    } else if hovered {
        pal.accent.gamma_multiply(1.0).lerp_to_gamma(pal.text, 0.12)
    } else {
        pal.accent
    };
    ui.painter().circle_filled(rect.center(), 20.0, fill);
    let icon_rect = Rect::from_center_size(rect.center() + vec2(if playing { 0.0 } else { 1.0 }, 0.0), Vec2::splat(18.0));
    paint(ui.painter(), icon_rect, if playing { Icon::Pause } else { Icon::Play }, pal.bg);
    resp.on_hover_text(if playing { "Pause (Space)" } else { "Play (Space)" })
}

/// Accent-filled text button with an icon, for the main action on a screen.
pub fn primary(ui: &mut Ui, pal: &Palette, icon: Icon, label: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), egui::FontId::proportional(14.0), pal.bg);
    let w = 16.0 + 16.0 + 8.0 + galley.size().x + 16.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    let fill = if resp.hovered() { pal.accent.lerp_to_gamma(pal.text, 0.12) } else { pal.accent };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    paint(ui.painter(), Rect::from_min_size(rect.min + vec2(14.0, 8.0), Vec2::splat(16.0)), icon, pal.bg);
    ui.painter().galley(pos2(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0), galley, pal.bg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A pill-shaped chip; used for the snap divisor row.
pub fn chip(ui: &mut Ui, pal: &Palette, label: &str, selected: bool, tip: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), egui::FontId::proportional(12.5), if selected { pal.bg } else { pal.text });
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 14.0, 24.0), Sense::click());
    let fill = if selected {
        pal.accent
    } else if resp.hovered() {
        pal.text.gamma_multiply(0.12)
    } else {
        pal.text.gamma_multiply(0.05)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, if selected { pal.bg } else { pal.text });
    resp.on_hover_text(tip)
}

/// Thin labelled group frame used on the tool strip.
pub fn group<R>(ui: &mut Ui, pal: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(pal.text.gamma_multiply(0.04))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(6, 3))
        .stroke(Stroke::new(1.0, pal.border))
        .show(ui, |ui| ui.horizontal(|ui| add(ui)).inner)
        .inner
}

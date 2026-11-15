//! Colour palettes (5 light, 5 dark) and smooth blending between them.

use std::time::Instant;

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub name: &'static str,
    pub dark: bool,
    /// Main window background.
    pub bg: Color32,
    /// Raised surfaces: windows, menus, cards.
    pub panel: Color32,
    /// Text fields and other sunken areas.
    pub surface: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    /// Timeline lanes.
    pub lane: Color32,
    pub wave_bg: Color32,
    pub wave: Color32,
    pub grid_measure: Color32,
    pub grid_beat: Color32,
    pub grid_sub: Color32,
    pub playhead: Color32,
    pub red_line: Color32,
    pub vol_line: Color32,
    pub vol_fill: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warn: Color32,
    pub tracks: [Color32; 8],
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn rgba(hex: u32, a: u8) -> Color32 {
    // Premultiplied, so blending stays correct.
    let r = ((hex >> 16) & 0xff) * a as u32 / 255;
    let g = ((hex >> 8) & 0xff) * a as u32 / 255;
    let b = (hex & 0xff) * a as u32 / 255;
    Color32::from_rgba_premultiplied(r as u8, g as u8, b as u8, a)
}

pub const DARK: [Palette; 5] = [
    Palette {
        name: "Midnight",
        dark: true,
        bg: rgb(0x16181f), panel: rgb(0x1e2129), surface: rgb(0x0f1116), border: rgb(0x2c303b),
        text: rgb(0xe6e8ee), text_weak: rgb(0x8a90a0), accent: rgb(0x6e8cff),
        lane: rgb(0x1a1d26), wave_bg: rgb(0x12141b), wave: rgb(0x6982e6),
        grid_measure: rgb(0x8c93a6), grid_beat: rgb(0x4a5062), grid_sub: rgba(0x8c96c8, 48),
        playhead: rgb(0xffe678), red_line: rgb(0xdc4646), vol_line: rgb(0x8aa2ff), vol_fill: rgba(0x6e8cff, 52),
        danger: rgb(0xf06e6e), success: rgb(0x78d28c), warn: rgb(0xf0b450),
        tracks: [rgb(0x6e8cff), rgb(0xff8c5a), rgb(0x5ac88c), rgb(0xf0c850), rgb(0xc86ee6), rgb(0x50c8dc), rgb(0xf06482), rgb(0xaaaaaa)],
    },
    Palette {
        // Inspired by osu!'s pink-on-plum look.
        name: "osu! Pink",
        dark: true,
        bg: rgb(0x1f171b), panel: rgb(0x2a1f25), surface: rgb(0x150f12), border: rgb(0x3d2c35),
        text: rgb(0xf5eaf0), text_weak: rgb(0xa8909c), accent: rgb(0xff66aa),
        lane: rgb(0x251b20), wave_bg: rgb(0x170f13), wave: rgb(0xff66aa),
        grid_measure: rgb(0xb7a0ad), grid_beat: rgb(0x5e4753), grid_sub: rgba(0xd296b4, 44),
        playhead: rgb(0xffcc22), red_line: rgb(0xff4d5e), vol_line: rgb(0x66ccff), vol_fill: rgba(0x66ccff, 46),
        danger: rgb(0xff6670), success: rgb(0xb3d944), warn: rgb(0xffcc22),
        tracks: [rgb(0xff66aa), rgb(0x66ccff), rgb(0xffcc22), rgb(0xb3d944), rgb(0xaa66ff), rgb(0xff8866), rgb(0x66ffcc), rgb(0xd0c0c8)],
    },
    Palette {
        name: "Graphite",
        dark: true,
        bg: rgb(0x181818), panel: rgb(0x222222), surface: rgb(0x111111), border: rgb(0x333333),
        text: rgb(0xe8e8e8), text_weak: rgb(0x8e8e8e), accent: rgb(0x2dd4bf),
        lane: rgb(0x1d1d1d), wave_bg: rgb(0x131313), wave: rgb(0x5eead4),
        grid_measure: rgb(0x9a9a9a), grid_beat: rgb(0x4c4c4c), grid_sub: rgba(0xa0a0a0, 40),
        playhead: rgb(0xfbbf24), red_line: rgb(0xef4444), vol_line: rgb(0x5eead4), vol_fill: rgba(0x2dd4bf, 46),
        danger: rgb(0xf87171), success: rgb(0x4ade80), warn: rgb(0xfbbf24),
        tracks: [rgb(0x2dd4bf), rgb(0xfb923c), rgb(0xa3e635), rgb(0xfacc15), rgb(0xc084fc), rgb(0x38bdf8), rgb(0xf472b6), rgb(0xa1a1aa)],
    },
    Palette {
        name: "Forest",
        dark: true,
        bg: rgb(0x141a16), panel: rgb(0x1b231e), surface: rgb(0x0e1310), border: rgb(0x2a362e),
        text: rgb(0xe3ece5), text_weak: rgb(0x88998d), accent: rgb(0x7bd389),
        lane: rgb(0x18201b), wave_bg: rgb(0x101612), wave: rgb(0x7bd389),
        grid_measure: rgb(0x8fa596), grid_beat: rgb(0x46564b), grid_sub: rgba(0x96c8a0, 42),
        playhead: rgb(0xf4e285), red_line: rgb(0xe0605a), vol_line: rgb(0xa8e0b0), vol_fill: rgba(0x7bd389, 44),
        danger: rgb(0xef7a72), success: rgb(0x7bd389), warn: rgb(0xe8c15a),
        tracks: [rgb(0x7bd389), rgb(0xe8a05a), rgb(0x5ab4e8), rgb(0xe8d75a), rgb(0xb48ce8), rgb(0x5ae8c8), rgb(0xe87a9a), rgb(0xa8b4aa)],
    },
    Palette {
        name: "Ember",
        dark: true,
        bg: rgb(0x1c1714), panel: rgb(0x26201b), surface: rgb(0x13100d), border: rgb(0x3a3029),
        text: rgb(0xf2e8df), text_weak: rgb(0xa2938a), accent: rgb(0xff9a4d),
        lane: rgb(0x221c18), wave_bg: rgb(0x16120f), wave: rgb(0xffa766),
        grid_measure: rgb(0xb09c8c), grid_beat: rgb(0x5a4a3e), grid_sub: rgba(0xd2aa8c, 42),
        playhead: rgb(0xfff1a8), red_line: rgb(0xff5a4a), vol_line: rgb(0xffc48c), vol_fill: rgba(0xff9a4d, 44),
        danger: rgb(0xff6a5a), success: rgb(0x9cd67a), warn: rgb(0xffc04d),
        tracks: [rgb(0xff9a4d), rgb(0x6ab0ff), rgb(0x9cd67a), rgb(0xffd84d), rgb(0xd88cff), rgb(0x4dd8d0), rgb(0xff6a8a), rgb(0xb8aca4)],
    },
];

pub const LIGHT: [Palette; 5] = [
    Palette {
        name: "Paper",
        dark: false,
        bg: rgb(0xf6f4ef), panel: rgb(0xffffff), surface: rgb(0xffffff), border: rgb(0xdedad1),
        text: rgb(0x1f2329), text_weak: rgb(0x6b7079), accent: rgb(0x3b63d9),
        lane: rgb(0xece9e2), wave_bg: rgb(0xe6e2da), wave: rgb(0x3b63d9),
        grid_measure: rgb(0x6f7480), grid_beat: rgb(0xb4b0a8), grid_sub: rgba(0x5a6496, 40),
        playhead: rgb(0xd97706), red_line: rgb(0xd33a3a), vol_line: rgb(0x3b63d9), vol_fill: rgba(0x3b63d9, 40),
        danger: rgb(0xc53030), success: rgb(0x2f855a), warn: rgb(0xb7791f),
        tracks: [rgb(0x3b63d9), rgb(0xdd6b20), rgb(0x2f9e5c), rgb(0xc99a06), rgb(0x9b4dca), rgb(0x0e9aa7), rgb(0xd53f6b), rgb(0x6b7280)],
    },
    Palette {
        name: "osu! Pink",
        dark: false,
        bg: rgb(0xfbf3f7), panel: rgb(0xffffff), surface: rgb(0xffffff), border: rgb(0xf0d6e3),
        text: rgb(0x2b1a23), text_weak: rgb(0x8a6b7b), accent: rgb(0xe83e8c),
        lane: rgb(0xf6e8ef), wave_bg: rgb(0xf2e0e9), wave: rgb(0xff66aa),
        grid_measure: rgb(0x8a6b7b), grid_beat: rgb(0xd8bccb), grid_sub: rgba(0xb4648c, 40),
        playhead: rgb(0xe0a800), red_line: rgb(0xe0344a), vol_line: rgb(0x2a9fd6), vol_fill: rgba(0x2a9fd6, 40),
        danger: rgb(0xd6304a), success: rgb(0x6f9a1c), warn: rgb(0xc99200),
        tracks: [rgb(0xe83e8c), rgb(0x2a9fd6), rgb(0xd9a400), rgb(0x7fa61a), rgb(0x8a4de6), rgb(0xe8663e), rgb(0x1fb894), rgb(0x8a7a82)],
    },
    Palette {
        name: "Snow",
        dark: false,
        bg: rgb(0xf3f5f9), panel: rgb(0xffffff), surface: rgb(0xffffff), border: rgb(0xd9dee8),
        text: rgb(0x1c2233), text_weak: rgb(0x687087), accent: rgb(0x5b5bd6),
        lane: rgb(0xe9edf4), wave_bg: rgb(0xe2e7f0), wave: rgb(0x5b5bd6),
        grid_measure: rgb(0x687087), grid_beat: rgb(0xb3bacb), grid_sub: rgba(0x5050a0, 38),
        playhead: rgb(0xe8590c), red_line: rgb(0xe03131), vol_line: rgb(0x5b5bd6), vol_fill: rgba(0x5b5bd6, 38),
        danger: rgb(0xc92a2a), success: rgb(0x2b8a3e), warn: rgb(0xd9480f),
        tracks: [rgb(0x5b5bd6), rgb(0xe8590c), rgb(0x2b8a3e), rgb(0xd4a106), rgb(0xae3ec9), rgb(0x1098ad), rgb(0xe64980), rgb(0x6c757d)],
    },
    Palette {
        name: "Mint",
        dark: false,
        bg: rgb(0xf1f7f3), panel: rgb(0xffffff), surface: rgb(0xffffff), border: rgb(0xd3e4d8),
        text: rgb(0x16261b), text_weak: rgb(0x627a69), accent: rgb(0x16a34a),
        lane: rgb(0xe5f0e8), wave_bg: rgb(0xdcebe0), wave: rgb(0x22a755),
        grid_measure: rgb(0x627a69), grid_beat: rgb(0xadc6b4), grid_sub: rgba(0x3c8c50, 38),
        playhead: rgb(0xd97706), red_line: rgb(0xdc2626), vol_line: rgb(0x15803d), vol_fill: rgba(0x16a34a, 38),
        danger: rgb(0xdc2626), success: rgb(0x15803d), warn: rgb(0xb45309),
        tracks: [rgb(0x16a34a), rgb(0xea580c), rgb(0x2563eb), rgb(0xca8a04), rgb(0x9333ea), rgb(0x0d9488), rgb(0xdb2777), rgb(0x6b7280)],
    },
    Palette {
        name: "Sand",
        dark: false,
        bg: rgb(0xf8f1e7), panel: rgb(0xfffbf5), surface: rgb(0xffffff), border: rgb(0xe8d9c4),
        text: rgb(0x2e2218), text_weak: rgb(0x85725f), accent: rgb(0xd9622b),
        lane: rgb(0xf1e6d6), wave_bg: rgb(0xecdfcb), wave: rgb(0xd9622b),
        grid_measure: rgb(0x85725f), grid_beat: rgb(0xd2bea4), grid_sub: rgba(0xa06e46, 40),
        playhead: rgb(0x2f6fd6), red_line: rgb(0xc8302c), vol_line: rgb(0xb0501f), vol_fill: rgba(0xd9622b, 38),
        danger: rgb(0xc0392b), success: rgb(0x5a8a2f), warn: rgb(0xb7791f),
        tracks: [rgb(0xd9622b), rgb(0x2f6fd6), rgb(0x5a8a2f), rgb(0xc99a06), rgb(0x8e44ad), rgb(0x16a085), rgb(0xc0396b), rgb(0x7f7466)],
    },
];

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t)
}

pub fn lerp(a: &Palette, b: &Palette, t: f32) -> Palette {
    let m = |x: Color32, y: Color32| mix(x, y, t);
    let mut tracks = a.tracks;
    for (i, c) in tracks.iter_mut().enumerate() {
        *c = m(a.tracks[i], b.tracks[i]);
    }
    Palette {
        name: b.name,
        dark: if t < 0.5 { a.dark } else { b.dark },
        bg: m(a.bg, b.bg),
        panel: m(a.panel, b.panel),
        surface: m(a.surface, b.surface),
        border: m(a.border, b.border),
        text: m(a.text, b.text),
        text_weak: m(a.text_weak, b.text_weak),
        accent: m(a.accent, b.accent),
        lane: m(a.lane, b.lane),
        wave_bg: m(a.wave_bg, b.wave_bg),
        wave: m(a.wave, b.wave),
        grid_measure: m(a.grid_measure, b.grid_measure),
        grid_beat: m(a.grid_beat, b.grid_beat),
        grid_sub: m(a.grid_sub, b.grid_sub),
        playhead: m(a.playhead, b.playhead),
        red_line: m(a.red_line, b.red_line),
        vol_line: m(a.vol_line, b.vol_line),
        vol_fill: m(a.vol_fill, b.vol_fill),
        danger: m(a.danger, b.danger),
        success: m(a.success, b.success),
        warn: m(a.warn, b.warn),
        tracks,
    }
}

/// egui widget colours derived from a palette.
pub fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    let widget = mix(p.panel, p.text, if p.dark { 0.10 } else { 0.07 });
    let widget_hover = mix(p.panel, p.text, if p.dark { 0.18 } else { 0.13 });
    v.panel_fill = p.bg;
    v.window_fill = p.panel;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.extreme_bg_color = p.surface;
    v.text_edit_bg_color = Some(p.surface);
    v.faint_bg_color = p.lane;
    v.code_bg_color = p.lane;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.danger;
    v.weak_text_color = Some(p.text_weak);
    v.selection.bg_fill = mix(p.panel, p.accent, 0.55);
    v.selection.stroke = Stroke::new(1.0, p.text);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    for (state, bg, stroke) in [(&mut w.inactive, widget, Stroke::NONE), (&mut w.hovered, widget_hover, Stroke::new(1.0, p.accent)), (&mut w.open, widget_hover, Stroke::new(1.0, p.border))] {
        state.bg_fill = bg;
        state.weak_bg_fill = bg;
        state.bg_stroke = stroke;
        state.fg_stroke = Stroke::new(1.0, p.text);
        state.corner_radius = CornerRadius::same(6);
    }
    // egui also uses the "active" text colour for strong/bold labels, so keep it readable.
    let pressed = mix(p.panel, p.accent, if p.dark { 0.38 } else { 0.22 });
    w.active.bg_fill = pressed;
    w.active.weak_bg_fill = pressed;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.5, p.text);
    w.active.corner_radius = CornerRadius::same(6);
    v
}

/// Current theme plus an in-flight blend from the previous one.
pub struct ThemeState {
    from: Palette,
    to: Palette,
    started: Option<Instant>,
}

const BLEND_SECS: f32 = 0.45;

fn ease(t: f32) -> f32 {
    // Cubic ease-in-out: gentle start and finish.
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

impl ThemeState {
    pub fn new(p: Palette) -> Self {
        Self { from: p, to: p, started: None }
    }

    pub fn target(&self) -> &Palette {
        &self.to
    }

    /// Start blending toward `p` from whatever is on screen right now.
    pub fn switch_to(&mut self, p: Palette) {
        self.from = self.current();
        self.to = p;
        self.started = Some(Instant::now());
    }

    pub fn current(&self) -> Palette {
        match self.started {
            Some(s) => {
                let t = (s.elapsed().as_secs_f32() / BLEND_SECS).min(1.0);
                lerp(&self.from, &self.to, ease(t))
            }
            None => self.to,
        }
    }

    /// Apply the current colours to egui; keeps repainting while blending.
    pub fn apply(&mut self, ctx: &egui::Context) -> Palette {
        let p = self.current();
        if let Some(s) = self.started {
            if s.elapsed().as_secs_f32() >= BLEND_SECS {
                self.started = None;
            } else {
                ctx.request_repaint();
            }
        }
        ctx.set_visuals_of(egui::Theme::Dark, visuals(&p));
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_endpoints() {
        let a = &DARK[0];
        let b = &LIGHT[0];
        assert_eq!(lerp(a, b, 0.0).bg, a.bg);
        assert_eq!(lerp(a, b, 1.0).bg, b.bg);
        assert!(!lerp(a, b, 1.0).dark);
        assert_eq!(ease(0.0), 0.0);
        assert!((ease(1.0) - 1.0).abs() < 1e-6);
        assert!((ease(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn palettes_have_readable_text() {
        // Rough contrast check: text and background must differ clearly in brightness.
        let lum = |c: Color32| 0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32;
        for p in DARK.iter().chain(LIGHT.iter()) {
            assert!((lum(p.text) - lum(p.bg)).abs() > 150.0, "{} text contrast", p.name);
            assert_eq!(p.dark, lum(p.bg) < 128.0, "{} mode flag", p.name);
        }
    }
}

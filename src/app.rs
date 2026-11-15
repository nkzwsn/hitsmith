use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, CornerRadius, Event, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::audio::{self, Engine, Event as HitEvent, Peaks};
use crate::export;
use crate::icons::{self, Icon};
use crate::library;
use crate::osu::{self, Meta, RedLine};
use crate::theme::{self, Palette, ThemeState};
use crate::project::{self, Content, ControlPoint, Project, ProjectEntry, SampleEntry, SampleRef, Track};

const HEADER_W: f32 = 300.0;
const RULER_H: f32 = 20.0;
const WAVE_H: f32 = 84.0;
const LANE_H: f32 = 58.0;
const ROW_H: f32 = 68.0;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    last_dir: Option<PathBuf>,
    song_volume: f32,
    hit_volume: f32,
    dark_mode: bool,
    dark_palette: usize,
    light_palette: usize,
    scrub_sound: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { last_dir: None, song_volume: 0.7, hit_volume: 1.0, dark_mode: true, dark_palette: 0, light_palette: 0, scrub_sound: false }
    }
}

impl Settings {
    fn palette(&self) -> Palette {
        if self.dark_mode { theme::DARK[self.dark_palette.min(4)] } else { theme::LIGHT[self.light_palette.min(4)] }
    }
}

/// Mode switch plus palette swatches. Returns true when the user picked something.
fn theme_menu(ui: &mut egui::Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    let label = "Theme";
    ui.menu_button(label, |ui| {
        ui.set_min_width(250.0);
        ui.horizontal(|ui| {
            if ui.selectable_label(!settings.dark_mode, "Light").clicked() && settings.dark_mode {
                settings.dark_mode = false;
                changed = true;
            }
            if ui.selectable_label(settings.dark_mode, "Dark").clicked() && !settings.dark_mode {
                settings.dark_mode = true;
                changed = true;
            }
            ui.label(egui::RichText::new("Ctrl+T switches").small().weak());
        });
        for (dark, list) in [(true, &theme::DARK), (false, &theme::LIGHT)] {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(if dark { "Dark palettes" } else { "Light palettes" }).small().weak());
            for (i, p) in list.iter().enumerate() {
                let current = if dark { settings.dark_palette } else { settings.light_palette };
                let selected = settings.dark_mode == dark && current == i;
                if swatch(ui, p, selected).clicked() {
                    settings.dark_mode = dark;
                    if dark {
                        settings.dark_palette = i;
                    } else {
                        settings.light_palette = i;
                    }
                    changed = true;
                }
            }
        }
    });
    changed
}

/// One clickable palette preview drawn in its own colours.
fn swatch(ui: &mut egui::Ui, p: &Palette, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(240.0), 30.0), Sense::click());
    let painter = ui.painter();
    let stroke = if selected {
        Stroke::new(2.0, p.accent)
    } else if resp.hovered() {
        Stroke::new(1.0, p.accent)
    } else {
        Stroke::new(1.0, p.border)
    };
    painter.rect_filled(rect, CornerRadius::same(6), p.bg);
    painter.rect_stroke(rect, CornerRadius::same(6), stroke, StrokeKind::Inside);
    let mut x = rect.left() + 14.0;
    for c in [p.accent, p.tracks[1], p.tracks[2], p.tracks[3], p.tracks[4]] {
        painter.circle_filled(pos2(x, rect.center().y), 6.0, c);
        x += 15.0;
    }
    painter.text(pos2(x + 6.0, rect.center().y), Align2::LEFT_CENTER, p.name, FontId::proportional(13.0), p.text);
    if selected {
        painter.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, "current", FontId::proportional(11.0), p.text_weak);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

struct Picked {
    dir: PathBuf,
    osu_file: String,
    diffs: Vec<String>,
    meta: Meta,
    bpm: String,
    bg: Option<egui::TextureHandle>,
}

#[derive(Default)]
struct Home {
    projects: Vec<ProjectEntry>,
    picked: Option<Picked>,
    name: String,
    error: Option<String>,
    confirm_delete: Option<PathBuf>,
    /// Card thumbnails, loaded a few per frame so the list appears instantly.
    thumbs: HashMap<PathBuf, Option<egui::TextureHandle>>,
}

enum Screen {
    Home,
    Work(Box<Work>),
}

pub struct App {
    data_dir: PathBuf,
    projects_dir: PathBuf,
    settings: Settings,
    engine: Option<Engine>,
    audio_error: Option<String>,
    screen: Screen,
    home: Home,
    theme: ThemeState,
    pal: Palette,
    theme_changed: bool,
    logo: Option<egui::TextureHandle>,
}

fn data_dir() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    if let Some(dir) = exe_dir {
        let probe = dir.join(".write-test");
        if std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(probe);
            return dir;
        }
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("hitsmith")
}

fn load_system_fonts(ctx: &egui::Context) {
    // Japanese/Chinese/Korean titles: borrow a font Windows already has instead of shipping 40 MB.
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let candidates = ["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc", "msyh.ttc", "malgun.ttf"];
    let mut fonts = egui::FontDefinitions::default();
    let mut added = 0;
    for (i, file) in candidates.iter().enumerate() {
        let path = Path::new(&windir).join("Fonts").join(file);
        if let Ok(bytes) = std::fs::read(&path) {
            let key = format!("system-{i}");
            fonts.font_data.insert(key.clone(), Arc::new(egui::FontData::from_owned(bytes)));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push(key.clone());
            }
            added += 1;
            if added == 1 {
                break;
            }
        }
    }
    if added > 0 {
        ctx.set_fonts(fonts);
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        load_system_fonts(&cc.egui_ctx);
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        // Roomier, rounder defaults than stock egui.
        cc.egui_ctx.global_style_mut(|st| {
            st.spacing.item_spacing = vec2(8.0, 6.0);
            st.spacing.button_padding = vec2(10.0, 5.0);
            st.spacing.interact_size.y = 26.0;
            st.spacing.combo_height = 320.0;
            use egui::{FontFamily::Proportional, FontId, TextStyle};
            st.text_styles.insert(TextStyle::Body, FontId::new(14.0, Proportional));
            st.text_styles.insert(TextStyle::Button, FontId::new(14.0, Proportional));
            st.text_styles.insert(TextStyle::Heading, FontId::new(22.0, Proportional));
            st.text_styles.insert(TextStyle::Small, FontId::new(11.5, Proportional));
        });
        let data_dir = data_dir();
        let projects_dir = data_dir.join("projects");
        let _ = std::fs::create_dir_all(&projects_dir);
        let settings: Settings = std::fs::read(data_dir.join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let (engine, audio_error) = match Engine::new() {
            Ok(e) => {
                e.set_gains(settings.song_volume, settings.hit_volume);
                e.set_scrub_sound(settings.scrub_sound);
                (Some(e), None)
            }
            Err(e) => (None, Some(format!("Audio output unavailable: {e}"))),
        };
        let theme = ThemeState::new(settings.palette());
        let pal = *theme.target();
        let mut app = Self { data_dir, projects_dir, settings, engine, audio_error, screen: Screen::Home, home: Home::default(), theme, pal, theme_changed: false, logo: load_logo(&cc.egui_ctx) };
        app.refresh_projects();
        app
    }

    fn save_settings(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(&self.settings) {
            let _ = std::fs::write(self.data_dir.join("settings.json"), b);
        }
    }

    fn refresh_projects(&mut self) {
        self.home.projects = project::list_projects(&self.projects_dir);
    }

    fn pick_beatmap(&mut self, ctx: &egui::Context) {
        let start = self.settings.last_dir.clone().filter(|d| d.is_dir()).or_else(library::guess_songs_dir);
        let mut dialog = rfd::FileDialog::new().set_title("Open an osu! beatmap (.osu)").add_filter("osu! beatmap", &["osu"]);
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        let Some(file) = dialog.pick_file() else { return };
        self.select_beatmap(ctx, &file);
    }

    fn select_beatmap(&mut self, ctx: &egui::Context, file: &Path) {
        self.home.error = None;
        let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else { return };
        match osu::read_beatmap(file) {
            Ok(bm) => {
                let mut diffs: Vec<String> = std::fs::read_dir(dir)
                    .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.to_ascii_lowercase().ends_with(".osu")).collect())
                    .unwrap_or_default();
                diffs.sort();
                let bpms: Vec<f64> = bm.red_lines.iter().map(RedLine::bpm).collect();
                let (lo, hi) = bpms.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
                let bpm = if (hi - lo).abs() < 0.01 { format!("{lo:.0} BPM") } else { format!("{lo:.0}-{hi:.0} BPM") };
                let bg = load_background(ctx, &dir.join(&bm.meta.background));
                if self.home.name.trim().is_empty() {
                    // Suggest a name that isn't taken yet: "Title", "Title (2)", ...
                    let base = osu::sanitize_file_part(bm.meta.title.as_str());
                    let base = if base.is_empty() { "New project".to_string() } else { base };
                    let mut name = base.clone();
                    let mut n = 2;
                    while self.projects_dir.join(&name).exists() {
                        name = format!("{base} ({n})");
                        n += 1;
                    }
                    self.home.name = name;
                }
                if let Some(songs) = dir.parent() {
                    self.settings.last_dir = Some(songs.to_path_buf());
                    self.save_settings();
                }
                self.home.picked = Some(Picked { dir: dir.to_path_buf(), osu_file: name.to_string_lossy().into_owned(), diffs, meta: bm.meta, bpm, bg });
            }
            Err(e) => self.home.error = Some(format!("{e:#}")),
        }
    }

    fn open_project(&mut self, ctx: &egui::Context, entry: ProjectEntry) {
        match project::load(&entry.json()) {
            Ok(p) => match Work::open(ctx, entry, p, self.engine.as_ref()) {
                Ok(w) => self.screen = Screen::Work(Box::new(w)),
                Err(e) => self.home.error = Some(format!("{e:#}")),
            },
            Err(e) => self.home.error = Some(format!("{e:#}")),
        }
    }

    fn home_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let pal = self.pal;

        // Drag a .osu file anywhere onto the window to start a project.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(f) = dropped.iter().find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("osu"))) {
            self.select_beatmap(&ctx, f);
        } else if !dropped.is_empty() {
            self.home.error = Some("Drop a .osu file (any difficulty of the mapset).".into());
        }
        let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());

        egui::Panel::top("home-top").frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin::symmetric(18, 10))).show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(logo) = &self.logo {
                    ui.image((logo.id(), vec2(30.0, 30.0)));
                }
                ui.label(egui::RichText::new("hitsmith").size(20.0).strong());
                ui.label(egui::RichText::new("hitsounding workstation for osu!").color(pal.text_weak));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme_menu(ui, &mut self.settings) {
                        self.theme_changed = true;
                    }
                });
            });
        });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin::symmetric(18, 8))).show(ui, |ui| {
            if let Some(err) = &self.audio_error {
                ui.colored_label(pal.warn, err);
            }
            if let Some(err) = self.home.error.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(pal.danger, err);
                    if ui.small_button("Dismiss").clicked() {
                        self.home.error = None;
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Projects").size(24.0).strong());
                ui.label(egui::RichText::new(format!("{}", self.home.projects.len())).color(pal.text_weak));
            });
            ui.add_space(8.0);

            // Load at most two thumbnails per frame.
            let mut loaded = 0;
            for p in &self.home.projects {
                if let Some(bg) = &p.background {
                    if !self.home.thumbs.contains_key(bg) && loaded < 2 {
                        self.home.thumbs.insert(bg.clone(), load_background(&ctx, bg));
                        loaded += 1;
                    }
                }
            }
            if loaded > 0 {
                ctx.request_repaint();
            }

            let mut open = None;
            let mut delete = None;
            let mut new_clicked = false;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                    if new_project_card(ui, &pal).clicked() {
                        new_clicked = true;
                    }
                    for p in &self.home.projects {
                        let thumb = p.background.as_ref().and_then(|b| self.home.thumbs.get(b)).and_then(|t| t.as_ref());
                        match project_card(ui, &pal, p, thumb) {
                            CardAction::Open => open = Some(p.clone()),
                            CardAction::Delete => delete = Some(p.dir.clone()),
                            CardAction::None => {}
                        }
                    }
                });
                if self.home.projects.is_empty() {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Your projects will show up here.").color(pal.text_weak));
                }
            });
            if new_clicked {
                self.pick_beatmap(&ctx);
            }
            if let Some(p) = open {
                self.open_project(&ctx, p);
            }
            if delete.is_some() {
                self.home.confirm_delete = delete;
            }
        });

        if hovering_files {
            let screen = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop-overlay")));
            painter.rect_filled(screen, 0.0, pal.bg.gamma_multiply(0.85));
            let r = screen.shrink(40.0);
            painter.rect_stroke(r, CornerRadius::same(18), Stroke::new(2.0, pal.accent), StrokeKind::Inside);
            painter.text(r.center(), Align2::CENTER_CENTER, "Drop a .osu file to start a project", FontId::proportional(22.0), pal.text);
        }

        // New-project dialog: one step from a picked beatmap to a project.
        if self.home.picked.is_some() {
            let mut close = false;
            let mut create = false;
            let mut switch_to = None;
            egui::Modal::new(egui::Id::unique("new-project")).show(&ctx, |ui| {
                let p = self.home.picked.as_ref().unwrap();
                ui.set_width(520.0);
                if let Some(tex) = &p.bg {
                    let (rect, _) = ui.allocate_exact_size(vec2(520.0, 150.0), Sense::hover());
                    paint_cover(ui, rect, tex, CornerRadius::same(10));
                }
                ui.add_space(6.0);
                ui.label(egui::RichText::new(p.meta.display_title()).size(20.0).strong());
                ui.label(egui::RichText::new(format!("{}  ·  mapped by {}  ·  {}", p.meta.display_artist(), p.meta.creator, p.bpm)).color(pal.text_weak));
                ui.add_space(10.0);
                egui::Grid::new("new-project-grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                    ui.label("Timing from");
                    egui::ComboBox::from_id_salt("diff").width(380.0).selected_text(&p.osu_file).show_ui(ui, |ui| {
                        for d in &p.diffs {
                            if ui.selectable_label(*d == p.osu_file, d).clicked() {
                                switch_to = Some(p.dir.join(d));
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Project name");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.home.name).desired_width(380.0));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        create = true;
                    }
                    ui.end_row();
                });
                if let Some(err) = &self.home.error {
                    ui.colored_label(pal.danger, err);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if icons::primary(ui, &pal, Icon::Plus, "Create project").clicked() {
                        create = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if let Some(f) = switch_to {
                self.select_beatmap(&ctx, &f);
            }
            if create {
                let p = self.home.picked.as_ref().unwrap();
                let mut proj = Project::new(p.dir.clone(), p.osu_file.clone());
                if let Ok(bm) = osu::read_beatmap(&p.dir.join(&p.osu_file)) {
                    proj.bookmarks = bm.bookmarks;
                }
                match project::create(&self.projects_dir, &self.home.name, &proj) {
                    Ok(entry) => {
                        self.home.name.clear();
                        self.home.picked = None;
                        self.home.error = None;
                        self.refresh_projects();
                        self.open_project(&ctx, entry);
                    }
                    Err(e) => self.home.error = Some(format!("{e:#}")),
                }
            } else if close || ui.input(|i| i.key_pressed(Key::Escape)) {
                self.home.picked = None;
                self.home.name.clear();
                self.home.error = None;
            }
        }

        if let Some(dir) = self.home.confirm_delete.clone() {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut close = false;
            egui::Modal::new(egui::Id::unique("confirm-delete")).show(&ctx, |ui| {
                ui.heading("Delete project?");
                ui.label(format!("\"{name}\" and its imported samples will be moved to the Recycle Bin, so you can restore it from there. Exported difficulties in the mapset stay."));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new("Move to Recycle Bin").color(pal.danger)).clicked() {
                        if dir.parent() == Some(self.projects_dir.as_path()) && dir.join("project.json").is_file() {
                            if let Err(e) = project::move_to_recycle_bin(&dir) {
                                self.home.error = Some(format!("Couldn't delete: {e:#}"));
                            }
                        }
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.home.confirm_delete = None;
                self.refresh_projects();
            }
        }
    }

}

enum CardAction {
    None,
    Open,
    Delete,
}

const CARD: Vec2 = vec2(250.0, 210.0);

/// Draw a texture so it covers `rect` (cropping the overflow), with rounded corners.
fn paint_cover(ui: &mut egui::Ui, rect: Rect, tex: &egui::TextureHandle, radius: CornerRadius) {
    let size = tex.size_vec2();
    let (ta, ra) = (size.x / size.y, rect.width() / rect.height());
    let uv = if ta > ra {
        let w = ra / ta;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = ta / ra;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    };
    ui.put(rect, egui::Image::new((tex.id(), rect.size())).uv(uv).corner_radius(radius));
}

fn ago(t: std::time::SystemTime) -> String {
    let secs = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

fn new_project_card(ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(CARD, Sense::click());
    let hover = resp.hovered();
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(14), if hover { pal.accent.gamma_multiply(0.12) } else { pal.panel });
    painter.rect_stroke(rect, CornerRadius::same(14), Stroke::new(if hover { 2.0 } else { 1.5 }, if hover { pal.accent } else { pal.border }), StrokeKind::Inside);
    let c = rect.center() - vec2(0.0, 22.0);
    painter.circle_filled(c, 26.0, pal.accent);
    icons::paint(painter, Rect::from_center_size(c, Vec2::splat(24.0)), Icon::Plus, pal.bg);
    painter.text(c + vec2(0.0, 46.0), Align2::CENTER_CENTER, "New project", FontId::proportional(16.0), pal.text);
    painter.text(c + vec2(0.0, 68.0), Align2::CENTER_CENTER, "Open a .osu, or drop one here", FontId::proportional(12.0), pal.text_weak);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn project_card(ui: &mut egui::Ui, pal: &Palette, p: &ProjectEntry, thumb: Option<&egui::TextureHandle>) -> CardAction {
    let (rect, resp) = ui.allocate_exact_size(CARD, Sense::click());
    let hover = resp.hovered();
    let lift = if hover { -2.0 } else { 0.0 };
    let rect = rect.translate(vec2(0.0, lift));
    ui.painter().rect_filled(rect, CornerRadius::same(14), pal.panel);
    let img = Rect::from_min_size(rect.min, vec2(rect.width(), 118.0));
    match thumb {
        Some(t) => paint_cover(ui, img, t, CornerRadius { nw: 14, ne: 14, sw: 0, se: 0 }),
        None => {
            ui.painter().rect_filled(img, CornerRadius { nw: 14, ne: 14, sw: 0, se: 0 }, pal.lane);
            icons::paint(ui.painter(), Rect::from_center_size(img.center(), Vec2::splat(36.0)), Icon::Music, pal.text_weak);
        }
    }
    let painter = ui.painter();
    let x = rect.left() + 14.0;
    let clip = Rect::from_min_max(pos2(x, img.bottom()), pos2(rect.right() - 10.0, rect.bottom()));
    let tp = painter.with_clip_rect(clip);
    tp.text(pos2(x, img.bottom() + 12.0), Align2::LEFT_TOP, &p.name, FontId::proportional(16.0), pal.text);
    tp.text(pos2(x, img.bottom() + 36.0), Align2::LEFT_TOP, &p.mapset_label, FontId::proportional(12.0), pal.text_weak);
    tp.text(pos2(x, rect.bottom() - 14.0), Align2::LEFT_BOTTOM, format!("{} hits  ·  edited {}", p.hits, ago(p.modified)), FontId::proportional(11.5), pal.text_weak);
    painter.rect_stroke(rect, CornerRadius::same(14), Stroke::new(if hover { 2.0 } else { 1.0 }, if hover { pal.accent } else { pal.border }), StrokeKind::Inside);

    // Delete button, shown on hover in the image corner.
    let del = Rect::from_min_size(pos2(rect.right() - 38.0, rect.top() + 8.0), Vec2::splat(30.0));
    let del_resp = ui.interact(del, ui.scope_id().with(("del", &p.dir)), Sense::click());
    if hover || del_resp.hovered() {
        let bg = if del_resp.hovered() { pal.danger } else { Color32::from_black_alpha(150) };
        ui.painter().rect_filled(del, CornerRadius::same(8), bg);
        icons::paint(ui.painter(), del.shrink(7.0), Icon::Trash, Color32::WHITE);
    }
    if del_resp.clicked() {
        return CardAction::Delete;
    }
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        return CardAction::Open;
    }
    CardAction::None
}

fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(crate::LOGO_PNG).ok()?.thumbnail(64, 64).to_rgba8();
    let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    Some(ctx.load_texture("logo", color, egui::TextureOptions::LINEAR))
}

fn load_background(ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
    if !path.is_file() {
        return None;
    }
    let img = image::open(path).ok()?;
    let img = img.thumbnail(960, 540).to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("bg", color, egui::TextureOptions::LINEAR))
}

impl eframe::App for App {
    /// Runs every frame, even while the window is minimized (when `ui` is skipped).
    /// Keeping repaints going during playback means the playhead is in sync the
    /// moment the window comes back, instead of freezing until the next mouse move.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.engine.as_ref().is_some_and(|e| e.is_playing()) {
            let hidden = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
            // While minimized nothing is drawn, so a slow tick is enough to stay alive.
            ctx.request_repaint_after(Duration::from_millis(if hidden { 100 } else { 16 }));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ui.input(|i| i.modifiers.command && i.key_pressed(Key::T)) {
            self.settings.dark_mode = !self.settings.dark_mode;
            self.theme_changed = true;
        }
        if std::mem::take(&mut self.theme_changed) {
            self.theme.switch_to(self.settings.palette());
            self.save_settings();
        }
        self.pal = self.theme.apply(&ctx);
        let mut go_home = false;
        match &mut self.screen {
            Screen::Home => self.home_ui(ui),
            Screen::Work(w) => {
                let mut vols = (self.settings.song_volume, self.settings.hit_volume);
                w.pal = self.pal;
                w.scrub_sound = self.settings.scrub_sound;
                let settings = &mut self.settings;
                let mut picked = false;
                go_home = w.ui(ui, self.engine.as_ref(), &mut vols, &mut |ui: &mut egui::Ui| picked |= theme_menu(ui, settings));
                self.theme_changed |= picked;
                if w.scrub_sound != self.settings.scrub_sound {
                    self.settings.scrub_sound = w.scrub_sound;
                    self.save_settings();
                }
                if vols != (self.settings.song_volume, self.settings.hit_volume) {
                    self.settings.song_volume = vols.0;
                    self.settings.hit_volume = vols.1;
                    if let Some(e) = &self.engine {
                        e.set_gains(vols.0, vols.1);
                    }
                }
            }
        }
        if go_home {
            if let Screen::Work(w) = &mut self.screen {
                w.save_now();
            }
            if let Some(e) = &self.engine {
                e.stop_all();
            }
            self.save_settings();
            self.screen = Screen::Home;
            self.refresh_projects();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Screen::Work(w) = &mut self.screen {
            w.save_now();
        }
        self.save_settings();
    }
}

// ---------------------------------------------------------------------------
// Workstation

struct LoadedSong {
    pcm: Arc<Vec<i16>>,
    peaks: Peaks,
    duration_ms: f64,
}

enum Drag {
    /// Moving the selection; `delta` is the snapped offset from where the drag began.
    Notes { anchor: i64, delta: i64 },
    /// Rubber-band selection in screen coordinates.
    Box { start: Pos2, cur: Pos2 },
    Point { idx: usize, orig: ControlPoint, cur: ControlPoint },
    Pan { start_view: f64 },
}

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Select,
    Place,
}

/// A hit is identified by its track id and time.
type NoteKey = (u64, i64);

struct Work {
    entry: ProjectEntry,
    project: Project,
    meta: Meta,
    reds: Vec<RedLine>,
    song: Option<LoadedSong>,
    song_rx: Option<mpsc::Receiver<Result<LoadedSong, String>>>,
    song_error: Option<String>,
    library: Vec<SampleEntry>,
    loaded: HashMap<PathBuf, u32>,
    sample_bufs: Vec<Arc<Vec<f32>>>,
    sample_errors: HashMap<PathBuf, String>,
    undo: Vec<Content>,
    redo: Vec<Content>,
    view_start: f64,
    dirty: bool,
    last_edit: Instant,
    drag: Option<Drag>,
    export_open: bool,
    export_result: Option<Result<String, String>>,
    confirm_remove: Option<usize>,
    save_error: Option<String>,
    follow: bool,
    skin_dir: Option<PathBuf>,
    pal: Palette,
    tool: Tool,
    selected: HashSet<NoteKey>,
    clipboard: Vec<NoteKey>,
    speed: f64,
    wheel_acc: f32,
    div_acc: f32,
    /// Visible time span (ms) of the timeline, from the last frame.
    view_span: f64,
    /// Track lane rectangles from the last frame, for box selection across rows.
    row_rects: Vec<(usize, Rect)>,
    show_keys: bool,
    confirm_clear: bool,
    toast: Option<(String, Instant)>,
    renaming: Option<(u64, String)>,
    pos_now: f64,
    playing_now: bool,
    /// Mirrors the saved setting; the App writes it back when it changes.
    scrub_sound: bool,
}

impl Work {
    fn open(ctx: &egui::Context, entry: ProjectEntry, project: Project, engine: Option<&Engine>) -> anyhow::Result<Self> {
        let bm = osu::read_beatmap(&project.mapset_dir.join(&project.osu_file))?;
        let audio_path = project.mapset_dir.join(&bm.meta.audio_filename);
        let (tx, rx) = mpsc::channel();
        let rate = engine.map(|e| e.rate).unwrap_or(44_100);
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let r = audio::decode_file(&audio_path).map(|pcm| {
                let pcm = audio::resample(&pcm, rate);
                let peaks = audio::compute_peaks(&pcm, 64);
                LoadedSong { duration_ms: pcm.duration_ms(), peaks, pcm: Arc::new(audio::to_i16(&pcm.data)) }
            });
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
            ctx2.request_repaint();
        });
        let first = bm.red_lines[0].offset;
        let mut w = Self {
            skin_dir: library::current_skin_dir(&project.mapset_dir),
            entry,
            meta: bm.meta,
            reds: bm.red_lines,
            project,
            song: None,
            song_rx: Some(rx),
            song_error: None,
            library: Vec::new(),
            loaded: HashMap::new(),
            sample_bufs: Vec::new(),
            sample_errors: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            view_start: first - 500.0,
            dirty: false,
            last_edit: Instant::now(),
            drag: None,
            export_open: false,
            export_result: None,
            confirm_remove: None,
            save_error: None,
            follow: true,
            pal: theme::DARK[0],
            tool: Tool::Place,
            selected: HashSet::new(),
            clipboard: Vec::new(),
            speed: 1.0,
            wheel_acc: 0.0,
            div_acc: 0.0,
            view_span: 5000.0,
            row_rects: Vec::new(),
            show_keys: false,
            confirm_clear: false,
            toast: None,
            renaming: None,
            pos_now: 0.0,
            playing_now: false,
            scrub_sound: false,
        };
        if let Some(e) = engine {
            e.set_song(Arc::new(Vec::new()));
            e.set_samples(Vec::new());
            e.set_speed(1.0);
            e.seek_ms(0.0);
        }
        w.refresh_library();
        Ok(w)
    }

    fn refresh_library(&mut self) {
        let exported = export::read_manifest(&self.project.mapset_dir).files;
        self.library = project::sample_library(&self.entry.samples_dir(), &self.project.mapset_dir, &self.meta.audio_filename, self.skin_dir.as_deref(), &exported);
    }

    fn sample_path(&self, r: &SampleRef) -> Option<PathBuf> {
        self.library.iter().find(|e| &e.r == r).map(|e| e.path.clone())
    }

    /// Decode a sample (once) into the engine's sample table.
    fn ensure_sample(&mut self, path: &Path, engine: Option<&Engine>) -> Option<u32> {
        if let Some(&id) = self.loaded.get(path) {
            return Some(id);
        }
        let engine = engine?;
        match audio::decode_file(path) {
            Ok(pcm) => {
                let pcm = audio::resample(&pcm, engine.rate);
                let id = self.sample_bufs.len() as u32;
                self.sample_bufs.push(Arc::new(pcm.data));
                self.loaded.insert(path.to_path_buf(), id);
                engine.set_samples(self.sample_bufs.clone());
                Some(id)
            }
            Err(e) => {
                self.sample_errors.insert(path.to_path_buf(), format!("{e:#}"));
                None
            }
        }
    }

    fn rebuild_events(&mut self, engine: Option<&Engine>) {
        let Some(engine) = engine else { return };
        let rate = engine.rate as f64;
        let mut events = Vec::new();
        let tracks = self.project.content.tracks.clone();
        for t in &tracks {
            if !self.project.audible(t) || t.volume == 0 {
                continue;
            }
            let Some(r) = &t.sample else { continue };
            let Some(path) = self.sample_path(r) else { continue };
            let Some(id) = self.ensure_sample(&path, Some(engine)) else { continue };
            for &n in &t.notes {
                let gain = t.volume as f32 / 100.0 * self.project.volume_at(n) as f32 / 100.0;
                events.push(HitEvent { frame: (n.max(0) as f64 * rate / 1000.0) as u64, sample: id, gain });
            }
        }
        events.sort_by_key(|e| e.frame);
        engine.set_events(events);
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.project.content.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn changed(&mut self, engine: Option<&Engine>) {
        self.dirty = true;
        self.last_edit = Instant::now();
        self.rebuild_events(engine);
    }

    fn undo(&mut self, engine: Option<&Engine>) {
        if let Some(c) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project.content, c));
            self.changed(engine);
        }
    }

    fn redo(&mut self, engine: Option<&Engine>) {
        if let Some(c) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project.content, c));
            self.changed(engine);
        }
    }

    fn save_now(&mut self) {
        match project::save(&self.entry.json(), &self.project) {
            Ok(()) => {
                self.dirty = false;
                self.save_error = None;
            }
            Err(e) => self.save_error = Some(format!("Save failed: {e:#}")),
        }
    }

    fn snap(&self, ms: f64) -> i64 {
        osu::snap(&self.reds, ms, self.project.divisor).round() as i64
    }

    fn duration(&self) -> f64 {
        self.song.as_ref().map(|s| s.duration_ms).unwrap_or(60_000.0)
    }

    fn x_of(&self, rect: Rect, t: f64) -> f32 {
        rect.left() + ((t - self.view_start) * self.project.zoom as f64) as f32
    }

    fn t_of(&self, rect: Rect, x: f32) -> f64 {
        self.view_start + ((x - rect.left()) / self.project.zoom) as f64
    }

    fn add_track(&mut self, engine: Option<&Engine>) {
        self.checkpoint();
        let id = self.project.next_id;
        self.project.next_id += 1;
        let n = self.project.content.tracks.len();
        // Prefer a sample no other track uses yet.
        let used: Vec<&SampleRef> = self.project.content.tracks.iter().filter_map(|t| t.sample.as_ref()).collect();
        let sample = self.library.iter().find(|e| !used.contains(&&e.r)).or(self.library.first()).map(|e| e.r.clone());
        self.project.content.tracks.push(Track { id, name: format!("Track {}", n + 1), sample, volume: 80, muted: false, solo: false, color: (n % 8) as u8, notes: Vec::new() });
        self.changed(engine);
    }

    /// Returns true when the user wants to go back home.
    fn ui(&mut self, ui: &mut egui::Ui, engine: Option<&Engine>, vols: &mut (f32, f32), theme_ui: &mut dyn FnMut(&mut egui::Ui)) -> bool {
        let ctx = ui.ctx().clone();
        if let Some(rx) = &self.song_rx {
            if let Ok(r) = rx.try_recv() {
                match r {
                    Ok(song) => {
                        if let Some(e) = engine {
                            e.set_song(song.pcm.clone());
                        }
                        self.song = Some(song);
                        self.rebuild_events(engine);
                    }
                    Err(e) => self.song_error = Some(e),
                }
                self.song_rx = None;
            }
        }
        if self.dirty && self.last_edit.elapsed() > Duration::from_millis(700) && self.drag.is_none() {
            self.save_now();
        }
        let playing = engine.map(|e| e.is_playing()).unwrap_or(false);
        let pos = engine.map(|e| e.position_ms()).unwrap_or(0.0);
        self.pos_now = pos;
        self.playing_now = playing;
        if playing {
            ctx.request_repaint();
        }
        let mut go_home = false;

        // osu!-editor style keyboard shortcuts (ignored while typing in a text field).
        let typing = ctx.memory(|m| m.focused()).is_some() && ctx.egui_wants_keyboard_input();
        if !typing && self.handle_keys(&ctx, engine, playing, pos) {
            go_home = true;
        }

        let pal = self.pal;
        egui::Panel::top("work-top").frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 4 })).show(ui, |ui| {
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
            // Left: back + song.
            {
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)));
                if icons::button(ui, &pal, Icon::Back, "Save and go back to projects (Esc)", false, true).clicked() {
                    go_home = true;
                }
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(self.meta.display_title()).size(15.0).strong());
                    ui.label(egui::RichText::new(format!("{}  ·  {}", self.meta.display_artist(), self.entry.name)).size(12.0).color(pal.text_weak));
                });
            }
            // Centre: transport.
            {
                let center = Rect::from_center_size(row.center(), vec2(300.0, row.height()));
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(center).layout(egui::Layout::left_to_right(egui::Align::Center)));
                if icons::button(ui, &pal, Icon::Stop, "Stop and go to start (Home)", false, engine.is_some()).clicked() {
                    if let Some(e) = engine {
                        e.play(false);
                    }
                    self.seek(engine, 0.0);
                }
                if icons::play_button(ui, &pal, playing, engine.is_some() && self.song.is_some()).clicked() {
                    if let Some(e) = engine {
                        e.play(!playing);
                    }
                }
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(fmt_time(pos)).monospace().size(18.0).strong());
                    let bpm = osu::red_line_at(&self.reds, pos).bpm();
                    let speed = if (self.speed - 1.0).abs() > 0.001 { format!("  ·  {:.0}% speed", self.speed * 100.0) } else { String::new() };
                    ui.label(egui::RichText::new(format!("{bpm:.0} BPM{speed}")).size(11.5).color(pal.text_weak));
                });
            }
            // Right: export + more.
            {
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)));
                if icons::primary(ui, &pal, Icon::Export, "Export").clicked() {
                    self.export_open = true;
                    self.export_result = None;
                }
                ui.menu_button(egui::RichText::new("More").size(14.0), |ui| {
                    ui.set_min_width(220.0);
                    if ui.button("Import sample…").on_hover_text("Copy .wav/.ogg/.mp3 files into this project").clicked() {
                        if let Some(files) = rfd::FileDialog::new().add_filter("Audio", &project::AUDIO_EXTS).pick_files() {
                            let n = files.len();
                            for f in files {
                                if let Err(e) = project::import_sample(&self.entry.samples_dir(), &f) {
                                    self.save_error = Some(format!("{e:#}"));
                                }
                            }
                            self.refresh_library();
                            self.toast(format!("Imported {n} sample{}", if n == 1 { "" } else { "s" }));
                        }
                    }
                    if ui.button("Keyboard shortcuts").clicked() {
                        self.show_keys = true;
                    }
                    if ui.checkbox(&mut self.scrub_sound, "Scrub sound").on_hover_text("Wheel and arrow seeks glide like dragging a record (with sound) instead of jumping").changed() {
                        if let Some(e) = engine {
                            e.set_scrub_sound(self.scrub_sound);
                        }
                        self.toast(if self.scrub_sound { "Scrub sound on" } else { "Scrub sound off" });
                    }
                    if ui.button("Open mapset folder").clicked() {
                        let _ = std::process::Command::new("explorer").arg(&self.project.mapset_dir).spawn();
                    }
                    if ui.button(egui::RichText::new("Remove all hits…").color(pal.danger)).clicked() {
                        self.confirm_clear = true;
                    }
                });
                theme_ui(ui);
            }
        });

        // Tool strip.
        egui::Panel::top("work-tools").frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin { left: 12, right: 12, top: 2, bottom: 8 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                icons::group(ui, &pal, |ui| {
                    if icons::button(ui, &pal, Icon::Cursor, "Select tool (1): click or drag a box to select hits", self.tool == Tool::Select, true).clicked() {
                        self.tool = Tool::Select;
                    }
                    if icons::button(ui, &pal, Icon::Pencil, "Place tool (2): click the grid to place hits", self.tool == Tool::Place, true).clicked() {
                        self.tool = Tool::Place;
                    }
                });
                ui.add_space(6.0);
                icons::group(ui, &pal, |ui| {
                    ui.label(egui::RichText::new("Snap").size(12.0).color(pal.text_weak));
                    let mut shown: Vec<u32> = vec![1, 2, 3, 4, 6, 8, 12, 16];
                    if !shown.contains(&self.project.divisor) {
                        shown.push(self.project.divisor);
                        shown.sort_unstable();
                    }
                    for d in shown {
                        if icons::chip(ui, &pal, &format!("1/{d}"), self.project.divisor == d, "Ctrl+wheel or Shift+1..9").clicked() {
                            self.project.divisor = d;
                            self.dirty = true;
                        }
                    }
                });
                ui.add_space(6.0);
                icons::group(ui, &pal, |ui| {
                    if icons::button(ui, &pal, Icon::Undo, "Undo (Ctrl+Z)", false, !self.undo.is_empty()).clicked() {
                        self.undo(engine);
                    }
                    if icons::button(ui, &pal, Icon::Redo, "Redo (Ctrl+Y)", false, !self.redo.is_empty()).clicked() {
                        self.redo(engine);
                    }
                    if icons::button(ui, &pal, Icon::Follow, "Follow the playhead while playing", self.follow, true).clicked() {
                        self.follow = !self.follow;
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().slider_width = 80.0;
                    icons::group(ui, &pal, |ui| {
                        let mut z = self.project.zoom;
                        if ui.add(egui::Slider::new(&mut z, 0.02..=2.0).logarithmic(true).show_value(false)).on_hover_text("Zoom (Alt+wheel)").changed() {
                            self.project.zoom = z;
                            self.dirty = true;
                        }
                        ui.label(egui::RichText::new("Zoom").size(12.0).color(pal.text_weak));
                    });
                    icons::group(ui, &pal, |ui| {
                        ui.add(egui::Slider::new(&mut vols.1, 0.0..=1.5).show_value(false)).on_hover_text("Hitsound volume");
                        ui.label(egui::RichText::new("Hits").size(12.0).color(pal.text_weak));
                    });
                    icons::group(ui, &pal, |ui| {
                        ui.add(egui::Slider::new(&mut vols.0, 0.0..=1.0).show_value(false)).on_hover_text("Song volume");
                        ui.label(egui::RichText::new("Song").size(12.0).color(pal.text_weak));
                    });
                });
            });
        });

        egui::Panel::bottom("work-bottom").frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin::symmetric(12, 4))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let total = self.duration();
                ui.spacing_mut().slider_width = (ui.available_width() - 360.0).max(100.0);
                let mut v = self.view_start;
                if ui.add(egui::Slider::new(&mut v, -2000.0..=total).show_value(false)).on_hover_text("Scroll the timeline").changed() {
                    self.view_start = v;
                    self.follow = false;
                }
                let sel = self.selected.len();
                let info = if sel > 0 { format!("{sel} selected") } else { format!("{} hits", self.project.content.tracks.iter().map(|t| t.notes.len()).sum::<usize>()) };
                ui.label(egui::RichText::new(info).size(12.0).color(pal.text_weak));
                let speed_text = format!("{:.0}% speed", self.speed * 100.0);
                if ui.add(egui::Button::new(egui::RichText::new(speed_text).size(12.0)).frame(false)).on_hover_text("Ctrl+Up/Down: 25% steps, Ctrl+Shift+Up/Down: 5%. Click to reset.").clicked() {
                    self.set_speed(1.0, engine);
                }
                let status = if let Some(e) = &self.save_error {
                    egui::RichText::new(e).color(pal.danger)
                } else if self.dirty {
                    egui::RichText::new("Saving…").color(pal.text_weak)
                } else {
                    egui::RichText::new("Saved").color(pal.text_weak)
                };
                ui.label(status.size(12.0));
            });
        });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin { left: 12, right: 12, top: 4, bottom: 4 })).show(ui, |ui| {
            if let Some(e) = &self.song_error {
                ui.colored_label(self.pal.danger, format!("Couldn't load the song: {e}"));
            } else if self.song.is_none() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading song…");
                });
            }
            let full = ui.available_rect_before_wrap();
            let timeline_left = full.left() + HEADER_W + 8.0;
            let lane_right = (full.right() - 14.0).max(timeline_left + 100.0);
            let grid_w = lane_right - timeline_left;

            // Mouse wheel over the timeline works like the osu! editor:
            // wheel seeks (Shift = 4x), Ctrl+wheel changes snap, Alt+wheel zooms.
            // Raw wheel events are counted in notches so one notch = one step.
            self.view_span = grid_w as f64 / self.project.zoom as f64;
            let hover = ui.input(|i| i.pointer.hover_pos());
            if let Some(h) = hover.filter(|h| full.contains(*h) && h.x > timeline_left) {
                let notches: Vec<(f32, Modifiers)> = ui.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            Event::MouseWheel { unit, delta, modifiers, .. } => {
                                // Wheel up / away from you is positive y.
                                let d = delta.y + delta.x;
                                let n = match unit {
                                    egui::MouseWheelUnit::Line => d,
                                    egui::MouseWheelUnit::Point => d / 40.0,
                                    egui::MouseWheelUnit::Page => d * 3.0,
                                };
                                Some((n, *modifiers))
                            }
                            _ => None,
                        })
                        .collect()
                });
                let tr = Rect::from_min_size(pos2(timeline_left, full.top()), vec2(grid_w, full.height()));
                for (n, m) in notches {
                    if m.command {
                        // Wheel up = finer snap (1/4 -> 1/6), down = coarser.
                        self.div_acc += n;
                        while self.div_acc.abs() >= 1.0 {
                            let up = self.div_acc > 0.0;
                            self.div_acc -= 1.0f32.copysign(self.div_acc);
                            self.step_divisor(up);
                        }
                    } else if m.alt {
                        let anchor = self.t_of(tr, h.x);
                        self.project.zoom = (self.project.zoom * (n * 0.18).exp()).clamp(0.02, 2.0);
                        self.view_start = anchor - ((h.x - tr.left()) / self.project.zoom) as f64;
                        self.dirty = true;
                    } else {
                        self.wheel_acc += n;
                        let mult = if m.shift { 4 } else { 1 };
                        while self.wheel_acc.abs() >= 1.0 {
                            let forward = self.wheel_acc < 0.0;
                            self.wheel_acc -= 1.0f32.copysign(self.wheel_acc);
                            self.seek_ticks(engine, if forward { mult } else { -mult });
                        }
                    }
                }
                // Keep egui from also scrolling the track list or zooming.
                ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
            }
            if playing && self.follow {
                let span = grid_w as f64 / self.project.zoom as f64;
                if pos > self.view_start + span * 0.85 || pos < self.view_start {
                    self.view_start = pos - span * 0.15;
                }
            }

            // Ruler with bar numbers.
            let (_, rect, resp) = lane_row(ui, "ruler", RULER_H, timeline_left, lane_right);
            self.ruler(ui, rect, &resp, pos, engine);

            // Waveform row.
            let (header, rect, resp) = lane_row(ui, "wave", WAVE_H, timeline_left, lane_right);
            {
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(header));
                ui.shrink_clip_rect(header);
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Song").size(13.0).strong());
                ui.label(egui::RichText::new(&self.project.osu_file).size(11.5).color(pal.text_weak));
                ui.label(egui::RichText::new(format!("{} bookmarks", self.project.bookmarks.len())).size(11.5).color(pal.text_weak));
            }
            self.wave_lane(ui, rect, &resp, pos, engine);
            ui.add_space(4.0);

            // Volume (control point) lane.
            let (header, rect, resp) = lane_row(ui, "volume", LANE_H, timeline_left, lane_right);
            {
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(header));
                ui.shrink_clip_rect(header);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Volume").size(13.0).strong());
                    ui.label(egui::RichText::new(format!("{}%", self.project.volume_at(pos as i64))).size(13.0).color(pal.accent));
                });
                ui.label(egui::RichText::new("Click to add a point, drag to change it").size(11.5).color(pal.text_weak));
            }
            self.volume_lane(ui, rect, &resp, pos, engine);
            ui.add_space(6.0);
            ui.separator();

            if self.project.content.tracks.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("No tracks yet").size(18.0).strong());
                    ui.label(egui::RichText::new("A track is one hitsound sample. Add one, then click the grid to place hits.").color(pal.text_weak));
                    ui.add_space(10.0);
                    if icons::primary(ui, &pal, Icon::Plus, "Add track").clicked() {
                        self.add_track(engine);
                    }
                    if self.library.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("No samples found in the mapset or your skin. Use \"Import sample…\" to add some.").weak());
                    }
                });
            }

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let count = self.project.content.tracks.len();
                let mut rects = Vec::with_capacity(count);
                for ti in 0..count {
                    let id = self.project.content.tracks[ti].id;
                    let (header, rect, resp) = lane_row(ui, ("track", id), ROW_H, timeline_left, lane_right);
                    {
                        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(header).layout(egui::Layout::left_to_right(egui::Align::Center)));
                        ui.shrink_clip_rect(header);
                        self.track_header(ui, ti, engine);
                    }
                    rects.push((ti, rect));
                    self.track_lane(ui, ti, rect, &resp, pos, engine);
                    ui.add_space(2.0);
                }
                self.row_rects = rects;
                if count > 0 {
                    ui.add_space(4.0);
                    if ui.add(egui::Button::new(egui::RichText::new("+  Add track").color(pal.text_weak)).frame(false)).clicked() {
                        self.add_track(engine);
                    }
                }
            });
        });

        if let Some(ti) = self.confirm_remove {
            let name = self.project.content.tracks.get(ti).map(|t| t.name.clone()).unwrap_or_default();
            let mut close = false;
            egui::Modal::new(egui::Id::unique("confirm-remove")).show(&ctx, |ui| {
                ui.heading("Remove track?");
                ui.label(format!("\"{name}\" and all its hits will be removed. You can undo this."));
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        self.checkpoint();
                        if ti < self.project.content.tracks.len() {
                            self.project.content.tracks.remove(ti);
                        }
                        self.changed(engine);
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm_remove = None;
            }
        }

        self.draw_toast(&ctx);
        if self.show_keys {
            let mut open = true;
            egui::Window::new("Keyboard shortcuts").open(&mut open).collapsible(false).default_width(460.0).show(&ctx, |ui| {
                ui.label(egui::RichText::new("Same keys as the osu! editor where they make sense.").weak());
                egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                    for (group, rows) in SHORTCUTS {
                        ui.add_space(6.0);
                        ui.strong(*group);
                        egui::Grid::new(*group).num_columns(2).spacing([18.0, 3.0]).show(ui, |ui| {
                            for (k, what) in rows.iter() {
                                ui.label(egui::RichText::new(*k).monospace());
                                ui.label(*what);
                                ui.end_row();
                            }
                        });
                    }
                });
            });
            self.show_keys = open;
        }

        if self.confirm_clear {
            let mut close = false;
            egui::Modal::new(egui::Id::unique("confirm-clear")).show(&ctx, |ui| {
                ui.heading("Remove all hits?");
                ui.label("Every hit on every track will be removed. Tracks and volume points stay. You can undo this.");
                ui.horizontal(|ui| {
                    if ui.button("Remove all").clicked() {
                        self.checkpoint();
                        for t in &mut self.project.content.tracks {
                            t.notes.clear();
                        }
                        self.selected.clear();
                        self.changed(engine);
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm_clear = false;
            }
        }

        if self.export_open {
            let mut open = true;
            egui::Window::new("Export hitsound difficulty").open(&mut open).collapsible(false).resizable(false).show(&ctx, |ui| {
                ui.label("Writes a new difficulty plus generated samples into the mapset folder.");
                ui.label(egui::RichText::new(self.project.mapset_dir.display().to_string()).small().weak());
                ui.add_space(6.0);
                egui::Grid::new("export-grid").num_columns(2).show(ui, |ui| {
                    ui.label("Difficulty name");
                    if ui.text_edit_singleline(&mut self.project.export.diff_name).changed() {
                        self.dirty = true;
                    }
                    ui.end_row();
                    ui.label("First sample index");
                    if ui.add(egui::DragValue::new(&mut self.project.export.start_index).range(1..=9999)).on_hover_text("Indices already used by the mapset are skipped").changed() {
                        self.dirty = true;
                    }
                    ui.end_row();
                });
                ui.add_space(6.0);
                if ui.button("Export").clicked() {
                    self.save_now();
                    self.export_result = Some(match export::export(&self.project, &self.library) {
                        Ok(r) => {
                            let mut msg = format!(
                                "Wrote \"{}\": {} circles, {} sample files (normal-hitnormal{}..{}).",
                                r.osu_file, r.circles, r.sample_files, r.first_index, r.last_index
                            );
                            if r.clipped > 0 {
                                msg.push_str(&format!(" {} mixed samples clip; lower some track volumes.", r.clipped));
                            }
                            Ok(msg)
                        }
                        Err(e) => Err(format!("{e:#}")),
                    });
                    self.refresh_library();
                }
                match &self.export_result {
                    Some(Ok(m)) => {
                        ui.colored_label(self.pal.success, m);
                        if ui.button("Open mapset folder").clicked() {
                            let _ = std::process::Command::new("explorer").arg(&self.project.mapset_dir).spawn();
                        }
                    }
                    Some(Err(e)) => {
                        ui.colored_label(self.pal.danger, e);
                    }
                    None => {}
                }
            });
            self.export_open = open;
        }
        go_home
    }

    fn track_header(&mut self, ui: &mut egui::Ui, ti: usize, engine: Option<&Engine>) {
        let pal = self.pal;
        let library = self.library.clone();
        let full = ui.max_rect();
        let mut edited = false;
        let mut audition = None;
        let mut remove = false;
        let pos = self.pos_now;
        let playing = self.playing_now;
        let renaming = self.renaming.clone();
        let t = &mut self.project.content.tracks[ti];
        let color = pal.tracks[t.color as usize % 8];

        // Card background and colour bar that pulses when this track plays a hit.
        let card = full.shrink2(vec2(0.0, 2.0));
        ui.painter().rect_filled(card, CornerRadius::same(8), pal.panel);
        let lit = playing && t.notes.iter().rev().find(|&&n| (n as f64) <= pos).is_some_and(|&n| pos - (n as f64) < 120.0);
        let bar = Rect::from_min_size(card.min, vec2(if lit { 7.0 } else { 4.0 }, card.height()));
        ui.painter().rect_filled(bar, CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, if lit { color.lerp_to_gamma(Color32::WHITE, 0.35) } else { color });

        let inner = card.shrink2(vec2(12.0, 5.0)).translate(vec2(4.0, 0.0));
        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
        let mut commit_rename = None;
        ui.horizontal(|ui| {
            ui.set_height(24.0);
            match &renaming {
                Some((id, buf)) if *id == t.id => {
                    let mut b = buf.clone();
                    let r = ui.add(egui::TextEdit::singleline(&mut b).desired_width(130.0));
                    r.request_focus();
                    if r.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        commit_rename = Some(Some(b.clone()));
                    } else if ui.input(|i| i.key_pressed(Key::Escape)) {
                        commit_rename = Some(None);
                    } else {
                        commit_rename = Some(Some(format!("\u{0}{b}")));
                    }
                }
                _ => {
                    let name = egui::Label::new(egui::RichText::new(&t.name).size(13.5).strong()).truncate().sense(Sense::click());
                    let r = ui.scope(|ui| {
                        ui.set_max_width(130.0);
                        ui.add(name)
                    }).inner.on_hover_text("Double-click to rename");
                    if r.double_clicked() {
                        commit_rename = Some(Some(format!("\u{0}{}", t.name)));
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if icons::button(ui, &pal, Icon::Trash, "Remove track", false, true).clicked() {
                    remove = true;
                }
                if icons::button(ui, &pal, Icon::Play, "Preview sample", false, t.sample.is_some()).clicked() {
                    audition = t.sample.clone().map(|s| (s, t.volume));
                }
                if icons::button(ui, &pal, Icon::Headphones, "Solo", t.solo, true).clicked() {
                    t.solo = !t.solo;
                    edited = true;
                }
                if icons::button(ui, &pal, if t.muted { Icon::SpeakerOff } else { Icon::Speaker }, "Mute", t.muted, true).clicked() {
                    t.muted = !t.muted;
                    edited = true;
                }
            });
        });
        ui.horizontal(|ui| {
            let label = t.sample.as_ref().map(|s| s.file.clone()).unwrap_or_else(|| "Choose a sample".into());
            egui::ComboBox::from_id_salt(("sample", t.id)).width(160.0).selected_text(egui::RichText::new(label).size(12.5)).show_ui(ui, |ui| {
                let mut last = None;
                for e in &library {
                    if last != Some(e.r.source) {
                        ui.label(egui::RichText::new(e.r.source.label()).size(11.0).color(pal.text_weak));
                        last = Some(e.r.source);
                    }
                    let sel = t.sample.as_ref() == Some(&e.r);
                    if ui.selectable_label(sel, &e.r.file).clicked() && !sel {
                        t.sample = Some(e.r.clone());
                        edited = true;
                        audition = Some((e.r.clone(), t.volume));
                    }
                }
            });
            ui.spacing_mut().slider_width = 56.0;
            let r = ui.add(egui::Slider::new(&mut t.volume, 0..=100).show_value(false)).on_hover_text("Track volume");
            ui.label(egui::RichText::new(format!("{}%", t.volume)).size(12.0).color(pal.text_weak));
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                edited = true;
            }
        });
        match commit_rename {
            Some(Some(b)) if b.starts_with('\u{0}') => {
                let id = t.id;
                self.renaming = Some((id, b[1..].to_string()));
            }
            Some(Some(b)) => {
                let b = b.trim().to_string();
                if !b.is_empty() && b != self.project.content.tracks[ti].name {
                    self.checkpoint();
                    self.project.content.tracks[ti].name = b;
                    edited = true;
                }
                self.renaming = None;
            }
            Some(None) => self.renaming = None,
            None => {}
        }
        if edited {
            self.dirty = true;
            self.last_edit = Instant::now();
            self.rebuild_events(engine);
        }
        if remove {
            self.confirm_remove = Some(ti);
        }
        if let Some((s, vol)) = audition {
            if let Some(p) = self.sample_path(&s) {
                if let Some(id) = self.ensure_sample(&p, engine) {
                    if let Some(e) = engine {
                        e.audition(id, vol as f32 / 100.0);
                    }
                }
            }
        }
    }

    fn ruler(&mut self, ui: &mut egui::Ui, rect: Rect, resp: &egui::Response, pos: f64, engine: Option<&Engine>) {
        let painter = ui.painter_at(rect);
        let t0 = self.t_of(rect, rect.left());
        let t1 = self.t_of(rect, rect.right());
        let mut last_label = f32::MIN;
        osu::grid_lines(&self.reds, t0, t1, 1, |t, _, measure| {
            let x = self.x_of(rect, t);
            if measure {
                painter.vline(x, rect.y_range(), Stroke::new(1.0, self.pal.grid_measure));
                if x - last_label > 34.0 {
                    let bar = osu::measure_number(&self.reds, t);
                    painter.text(pos2(x + 4.0, rect.center().y), Align2::LEFT_CENTER, format!("{bar}"), FontId::proportional(11.0), self.pal.text_weak);
                    last_label = x;
                }
            } else {
                painter.vline(x, (rect.bottom() - 5.0)..=rect.bottom(), Stroke::new(1.0, self.pal.grid_beat));
            }
        });
        let x = self.x_of(rect, pos);
        if rect.x_range().contains(x) {
            let tri = vec![pos2(x - 6.0, rect.top() + 4.0), pos2(x + 6.0, rect.top() + 4.0), pos2(x, rect.bottom())];
            painter.add(egui::Shape::convex_polygon(tri, self.pal.playhead, Stroke::NONE));
        }
        if resp.clicked() || resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let t = self.t_of(rect, p.x).max(0.0);
                if let Some(e) = engine {
                    e.seek_ms(t);
                }
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    fn draw_grid(&self, painter: &egui::Painter, rect: Rect, beats_only: bool) {
        let t0 = self.t_of(rect, rect.left());
        let t1 = self.t_of(rect, rect.right());
        let rl = osu::red_line_at(&self.reds, t0);
        let px_per_div = rl.beat_length / self.project.divisor as f64 * self.project.zoom as f64;
        let div = if beats_only || px_per_div < 6.0 { 1 } else { self.project.divisor };
        let beat_px = rl.beat_length * self.project.zoom as f64;
        osu::grid_lines(&self.reds, t0, t1, div, |t, sub, measure| {
            if beat_px < 4.0 && !measure {
                return;
            }
            let x = self.x_of(rect, t);
            let c = if measure {
                self.pal.grid_measure
            } else if sub == 0 {
                self.pal.grid_beat
            } else {
                self.pal.grid_sub
            };
            painter.vline(x, rect.y_range(), Stroke::new(1.0, c));
        });
        for r in &self.reds {
            let x = self.x_of(rect, r.offset);
            if rect.x_range().contains(x) {
                painter.vline(x, rect.y_range(), Stroke::new(1.5, self.pal.red_line));
            }
        }
    }

    fn draw_playhead(&self, painter: &egui::Painter, rect: Rect, pos: f64) {
        let x = self.x_of(rect, pos);
        if rect.x_range().contains(x) {
            painter.vline(x, rect.y_range(), Stroke::new(2.0, self.pal.playhead));
        }
    }

    fn wave_lane(&mut self, ui: &mut egui::Ui, rect: Rect, resp: &egui::Response, pos: f64, engine: Option<&Engine>) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(4), self.pal.wave_bg);
        self.draw_grid(&painter, rect, true);
        if let Some(song) = &self.song {
            let p = &song.peaks;
            let mid = rect.center().y;
            let half = rect.height() * 0.45;
            let frames_per_ms = p.rate as f64 / 1000.0;
            let mut x = rect.left();
            while x < rect.right() {
                let a = self.t_of(rect, x) * frames_per_ms / p.block as f64;
                let b = self.t_of(rect, x + 1.0) * frames_per_ms / p.block as f64;
                let (ia, ib) = (a.max(0.0) as usize, (b.max(0.0) as usize).max(a.max(0.0) as usize + 1));
                if ia < p.mins.len() {
                    let ib = ib.min(p.mins.len());
                    let lo = p.mins[ia..ib].iter().cloned().fold(0.0, f32::min);
                    let hi = p.maxs[ia..ib].iter().cloned().fold(0.0, f32::max);
                    painter.vline(x, (mid - hi * half)..=(mid - lo * half).max(mid - hi * half + 1.0), Stroke::new(1.0, self.pal.wave));
                }
                x += 1.0;
            }
        }
        for &b in &self.project.bookmarks {
            let x = self.x_of(rect, b as f64);
            if rect.x_range().contains(x) {
                painter.vline(x, rect.y_range(), Stroke::new(1.5, self.pal.accent.gamma_multiply(0.8)));
                let tri = vec![pos2(x - 5.0, rect.top()), pos2(x + 5.0, rect.top()), pos2(x, rect.top() + 7.0)];
                painter.add(egui::Shape::convex_polygon(tri, self.pal.accent, Stroke::NONE));
            }
        }
        self.draw_playhead(&painter, rect, pos);

        if resp.drag_started() {
            self.drag = Some(Drag::Pan { start_view: self.view_start });
        }
        if let Some(Drag::Pan { start_view }) = &self.drag {
            let total = resp.total_drag_delta().map(|d| d.x).unwrap_or(0.0);
            self.view_start = start_view - (total / self.project.zoom) as f64;
            self.follow = false;
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
        if resp.clicked() {
            if let (Some(p), Some(e)) = (resp.interact_pointer_pos(), engine) {
                e.seek_ms(self.t_of(rect, p.x).max(0.0));
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    fn vol_y(rect: Rect, v: u8) -> f32 {
        rect.bottom() - 4.0 - (rect.height() - 8.0) * v as f32 / 100.0
    }

    fn y_vol(rect: Rect, y: f32) -> u8 {
        let v = ((rect.bottom() - 4.0 - y) / (rect.height() - 8.0) * 100.0).round();
        (v.clamp(5.0, 100.0) as u8 / 5 * 5).max(5)
    }

    fn volume_lane(&mut self, ui: &mut egui::Ui, rect: Rect, resp: &egui::Response, pos: f64, engine: Option<&Engine>) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(4), self.pal.lane);
        self.draw_grid(&painter, rect, true);
        let mut cps = self.project.content.control_points.clone();
        if let Some(Drag::Point { idx, cur, .. }) = &self.drag {
            if *idx < cps.len() {
                cps[*idx] = *cur;
                cps.sort_by_key(|c| c.time);
            }
        }
        let fill = self.pal.vol_fill;
        for (i, c) in cps.iter().enumerate() {
            let x0 = if i == 0 { rect.left() } else { self.x_of(rect, c.time as f64).max(rect.left()) };
            let x1 = cps.get(i + 1).map(|n| self.x_of(rect, n.time as f64)).unwrap_or(rect.right()).min(rect.right());
            if x1 <= rect.left() || x0 >= rect.right() {
                continue;
            }
            let y = Self::vol_y(rect, c.volume);
            painter.rect_filled(Rect::from_min_max(pos2(x0, y), pos2(x1, rect.bottom())), 0.0, fill);
            painter.hline(x0..=x1, y, Stroke::new(1.5, self.pal.vol_line));
        }
        for c in &cps {
            let x = self.x_of(rect, c.time as f64);
            if rect.x_range().contains(x) {
                let y = Self::vol_y(rect, c.volume);
                painter.circle_filled(pos2(x, y), 5.0, self.pal.vol_line);
                painter.text(pos2(x + 7.0, y - 2.0), Align2::LEFT_BOTTOM, format!("{}%", c.volume), FontId::proportional(11.0), self.pal.text);
            }
        }
        self.draw_playhead(&painter, rect, pos);

        let hit = |w: &Self, p: Pos2| -> Option<usize> {
            w.project.content.control_points.iter().enumerate().filter(|(_, c)| (w.x_of(rect, c.time as f64) - p.x).abs() < 8.0).map(|(i, _)| i).next()
        };
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(i) = hit(self, p) {
                    let c = self.project.content.control_points[i];
                    self.drag = Some(Drag::Point { idx: i, orig: c, cur: c });
                }
            }
        }
        if let (Some(Drag::Point { orig, .. }), Some(p)) = (&self.drag, resp.interact_pointer_pos()) {
            let orig_x = self.x_of(rect, orig.time as f64);
            let t = self.snap(self.t_of(rect, p.x)).max(0);
            let volume = Self::y_vol(rect, p.y);
            if let Some(Drag::Point { orig, cur, .. }) = &mut self.drag {
                // Small horizontal jitter while adjusting volume shouldn't move the point.
                cur.time = if (p.x - orig_x).abs() > 6.0 { t } else { orig.time };
                cur.volume = volume;
            }
        }
        if resp.drag_stopped() {
            if let Some(Drag::Point { idx, orig, cur }) = self.drag.take() {
                if orig != cur {
                    let taken = self.project.content.control_points.iter().enumerate().any(|(i, c)| i != idx && c.time == cur.time);
                    if !taken {
                        self.checkpoint();
                        self.project.content.control_points[idx] = cur;
                        self.project.content.control_points.sort_by_key(|c| c.time);
                        self.changed(engine);
                    }
                }
            }
        }
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if hit(self, p).is_none() {
                    let t = self.snap(self.t_of(rect, p.x)).max(0);
                    if !self.project.content.control_points.iter().any(|c| c.time == t) {
                        self.checkpoint();
                        self.project.content.control_points.push(ControlPoint { time: t, volume: Self::y_vol(rect, p.y) });
                        self.project.content.control_points.sort_by_key(|c| c.time);
                        self.changed(engine);
                    }
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(i) = hit(self, p) {
                    if self.project.content.control_points.len() > 1 {
                        self.checkpoint();
                        self.project.content.control_points.remove(i);
                        self.changed(engine);
                    }
                }
            }
        }
    }

    fn track_lane(&mut self, ui: &mut egui::Ui, ti: usize, rect: Rect, resp: &egui::Response, pos: f64, engine: Option<&Engine>) {
        let painter = ui.painter_at(rect);
        let track = &self.project.content.tracks[ti];
        let tid = track.id;
        let color = self.pal.tracks[track.color as usize % 8];
        let dim = !self.project.audible(track);
        painter.rect_filled(rect, CornerRadius::same(4), self.pal.lane);
        self.draw_grid(&painter, rect, false);
        let t0 = self.t_of(rect, rect.left() - 10.0) as i64;
        let t1 = self.t_of(rect, rect.right() + 10.0) as i64;
        let delta = match &self.drag {
            Some(Drag::Notes { delta, .. }) => *delta,
            _ => 0,
        };
        let fillc = if dim { color.gamma_multiply(0.35) } else { color };
        let h = rect.height() - 14.0;
        let notes = &track.notes;
        let start = notes.partition_point(|&n| n < t0 - delta.abs());
        let hover_x = resp.hover_pos().map(|p| p.x);
        let playing = self.playing_now;
        for &n in &notes[start..] {
            if n > t1 + delta.abs() {
                break;
            }
            let sel = self.selected.contains(&(tid, n));
            let shown = if sel { n + delta } else { n };
            let x = self.x_of(rect, shown as f64);
            let hovered = self.drag.is_none() && hover_x.is_some_and(|hx| (hx - x).abs() < 7.0);
            // Flash for ~180 ms after the playhead passes a hit.
            let since = pos - n as f64;
            if playing && (0.0..180.0).contains(&since) {
                let k = 1.0 - (since / 180.0) as f32;
                let glow = Rect::from_center_size(pos2(x, rect.center().y), vec2(10.0 + 18.0 * k, h + 6.0 * k));
                painter.rect_filled(glow, CornerRadius::same(8), color.gamma_multiply(0.45 * k));
            }
            let w = if sel || hovered { 12.0 } else { 10.0 };
            let r = Rect::from_center_size(pos2(x, rect.center().y), vec2(w, h));
            let fill = if hovered { fillc.lerp_to_gamma(Color32::WHITE, 0.25) } else { fillc };
            painter.rect_filled(r, CornerRadius::same(5), fill);
            if sel {
                painter.rect_stroke(r.expand(2.0), CornerRadius::same(7), Stroke::new(2.0, self.pal.text), StrokeKind::Outside);
            } else {
                painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, Color32::from_black_alpha(90)), StrokeKind::Inside);
            }
        }
        // Rubber band.
        if let Some(Drag::Box { start, cur }) = &self.drag {
            let b = Rect::from_two_pos(*start, *cur).intersect(rect);
            if b.is_positive() {
                painter.rect_filled(b, 0.0, self.pal.accent.gamma_multiply(0.15));
                painter.rect_stroke(b, 0.0, Stroke::new(1.0, self.pal.accent), StrokeKind::Inside);
            }
        }
        // Hover ghost for the snapped position (Place tool).
        if self.drag.is_none() && self.tool == Tool::Place {
            if let Some(hp) = resp.hover_pos() {
                let t = self.snap(self.t_of(rect, hp.x));
                let x = self.x_of(rect, t as f64);
                painter.rect_stroke(Rect::from_center_size(pos2(x, rect.center().y), vec2(9.0, h)), CornerRadius::same(3), Stroke::new(1.0, color.gamma_multiply(0.6)), StrokeKind::Inside);
            }
        }
        self.draw_playhead(&painter, rect, pos);

        let near = |w: &Self, p: Pos2| -> Option<i64> {
            let t = w.t_of(rect, p.x);
            let tol = (7.0 / w.project.zoom) as f64;
            let notes = &w.project.content.tracks[ti].notes;
            let i = notes.partition_point(|&n| (n as f64) < t - tol);
            notes.get(i).copied().filter(|&n| ((n as f64) - t).abs() <= tol)
        };
        let ctrl = ui.input(|i| i.modifiers.command);
        let shift = ui.input(|i| i.modifiers.shift);

        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(n) = near(self, p) {
                    if !self.selected.contains(&(tid, n)) {
                        if !ctrl {
                            self.selected.clear();
                        }
                        self.selected.insert((tid, n));
                    }
                    self.drag = Some(Drag::Notes { anchor: n, delta: 0 });
                } else if self.tool == Tool::Select {
                    if !ctrl {
                        self.selected.clear();
                    }
                    self.drag = Some(Drag::Box { start: p, cur: p });
                }
            }
        }
        if let Some(p) = resp.interact_pointer_pos() {
            let raw = self.t_of(rect, p.x);
            // Shift while dragging ignores the snap divisor, like osu!.
            let target = if shift { raw.round() as i64 } else { self.snap(raw) };
            match &mut self.drag {
                Some(Drag::Notes { anchor, delta }) if resp.dragged() => *delta = target - *anchor,
                Some(Drag::Box { cur, .. }) if resp.dragged() => *cur = p,
                _ => {}
            }
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Notes { delta, .. }) if delta != 0 => self.move_selection(delta, engine),
                Some(Drag::Box { start, cur }) => {
                    let b = Rect::from_two_pos(start, cur);
                    let rows = self.row_rects.clone();
                    for (row, r) in rows {
                        if row >= self.project.content.tracks.len() || !(r.top() < b.bottom() && r.bottom() > b.top()) {
                            continue;
                        }
                        let (lo, hi) = (self.t_of(r, b.left()), self.t_of(r, b.right()));
                        let t = &self.project.content.tracks[row];
                        for &n in &t.notes {
                            if (n as f64) >= lo && (n as f64) <= hi {
                                self.selected.insert((t.id, n));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(n) = near(self, p) {
                    if ctrl {
                        if !self.selected.remove(&(tid, n)) {
                            self.selected.insert((tid, n));
                        }
                    } else {
                        self.selected.clear();
                        self.selected.insert((tid, n));
                    }
                    self.audition_track(ti, n, engine);
                } else if self.tool == Tool::Place {
                    let t = self.snap(self.t_of(rect, p.x));
                    if t >= 0 && (t as f64) <= self.duration() {
                        self.checkpoint();
                        self.project.content.tracks[ti].insert_note(t);
                        self.changed(engine);
                        self.audition_track(ti, t, engine);
                    }
                } else if !ctrl {
                    self.selected.clear();
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(n) = near(self, p) {
                    self.checkpoint();
                    self.project.content.tracks[ti].remove_note(n);
                    self.selected.remove(&(tid, n));
                    self.changed(engine);
                }
            }
        }
        if resp.hovered() || resp.dragged() {
            let over_hit = resp.hover_pos().is_some_and(|p| near(self, p).is_some());
            let cursor = match (&self.drag, self.tool) {
                (Some(Drag::Notes { .. }), _) => egui::CursorIcon::Grabbing,
                _ if over_hit => egui::CursorIcon::Grab,
                (_, Tool::Place) => egui::CursorIcon::Crosshair,
                _ => egui::CursorIcon::Default,
            };
            ui.ctx().set_cursor_icon(cursor);
        }
        if resp.hovered() && self.drag.is_none() && self.project.content.tracks[ti].sample.is_none() {
            let hint = if self.project.content.tracks[ti].sample.is_none() {
                "Pick a sample for this track first"
            } else if self.tool == Tool::Place {
                "Click: add · drag: move · right-click: delete · 1: Select tool"
            } else {
                "Click: select · drag: box select or move · Ctrl: add to selection · 2: Place tool"
            };
            resp.clone().on_hover_text_at_pointer(hint);
        }
        if let Some(path) = self.project.content.tracks[ti].sample.as_ref().and_then(|s| self.sample_path(s)) {
            if let Some(err) = self.sample_errors.get(&path) {
                painter.text(rect.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, format!("Can't play sample: {err}"), FontId::proportional(11.0), self.pal.danger);
            }
        } else if let Some(s) = &self.project.content.tracks[ti].sample {
            painter.text(rect.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, format!("Missing sample: {}", s.file), FontId::proportional(11.0), self.pal.danger);
        }
    }
}

const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("Playback", &[
        ("Space / C", "Play or pause"),
        ("X", "Play from the start"),
        ("Z / V", "Jump to first / last hit"),
        ("Left / Right", "Step one snap tick (Shift: 4 ticks)"),
        ("Up / Down", "Previous / next timing or volume point"),
        ("Mouse wheel", "Seek (Shift: 4x)"),
        ("Ctrl+Up / Down", "Playback speed -/+ 25% (add Shift: 5%)"),
    ]),
    ("Snap and zoom", &[
        ("Ctrl+wheel", "Change snap divisor"),
        ("Shift+1..9", "Snap 1/1 to 1/9"),
        ("Ctrl+M", "Next snap divisor"),
        ("Alt+wheel", "Zoom the timeline"),
    ]),
    ("Editing", &[
        ("1 / 2", "Select tool / Place tool"),
        ("Q / W / E / R", "Toggle normal / whistle / finish / clap hit at the playhead"),
        ("Ctrl+A", "Select all hits"),
        ("Ctrl+X / C / V", "Cut / copy / paste at the playhead"),
        ("Ctrl+D", "Clone selection one measure later"),
        ("J / K", "Nudge selection back / forward one tick"),
        ("Delete", "Delete selection"),
        ("Shift (dragging)", "Ignore snap"),
        ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
        ("Ctrl+N", "Remove all hits (asks first)"),
    ]),
    ("Points and bookmarks", &[
        ("Ctrl+Shift+P", "Add volume point at the playhead"),
        ("Ctrl+I", "Delete the volume point in effect"),
        ("Ctrl+B", "Add bookmark"),
        ("Ctrl+Shift+B", "Remove bookmark at the playhead"),
        ("Ctrl+Left / Right", "Previous / next bookmark"),
    ]),
    ("General", &[
        ("Ctrl+S", "Save now (also autosaves)"),
        ("Esc", "Close dialog, or back to projects"),
        ("Ctrl+T", "Switch light / dark"),
    ]),
];

/// Consume a key press whose modifiers match exactly. Also matches the physical
/// key, so Shift+1 works even though the logical key is "!".
fn take_key(ctx: &egui::Context, mods: Modifiers, key: Key) -> bool {
    ctx.input_mut(|i| {
        let mut hit = false;
        i.events.retain(|e| {
            if let Event::Key { key: k, physical_key, pressed: true, modifiers: m, .. } = e {
                if !hit && (*k == key || *physical_key == Some(key)) && m.matches_exact(mods) {
                    hit = true;
                    return false;
                }
            }
            true
        });
        hit
    })
}

impl Work {
    /// Handles all editor shortcuts. Returns true to go back to the project list.
    fn handle_keys(&mut self, ctx: &egui::Context, engine: Option<&Engine>, playing: bool, pos: f64) -> bool {
        let none = Modifiers::NONE;
        let shift = Modifiers::SHIFT;
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let k = |m: Modifiers, key: Key| take_key(ctx, m, key);

        // Transport.
        if k(none, Key::Space) || k(none, Key::C) {
            if let Some(e) = engine {
                e.play(!playing);
            }
        }
        if k(none, Key::X) {
            self.seek(engine, 0.0);
            if let Some(e) = engine {
                e.play(true);
            }
        }
        if k(none, Key::Z) {
            let first = self.project.content.tracks.iter().filter_map(|t| t.notes.first()).min().copied();
            self.seek(engine, first.map(|t| t as f64).unwrap_or(0.0));
        }
        if k(none, Key::V) {
            let last = self.project.content.tracks.iter().filter_map(|t| t.notes.last()).max().copied();
            self.seek(engine, last.map(|t| t as f64).unwrap_or(self.duration()));
        }
        if k(none, Key::Home) {
            self.seek(engine, 0.0);
        }
        for (key, n) in [(Key::ArrowLeft, -1), (Key::ArrowRight, 1)] {
            if k(none, key) {
                self.seek_ticks(engine, n);
            }
            if k(shift, key) {
                self.seek_ticks(engine, n * 4);
            }
        }
        for (key, dir) in [(Key::ArrowUp, -1i32), (Key::ArrowDown, 1)] {
            if k(cmd_shift, key) {
                self.set_speed(self.speed - dir as f64 * 0.05, engine);
            }
            if k(cmd, key) {
                self.set_speed(self.speed - dir as f64 * 0.25, engine);
            }
            if k(none, key) {
                let mut points: Vec<f64> = self.reds.iter().map(|r| r.offset).collect();
                points.extend(self.project.content.control_points.iter().map(|c| c.time as f64));
                points.sort_by(f64::total_cmp);
                let target = if dir < 0 { points.iter().rev().find(|&&t| t < pos - 1.0) } else { points.iter().find(|&&t| t > pos + 1.0) };
                if let Some(&t) = target {
                    self.seek(engine, t);
                }
            }
        }

        // Bookmarks.
        if k(cmd_shift, Key::B) {
            let tol = self.step_ms(pos).max(20.0);
            let near = self.project.bookmarks.iter().position(|&b| ((b as f64) - pos).abs() <= tol);
            if let Some(i) = near {
                self.project.bookmarks.remove(i);
                self.dirty = true;
                self.toast("Bookmark removed");
            }
        }
        if k(cmd, Key::B) {
            let t = pos.round() as i64;
            if let Err(i) = self.project.bookmarks.binary_search(&t) {
                self.project.bookmarks.insert(i, t);
                self.dirty = true;
                self.toast(format!("Bookmark at {}", fmt_time(t as f64)));
            }
        }
        if k(cmd, Key::ArrowLeft) {
            if let Some(&b) = self.project.bookmarks.iter().rev().find(|&&b| (b as f64) < pos - 1.0) {
                self.seek(engine, b as f64);
            }
        }
        if k(cmd, Key::ArrowRight) {
            if let Some(&b) = self.project.bookmarks.iter().find(|&&b| (b as f64) > pos + 1.0) {
                self.seek(engine, b as f64);
            }
        }

        // Snap divisor.
        for (n, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].into_iter().enumerate() {
            if k(shift, key) {
                self.project.divisor = n as u32 + 1;
                self.dirty = true;
                self.toast(format!("Snap 1/{}", n + 1));
            }
        }
        if k(cmd, Key::M) {
            self.step_divisor(true);
        }

        // Tools.
        if k(none, Key::Num1) {
            self.tool = Tool::Select;
            self.toast("Select tool");
        }
        if k(none, Key::Num2) {
            self.tool = Tool::Place;
            self.toast("Place tool");
        }

        // Undo, save, clear.
        if k(cmd_shift, Key::Z) || k(cmd, Key::Y) {
            if !self.redo.is_empty() {
                self.toast("Redo");
            }
            self.redo(engine);
        }
        if k(cmd, Key::Z) {
            if !self.undo.is_empty() {
                self.toast("Undo");
            }
            self.undo(engine);
        }
        if k(cmd, Key::S) {
            self.save_now();
            self.toast("Saved");
        }
        if k(cmd, Key::N) {
            self.confirm_clear = true;
        }

        // Selection and clipboard.
        if k(cmd, Key::A) {
            self.selected = self.project.content.tracks.iter().flat_map(|t| t.notes.iter().map(move |&n| (t.id, n))).collect();
            self.toast(format!("Selected {} hits", self.selected.len()));
        }
        if k(cmd, Key::C) && !self.selected.is_empty() {
            self.copy_selection();
            self.toast(format!("Copied {} hits", self.clipboard.len()));
        }
        if k(cmd, Key::X) && !self.selected.is_empty() {
            self.copy_selection();
            self.delete_selection(engine);
            self.toast(format!("Cut {} hits", self.clipboard.len()));
        }
        if k(cmd, Key::V) {
            let at = self.snap(pos).max(0);
            self.paste_at(at, engine);
            if !self.clipboard.is_empty() {
                self.toast(format!("Pasted {} hits", self.clipboard.len()));
            }
        }
        if k(cmd, Key::D) && !self.selected.is_empty() {
            self.clone_selection(engine);
            self.toast("Cloned one measure later");
        }
        if (k(none, Key::Delete) || k(none, Key::Backspace)) && !self.selected.is_empty() {
            let n = self.selected.len();
            self.delete_selection(engine);
            self.toast(format!("Deleted {n} hits"));
        }
        if k(none, Key::J) {
            self.nudge_selection(-1, engine);
        }
        if k(none, Key::K) {
            self.nudge_selection(1, engine);
        }

        // Hitsound keys: toggle a hit at the playhead on the matching track.
        for (key, kind, fallback) in [(Key::Q, "normal", 0usize), (Key::W, "whistle", 1), (Key::E, "finish", 2), (Key::R, "clap", 3)] {
            if k(none, key) {
                self.toggle_kind_at(kind, fallback, pos, engine);
            }
        }

        // Volume points.
        if k(cmd_shift, Key::P) {
            let t = self.snap(pos).max(0);
            if !self.project.content.control_points.iter().any(|c| c.time == t) {
                self.checkpoint();
                let v = self.project.volume_at(t);
                self.project.content.control_points.push(ControlPoint { time: t, volume: v });
                self.project.content.control_points.sort_by_key(|c| c.time);
                self.changed(engine);
            }
        }
        if k(cmd, Key::I) {
            let cps = &self.project.content.control_points;
            let i = cps.partition_point(|c| (c.time as f64) <= pos + 0.5);
            if i >= 2 {
                self.checkpoint();
                self.project.content.control_points.remove(i - 1);
                self.changed(engine);
            }
        }

        if k(none, Key::Escape) {
            if self.show_keys || self.export_open || self.confirm_clear || self.confirm_remove.is_some() {
                self.show_keys = false;
                self.export_open = false;
                self.confirm_clear = false;
                self.confirm_remove = None;
            } else if !self.selected.is_empty() {
                self.selected.clear();
            } else {
                return true;
            }
        }
        false
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = &self.toast else { return };
        let age = at.elapsed().as_secs_f32();
        if age > 1.8 {
            self.toast = None;
            return;
        }
        ctx.request_repaint();
        let alpha = if age < 0.12 { age / 0.12 } else if age > 1.4 { (1.8 - age) / 0.4 } else { 1.0 };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("toast")));
        let screen = ctx.content_rect();
        let galley = painter.layout_no_wrap(msg.clone(), FontId::proportional(14.0), self.pal.text.gamma_multiply(alpha));
        let size = galley.size() + vec2(28.0, 16.0);
        let rect = Rect::from_center_size(pos2(screen.center().x, screen.bottom() - 70.0 + (1.0 - alpha) * 8.0), size);
        painter.rect_filled(rect, CornerRadius::same(10), self.pal.panel.gamma_multiply(alpha));
        painter.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, self.pal.border.gamma_multiply(alpha)), StrokeKind::Inside);
        painter.galley(rect.center() - galley.size() / 2.0, galley, self.pal.text);
    }

    fn step_ms(&self, t: f64) -> f64 {
        osu::red_line_at(&self.reds, t).beat_length / self.project.divisor as f64
    }

    /// Ctrl+wheel / Ctrl+M step through the usual osu! divisors (1/5, 1/7, 1/9 only via Shift+number).
    fn step_divisor(&mut self, finer: bool) {
        const STEPS: [u32; 8] = [1, 2, 3, 4, 6, 8, 12, 16];
        let cur = self.project.divisor;
        let next = if finer { STEPS.iter().find(|&&d| d > cur).copied() } else { STEPS.iter().rev().find(|&&d| d < cur).copied() };
        if let Some(d) = next {
            self.project.divisor = d;
            self.dirty = true;
            self.toast(format!("Snap 1/{d}"));
        }
    }

    fn set_speed(&mut self, speed: f64, engine: Option<&Engine>) {
        // Round to 5% so repeated steps don't drift.
        let new = ((speed.clamp(0.25, 1.0) * 20.0).round()) / 20.0;
        if (new - self.speed).abs() > 0.001 {
            self.toast(format!("Speed {:.0}%", new * 100.0));
        }
        self.speed = new;
        if let Some(e) = engine {
            e.set_speed(self.speed);
        }
    }

    /// Seek and keep the playhead on screen.
    fn seek(&mut self, engine: Option<&Engine>, t: f64) {
        let t = t.clamp(0.0, self.duration());
        if let Some(e) = engine {
            e.seek_ms(t);
        }
        if t < self.view_start || t > self.view_start + self.view_span * 0.9 {
            self.view_start = t - self.view_span * 0.25;
        }
    }

    fn seek_ticks(&mut self, engine: Option<&Engine>, n: i32) {
        // Count from where the head is heading, so fast notches add up during a scrub.
        let pos = engine.map(|e| e.heading_ms()).unwrap_or(0.0);
        let playing = engine.is_some_and(|e| e.is_playing());
        let forward = n > 0;
        // While playing, the song has moved on a little since the last jump. Going back,
        // treat anything within ~half a tick after a tick as "on" that tick, so each notch
        // really goes back one more tick instead of landing on the same one again.
        let mut t = pos;
        if playing && !forward {
            t -= (self.step_ms(pos) * 0.5).min(150.0);
        }
        for _ in 0..n.abs() {
            t = osu::adjacent_tick(&self.reds, t, self.project.divisor, forward);
        }
        // Glide there like a DJ dragging the record, instead of a hard cut.
        let t = t.clamp(0.0, self.duration());
        if let Some(e) = engine {
            e.scrub_to_ms(t);
        }
        if t < self.view_start || t > self.view_start + self.view_span * 0.9 {
            self.view_start = t - self.view_span * 0.25;
        }
    }

    fn audition_track(&mut self, ti: usize, t: i64, engine: Option<&Engine>) {
        let tr = self.project.content.tracks[ti].clone();
        let Some(s) = &tr.sample else { return };
        if let (Some(path), Some(e)) = (self.sample_path(s), engine) {
            if let Some(id) = self.ensure_sample(&path, Some(e)) {
                if !e.is_playing() {
                    e.audition(id, tr.volume as f32 / 100.0 * self.project.volume_at(t) as f32 / 100.0);
                }
            }
        }
    }

    fn track_index(&self, id: u64) -> Option<usize> {
        self.project.content.tracks.iter().position(|t| t.id == id)
    }

    fn move_selection(&mut self, delta: i64, engine: Option<&Engine>) {
        if self.selected.is_empty() || delta == 0 {
            return;
        }
        let dur = self.duration() as i64;
        let moved: Vec<NoteKey> = self.selected.iter().map(|&(id, t)| (id, t + delta)).collect();
        if moved.iter().any(|&(_, t)| t < 0 || t > dur) {
            return;
        }
        self.checkpoint();
        let old: Vec<NoteKey> = self.selected.drain().collect();
        for &(id, t) in &old {
            if let Some(ti) = self.track_index(id) {
                self.project.content.tracks[ti].remove_note(t);
            }
        }
        for &(id, t) in &moved {
            if let Some(ti) = self.track_index(id) {
                self.project.content.tracks[ti].insert_note(t);
                self.selected.insert((id, t));
            }
        }
        self.changed(engine);
    }

    fn nudge_selection(&mut self, dir: i64, engine: Option<&Engine>) {
        let Some(&(_, first)) = self.selected.iter().min_by_key(|k| k.1) else { return };
        let target = osu::snap(&self.reds, first as f64 + dir as f64 * self.step_ms(first as f64), self.project.divisor).round() as i64;
        self.move_selection(target - first, engine);
    }

    fn copy_selection(&mut self) {
        let Some(min) = self.selected.iter().map(|k| k.1).min() else { return };
        self.clipboard = self.selected.iter().map(|&(id, t)| (id, t - min)).collect();
    }

    fn delete_selection(&mut self, engine: Option<&Engine>) {
        if self.selected.is_empty() {
            return;
        }
        self.checkpoint();
        for (id, t) in self.selected.drain().collect::<Vec<_>>() {
            if let Some(ti) = self.track_index(id) {
                self.project.content.tracks[ti].remove_note(t);
            }
        }
        self.changed(engine);
    }

    fn paste_at(&mut self, at: i64, engine: Option<&Engine>) {
        if self.clipboard.is_empty() {
            return;
        }
        let dur = self.duration() as i64;
        self.checkpoint();
        self.selected.clear();
        for (id, rel) in self.clipboard.clone() {
            let t = at + rel;
            if t > dur {
                continue;
            }
            if let Some(ti) = self.track_index(id) {
                self.project.content.tracks[ti].insert_note(t);
                self.selected.insert((id, t));
            }
        }
        self.changed(engine);
    }

    fn clone_selection(&mut self, engine: Option<&Engine>) {
        let (Some(min), Some(max)) = (self.selected.iter().map(|k| k.1).min(), self.selected.iter().map(|k| k.1).max()) else { return };
        let rl = osu::red_line_at(&self.reds, min as f64);
        let measure = rl.beat_length * rl.meter as f64;
        let measures = (((max - min) as f64 + 1.0) / measure).ceil().max(1.0);
        let shift = (measure * measures).round() as i64;
        self.copy_selection();
        self.paste_at(min + shift, engine);
    }

    /// Q/W/E/R: toggle a hit on the track whose sample is that kind of hitsound.
    fn toggle_kind_at(&mut self, kind: &str, fallback: usize, pos: f64, engine: Option<&Engine>) {
        let tracks = &self.project.content.tracks;
        let ti = tracks
            .iter()
            .position(|t| t.sample.as_ref().is_some_and(|s| s.file.to_ascii_lowercase().contains(kind)))
            .or_else(|| (fallback < tracks.len()).then_some(fallback));
        let Some(ti) = ti else { return };
        let t = self.snap(pos).max(0);
        self.checkpoint();
        let tid = self.project.content.tracks[ti].id;
        if self.project.content.tracks[ti].remove_note(t) {
            self.selected.remove(&(tid, t));
        } else {
            self.project.content.tracks[ti].insert_note(t);
            self.audition_track(ti, t, engine);
        }
        self.changed(engine);
    }
}

/// Reserve a full-width row and split it into a fixed header column and a lane
/// rectangle, so every lane shares the exact same x range.
fn lane_row(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, height: f32, lane_left: f32, lane_right: f32) -> (Rect, Rect, egui::Response) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let header = Rect::from_min_max(row.min, pos2(lane_left - 8.0, row.bottom()));
    let lane = Rect::from_min_max(pos2(lane_left, row.top()), pos2(lane_right, row.bottom()));
    let resp = ui.interact(lane, ui.scope_id().with(id), Sense::click_and_drag());
    (header, lane, resp)
}

fn fmt_time(ms: f64) -> String {
    let ms = ms.max(0.0) as u64;
    format!("{:02}:{:02}.{:03}", ms / 60_000, (ms / 1000) % 60, ms % 1000)
}

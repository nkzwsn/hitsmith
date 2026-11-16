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
const LANE_H: f32 = 76.0;
const ROW_H: f32 = 68.0;
/// Row height in compact mode.
const ROW_H_COMPACT: f32 = 36.0;

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
    /// Show the "Getting started" guide on the home screen.
    show_guide: bool,
    /// Overall output volume (Alt+wheel).
    master_volume: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { last_dir: None, song_volume: 0.7, hit_volume: 1.0, dark_mode: true, dark_palette: 0, light_palette: 0, scrub_sound: false, show_guide: true, master_volume: 1.0 }
    }
}

impl Settings {
    fn palette(&self) -> Palette {
        if self.dark_mode { theme::DARK[self.dark_palette.min(4)] } else { theme::LIGHT[self.light_palette.min(4)] }
    }
}

/// Theme picker: an icon button that rolls a strip of options out to the left:
/// a light/dark switch and the five palettes of the current mode.
/// Returns true when the user picked something.
fn theme_menu(ui: &mut egui::Ui, settings: &mut Settings) -> bool {
    const SLOT: f32 = 28.0;
    const PAD: f32 = 6.0;
    const DIVIDER: f32 = 10.0;
    let id = egui::Id::unique("theme-roll");
    let ctx = ui.ctx().clone();
    let mut open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let pal = settings.palette();
    let mut changed = false;

    let icon = icons::button(ui, &pal, Icon::Theme, "Theme (Ctrl+T switches light / dark)", open, true);
    if icon.clicked() {
        open = !open;
    }
    let t = ctx.animate_bool_with_time_and_easing(id.with("t"), open, 0.32, egui::emath::easing::cubic_out);
    let mut strip = Rect::NOTHING;
    if t > 0.001 {
        let full_w = PAD * 2.0 + SLOT * 7.0 + DIVIDER;
        let (rect, _) = ui.allocate_exact_size(vec2(full_w * t, 30.0), Sense::hover());
        strip = rect;
        let painter = ui.painter().with_clip_rect(rect.expand(3.0));
        painter.rect_filled(rect, CornerRadius::same(15), pal.panel);
        painter.rect_stroke(rect, CornerRadius::same(15), Stroke::new(1.0, pal.border), StrokeKind::Inside);
        let right = rect.right();
        let cy = rect.center().y;
        // Items are anchored to the right edge, so they are revealed one by one as the strip grows left.
        let reveal = |cx: f32| (((rect.width() - (right - cx)) + SLOT * 0.5) / SLOT).clamp(0.0, 1.0);
        let list = if settings.dark_mode { &theme::DARK } else { &theme::LIGHT };
        let current = if settings.dark_mode { settings.dark_palette } else { settings.light_palette };
        for (k, p) in list.iter().enumerate() {
            let cx = right - PAD - SLOT * 0.5 - (4 - k) as f32 * SLOT;
            let a = reveal(cx);
            if a <= 0.0 {
                continue;
            }
            let c = pos2(cx, cy);
            let hit = Rect::from_center_size(c, Vec2::splat(SLOT - 2.0));
            let resp = ui.interact(hit, id.with(("swatch", k)), if a > 0.95 { Sense::click() } else { Sense::hover() });
            let selected = k == current;
            let ring = if selected { Stroke::new(2.0, pal.accent) } else if resp.hovered() { Stroke::new(1.5, pal.text) } else { Stroke::new(1.0, pal.border) };
            painter.circle_filled(c, 10.0 * a, p.bg);
            painter.circle_stroke(c, 10.0 * a, ring);
            painter.circle_filled(c, 5.0 * a, p.accent);
            if resp.clicked() {
                if settings.dark_mode {
                    settings.dark_palette = k;
                } else {
                    settings.light_palette = k;
                }
                changed = true;
            }
            resp.on_hover_text(p.name);
        }
        let div_x = right - PAD - SLOT * 5.0 - DIVIDER * 0.5;
        if reveal(div_x) > 0.0 {
            painter.vline(div_x, (cy - 8.0)..=(cy + 8.0), Stroke::new(1.0, pal.border));
        }
        for (dark, icon_kind, tip, n) in [(true, Icon::Moon, "Dark mode", 0.0), (false, Icon::Sun, "Light mode", 1.0)] {
            let cx = right - PAD - SLOT * 5.0 - DIVIDER - SLOT * 0.5 - n * SLOT;
            let a = reveal(cx);
            if a <= 0.0 {
                continue;
            }
            let c = pos2(cx, cy);
            let hit = Rect::from_center_size(c, Vec2::splat(SLOT - 2.0));
            let resp = ui.interact(hit, id.with(("mode", n as i32)), if a > 0.95 { Sense::click() } else { Sense::hover() });
            let on = settings.dark_mode == dark;
            if on || resp.hovered() {
                painter.circle_filled(c, 12.0 * a, if on { pal.accent.gamma_multiply(0.25) } else { pal.text.gamma_multiply(0.08) });
            }
            icons::paint(&painter, Rect::from_center_size(c, Vec2::splat(16.0 * a)), icon_kind, if on { pal.accent } else { pal.text });
            if resp.clicked() && !on {
                settings.dark_mode = dark;
                changed = true;
            }
            resp.on_hover_text(tip);
        }
        if t < 1.0 {
            ctx.request_repaint();
        }
    }
    // Close when clicking anywhere else, or with Esc.
    if open {
        let outside = ui.input(|i| i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| !strip.contains(p) && !icon.rect.contains(p)));
        if outside || ui.input(|i| i.key_pressed(Key::Escape)) {
            open = false;
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, open));
    changed
}

#[derive(Default)]
struct Home {
    projects: Vec<ProjectEntry>,
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
    titlebar: crate::titlebar::TitleBar,
    /// Palette the window icon was last drawn in.
    icon_palette: Option<(&'static str, bool)>,
    /// Screen change in progress: fading out of the workstation, then into the next screen.
    leaving: Option<Instant>,
    arriving: Option<Instant>,
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
                e.set_master(settings.master_volume);
                e.set_scrub_sound(settings.scrub_sound);
                (Some(e), None)
            }
            Err(e) => (None, Some(format!("Audio output unavailable: {e}"))),
        };
        let theme = ThemeState::new(settings.palette());
        let pal = *theme.target();
        let mut app = Self { data_dir, projects_dir, settings, engine, audio_error, screen: Screen::Home, home: Home::default(), theme, pal, theme_changed: false, titlebar: Default::default(), icon_palette: None, leaving: None, arriving: None };
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

    /// The osu! Songs folder, used as the place to unpack .osz files.
    fn songs_dir(&self) -> PathBuf {
        let is_songs = |d: &Path| d.is_dir() && d.parent().is_some_and(|p| p.join("osu!.exe").is_file());
        library::guess_songs_dir()
            .filter(|d| is_songs(d))
            .or_else(|| self.settings.last_dir.clone().filter(|d| is_songs(d)))
            .unwrap_or_else(|| self.data_dir.join("mapsets"))
    }

    fn pick_beatmap(&mut self, ctx: &egui::Context) {
        let start = self.settings.last_dir.clone().filter(|d| d.is_dir()).or_else(library::guess_songs_dir);
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open a mapset (.osz) or a difficulty (.osu)")
            .add_filter("osu! mapset or difficulty", &["osz", "osu"]);
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        let Some(file) = dialog.pick_file() else { return };
        self.start_from_path(ctx, &file);
    }

    /// Start a project from a dropped or picked .osz, mapset folder or .osu,
    /// with starter hitsound tracks, and open it straight away.
    fn start_from_path(&mut self, ctx: &egui::Context, path: &Path) {
        self.home.error = None;
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let result = (|| -> anyhow::Result<(PathBuf, Option<String>)> {
            if path.is_dir() {
                Ok((path.to_path_buf(), None))
            } else if ext == "osz" {
                let songs = self.songs_dir();
                std::fs::create_dir_all(&songs)?;
                Ok((project::import_osz(path, &songs)?, None))
            } else if ext == "osu" {
                let dir = path.parent().ok_or_else(|| anyhow::anyhow!("bad path"))?.to_path_buf();
                Ok((dir, path.file_name().map(|n| n.to_string_lossy().into_owned())))
            } else {
                anyhow::bail!("Drop a mapset (.osz), a mapset folder, or a difficulty (.osu).")
            }
        })();
        match result.and_then(|(dir, osu)| self.quick_create(ctx, &dir, osu)) {
            Ok(()) => {}
            Err(e) => self.home.error = Some(format!("{e:#}")),
        }
    }

    fn quick_create(&mut self, ctx: &egui::Context, dir: &Path, osu_file: Option<String>) -> anyhow::Result<()> {
        let osu_file = match osu_file {
            Some(f) => f,
            None => {
                let mut diffs: Vec<String> = std::fs::read_dir(dir)?
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.to_ascii_lowercase().ends_with(".osu"))
                    .collect();
                diffs.sort();
                // Prefer the mapset's hitsound difficulty when it has one (by name, or by its
                // objects all stacked on one spot), else the first; never one hitsmith exported itself.
                let ours = export::read_manifest(dir).all_files();
                let candidates: Vec<usize> = (0..diffs.len()).filter(|&i| !ours.iter().any(|o| o.eq_ignore_ascii_case(&diffs[i]))).collect();
                let by_name = candidates.iter().copied().find(|&i| diffs[i].to_ascii_lowercase().contains("hitsound"));
                let by_content = || {
                    candidates.iter().copied().find(|&i| {
                        let text = std::fs::read_to_string(dir.join(&diffs[i])).unwrap_or_default();
                        crate::hsimport::is_hitsound_diff(&diffs[i], &text)
                    })
                };
                let pick = by_name.or_else(by_content).unwrap_or(0);
                diffs.into_iter().nth(pick).ok_or_else(|| anyhow::anyhow!("No difficulty (.osu) found in {}", dir.display()))?
            }
        };
        let bm = osu::read_beatmap(&dir.join(&osu_file))?;
        // A free project name: "Title", "Title (2)", ...
        let base = osu::sanitize_file_part(bm.meta.title.as_str());
        let base = if base.is_empty() { "New project".to_string() } else { base };
        let mut name = base.clone();
        let mut n = 2;
        while self.projects_dir.join(&name).exists() {
            name = format!("{base} ({n})");
            n += 1;
        }
        let mut proj = Project::new(dir.to_path_buf(), osu_file);
        proj.bookmarks = bm.bookmarks;
        // Starter tracks from the mapset's own hitsounds (or the skin's).
        let exported = export::read_manifest(dir).all_files();
        let lib = project::sample_library(&self.projects_dir.join(&name).join("samples"), dir, &bm.meta.audio_filename, library::current_skin_dir(dir).as_deref(), &exported);
        // A hitsound difficulty brings its hits along; otherwise start with empty starter tracks.
        let src_text = std::fs::read_to_string(dir.join(&proj.osu_file)).unwrap_or_default();
        let ours = export::read_manifest(dir).all_files();
        let is_ours = ours.iter().any(|o| o.eq_ignore_ascii_case(&proj.osu_file));
        if !is_ours && crate::hsimport::is_hitsound_diff(&proj.osu_file, &src_text) {
            let imp = crate::hsimport::import(&src_text, &lib, 1);
            proj.content.tracks = imp.tracks;
            proj.content.control_points = imp.control_points;
        } else {
            proj.content.tracks = project::default_tracks(&lib);
        }
        proj.next_id = proj.content.tracks.len() as u64 + 1;
        let entry = project::create(&self.projects_dir, &name, &proj)?;
        if let Some(songs) = dir.parent() {
            self.settings.last_dir = Some(songs.to_path_buf());
            self.save_settings();
        }
        self.refresh_projects();
        self.open_project(ctx, entry);
        Ok(())
    }

    /// Paint the background colour over the whole window at `alpha` (screen transitions).
    fn cover(&self, ctx: &egui::Context, alpha: f32) {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("screen-fade")));
        painter.rect_filled(ctx.content_rect(), 0.0, self.pal.bg.gamma_multiply(alpha.clamp(0.0, 1.0)));
    }

    fn open_project(&mut self, ctx: &egui::Context, entry: ProjectEntry) {
        match project::load(&entry.json()) {
            Ok(p) => match Work::open(ctx, entry, p, self.engine.as_ref()) {
                Ok(w) => {
                    self.screen = Screen::Work(Box::new(w));
                    self.arriving = Some(Instant::now());
                }
                Err(e) => self.home.error = Some(format!("{e:#}")),
            },
            Err(e) => self.home.error = Some(format!("{e:#}")),
        }
    }

    fn home_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let pal = self.pal;

        // Drop a .osz, a mapset folder or a .osu anywhere on the window to start a project.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(f) = dropped.first() {
            self.start_from_path(&ctx, f);
            if !matches!(self.screen, Screen::Home) {
                return;
            }
        }
        let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());

        egui::Panel::top("home-top").frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin::symmetric(18, 10))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                icons::paint_logo(ui.painter(), logo_rect, &pal, pal.bg);
                ui.label(egui::RichText::new("hitsmith").size(20.0).strong());
                ui.label(egui::RichText::new("hitsounding workstation for osu!").color(pal.text_weak));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme_menu(ui, &mut self.settings) {
                        self.theme_changed = true;
                    }
                    let tip = if self.settings.show_guide { "Hide the getting-started guide" } else { "Show the getting-started guide" };
                    if ui.selectable_label(self.settings.show_guide, "Guide").on_hover_text(tip).clicked() {
                        self.settings.show_guide = !self.settings.show_guide;
                        self.save_settings();
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
            if self.settings.show_guide && guide_panel(ui, &pal) {
                self.settings.show_guide = false;
                self.save_settings();
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
            painter.text(r.center(), Align2::CENTER_CENTER, "Drop a .osz, mapset folder or .osu to start a project", FontId::proportional(22.0), pal.text);
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

/// The "Getting started" guide on the home screen. Returns true when dismissed.
fn guide_panel(ui: &mut egui::Ui, pal: &Palette) -> bool {
    const STEPS: [(Icon, &str, &str); 3] = [
        (Icon::Pencil, "Place hits", "Click the grid to place a hit, or press a sample's key (Q W E R) at the playhead."),
        (Icon::Speaker, "Shape the volume", "Click the Volume lane to add a point, then drag it up or down."),
        (Icon::Export, "Export", "Writes a hitsound difficulty and its samples into the mapset folder, ready for osu!."),
    ];
    let mut dismissed = false;
    egui::Frame::new()
        .fill(pal.panel)
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Getting started").size(18.0).strong());
                    ui.label(egui::RichText::new("Drop a mapset (.osz) or its folder anywhere on this window, or click New project. hitsmith sets up the project and Normal / Whistle / Finish / Clap samples for you. Then:").color(pal.text_weak));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Got it").on_hover_text("Hide this guide (the Guide button brings it back)").clicked() {
                        dismissed = true;
                    }
                });
            });
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                for (n, (icon, title, text)) in STEPS.iter().enumerate() {
                    let (rect, _) = ui.allocate_exact_size(vec2(214.0, 116.0), Sense::hover());
                    let painter = ui.painter();
                    painter.rect_filled(rect, CornerRadius::same(10), pal.bg);
                    painter.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, pal.border), StrokeKind::Inside);
                    // Step number and icon.
                    let badge = pos2(rect.left() + 22.0, rect.top() + 22.0);
                    painter.circle_filled(badge, 12.0, pal.accent);
                    painter.text(badge, Align2::CENTER_CENTER, format!("{}", n + 1), FontId::proportional(13.0), pal.bg);
                    icons::paint(painter, Rect::from_center_size(pos2(rect.right() - 22.0, rect.top() + 22.0), Vec2::splat(18.0)), *icon, pal.text_weak);
                    painter.text(pos2(rect.left() + 42.0, badge.y), Align2::LEFT_CENTER, *title, FontId::proportional(14.5), pal.text);
                    // Description, wrapped inside the card.
                    let galley = painter.layout(text.to_string(), FontId::proportional(12.0), pal.text_weak, rect.width() - 24.0);
                    painter.galley(pos2(rect.left() + 12.0, rect.top() + 44.0), galley, pal.text_weak);
                }
            });
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Tip: Space plays, the mouse wheel seeks, Ctrl+wheel changes the snap. The workstation's More menu lists every shortcut.")
                    .size(12.0)
                    .color(pal.text_weak),
            );
        });
    ui.add_space(14.0);
    dismissed
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
    painter.text(c + vec2(0.0, 68.0), Align2::CENTER_CENTER, "Open or drop a .osz, folder or .osu", FontId::proportional(12.0), pal.text_weak);
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

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
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
        // The native title bar blends along with the rest of the theme.
        self.titlebar.apply(frame, &self.pal, ctx.system_theme());
        // The window/taskbar icon is redrawn in the new palette once a theme is picked.
        let target = *self.theme.target();
        // Wait a couple of frames: an icon sent while the window is still being created is dropped.
        if self.icon_palette != Some((target.name, target.dark)) && ctx.cumulative_frame_nr() >= 2 {
            self.icon_palette = Some((target.name, target.dark));
            let size = 32;
            let rgba = icons::logo_rgba(size, &target);
            ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(Arc::new(egui::IconData { rgba, width: size, height: size }))));
        }
        let mut go_home = false;
        match &mut self.screen {
            Screen::Home => self.home_ui(ui),
            Screen::Work(w) => {
                let mut vols = (self.settings.song_volume, self.settings.hit_volume, self.settings.master_volume);
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
                if vols != (self.settings.song_volume, self.settings.hit_volume, self.settings.master_volume) {
                    self.settings.song_volume = vols.0;
                    self.settings.hit_volume = vols.1;
                    self.settings.master_volume = vols.2;
                    if let Some(e) = &self.engine {
                        e.set_gains(vols.0, vols.1);
                        e.set_master(vols.2);
                    }
                }
            }
        }
        // Closing the window while a project is open: save first, and if that fails, ask.
        if ctx.input(|i| i.viewport().close_requested()) {
            if let Screen::Work(w) = &mut self.screen {
                if !w.force_leave {
                    w.save_now();
                    if w.save_error.is_some() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                        w.leave_prompt = true;
                        w.close_after_leave = true;
                    }
                }
            }
        }
        if go_home && self.leaving.is_none() {
            let mut closing = false;
            if let Screen::Work(w) = &mut self.screen {
                if !w.force_leave {
                    w.save_now();
                    if w.save_error.is_some() {
                        w.leave_prompt = true;
                        return;
                    }
                }
                if w.close_after_leave {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    closing = true;
                }
            }
            if !closing {
                // Fade the music and the screen out, then fade the projects in, instead of a hard cut.
                if let Some(e) = &self.engine {
                    e.play(false);
                }
                self.leaving = Some(Instant::now());
            }
        }
        const FADE_OUT: f32 = 0.16;
        const FADE_IN: f32 = 0.22;
        if let Some(t) = self.leaving {
            let k = t.elapsed().as_secs_f32() / FADE_OUT;
            if k >= 1.0 {
                self.leaving = None;
                if let Some(e) = &self.engine {
                    e.stop_all();
                }
                self.save_settings();
                self.screen = Screen::Home;
                self.refresh_projects();
                self.arriving = Some(Instant::now());
                self.cover(&ctx, 1.0);
            } else {
                self.cover(&ctx, ease(k));
            }
            ctx.request_repaint();
        } else if let Some(t) = self.arriving {
            let k = t.elapsed().as_secs_f32() / FADE_IN;
            if k >= 1.0 {
                self.arriving = None;
            } else {
                self.cover(&ctx, 1.0 - ease(k));
                ctx.request_repaint();
            }
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
    /// Moving the selection; `delta` is the snapped offset from where the drag began,
    /// `rows` how many sample rows down (negative: up) it's being moved.
    Notes { anchor: i64, delta: i64, from: usize, rows: i32 },
    /// Rubber-band selection in screen coordinates.
    Box { start: Pos2, cur: Pos2 },
    /// Dragging a volume point. `axis`: 0 = undecided, 1 = volume (vertical), 2 = time (horizontal).
    Point { idx: usize, orig: ControlPoint, cur: ControlPoint, axis: u8 },
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
    library: Arc<Vec<SampleEntry>>,
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
    /// Track id waiting for "Remove sample?" confirmation.
    confirm_remove: Option<u64>,
    save_error: Option<String>,
    follow: bool,
    skin_dir: Option<PathBuf>,
    pal: Palette,
    /// Every difficulty (.osu) in the mapset, for the timing picker.
    diffs: Vec<String>,
    tool: Tool,
    selected: HashSet<NoteKey>,
    clipboard: Vec<NoteKey>,
    speed: f64,
    wheel_acc: f32,
    /// Volume lane rect from the last frame (the wheel is handled before the lane is laid out).
    vol_lane_rect: Option<Rect>,
    /// Point being changed with the wheel and when, so a burst of notches is one undo step.
    vol_wheel: Option<(usize, Instant)>,
    div_acc: f32,
    /// Visible time span (ms) of the timeline, from the last frame.
    view_span: f64,
    /// Track lane rectangles from the last frame, for box selection across rows.
    row_rects: Vec<(usize, Rect)>,
    show_keys: bool,
    confirm_clear: bool,
    /// Track id whose hit key is being reassigned (waiting for a key press).
    capturing_key: Option<u64>,
    /// A key cap was clicked this frame (so that click doesn't cancel the capture).
    capture_clicked: bool,
    /// Saving failed while leaving: ask whether to retry, leave anyway or stay.
    leave_prompt: bool,
    /// The user chose to leave without saving.
    force_leave: bool,
    /// The leave was triggered by closing the window.
    close_after_leave: bool,
    toast: Option<(String, Instant)>,
    renaming: Option<(u64, String)>,
    pos_now: f64,
    playing_now: bool,
    /// Set while scrolling by hand: Follow waits until shortly after the last scroll.
    follow_hold: Option<Instant>,
    /// The ruler is being held (playhead dragged); playback resumes on release if it was playing.
    ruler_held: bool,
    ruler_resume: bool,
    /// Mirrors the saved setting; the App writes it back when it changes.
    scrub_sound: bool,
}

impl Work {
    fn open(ctx: &egui::Context, entry: ProjectEntry, project: Project, engine: Option<&Engine>) -> anyhow::Result<Self> {
        let bm = osu::read_beatmap(&project.mapset_dir.join(&project.osu_file))?;
        let diffs = list_diffs(&project.mapset_dir);
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
        let view_start = project.last_view.unwrap_or(first - 500.0);
        let mut w = Self {
            skin_dir: library::current_skin_dir(&project.mapset_dir),
            entry,
            meta: bm.meta,
            reds: bm.red_lines,
            project,
            song: None,
            song_rx: Some(rx),
            song_error: None,
            library: Arc::new(Vec::new()),
            loaded: HashMap::new(),
            sample_bufs: Vec::new(),
            sample_errors: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            view_start,
            dirty: false,
            last_edit: Instant::now(),
            drag: None,
            export_open: false,
            export_result: None,
            confirm_remove: None,
            save_error: None,
            follow: true,
            pal: theme::DARK[0],
            diffs,
            tool: Tool::Place,
            selected: HashSet::new(),
            clipboard: Vec::new(),
            speed: 1.0,
            wheel_acc: 0.0,
            vol_lane_rect: None,
            vol_wheel: None,
            div_acc: 0.0,
            view_span: 5000.0,
            row_rects: Vec::new(),
            show_keys: false,
            confirm_clear: false,
            capturing_key: None,
            capture_clicked: false,
            leave_prompt: false,
            force_leave: false,
            close_after_leave: false,
            toast: None,
            renaming: None,
            pos_now: 0.0,
            playing_now: false,
            follow_hold: None,
            ruler_held: false,
            ruler_resume: false,
            scrub_sound: false,
        };
        if let Some(e) = engine {
            e.set_song(Arc::new(Vec::new()));
            e.set_samples(Vec::new());
            e.set_speed(1.0);
            e.seek_ms(w.project.last_position);
        }
        w.pos_now = w.project.last_position;
        w.refresh_library();
        Ok(w)
    }

    /// Add the timing difficulty's hitsounds as tracks (one per sound) and use its volumes.
    fn import_hitsounds(&mut self, engine: Option<&Engine>) {
        let text = match std::fs::read_to_string(self.project.mapset_dir.join(&self.project.osu_file)) {
            Ok(t) => t,
            Err(e) => {
                self.toast(format!("Couldn't read the difficulty: {e}"));
                return;
            }
        };
        let imp = crate::hsimport::import(&text, &self.library, self.project.next_id);
        if imp.hits == 0 {
            self.toast("That difficulty has no hits to import");
            return;
        }
        self.checkpoint();
        let n_tracks = imp.tracks.len();
        self.project.next_id += n_tracks as u64;
        self.project.content.tracks.extend(imp.tracks);
        self.project.content.control_points = imp.control_points;
        self.selected.clear();
        self.changed(engine);
        let missing = if imp.missing.is_empty() { String::new() } else { format!(" ({} need a sound file)", imp.missing.len()) };
        self.toast(format!("Imported {} hits into {n_tracks} samples{missing}", imp.hits));
    }

    /// Use another difficulty of the mapset for timing (and as the export template).
    fn switch_timing_diff(&mut self, osu_file: String, engine: Option<&Engine>) {
        match osu::read_beatmap(&self.project.mapset_dir.join(&osu_file)) {
            Ok(bm) => {
                self.project.osu_file = osu_file;
                self.reds = bm.red_lines;
                self.meta = bm.meta;
                self.dirty = true;
                self.last_edit = Instant::now();
                self.rebuild_events(engine);
                let bpm = self.reds.first().map(|r| r.bpm()).unwrap_or(0.0);
                self.toast(format!("Timing now from this difficulty ({bpm:.0} BPM)"));
            }
            Err(e) => self.toast(format!("Couldn't read that difficulty: {e:#}")),
        }
    }

    fn refresh_library(&mut self) {
        self.sample_errors.clear();
        let exported = export::read_manifest(&self.project.mapset_dir).all_files();
        self.library = Arc::new(project::sample_library(&self.entry.samples_dir(), &self.project.mapset_dir, &self.meta.audio_filename, self.skin_dir.as_deref(), &exported));
    }

    fn sample_path(&self, r: &SampleRef) -> Option<PathBuf> {
        self.library.iter().find(|e| &e.r == r).map(|e| e.path.clone())
    }

    /// Decode a sample (once) into the engine's sample table.
    fn ensure_sample(&mut self, path: &Path, engine: Option<&Engine>) -> Option<u32> {
        if let Some(&id) = self.loaded.get(path) {
            return Some(id);
        }
        if self.sample_errors.contains_key(path) {
            return None;
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
            self.after_history_jump(engine);
        }
    }

    fn redo(&mut self, engine: Option<&Engine>) {
        if let Some(c) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project.content, c));
            self.after_history_jump(engine);
        }
    }

    /// After undo/redo the hits may be different, so anything pointing at them is stale.
    fn after_history_jump(&mut self, engine: Option<&Engine>) {
        self.selected.clear();
        self.drag = None;
        self.renaming = None;
        self.changed(engine);
    }

    fn save_now(&mut self) {
        // Remember where you were, so reopening the project puts you back there.
        self.project.last_position = self.pos_now.max(0.0);
        self.project.last_view = Some(self.view_start);
        match project::save(&self.entry.json(), &self.project) {
            Ok(()) => {
                self.dirty = false;
                self.save_error = None;
            }
            Err(e) => {
                self.save_error = Some(format!("Save failed: {e:#}"));
                // Try again in a couple of seconds, not every frame.
                self.last_edit = Instant::now() + Duration::from_secs(2);
            }
        }
    }

    fn snap(&self, ms: f64) -> i64 {
        osu::snap(&self.reds, ms, self.project.divisor).round() as i64
    }

    fn duration(&self) -> f64 {
        match &self.song {
            Some(s) => s.duration_ms,
            // Song still loading or failed to decode: don't cap editing at an arbitrary length.
            None => {
                let last = self.project.content.tracks.iter().filter_map(|t| t.notes.last()).max().copied().unwrap_or(0);
                (last as f64 + 60_000.0).max(600_000.0)
            }
        }
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
        self.project.content.tracks.push(Track { id, name: format!("Sample {}", n + 1), sample, volume: 80, muted: false, solo: false, color: (n % 8) as u8, notes: Vec::new(), key: None });
        self.changed(engine);
    }

    /// Returns true when the user wants to go back home.
    fn ui(&mut self, ui: &mut egui::Ui, engine: Option<&Engine>, vols: &mut (f32, f32, f32), theme_ui: &mut dyn FnMut(&mut egui::Ui)) -> bool {
        let ctx = ui.ctx().clone();
        if let Some(rx) = &self.song_rx {
            if let Ok(r) = rx.try_recv() {
                match r {
                    Ok(song) => {
                        if let Some(e) = engine {
                            e.set_song(song.pcm.clone());
                            // set_song rewinds to 0: go back to where you left off.
                            e.seek_ms(self.project.last_position.min(song.duration_ms));
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
        let mut pos = engine.map(|e| e.position_ms()).unwrap_or(0.0);
        // Prediction can overshoot a hair before the next audio report; never step backwards
        // by such tiny amounts (real backward seeks are much larger than this).
        if playing && pos < self.pos_now && self.pos_now - pos < 6.0 {
            pos = self.pos_now;
        }
        // Starting playback turns Follow back on (scrolling by hand turns it off).
        if playing && !self.playing_now {
            self.follow = true;
        }
        self.pos_now = pos;
        self.playing_now = playing;
        if playing {
            ctx.request_repaint();
        }
        let mut go_home = false;

        // osu!-editor style keyboard shortcuts (ignored while typing in a text field).
        let typing = ctx.memory(|m| m.focused()).is_some() && ctx.egui_wants_keyboard_input();
        let modal_open = self.confirm_remove.is_some() || self.confirm_clear || self.leave_prompt;
        if modal_open {
            // Only Esc works behind a confirmation dialog: it cancels it.
            if take_key(&ctx, Modifiers::NONE, Key::Escape) {
                self.confirm_remove = None;
                self.confirm_clear = false;
                self.leave_prompt = false;
                self.close_after_leave = false;
            }
        } else if !typing && self.handle_keys(&ctx, engine, playing, pos) {
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
                    if ui.button("Import sound files…").on_hover_text("Copy .wav/.ogg/.mp3 files into this project so samples can use them").clicked() {
                        if let Some(files) = rfd::FileDialog::new().add_filter("Audio", &project::AUDIO_EXTS).pick_files() {
                            let n = files.len();
                            for f in files {
                                if let Err(e) = project::import_sample(&self.entry.samples_dir(), &f) {
                                    self.save_error = Some(format!("{e:#}"));
                                }
                            }
                            self.refresh_library();
                            self.toast(format!("Imported {n} sound file{}", if n == 1 { "" } else { "s" }));
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
                    if ui.button("Import hitsounds from timing difficulty").on_hover_text("Add the hits of the difficulty chosen under Song as samples, with its volume points").clicked() {
                        self.import_hitsounds(engine);
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
                ui.add_space(6.0);
                icons::group(ui, &pal, |ui| {
                    let n = self.project.content.tracks.len();
                    if icons::chip(ui, &pal, "Compact", self.project.compact_tracks, &format!("Slim one-line rows to see all {n} samples at once")).clicked() {
                        self.project.compact_tracks = !self.project.compact_tracks;
                        self.dirty = true;
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
                        let r = ui.add(egui::Slider::new(&mut vols.1, 0.0..=1.5).show_value(false)).on_hover_text("Hitsound volume (wheel: 5%)");
                        wheel_nudge(ui, &r, &mut vols.1, 0.05, 0.0, 1.5);
                        ui.label(egui::RichText::new("Hits").size(12.0).color(pal.text_weak));
                    });
                    icons::group(ui, &pal, |ui| {
                        let r = ui.add(egui::Slider::new(&mut vols.0, 0.0..=1.0).show_value(false)).on_hover_text("Song volume (wheel: 5%)");
                        wheel_nudge(ui, &r, &mut vols.0, 0.05, 0.0, 1.0);
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
                let r = ui.add(egui::Slider::new(&mut v, -2000.0..=total).show_value(false)).on_hover_text("Scroll the timeline");
                if r.changed() {
                    self.view_start = v;
                }
                if r.changed() || r.dragged() {
                    // Hold Follow while scrolling by hand; it catches up shortly after.
                    self.follow_hold = Some(Instant::now());
                }
                let sel = self.selected.len();
                let n_tracks = self.project.content.tracks.len();
                let hits: usize = self.project.content.tracks.iter().map(|t| t.notes.len()).sum();
                let info = if sel > 0 { format!("{sel} selected") } else { format!("{n_tracks} samples · {hits} hits") };
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
            let over_popup = |h: Pos2| ctx.layer_id_at(h).is_some_and(|l| l.order != egui::Order::Background);
            let on_timeline = hover.is_some_and(|h| full.contains(h) && h.x > timeline_left && !over_popup(h));
            if !on_timeline && hover.is_some_and(|h| !over_popup(h)) {
                // Alt+wheel anywhere else: overall volume.
                let n: f32 = wheel_notches(ui).iter().filter(|(_, m)| m.alt).map(|(n, _)| n).sum();
                if n != 0.0 {
                    vols.2 = (((vols.2 + n * 0.05) * 20.0).round() / 20.0).clamp(0.0, 1.5);
                    self.toast(format!("Volume {:.0}%", vols.2 * 100.0));
                    ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
                }
            }
            // A burst of wheel notches on a volume point ends after a short pause.
            if self.vol_wheel.is_some_and(|(_, t)| t.elapsed() > Duration::from_millis(700)) {
                self.vol_wheel = None;
            }
            if let Some(h) = hover.filter(|_| on_timeline) {
                let notches = wheel_notches(ui);
                let tr = Rect::from_min_size(pos2(timeline_left, full.top()), vec2(grid_w, full.height()));
                // Over a volume point, the plain wheel changes its volume instead of seeking.
                let vol_point = self.vol_lane_rect.filter(|r| r.contains(h) && self.drag.is_none()).and_then(|r| {
                    self.project
                        .content
                        .control_points
                        .iter()
                        .enumerate()
                        .map(|(i, c)| (i, (self.x_of(r, c.time as f64) - h.x).abs()))
                        .filter(|&(_, d)| d < 9.0)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(i, _)| i)
                });
                for (n, m) in notches {
                    if let (Some(i), false, false) = (vol_point, m.command, m.alt) {
                        self.wheel_acc += n;
                        while self.wheel_acc.abs() >= 1.0 {
                            let up = self.wheel_acc > 0.0;
                            self.wheel_acc -= 1.0f32.copysign(self.wheel_acc);
                            let step: i32 = if m.shift { 1 } else { 5 };
                            let old = self.project.content.control_points[i].volume;
                            let new = (old as i32 + if up { step } else { -step }).clamp(5, 100) as u8;
                            if new != old {
                                if self.vol_wheel.map(|(j, _)| j) != Some(i) {
                                    self.checkpoint();
                                }
                                self.vol_wheel = Some((i, Instant::now()));
                                self.project.content.control_points[i].volume = new;
                                self.changed(engine);
                            }
                        }
                    } else if m.command {
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
            // Follow pauses for a moment after scrolling by hand, then catches up with the playhead.
            let holding = self.follow_hold.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
            if !holding {
                self.follow_hold = None;
            }
            if playing && self.follow && !holding {
                // Scroll continuously with the playhead about a quarter of the way in, eased by
                // frame time so it's equally smooth at 60, 144 or 240 Hz. Far jumps snap.
                let span = grid_w as f64 / self.project.zoom as f64;
                let target = pos - span * 0.25;
                if pos < self.view_start || pos > self.view_start + span {
                    self.view_start = target;
                } else {
                    let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1) as f64;
                    self.view_start += (target - self.view_start) * (1.0 - (-dt * 8.0).exp());
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
                ui.label(egui::RichText::new("Difficulty").size(13.0).strong());
                // Which difficulty provides the timing (and the template for export).
                let current = self.project.osu_file.clone();
                let shown = current.rsplit_once('[').map(|(_, v)| v.trim_end_matches(".osu").trim_end_matches(']').to_string()).unwrap_or_else(|| current.clone());
                let mut switch_to = None;
                egui::ComboBox::from_id_salt("timing-diff")
                    .width(header.width() - 8.0)
                    .truncate()
                    .selected_text(egui::RichText::new(shown).size(12.0))
                    .show_ui(ui, |ui| {
                        for d in self.diffs.iter() {
                            let label = d.rsplit_once('[').map(|(_, v)| v.trim_end_matches(".osu").trim_end_matches(']').to_string()).unwrap_or_else(|| d.clone());
                            if ui.selectable_label(*d == current, label).on_hover_text(d.as_str()).clicked() && *d != current {
                                switch_to = Some(d.clone());
                            }
                        }
                    });
                if let Some(d) = switch_to {
                    self.switch_timing_diff(d, engine);
                }
                ui.label(egui::RichText::new(format!("{} bookmarks", self.project.bookmarks.len())).size(11.5).color(pal.text_weak));
            }
            self.wave_lane(ui, rect, &resp, pos, engine);
            ui.add_space(4.0);

            // Volume (control point) lane.
            let (header, rect, resp) = lane_row(ui, "volume", LANE_H, timeline_left, lane_right);
            self.vol_lane_rect = Some(rect);
            {
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(header));
                ui.shrink_clip_rect(header);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Volume").size(13.0).strong());
                    ui.label(egui::RichText::new(format!("{}%", self.project.volume_at(pos as i64))).size(13.0).color(pal.accent));
                });
                ui.label(egui::RichText::new("Click to add a point, drag or wheel to change it").size(11.5).color(pal.text_weak));
            }
            self.volume_lane(ui, rect, &resp, pos, engine);
            ui.add_space(6.0);
            ui.separator();

            if self.project.content.tracks.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("No samples yet").size(18.0).strong());
                    ui.label(egui::RichText::new("Add a sample, pick its sound file, then click its row to place hits.").color(pal.text_weak));
                    ui.add_space(10.0);
                    if icons::primary(ui, &pal, Icon::Plus, "Add sample").clicked() {
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
                    let row_h = if self.project.compact_tracks { ROW_H_COMPACT } else { ROW_H };
                    let (header, rect, resp) = lane_row(ui, ("track", id), row_h, timeline_left, lane_right);
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
                    if ui.add(egui::Button::new(egui::RichText::new("+  Add sample").color(pal.text_weak)).frame(false)).clicked() {
                        self.add_track(engine);
                    }
                }
            });
        });

        if let Some(id) = self.confirm_remove {
            let ti = self.track_index(id);
            let name = ti.and_then(|i| self.project.content.tracks.get(i)).map(|t| t.name.clone()).unwrap_or_default();
            let mut close = false;
            egui::Modal::new(egui::Id::unique("confirm-remove")).show(&ctx, |ui| {
                ui.heading("Remove sample?");
                ui.label(format!("\"{name}\" and all its hits will be removed. You can undo this."));
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        if let Some(ti) = self.track_index(id) {
                            self.checkpoint();
                            self.project.content.tracks.remove(ti);
                            self.selected.retain(|k| k.0 != id);
                            self.changed(engine);
                        }
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

        if !self.selected.is_empty() {
            let pal = self.pal;
            let n = self.selected.len();
            egui::Area::new(egui::Id::unique("selection-bar"))
                .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -52.0))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(pal.panel)
                        .stroke(Stroke::new(1.0, pal.border))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(egui::Margin::symmetric(12, 6))
                        .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(90) })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("{n} selected")).strong());
                                ui.add_space(8.0);
                                let del = egui::Button::new(egui::RichText::new("Delete").color(pal.bg)).fill(pal.danger);
                                if ui.add(del).on_hover_text("Delete the selected hits (Delete key)").clicked() {
                                    self.delete_selection(engine);
                                    self.toast(format!("Deleted {n} hits"));
                                }
                                if ui.button("Copy").on_hover_text("Ctrl+C; paste at the playhead with Ctrl+V").clicked() {
                                    self.copy_selection();
                                    self.toast(format!("Copied {n} hits"));
                                }
                                if ui.button("Deselect").on_hover_text("Esc").clicked() {
                                    self.selected.clear();
                                }
                            });
                        });
                });
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

        if self.leave_prompt {
            let err = self.save_error.clone().unwrap_or_default();
            egui::Modal::new(egui::Id::unique("leave-prompt")).show(&ctx, |ui| {
                ui.heading("Couldn't save the project");
                ui.label(err);
                ui.label("Your latest changes aren't on disk yet.");
                ui.horizontal(|ui| {
                    if ui.button("Try again").clicked() {
                        self.save_now();
                        if self.save_error.is_none() {
                            self.leave_prompt = false;
                            go_home = true;
                        }
                    }
                    if ui.button(egui::RichText::new("Leave without saving").color(self.pal.danger)).clicked() {
                        self.leave_prompt = false;
                        self.force_leave = true;
                        go_home = true;
                    }
                    if ui.button("Stay").clicked() {
                        self.leave_prompt = false;
                        self.close_after_leave = false;
                    }
                });
            });
        }

        if self.confirm_clear {
            let mut close = false;
            egui::Modal::new(egui::Id::unique("confirm-clear")).show(&ctx, |ui| {
                ui.heading("Remove all hits?");
                ui.label("Every hit will be removed. Samples and volume points stay. You can undo this.");
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
                ui.label(egui::RichText::new("Mute and solo only affect playback here; every sample is exported.").size(12.0).color(self.pal.text_weak));
                const MODES: [&str; 4] = ["osu!standard", "osu!taiko", "osu!catch", "osu!mania"];
                let source_mode = (self.meta.mode as usize).min(3);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Game mode");
                    let current = match self.project.export.mode {
                        None => format!("Same as timing difficulty ({})", MODES[source_mode]),
                        Some(m) => MODES[(m as usize).min(3)].to_string(),
                    };
                    egui::ComboBox::from_id_salt("export-mode").width(260.0).selected_text(current).show_ui(ui, |ui| {
                        if ui.selectable_label(self.project.export.mode.is_none(), format!("Same as timing difficulty ({})", MODES[source_mode])).clicked() {
                            self.project.export.mode = None;
                            self.dirty = true;
                        }
                        for (i, name) in MODES.iter().enumerate() {
                            if ui.selectable_label(self.project.export.mode == Some(i as u8), *name).clicked() {
                                self.project.export.mode = Some(i as u8);
                                self.dirty = true;
                            }
                        }
                    });
                });
                let note = match self.project.export.mode.map(|m| m as usize).unwrap_or(source_mode) {
                    0 => "A normal hitsound difficulty: circles that play your sounds. Works for copying hitsounds to any mode.",
                    1 => "In taiko, whistle/clap/finish change note types, so the export is all dons that play your sounds.",
                    2 => "Catch uses the same hitsounds as standard: fruits that play your sounds.",
                    _ => "osu!mania, keysounded: every sound is its own note naming its file; the key count is the most sounds that play at once (up to 10).",
                };
                ui.label(egui::RichText::new(note).size(12.0).color(self.pal.text_weak));
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
                        Ok(r) if r.keys.is_some() => Ok(format!(
                            "Wrote \"{}\": {} keysounded notes on {} keys ({} sound files copied in).",
                            r.osu_file,
                            r.circles,
                            r.keys.unwrap_or(0),
                            r.sample_files
                        )),
                        Ok(r) => {
                            let mut msg = format!(
                                "Wrote \"{}\": {} circles, {} sample files (normal-hitnormal{}..{}).",
                                r.osu_file, r.circles, r.sample_files, r.first_index, r.last_index
                            );
                            if r.clipped > 0 {
                                msg.push_str(&format!(" {} mixed sounds clip; lower some sample volumes.", r.clipped));
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
        // Key capture ends if its track is gone, a rename starts, or you click anywhere else.
        if let Some(id) = self.capturing_key {
            let clicked_elsewhere = ctx.input(|i| i.pointer.any_click()) && !self.capture_clicked;
            if self.track_index(id).is_none() || self.renaming.is_some() || clicked_elsewhere {
                self.capturing_key = None;
            }
        }
        self.capture_clicked = false;

        // A drag whose widget vanished (e.g. undo removed the track) never gets its
        // "stopped" event; drop it once the mouse is released.
        // egui drops its own drag when the window loses focus or is minimized, even if we
        // never saw the mouse release; follow it so a stale drag can't freeze the view.
        if self.drag.is_some() && (ctx.dragged_id().is_none() || !ctx.input(|i| i.pointer.any_down())) {
            self.drag = None;
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
        let eff_key = project::effective_keys(&self.project.content.tracks).swap_remove(ti);
        let capturing = self.capturing_key;
        let mut start_capture = false;
        let mut settings_changed = false;
        let before = {
            let t = &self.project.content.tracks[ti];
            (t.sample.clone(), t.volume, t.muted, t.solo)
        };
        let mut volume_before = before.1;
        let mut live_volume = false;
        let mut retry_sample: Option<PathBuf> = None;
        let t = &mut self.project.content.tracks[ti];
        let color = pal.tracks[t.color as usize % 8];

        // Card background and colour bar that pulses when this track plays a hit.
        let compact = self.project.compact_tracks;
        let card = full.shrink2(vec2(0.0, if compact { 1.0 } else { 2.0 }));
        ui.painter().rect_filled(card, CornerRadius::same(if compact { 6 } else { 8 }), pal.panel);
        let last_played = t.notes.partition_point(|&n| (n as f64) <= pos);
        let lit = playing && last_played > 0 && pos - (t.notes[last_played - 1] as f64) < 120.0;
        let bar = Rect::from_min_size(card.min, vec2(if lit { 7.0 } else { 4.0 }, card.height()));
        ui.painter().rect_filled(bar, CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, if lit { color.lerp_to_gamma(Color32::WHITE, 0.35) } else { color });

        let inner = card.shrink2(vec2(12.0, if compact { 1.0 } else { 5.0 })).translate(vec2(4.0, 0.0));
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
                    let tip = match (&t.sample, compact) {
                        (Some(s), true) => format!("Sample: {} · double-click to rename (turn off Compact to change the sound file)", s.file),
                        _ => "Double-click to rename".to_string(),
                    };
                    let r = ui.scope(|ui| {
                        ui.set_max_width(if compact { 118.0 } else { 130.0 });
                        ui.add(name)
                    }).inner.on_hover_text(tip);
                    if r.double_clicked() {
                        commit_rename = Some(Some(format!("\u{0}{}", t.name)));
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if compact && key_badge(ui, &pal, eff_key.as_deref(), t.key.is_none(), capturing == Some(t.id)).clicked() {
                    start_capture = true;
                }
                if icons::button(ui, &pal, Icon::Trash, "Remove sample", false, true).clicked() {
                    remove = true;
                }
                if icons::button(ui, &pal, Icon::Play, "Preview the sound", false, t.sample.is_some()).clicked() {
                    audition = t.sample.clone().map(|s| (s, t.volume));
                }
                if icons::button(ui, &pal, Icon::Headphones, "Solo", t.solo, true).clicked() {
                    t.solo = !t.solo;
                    edited = true;
                    settings_changed = true;
                }
                if icons::button(ui, &pal, if t.muted { Icon::SpeakerOff } else { Icon::Speaker }, "Mute", t.muted, true).clicked() {
                    t.muted = !t.muted;
                    edited = true;
                    settings_changed = true;
                }
            });
        });
        if !compact {
            // Second row in fixed columns, laid out from the right edge: key cap (under the
            // trash icon), volume %, volume slider, then the sample picker fills the rest.
            // That keeps every track's controls lined up whatever the sample name is.
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if key_badge(ui, &pal, eff_key.as_deref(), t.key.is_none(), capturing == Some(t.id)).clicked() {
                    start_capture = true;
                }
                ui.add_sized(vec2(32.0, 20.0), egui::Label::new(egui::RichText::new(format!("{}%", t.volume)).size(12.0).color(pal.text_weak)));
                ui.spacing_mut().slider_width = 52.0;
                // One undo step per gesture: remember the volume when the press starts,
                // compare when it's released.
                let vol_id = egui::Id::unique(("vol-gesture-start", t.id));
                let vol_at_start = t.volume;
                let r = ui.add(egui::Slider::new(&mut t.volume, 0..=100).show_value(false)).on_hover_text("Sample volume (wheel: 5%)");
                // The wheel nudges 5%; a burst of notches counts as one gesture (held until it pauses).
                let wheel_id = egui::Id::unique(("vol-wheel-at", t.id));
                let mut wheeled = false;
                if r.hovered() {
                    let n: f32 = wheel_notches(ui).iter().filter(|(_, m)| !m.alt && !m.command).map(|(n, _)| n).sum();
                    ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
                    if n != 0.0 {
                        let v = ((t.volume as f32 + n * 5.0) / 5.0).round() * 5.0;
                        t.volume = v.clamp(0.0, 100.0) as u8;
                        live_volume = true;
                        wheeled = true;
                        // Read the time first: calling ui.input inside ui.data_mut locks egui twice and freezes.
                        let now = ui.input(|i| i.time);
                        ui.data_mut(|d| d.insert_temp(wheel_id, now));
                        ui.ctx().request_repaint_after(Duration::from_millis(700));
                    }
                }
                let wheel_burst = ui.data(|d| d.get_temp::<f64>(wheel_id)).is_some_and(|at| ui.input(|i| i.time) - at < 0.6);
                if !wheel_burst {
                    ui.data_mut(|d| d.remove::<f64>(wheel_id));
                }
                let held = r.is_pointer_button_down_on() || r.dragged() || wheel_burst || wheeled;
                let started: Option<u8> = ui.data(|d| d.get_temp(vol_id));
                if held && started.is_none() {
                    ui.data_mut(|d| d.insert_temp(vol_id, vol_at_start));
                }
                if r.changed() {
                    live_volume = true; // hear it while dragging
                }
                if !held {
                    if let Some(start) = started {
                        ui.data_mut(|d| d.remove::<u8>(vol_id));
                        if start != t.volume {
                            volume_before = start;
                            edited = true;
                            settings_changed = true;
                        }
                    } else if r.changed() {
                        // Keyboard change on the focused slider.
                        edited = true;
                        settings_changed = true;
                    }
                }
                let label = t.sample.as_ref().map(|s| s.file.clone()).unwrap_or_else(|| "Choose a sound file".into());
                let width = ui.available_width();
                egui::ComboBox::from_id_salt(("sample", t.id))
                    .width(width)
                    .truncate()
                    .selected_text(egui::RichText::new(label).size(12.5))
                    .show_ui(ui, |ui| {
                        let mut last = None;
                        for e in library.iter() {
                            if last != Some(e.r.source) {
                                ui.label(egui::RichText::new(e.r.source.label()).size(11.0).color(pal.text_weak));
                                last = Some(e.r.source);
                            }
                            let sel = t.sample.as_ref() == Some(&e.r);
                            if ui.selectable_label(sel, &e.r.file).clicked() {
                                retry_sample = Some(e.path.clone());
                                if !sel {
                                    t.sample = Some(e.r.clone());
                                    settings_changed = true;
                                }
                                edited = true;
                                audition = Some((e.r.clone(), t.volume));
                            }
                        }
                    });
            });
        }
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
        if let Some(path) = retry_sample {
            self.sample_errors.remove(&path);
        }
        if settings_changed {
            // Undo snapshot = the project as it was before this change.
            let mut snap = self.project.content.clone();
            let tr = &mut snap.tracks[ti];
            tr.sample = before.0;
            tr.volume = volume_before;
            tr.muted = before.2;
            tr.solo = before.3;
            self.undo.push(snap);
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        if edited {
            self.dirty = true;
            self.last_edit = Instant::now();
            self.rebuild_events(engine);
        } else if live_volume {
            self.rebuild_events(engine);
        }
        if remove {
            self.confirm_remove = Some(self.project.content.tracks[ti].id);
        }
        if start_capture {
            self.capture_clicked = true;
            let id = self.project.content.tracks[ti].id;
            self.capturing_key = if self.capturing_key == Some(id) { None } else { Some(id) };
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
                let bar = osu::measure_number(&self.reds, t);
                if x - last_label > 34.0 && bar >= 1 {
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
        // Holding the ruler while playing: fade the song out, move silently with the mouse,
        // and resume from where it's released (re-seeking every frame sounded like a skipping CD).
        // Left button only: a right-click opens the bar menu without moving the playhead.
        let down = (resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down())) || resp.dragged_by(egui::PointerButton::Primary);
        if down {
            if !self.ruler_held {
                self.ruler_held = true;
                self.ruler_resume = self.playing_now;
                if let (true, Some(e)) = (self.ruler_resume, engine) {
                    e.play(false);
                }
            }
            if let (Some(p), Some(e)) = (resp.interact_pointer_pos(), engine) {
                e.seek_ms(self.t_of(rect, p.x).max(0.0));
            }
        } else if self.ruler_held {
            self.ruler_held = false;
            if let (Some(p), Some(e)) = (resp.interact_pointer_pos().or(ui.input(|i| i.pointer.latest_pos())), engine) {
                e.seek_ms(self.t_of(rect, p.x).max(0.0));
                if self.ruler_resume {
                    e.play(true);
                }
            }
            self.ruler_resume = false;
        }
        // Bars: double-click selects every hit in that bar; right-click has bar actions.
        if resp.double_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let add = ui.input(|i| i.modifiers.command);
                let (a, b) = self.bar_span(self.t_of(rect, p.x));
                let n = self.select_range(a, b, add);
                self.toast(format!("Bar {}: {n} hits selected", osu::measure_number(&self.reds, a as f64)));
            }
        }
        let menu_id = egui::Id::unique("ruler-bar-menu");
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let t = self.t_of(rect, p.x);
                ui.data_mut(|d| d.insert_temp(menu_id, t));
            }
        }
        resp.context_menu(|ui| {
            let t: f64 = ui.data(|d| d.get_temp(menu_id)).unwrap_or(pos);
            let (a, b) = self.bar_span(t);
            let bar = osu::measure_number(&self.reds, a as f64);
            if ui.button(format!("Select bar {bar}")).clicked() {
                let n = self.select_range(a, b, false);
                self.toast(format!("Bar {bar}: {n} hits selected"));
                ui.close();
            }
            if ui.button(format!("Repeat bar {bar} into bar {}", bar + 1)).on_hover_text("Copies its hits one bar later and selects the copies; Ctrl+D keeps going").clicked() {
                let n = self.select_range(a, b, false);
                if n > 0 {
                    self.shift_selection(b - a, 0, true, engine);
                    self.toast(format!("Repeated bar {bar} ({n} hits). Ctrl+D repeats again"));
                }
                ui.close();
            }
            if ui.button(format!("Clear bar {bar}")).clicked() {
                let n = self.select_range(a, b, false);
                if n > 0 {
                    self.delete_selection(engine);
                    self.toast(format!("Cleared bar {bar} ({n} hits)"));
                }
                ui.close();
            }
        });
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            resp.clone().on_hover_text_at_pointer("Click: seek · double-click: select the bar's hits · right-click: bar actions");
        }
    }

    /// Start and end (exclusive) of the bar containing `t`, cut short by a following red line.
    fn bar_span(&self, t: f64) -> (i64, i64) {
        let rl = osu::red_line_at(&self.reds, t);
        let len = rl.beat_length * rl.meter.max(1) as f64;
        let start = rl.offset + ((t - rl.offset) / len + 1e-6).floor() * len;
        let next_red = self.reds.iter().map(|r| r.offset).find(|&o| o > start + 0.5).unwrap_or(f64::INFINITY);
        (start.round() as i64, (start + len).min(next_red).round() as i64)
    }

    /// Select every hit in [a, b); returns how many. `add` keeps the current selection.
    fn select_range(&mut self, a: i64, b: i64, add: bool) -> usize {
        if !add {
            self.selected.clear();
        }
        let mut n = 0;
        for t in &self.project.content.tracks {
            for &x in t.notes.iter().filter(|&&x| x >= a && x < b) {
                self.selected.insert((t.id, x));
                n += 1;
            }
        }
        n
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
        if let (Some(Drag::Pan { start_view }), true) = (&self.drag, resp.dragged()) {
            let total = resp.total_drag_delta().map(|d| d.x).unwrap_or(0.0);
            self.view_start = start_view - (total / self.project.zoom) as f64;
            self.follow_hold = Some(Instant::now());
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

    /// Vertical padding so points at 5% and 100% stay fully inside the lane.
    const VOL_PAD: f32 = 10.0;

    fn vol_y(rect: Rect, v: u8) -> f32 {
        rect.bottom() - Self::VOL_PAD - (rect.height() - 2.0 * Self::VOL_PAD) * v as f32 / 100.0
    }

    /// Volume at a y position: 5% steps, or 1% steps when `fine`.
    fn y_vol(rect: Rect, y: f32, fine: bool) -> u8 {
        let raw = (rect.bottom() - Self::VOL_PAD - y) / (rect.height() - 2.0 * Self::VOL_PAD) * 100.0;
        let v = if fine { raw.round() } else { (raw / 5.0).round() * 5.0 };
        v.clamp(5.0, 100.0) as u8
    }

    fn volume_lane(&mut self, ui: &mut egui::Ui, rect: Rect, resp: &egui::Response, pos: f64, engine: Option<&Engine>) {
        let pal = self.pal;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(4), pal.lane);
        self.draw_grid(&painter, rect, true);

        // Working copy with the drag preview applied (moves are clamped between neighbours, so order holds).
        let mut cps = self.project.content.control_points.clone();
        let mut dragging = None;
        if let Some(Drag::Point { idx, cur, .. }) = &self.drag {
            if *idx < cps.len() {
                cps[*idx] = *cur;
                dragging = Some(*idx);
            }
        }
        let nearest = |w: &Self, p: Pos2| -> Option<usize> {
            w.project
                .content
                .control_points
                .iter()
                .enumerate()
                .map(|(i, c)| (i, (w.x_of(rect, c.time as f64) - p.x).abs()))
                .filter(|&(_, d)| d < 9.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        let hover = resp.hover_pos();
        let hovered = if self.drag.is_none() { hover.and_then(|p| nearest(self, p)) } else { None };

        // Step graph.
        for (i, c) in cps.iter().enumerate() {
            let x0 = if i == 0 { rect.left() } else { self.x_of(rect, c.time as f64).max(rect.left()) };
            let x1 = cps.get(i + 1).map(|n| self.x_of(rect, n.time as f64)).unwrap_or(rect.right()).min(rect.right());
            if x1 <= rect.left() || x0 >= rect.right() {
                continue;
            }
            let y = Self::vol_y(rect, c.volume);
            painter.rect_filled(Rect::from_min_max(pos2(x0, y), pos2(x1, rect.bottom())), 0.0, pal.vol_fill);
            painter.hline(x0..=x1, y, Stroke::new(1.5, pal.vol_line));
            if let Some(n) = cps.get(i + 1) {
                let xn = self.x_of(rect, n.time as f64);
                if rect.x_range().contains(xn) {
                    painter.vline(xn, Self::vol_y(rect, c.volume).min(Self::vol_y(rect, n.volume))..=Self::vol_y(rect, c.volume).max(Self::vol_y(rect, n.volume)), Stroke::new(1.5, pal.vol_line));
                }
            }
        }

        // Points, with labels that stay inside the lane and don't overlap.
        let xs: Vec<f32> = cps.iter().map(|c| self.x_of(rect, c.time as f64)).collect();
        for (i, c) in cps.iter().enumerate() {
            let x = xs[i];
            if !(rect.left() - 10.0..=rect.right() + 10.0).contains(&x) {
                continue;
            }
            let y = Self::vol_y(rect, c.volume);
            let active = Some(i) == dragging || Some(i) == hovered;
            painter.circle_filled(pos2(x, y), if active { 6.5 } else { 4.5 }, pal.vol_line);
            painter.circle_stroke(pos2(x, y), if active { 6.5 } else { 4.5 }, Stroke::new(1.5, pal.lane));
            let room = xs.get(i + 1).map(|n| n - x).unwrap_or(f32::MAX);
            if active || room > 46.0 {
                let text = format!("{}%", c.volume);
                let (anchor, ty) = if y - 16.0 < rect.top() { (Align2::LEFT_TOP, y + 3.0) } else { (Align2::LEFT_BOTTOM, y - 3.0) };
                painter.text(pos2(x + 8.0, ty), anchor, text, FontId::proportional(if active { 12.5 } else { 11.0 }), if active { pal.text } else { pal.text_weak });
            }
        }

        // Ghost preview of what a click would add.
        if self.drag.is_none() && hovered.is_none() {
            if let Some(p) = hover {
                let t = self.snap(self.t_of(rect, p.x)).max(0);
                let gx = self.x_of(rect, t as f64);
                let v = Self::y_vol(rect, p.y, ui.input(|i| i.modifiers.shift));
                let gy = Self::vol_y(rect, v);
                painter.circle_stroke(pos2(gx, gy), 4.5, Stroke::new(1.5, pal.vol_line.gamma_multiply(0.7)));
                painter.text(pos2(gx + 8.0, gy - 3.0), Align2::LEFT_BOTTOM, format!("+ {v}%"), FontId::proportional(11.0), pal.text_weak);
            }
        }
        self.draw_playhead(&painter, rect, pos);

        // Interaction.
        let fine = ui.input(|i| i.modifiers.shift);
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(i) = nearest(self, p) {
                    let c = self.project.content.control_points[i];
                    // The first point anchors the start of the song, so it only changes volume.
                    self.drag = Some(Drag::Point { idx: i, orig: c, cur: c, axis: if i == 0 { 1 } else { 0 } });
                }
            }
        }
        if resp.dragged() {
            let origin = ui.input(|i| i.pointer.press_origin());
            if let (Some(p), Some(o)) = (resp.interact_pointer_pos(), origin) {
                let snapped_t = self.snap(self.t_of(rect, p.x)).max(0);
                let cps = &self.project.content.control_points;
                if let Some(Drag::Point { idx, orig, cur, axis }) = &mut self.drag {
                    let d = p - o;
                    if *axis == 0 && d.length() > 4.0 {
                        *axis = if d.x.abs() > d.y.abs() { 2 } else { 1 };
                    }
                    match *axis {
                        1 => {
                            cur.time = orig.time;
                            cur.volume = Self::y_vol(rect, p.y, fine);
                        }
                        2 => {
                            // Stop at the neighbours instead of jumping over or onto them.
                            let lo = cps.get(*idx - 1).map(|c| c.time + 1).unwrap_or(0);
                            let hi = cps.get(*idx + 1).map(|c| c.time - 1).unwrap_or(i64::MAX);
                            cur.time = snapped_t.clamp(lo, hi.max(lo));
                            cur.volume = orig.volume;
                        }
                        _ => {}
                    }
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(Drag::Point { idx, orig, cur, .. }) = self.drag.take() {
                if orig != cur && idx < self.project.content.control_points.len() {
                    self.checkpoint();
                    self.project.content.control_points[idx] = cur;
                    self.project.content.control_points.sort_by_key(|c| c.time);
                    self.changed(engine);
                }
            }
        }
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if nearest(self, p).is_none() {
                    let t = self.snap(self.t_of(rect, p.x)).max(0);
                    if !self.project.content.control_points.iter().any(|c| c.time == t) {
                        self.checkpoint();
                        self.project.content.control_points.push(ControlPoint { time: t, volume: Self::y_vol(rect, p.y, fine) });
                        self.project.content.control_points.sort_by_key(|c| c.time);
                        self.changed(engine);
                    }
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(i) = nearest(self, p) {
                    if self.project.content.control_points.len() > 1 {
                        self.checkpoint();
                        self.project.content.control_points.remove(i);
                        self.changed(engine);
                        self.toast("Volume point deleted");
                    }
                }
            }
        }
        if resp.hovered() || resp.dragged() {
            let cursor = match &self.drag {
                Some(Drag::Point { axis: 2, .. }) => egui::CursorIcon::ResizeHorizontal,
                Some(Drag::Point { .. }) => egui::CursorIcon::ResizeVertical,
                _ if hovered.is_some() => egui::CursorIcon::Grab,
                _ => egui::CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(cursor);
        }
        if hovered.is_some() {
            resp.clone().on_hover_text_at_pointer("Drag up/down: volume (Shift: 1% steps) · drag sideways: move · right-click: delete");
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
        let (delta, rows) = match &self.drag {
            Some(Drag::Notes { delta, rows, .. }) => (*delta, *rows),
            _ => (0, 0),
        };
        // Ctrl while dragging hits copies them instead of moving: originals stay, copies follow the mouse.
        // Dragging to another row moves (or copies) the hits to that sample.
        let copying = (delta != 0 || rows != 0) && ui.input(|i| i.modifiers.command);
        let ghosts = copying || rows != 0;
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
            let shown = if sel && !ghosts { n + delta } else { n };
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
            let fill = if hovered { fillc.lerp_to_gamma(Color32::WHITE, 0.25) } else if sel && ghosts && !copying { fillc.gamma_multiply(0.3) } else { fillc };
            painter.rect_filled(r, CornerRadius::same(5), fill);
            if sel && !ghosts {
                painter.rect_stroke(r.expand(2.0), CornerRadius::same(7), Stroke::new(2.0, self.pal.text), StrokeKind::Outside);
            } else {
                painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, Color32::from_black_alpha(90)), StrokeKind::Inside);
            }
        }
        // Where dragged hits will land in this row: selected hits of the row `rows` above, shifted by `delta`.
        if ghosts {
            let src = ti as i64 - rows as i64;
            if let Some(st) = usize::try_from(src).ok().and_then(|i| self.project.content.tracks.get(i)) {
                for &n in &st.notes {
                    if !self.selected.contains(&(st.id, n)) || n + delta < t0 || n + delta > t1 {
                        continue;
                    }
                    let c = Rect::from_center_size(pos2(self.x_of(rect, (n + delta) as f64), rect.center().y), vec2(12.0, h));
                    painter.rect_filled(c, CornerRadius::same(5), fillc.gamma_multiply(0.8));
                    painter.rect_stroke(c.expand(2.0), CornerRadius::same(7), Stroke::new(2.0, self.pal.text), StrokeKind::Outside);
                }
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
                    self.drag = Some(Drag::Notes { anchor: n, delta: 0, from: ti, rows: 0 });
                } else {
                    // Dragging across empty grid draws a selection box in either tool.
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
            // Row under the pointer, kept only if every selected hit has a sample row to land on.
            let over_row = self.row_rects.iter().find(|(_, r)| r.y_range().contains(p.y)).map(|(i, _)| *i);
            let n_tracks = self.project.content.tracks.len() as i64;
            let sel_rows: Vec<i64> = self.selected.iter().filter_map(|&(id, _)| self.track_index(id)).map(|i| i as i64).collect();
            match &mut self.drag {
                Some(Drag::Notes { anchor, delta, from, rows }) if resp.dragged() => {
                    *delta = target - *anchor;
                    if let Some(r) = over_row {
                        let d = r as i64 - *from as i64;
                        if sel_rows.iter().all(|&i| (0..n_tracks).contains(&(i + d))) {
                            *rows = d as i32;
                        }
                    }
                }
                Some(Drag::Box { cur, .. }) if resp.dragged() => *cur = p,
                _ => {}
            }
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Notes { delta, rows, .. }) if delta != 0 || rows != 0 => self.shift_selection(delta, rows, ctrl, engine),
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
        if resp.double_clicked() {
            // Double-click a hit: select every hit of that sample (Ctrl adds to the selection).
            if let Some(p) = resp.interact_pointer_pos() {
                if near(self, p).is_some() {
                    if !ctrl {
                        self.selected.clear();
                    }
                    let n = self.project.content.tracks[ti].notes.len();
                    for &t in &self.project.content.tracks[ti].notes {
                        self.selected.insert((tid, t));
                    }
                    self.toast(format!("Selected all {n} hits of this sample"));
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
                (Some(Drag::Notes { .. }), _) if ctrl => egui::CursorIcon::Copy,
                (Some(Drag::Notes { .. }), _) => egui::CursorIcon::Grabbing,
                _ if over_hit => egui::CursorIcon::Grab,
                (_, Tool::Place) => egui::CursorIcon::Crosshair,
                _ => egui::CursorIcon::Default,
            };
            ui.ctx().set_cursor_icon(cursor);
        }
        if resp.hovered() && self.drag.is_none() && self.project.content.tracks[ti].sample.is_none() {
            let hint = if self.project.content.tracks[ti].sample.is_none() {
                "Pick a sound file for this sample first"
            } else if self.tool == Tool::Place {
                "Click: add · drag a hit: move, also to another sample (Ctrl: copy) · right-click: delete"
            } else {
                "Click: select · double-click: all hits of this sample · drag: move · Ctrl+drag a hit: copy"
            };
            resp.clone().on_hover_text_at_pointer(hint);
        }
        if let Some(path) = self.project.content.tracks[ti].sample.as_ref().and_then(|s| self.sample_path(s)) {
            if let Some(err) = self.sample_errors.get(&path) {
                let short = err.replace(&path.display().to_string(), &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                painter.text(rect.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, format!("Can't play sample: {short}"), FontId::proportional(11.0), self.pal.danger);
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
        ("Ctrl+Up / Down", "Playback speed +/- 25% (add Shift: 5%)"),
    ]),
    ("Snap and zoom", &[
        ("Ctrl+wheel", "Change snap divisor"),
        ("Shift+1..9", "Snap 1/1 to 1/9"),
        ("Ctrl+M", "Next snap divisor"),
        ("Alt+wheel", "Zoom the timeline"),
    ]),
    ("Editing", &[
        ("1 / 2", "Select tool / Place tool"),
        ("Sample keys", "Toggle a hit at the playhead (default Q / W / E / R; click a sample's key cap to change)"),
        ("Ctrl+A", "Select all hits"),
        ("Ctrl+X / C / V", "Cut / copy / paste at the playhead"),
        ("Ctrl+D", "Clone selection one measure later"),
        ("J / K", "Nudge selection back / forward one tick"),
        ("Delete", "Delete selection"),
        ("Shift (dragging)", "Ignore snap"),
        ("Ctrl+drag a hit", "Copy the selected hits to where you drop them"),
        ("Drag a hit up / down", "Move the selected hits to another sample (Ctrl: copy)"),
        ("Double-click a hit", "Select every hit of that sample"),
        ("Double-click a bar", "Select every hit in that bar (on the ruler; Ctrl adds)"),
        ("Right-click a bar", "Select, repeat into the next bar, or clear that bar"),
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
    // Holding a key only repeats for keys that are meant to be held (seeking, nudging);
    // otherwise holding Space or a track key would toggle over and over.
    let repeats = matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown | Key::J | Key::K);
    // Match the key's label (logical key), so shortcuts follow the user's keyboard layout.
    // Digits also match their position, so Shift+1 still works although its label is "!".
    let is_digit = matches!(key, Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9);
    ctx.input_mut(|i| {
        let mut hit = false;
        i.events.retain(|e| {
            if let Event::Key { key: k, physical_key, pressed: true, repeat, modifiers: m, .. } = e {
                let same = *k == key || (is_digit && *physical_key == Some(key));
                if !hit && same && m.matches_exact(mods) {
                    if *repeat && !repeats {
                        return false; // swallow the repeat, but don't act on it
                    }
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

        // Waiting for a new hit key for a track: the next key press is the answer.
        if let Some(id) = self.capturing_key {
            if self.capture_key(ctx, id, engine) {
                return false;
            }
        }

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

        // Per-track keys (default Q/W/E/R): toggle a hit at the playhead on that track.
        let keys = project::effective_keys(&self.project.content.tracks);
        for (ti, name) in keys.into_iter().enumerate() {
            if let Some(key) = name.as_deref().and_then(Key::from_name) {
                if k(none, key) {
                    self.toggle_track_at(ti, pos, engine);
                }
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

        let popup_open = ctx.any_popup_open() || ctx.data(|d| d.get_temp::<bool>(egui::Id::unique("theme-roll"))).unwrap_or(false);
        if !popup_open && k(none, Key::Escape) {
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
        let rect = Rect::from_center_size(pos2(screen.center().x, screen.bottom() - 120.0 + (1.0 - alpha) * 8.0), size);
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

    /// Move (or with `copy`, duplicate) the selected hits by `delta` ms and `rows` sample rows;
    /// the moved hits or copies become the selection.
    fn shift_selection(&mut self, delta: i64, rows: i32, copy: bool, engine: Option<&Engine>) {
        if self.selected.is_empty() || (delta == 0 && rows == 0) {
            return;
        }
        let dur = self.duration() as i64;
        let n = self.project.content.tracks.len() as i64;
        let plan: Vec<(NoteKey, usize)> = self
            .selected
            .iter()
            .filter_map(|&(id, t)| {
                let to = self.track_index(id)? as i64 + rows as i64;
                ((0..n).contains(&to) && (0..=dur).contains(&(t + delta))).then_some(((id, t), to as usize))
            })
            .collect();
        if plan.len() != self.selected.len() {
            return; // some hit would land outside the song or the sample list
        }
        self.checkpoint();
        if !copy {
            for &((id, t), _) in &plan {
                if let Some(ti) = self.track_index(id) {
                    self.project.content.tracks[ti].remove_note(t);
                }
            }
        }
        self.selected.clear();
        for &((_, t), to) in &plan {
            let track = &mut self.project.content.tracks[to];
            track.insert_note(t + delta);
            self.selected.insert((track.id, t + delta));
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
        let saved = std::mem::take(&mut self.clipboard);
        self.copy_selection();
        self.paste_at(min + shift, engine);
        self.clipboard = saved;
    }

    /// Toggle a hit at the playhead (snapped) on track `ti`.
    fn toggle_track_at(&mut self, ti: usize, pos: f64, engine: Option<&Engine>) {
        if ti >= self.project.content.tracks.len() {
            return;
        }
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

    /// Handle the key press that assigns a track's hit key. Returns true if a key was consumed.
    fn capture_key(&mut self, ctx: &egui::Context, id: u64, engine: Option<&Engine>) -> bool {
        let _ = engine;
        let pressed = ctx.input_mut(|i| {
            let mut found = None;
            i.events.retain(|e| {
                if found.is_none() {
                    if let Event::Key { key, pressed: true, repeat: false, modifiers, .. } = e {
                        if !modifiers.command && !modifiers.alt {
                            // The key as labelled on the user's keyboard (works on AZERTY etc.).
                            found = Some(*key);
                            return false;
                        }
                    }
                }
                true
            });
            found
        });
        let Some(key) = pressed else { return false };
        let Some(ti) = self.track_index(id) else {
            self.capturing_key = None;
            return true;
        };
        match key {
            Key::Escape => {}
            Key::Backspace | Key::Delete => {
                self.checkpoint();
                self.project.content.tracks[ti].key = Some(String::new());
                self.changed(engine);
                self.toast("Key removed");
            }
            k if RESERVED_KEYS.iter().any(|(r, _)| *r == k) => {
                let what = RESERVED_KEYS.iter().find(|(r, _)| *r == k).map(|(_, w)| *w).unwrap_or("");
                self.toast(format!("{} is already used for {what}", k.name()));
                return true; // keep waiting for another key
            }
            k => {
                let name = k.name().to_string();
                self.checkpoint();
                // The key moves here: any other track using it loses it.
                let eff = project::effective_keys(&self.project.content.tracks);
                for (j, other) in self.project.content.tracks.iter_mut().enumerate() {
                    if j != ti && eff[j].as_deref() == Some(name.as_str()) {
                        other.key = Some(String::new());
                    }
                }
                self.project.content.tracks[ti].key = Some(name.clone());
                self.changed(engine);
                self.toast(format!("{} now uses {name}", self.project.content.tracks[ti].name));
            }
        }
        self.capturing_key = None;
        true
    }
}

/// Keys the editor already uses without modifiers, so they can't be track keys.
const RESERVED_KEYS: &[(Key, &str)] = &[
    (Key::Space, "play / pause"),
    (Key::C, "play / pause"),
    (Key::X, "play from start"),
    (Key::Z, "jump to first hit"),
    (Key::V, "jump to last hit"),
    (Key::J, "nudge back"),
    (Key::K, "nudge forward"),
    (Key::Num1, "the Select tool"),
    (Key::Num2, "the Place tool"),
    (Key::ArrowLeft, "seeking"),
    (Key::ArrowRight, "seeking"),
    (Key::ArrowUp, "seeking"),
    (Key::ArrowDown, "seeking"),
    (Key::Home, "going to the start"),
    (Key::Enter, "confirming"),
    (Key::Tab, "moving focus"),
];

/// The small key cap on a track header. Defaults are drawn dimmer.
fn key_badge(ui: &mut egui::Ui, pal: &Palette, key: Option<&str>, is_default: bool, capturing: bool) -> egui::Response {
    let label = if capturing {
        "?".to_string()
    } else {
        key.map(|k| {
            // Show the key's symbol (";" rather than "Semicolon"), shortened if still long.
            let shown = Key::from_name(k).map(|k| k.symbol_or_name().to_string()).unwrap_or_else(|| k.to_string());
            if shown.chars().count() > 4 { shown.chars().take(3).collect() } else { shown }
        })
        .unwrap_or_else(|| "–".into())
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 22.0), Sense::click());
    let (fill, stroke, text) = if capturing {
        (pal.accent.gamma_multiply(0.25), Stroke::new(1.5, pal.accent), pal.accent)
    } else if resp.hovered() {
        (pal.text.gamma_multiply(0.12), Stroke::new(1.0, pal.text_weak), pal.text)
    } else if is_default || key.is_none() {
        (Color32::TRANSPARENT, Stroke::new(1.0, pal.border), pal.text_weak)
    } else {
        (pal.text.gamma_multiply(0.08), Stroke::new(1.0, pal.text_weak), pal.text)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(5), fill);
    painter.rect_stroke(rect, CornerRadius::same(5), stroke, StrokeKind::Inside);
    painter.text(rect.center(), Align2::CENTER_CENTER, label, FontId::monospace(12.0), text);
    let tip = if capturing {
        "Press a key for this sample · Backspace: no key · Esc: cancel".to_string()
    } else {
        match (key, is_default) {
            (Some(k), true) => format!("Key {k} toggles a hit at the playhead (default). Click to change."),
            (Some(k), false) => format!("Key {k} toggles a hit at the playhead. Click to change."),
            (None, _) => "No key. Click to assign one.".to_string(),
        }
    };
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Difficulty files (.osu) in a mapset folder, sorted.
fn list_diffs(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.to_ascii_lowercase().ends_with(".osu")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Reserve a full-width row and split it into a fixed header column and a lane
/// rectangle, so every lane shares the exact same x range.
/// This frame's mouse wheel input in notches (one notch = 1.0, wheel up / away = positive).
fn wheel_notches(ui: &egui::Ui) -> Vec<(f32, Modifiers)> {
    ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                Event::MouseWheel { unit, delta, modifiers, .. } => {
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
    })
}

/// Wheel over a hovered slider nudges its value by `step` per notch (Alt is left for the overall volume).
fn wheel_nudge(ui: &mut egui::Ui, r: &egui::Response, v: &mut f32, step: f32, lo: f32, hi: f32) {
    if !r.hovered() {
        return;
    }
    let n: f32 = wheel_notches(ui).iter().filter(|(_, m)| !m.alt && !m.command).map(|(n, _)| n).sum();
    if n != 0.0 {
        *v = (((*v + n * step) / step).round() * step).clamp(lo, hi);
    }
}

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

/// Smoothstep easing for fades.
fn ease(k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    k * k * (3.0 - 2.0 * k)
}

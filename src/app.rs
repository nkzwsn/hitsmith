use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::audio::{self, Engine, Event, Peaks};
use crate::export;
use crate::library;
use crate::osu::{self, Meta, RedLine};
use crate::theme::{self, Palette, ThemeState};
use crate::project::{self, Content, ControlPoint, Project, ProjectEntry, SampleEntry, SampleRef, Track};

const HEADER_W: f32 = 290.0;
const WAVE_H: f32 = 84.0;
const LANE_H: f32 = 58.0;
const ROW_H: f32 = 54.0;
const DIVISORS: [u32; 8] = [1, 2, 3, 4, 6, 8, 12, 16];

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    last_dir: Option<PathBuf>,
    song_volume: f32,
    hit_volume: f32,
    dark_mode: bool,
    dark_palette: usize,
    light_palette: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self { last_dir: None, song_volume: 0.7, hit_volume: 1.0, dark_mode: true, dark_palette: 0, light_palette: 0 }
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
        let data_dir = data_dir();
        let projects_dir = data_dir.join("projects");
        let _ = std::fs::create_dir_all(&projects_dir);
        let settings: Settings = std::fs::read(data_dir.join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let (engine, audio_error) = match Engine::new() {
            Ok(e) => {
                e.set_gains(settings.song_volume, settings.hit_volume);
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
                    self.home.name = osu::sanitize_file_part(bm.meta.title.as_str());
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
        egui::Panel::top("home-top").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if let Some(logo) = &self.logo {
                    ui.image((logo.id(), vec2(28.0, 28.0)));
                }
                ui.heading("hitsmith");
                ui.label(egui::RichText::new("hitsounding workstation for osu!").weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme_menu(ui, &mut self.settings) {
                        self.theme_changed = true;
                    }
                });
            });
            ui.add_space(6.0);
        });
        egui::Panel::right("home-projects").default_size(380.0).show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Projects");
                if ui.small_button("Refresh").on_hover_text("Refresh").clicked() {
                    self.refresh_projects();
                }
            });
            ui.add_space(4.0);
            if self.home.projects.is_empty() {
                ui.label(egui::RichText::new("No projects yet. Open a beatmap to start one.").weak());
            }
            let mut open = None;
            let mut delete = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for p in &self.home.projects {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.strong(&p.name);
                                ui.label(egui::RichText::new(&p.mapset_label).small().weak());
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Delete").clicked() {
                                    delete = Some(p.dir.clone());
                                }
                                if ui.button("Open").clicked() {
                                    open = Some(p.clone());
                                }
                            });
                        });
                    });
                }
            });
            if let Some(p) = open {
                self.open_project(&ctx, p);
            }
            if delete.is_some() {
                self.home.confirm_delete = delete;
            }
        });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.add_space(8.0);
            if let Some(err) = &self.audio_error {
                ui.colored_label(self.pal.warn, err);
            }
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(egui::RichText::new("Open beatmap...").size(16.0)).min_size(vec2(180.0, 34.0))).clicked() {
                    self.pick_beatmap(&ctx);
                }
                ui.label(egui::RichText::new("Pick any difficulty (.osu) of the mapset. Its timing is used for the grid.").weak());
            });
            ui.add_space(10.0);
            if let Some(err) = &self.home.error {
                ui.colored_label(self.pal.danger, err);
            }
            let mut switch_to = None;
            let mut create = false;
            if let Some(p) = &self.home.picked {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if let Some(tex) = &p.bg {
                        let size = tex.size_vec2();
                        let mut w = ui.available_width().min(640.0);
                        let mut h = w * size.y / size.x;
                        if h > 220.0 {
                            h = 220.0;
                            w = h * size.x / size.y;
                        }
                        ui.image((tex.id(), vec2(w, h)));
                    }
                    ui.heading(format!("{} - {}", p.meta.display_artist(), p.meta.display_title()));
                    ui.label(format!("Mapped by {}  ·  {}", p.meta.creator, p.bpm));
                    ui.label(egui::RichText::new(p.dir.display().to_string()).small().weak());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Timing from");
                        egui::ComboBox::from_id_salt("diff").width(360.0).selected_text(&p.osu_file).show_ui(ui, |ui| {
                            for d in &p.diffs {
                                if ui.selectable_label(*d == p.osu_file, d).clicked() {
                                    switch_to = Some(p.dir.join(d));
                                }
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Project name");
                        let r = ui.add(egui::TextEdit::singleline(&mut self.home.name).desired_width(260.0));
                        if (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) || ui.button("Create project").clicked() {
                            create = true;
                        }
                    });
                });
            } else {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("No beatmap selected").size(18.0).weak());
                });
            }
            if let Some(f) = switch_to {
                self.select_beatmap(&ctx, &f);
            }
            if create {
                let p = self.home.picked.as_ref().unwrap();
                let proj = Project::new(p.dir.clone(), p.osu_file.clone());
                match project::create(&self.projects_dir, &self.home.name, &proj) {
                    Ok(entry) => {
                        self.home.name.clear();
                        self.refresh_projects();
                        self.open_project(&ctx, entry);
                    }
                    Err(e) => self.home.error = Some(format!("{e:#}")),
                }
            }
        });

        if let Some(dir) = self.home.confirm_delete.clone() {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut close = false;
            egui::Modal::new(egui::Id::unique("confirm-delete")).show(&ctx, |ui| {
                ui.heading("Delete project?");
                ui.label(format!("\"{name}\" and its imported samples will be moved to the Recycle Bin, so you can restore it from there. Exported difficulties in the mapset stay."));
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
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
                let settings = &mut self.settings;
                let mut picked = false;
                go_home = w.ui(ui, self.engine.as_ref(), &mut vols, &mut |ui: &mut egui::Ui| picked |= theme_menu(ui, settings));
                self.theme_changed |= picked;
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
    Note { track: usize, orig: i64, cur: i64 },
    Point { idx: usize, orig: ControlPoint, cur: ControlPoint },
    Pan { start_view: f64 },
}

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
        };
        if let Some(e) = engine {
            e.set_song(Arc::new(Vec::new()));
            e.set_samples(Vec::new());
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
                events.push(Event { frame: (n.max(0) as f64 * rate / 1000.0) as u64, sample: id, gain });
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
        if playing {
            ctx.request_repaint();
        }
        let mut go_home = false;

        // Keyboard shortcuts (ignored while typing in a text field).
        let typing = ctx.memory(|m| m.focused()).is_some() && ctx.egui_wants_keyboard_input();
        if !typing {
            let (space, undo, redo, save, home) = ui.input(|i| {
                let cmd = i.modifiers.command;
                (
                    i.key_pressed(Key::Space),
                    cmd && i.key_pressed(Key::Z) && !i.modifiers.shift,
                    cmd && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))),
                    cmd && i.key_pressed(Key::S),
                    i.key_pressed(Key::Home),
                )
            });
            if space {
                if let Some(e) = engine {
                    e.play(!playing);
                }
            }
            if undo {
                self.undo(engine);
            }
            if redo {
                self.redo(engine);
            }
            if save {
                self.save_now();
            }
            if home {
                if let Some(e) = engine {
                    e.seek_ms(0.0);
                }
                self.view_start = -500.0;
            }
        }

        egui::Panel::top("work-top").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("< Projects").on_hover_text("Save and go back").clicked() {
                    go_home = true;
                }
                ui.separator();
                let play_label = if playing { "Pause" } else { "Play" };
                if ui.add_enabled(engine.is_some() && self.song.is_some(), egui::Button::new(play_label).min_size(vec2(58.0, 0.0))).on_hover_text("Space").clicked() {
                    if let Some(e) = engine {
                        e.play(!playing);
                    }
                }
                if ui.button("Stop").on_hover_text("Stop and go to start (Home)").clicked() {
                    if let Some(e) = engine {
                        e.play(false);
                        e.seek_ms(0.0);
                    }
                    self.view_start = -500.0;
                }
                ui.label(egui::RichText::new(fmt_time(pos)).monospace().size(16.0));
                ui.label(format!("{:.0} BPM", osu::red_line_at(&self.reds, pos).bpm()));
                ui.separator();
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).on_hover_text("Undo (Ctrl+Z)").clicked() {
                    self.undo(engine);
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo")).on_hover_text("Redo (Ctrl+Y)").clicked() {
                    self.redo(engine);
                }
                ui.separator();
                ui.label("Snap");
                egui::ComboBox::from_id_salt("div").width(60.0).selected_text(format!("1/{}", self.project.divisor)).show_ui(ui, |ui| {
                    for d in DIVISORS {
                        if ui.selectable_label(self.project.divisor == d, format!("1/{d}")).clicked() {
                            self.project.divisor = d;
                            self.dirty = true;
                        }
                    }
                });
                ui.toggle_value(&mut self.follow, "Follow").on_hover_text("Keep the playhead in view while playing");
                ui.separator();
                ui.spacing_mut().slider_width = 70.0;
                ui.label("Song");
                ui.add(egui::Slider::new(&mut vols.0, 0.0..=1.0).show_value(false)).on_hover_text("Song volume");
                ui.label("Hits");
                ui.add(egui::Slider::new(&mut vols.1, 0.0..=1.5).show_value(false)).on_hover_text("Hitsound volume");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme_ui(ui);
                    if ui.button("Export…").clicked() {
                        self.export_open = true;
                        self.export_result = None;
                    }
                    if ui.button("+ Track").clicked() {
                        self.add_track(engine);
                    }
                    if ui.button("Import sample…").on_hover_text("Copy a .wav/.ogg/.mp3 into this project").clicked() {
                        if let Some(files) = rfd::FileDialog::new().add_filter("Audio", &project::AUDIO_EXTS).pick_files() {
                            for f in files {
                                if let Err(e) = project::import_sample(&self.entry.samples_dir(), &f) {
                                    self.save_error = Some(format!("{e:#}"));
                                }
                            }
                            self.refresh_library();
                        }
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("work-bottom").show(ui, |ui| {
            ui.horizontal(|ui| {
                let total = self.duration();
                ui.spacing_mut().slider_width = (ui.available_width() - 330.0).max(100.0);
                let mut v = self.view_start;
                if ui.add(egui::Slider::new(&mut v, -2000.0..=total).show_value(false)).on_hover_text("Scroll the timeline").changed() {
                    self.view_start = v;
                    self.follow = false;
                }
                ui.label("Zoom");
                let mut z = self.project.zoom;
                ui.spacing_mut().slider_width = 120.0;
                if ui.add(egui::Slider::new(&mut z, 0.02..=2.0).logarithmic(true).show_value(false)).on_hover_text("Ctrl + mouse wheel also zooms").changed() {
                    self.project.zoom = z;
                    self.dirty = true;
                }
                let status = if let Some(e) = &self.save_error {
                    egui::RichText::new(e).color(self.pal.danger)
                } else if self.dirty {
                    egui::RichText::new("Saving…").weak()
                } else {
                    egui::RichText::new("Saved").weak()
                };
                ui.label(status);
            });
        });

        egui::CentralPanel::default_margins().show(ui, |ui| {
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

            // Ctrl+wheel zoom around the mouse, wheel pans over the top lanes, shift+wheel pans anywhere.
            let (zoom_delta, scroll, hover, shift) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta(), i.pointer.hover_pos(), i.modifiers.shift));
            if let Some(h) = hover {
                if full.contains(h) && h.x > timeline_left {
                    let tr = Rect::from_min_size(pos2(timeline_left, full.top()), vec2(grid_w, full.height()));
                    if zoom_delta != 1.0 {
                        let anchor = self.t_of(tr, h.x);
                        self.project.zoom = (self.project.zoom * zoom_delta).clamp(0.02, 2.0);
                        self.view_start = anchor - ((h.x - tr.left()) / self.project.zoom) as f64;
                        self.dirty = true;
                    }
                    let over_top = h.y < full.top() + WAVE_H + LANE_H + 8.0;
                    let dx = if shift { scroll.x + scroll.y } else if over_top { scroll.y + scroll.x } else { scroll.x };
                    if dx != 0.0 {
                        self.view_start -= (dx / self.project.zoom) as f64;
                        self.follow = false;
                        ui.input_mut(|i| {
                            if shift || over_top {
                                i.smooth_scroll_delta = Vec2::ZERO;
                            }
                        });
                    }
                }
            }
            if playing && self.follow {
                let span = grid_w as f64 / self.project.zoom as f64;
                if pos > self.view_start + span * 0.85 || pos < self.view_start {
                    self.view_start = pos - span * 0.15;
                }
            }

            // Waveform row.
            let (header, rect, resp) = lane_row(ui, "wave", WAVE_H, timeline_left, lane_right);
            {
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(header));
                ui.shrink_clip_rect(header);
                ui.label(egui::RichText::new(&self.meta.title).strong());
                ui.label(egui::RichText::new(format!("{} · {}", self.meta.artist, self.project.osu_file)).small().weak());
                ui.label(egui::RichText::new("Click to seek · drag to scroll").small().weak());
            }
            self.wave_lane(ui, rect, &resp, pos, engine);
            ui.add_space(4.0);

            // Volume (control point) lane.
            let (header, rect, resp) = lane_row(ui, "volume", LANE_H, timeline_left, lane_right);
            {
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(header));
                ui.shrink_clip_rect(header);
                ui.label(egui::RichText::new(format!("Volume  {}%", self.project.volume_at(pos as i64))).strong());
                ui.label(egui::RichText::new("Click: add point · drag: move/volume · right-click: delete").small().weak());
            }
            self.volume_lane(ui, rect, &resp, pos, engine);
            ui.add_space(6.0);
            ui.separator();

            if self.project.content.tracks.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("No tracks yet").size(18.0));
                    ui.label(egui::RichText::new("Add a track, pick its sample, then click the grid to place hits.").weak());
                    ui.add_space(8.0);
                    if ui.button("+ Add track").clicked() {
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
                for ti in 0..count {
                    let id = self.project.content.tracks[ti].id;
                    let (header, rect, resp) = lane_row(ui, ("track", id), ROW_H, timeline_left, lane_right);
                    {
                        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(header).layout(egui::Layout::left_to_right(egui::Align::Center)));
                        ui.shrink_clip_rect(header);
                        self.track_header(ui, ti, engine);
                    }
                    self.track_lane(ui, ti, rect, &resp, pos, engine);
                    ui.add_space(2.0);
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
        let mut edited = false;
        let mut audition = None;
        let mut remove = false;
        let library = self.library.clone();
        let t = &mut self.project.content.tracks[ti];
        let color = self.pal.tracks[t.color as usize % 8];
        let (bar, _) = ui.allocate_exact_size(vec2(4.0, ROW_H - 6.0), Sense::hover());
        ui.painter().rect_filled(bar, 2.0, color);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(104.0));
                if ui.selectable_label(t.muted, "M").on_hover_text("Mute").clicked() {
                    t.muted = !t.muted;
                    edited = true;
                }
                if ui.selectable_label(t.solo, "S").on_hover_text("Solo").clicked() {
                    t.solo = !t.solo;
                    edited = true;
                }
                if ui.small_button("Play").on_hover_text("Preview sample").clicked() {
                    audition = t.sample.clone().map(|s| (s, t.volume));
                }
                if ui.small_button("X").on_hover_text("Remove track").clicked() {
                    remove = true;
                }
            });
            ui.horizontal(|ui| {
                let label = t.sample.as_ref().map(|s| s.file.clone()).unwrap_or_else(|| "(no sample)".into());
                egui::ComboBox::from_id_salt(("sample", t.id)).width(160.0).selected_text(label).show_ui(ui, |ui| {
                    for e in &library {
                        let sel = t.sample.as_ref() == Some(&e.r);
                        if ui.selectable_label(sel, format!("{}  ·  {}", e.r.file, e.r.source.label())).clicked() && !sel {
                            t.sample = Some(e.r.clone());
                            edited = true;
                            audition = Some((e.r.clone(), t.volume));
                        }
                    }
                });
                let r = ui.add(egui::DragValue::new(&mut t.volume).range(0..=100).suffix("%")).on_hover_text("Track volume");
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    edited = true;
                }
            });
        });
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
        let color = self.pal.tracks[track.color as usize % 8];
        let dim = !self.project.audible(track);
        painter.rect_filled(rect, CornerRadius::same(4), self.pal.lane);
        self.draw_grid(&painter, rect, false);
        let t0 = self.t_of(rect, rect.left() - 10.0) as i64;
        let t1 = self.t_of(rect, rect.right() + 10.0) as i64;
        let dragging = match &self.drag {
            Some(Drag::Note { track, orig, cur }) if *track == ti => Some((*orig, *cur)),
            _ => None,
        };
        let start = track.notes.partition_point(|&n| n < t0);
        let fillc = if dim { color.gamma_multiply(0.35) } else { color };
        let h = rect.height() - 14.0;
        for &n in &track.notes[start..] {
            if n > t1 {
                break;
            }
            let shown = match dragging {
                Some((o, c)) if o == n => c,
                _ => n,
            };
            let x = self.x_of(rect, shown as f64);
            let r = Rect::from_center_size(pos2(x, rect.center().y), vec2(9.0, h));
            painter.rect_filled(r, CornerRadius::same(3), fillc);
            painter.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Inside);
        }
        if let Some((_, c)) = dragging {
            let x = self.x_of(rect, c as f64);
            painter.vline(x, rect.y_range(), Stroke::new(1.0, self.pal.text));
        }
        // Hover ghost for the snapped position.
        if self.drag.is_none() {
            if let Some(h) = resp.hover_pos() {
                let t = self.snap(self.t_of(rect, h.x));
                let x = self.x_of(rect, t as f64);
                painter.rect_stroke(Rect::from_center_size(pos2(x, rect.center().y), vec2(9.0, rect.height() - 14.0)), CornerRadius::same(3), Stroke::new(1.0, color.gamma_multiply(0.6)), StrokeKind::Inside);
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
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(n) = near(self, p) {
                    self.drag = Some(Drag::Note { track: ti, orig: n, cur: n });
                }
            }
        }
        if let Some(p) = resp.interact_pointer_pos() {
            let snapped = self.snap(self.t_of(rect, p.x)).max(0);
            if let Some(Drag::Note { track, cur, .. }) = &mut self.drag {
                if *track == ti {
                    *cur = snapped;
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(Drag::Note { track, orig, cur }) = self.drag.take() {
                if track == ti && orig != cur && !self.project.content.tracks[ti].notes.contains(&cur) {
                    self.checkpoint();
                    let t = &mut self.project.content.tracks[ti];
                    t.remove_note(orig);
                    t.insert_note(cur);
                    self.changed(engine);
                }
            }
        }
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if near(self, p).is_none() {
                    let t = self.snap(self.t_of(rect, p.x));
                    if t >= 0 && (t as f64) <= self.duration() {
                        self.checkpoint();
                        self.project.content.tracks[ti].insert_note(t);
                        self.changed(engine);
                        let tr = self.project.content.tracks[ti].clone();
                        if let Some(s) = &tr.sample {
                            if let (Some(path), Some(e)) = (self.sample_path(s), engine) {
                                if let Some(id) = self.ensure_sample(&path, Some(e)) {
                                    if !e.is_playing() {
                                        e.audition(id, tr.volume as f32 / 100.0 * self.project.volume_at(t) as f32 / 100.0);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(n) = near(self, p) {
                    self.checkpoint();
                    self.project.content.tracks[ti].remove_note(n);
                    self.changed(engine);
                }
            }
        }
        if resp.hovered() {
            let hint = if self.project.content.tracks[ti].sample.is_none() { "Pick a sample for this track first" } else { "Click: add · drag: move · right-click: delete" };
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

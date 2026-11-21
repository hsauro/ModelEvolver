//! Application state, top-level layout, run controls and persistence.

use std::collections::BTreeSet;
use std::rc::Rc;

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};
use web_time::Instant;

use evolver_core::config::{EvolverConfig, ObjectiveKind};
use evolver_core::ga::ExperimentResult;
use evolver_core::objective::Target;
use evolver_core::patterns::{PatternError, PatternSet};
use evolver_core::simulate::Trajectory;

use crate::files::{Files, OpenKind};
use crate::runner::{RunState, Runner, RunnerEvent, hardware_threads};

pub const ACCENT: Color32 = Color32::from_rgb(0x2b, 0x7b, 0xd9);
pub const SUCCESS: Color32 = Color32::from_rgb(0x2e, 0xa0, 0x43);
pub const WARN: Color32 = Color32::from_rgb(0xd0, 0x7a, 0x12);

/// A finished experiment, with the settings it ran under.
pub struct ResultEntry {
    pub r: ExperimentResult,
    pub cfg: Rc<EvolverConfig>,
    pub target: Rc<Target>,
    pub run: u32,
    pub secs: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    Progress,
    Results,
    Grid,
    Model,
}

/// What the Model tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Index into `results`.
    Result(usize),
    /// The current best network of a running experiment.
    Live(u32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub dark: bool,
    pub grid_cols: usize,
    pub grid_rows: usize,
    pub grid_all_species: bool,
    pub grid_include_failed: bool,
    /// End time of the grid plots (each visible model is re-simulated on 0..this).
    pub grid_t_end: f64,
    pub results_viable_only: bool,
    pub progress_log_y: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            dark: false,
            grid_cols: 4,
            grid_rows: 3,
            grid_all_species: false,
            grid_include_failed: false,
            grid_t_end: 100.0,
            results_viable_only: true,
            progress_log_y: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortCol {
    Id,
    Fitness,
    Reactions,
    Generations,
    Time,
}

/// Model tab state.
pub struct ModelView {
    pub show_reduced: bool,
    pub custom_window: bool,
    pub t_end: f64,
    pub n_points: usize,
    pub hidden: BTreeSet<usize>,
    pub cache: Option<(String, Result<Trajectory, String>)>,
}

impl Default for ModelView {
    fn default() -> Self {
        Self { show_reduced: true, custom_window: false, t_end: 300.0, n_points: 1000, hidden: BTreeSet::new(), cache: None }
    }
}

pub struct App {
    pub cfg: EvolverConfig,
    pub target: Target,
    pub target_name: String,
    pub prefs: Prefs,
    pub runner: Runner,
    pub files: Files,
    pub results: Vec<ResultEntry>,
    pub run_no: u32,
    run_cfg: Option<(Rc<EvolverConfig>, Rc<Target>)>,
    pub run_successes: u32,
    pub run_finished: u32,
    pub run_failed: u32,
    pub last_success: Option<Instant>,
    pub tab: Tab,
    pub selection: Option<Selection>,
    pub model: ModelView,
    pub status: Option<(String, Instant)>,
    pub pattern_errors: Vec<PatternError>,
    pub config_errors: Vec<String>,
    pub results_sort: (SortCol, bool),
    pub results_selected: Option<usize>,
    pub grid_page: usize,
    pub progress_selected: Option<u32>,
    pub cpu_threads: usize,
    /// Grid trajectories: (run, experiment, end time bits) -> trajectory.
    pub grid_cache: std::collections::HashMap<(u32, u32, u64), Option<Trajectory>>,
    /// UI cost: smoothed CPU time per frame (ms), and frames in the last second.
    ui_ms: f32,
    frames: Vec<Instant>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let storage = cc.storage;
        let get = |key: &str| storage.and_then(|s| s.get_string(key));
        let cfg = get("cfg").and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_else(EvolverConfig::oscillator_preset);
        let target = get("target").and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        let target_name = get("target_name").unwrap_or_else(|| "built-in square wave".into());
        let prefs: Prefs = get("prefs").and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        apply_theme(&cc.egui_ctx, prefs.dark);
        let mut app = Self {
            cfg,
            target,
            target_name,
            prefs,
            runner: Runner::new(cc.egui_ctx.clone()),
            files: Files::default(),
            results: Vec::new(),
            run_no: 0,
            run_cfg: None,
            run_successes: 0,
            run_finished: 0,
            run_failed: 0,
            last_success: None,
            tab: Tab::Progress,
            selection: None,
            model: ModelView::default(),
            status: None,
            pattern_errors: Vec::new(),
            config_errors: Vec::new(),
            results_sort: (SortCol::Fitness, true),
            results_selected: None,
            grid_page: 0,
            progress_selected: None,
            cpu_threads: hardware_threads(),
            grid_cache: std::collections::HashMap::new(),
            ui_ms: 0.0,
            frames: Vec::new(),
        };
        app.revalidate();
        app
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    /// Re-check the settings and the pattern text (call after any edit).
    pub fn revalidate(&mut self) {
        self.config_errors = self.cfg.validate().err().unwrap_or_default();
        self.pattern_errors =
            PatternSet::parse(&self.cfg.reaction_patterns, self.cfg.disallow_self_referential, self.cfg.disallow_autocatalysis).1;
    }

    pub fn workers(&self) -> usize {
        self.cfg.worker_count(self.cpu_threads)
    }

    pub fn is_idle(&self) -> bool {
        self.runner.state == RunState::Idle
    }

    fn start(&mut self) {
        self.revalidate();
        if !self.config_errors.is_empty() {
            self.set_status("Fix the settings marked in red before starting");
            return;
        }
        let seed = if self.cfg.seed == 0 {
            let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(1, |d| d.as_nanos());
            ((now as u32) % 1_000_000_000).max(1)
        } else {
            self.cfg.seed
        };
        self.run_no += 1;
        self.run_cfg = Some((Rc::new(self.cfg.clone()), Rc::new(self.target.clone())));
        self.run_successes = 0;
        self.run_finished = 0;
        self.run_failed = 0;
        self.last_success = None;
        self.progress_selected = None;
        self.runner.start(self.cfg.clone(), self.target.clone(), seed, self.workers());
        self.tab = Tab::Progress;
        self.set_status(format!("Run {} started, seed {seed}, {} workers", self.run_no, self.workers()));
    }

    pub fn run_settings(&self) -> Option<(Rc<EvolverConfig>, Rc<Target>)> {
        self.run_cfg.clone()
    }

    fn handle_runner(&mut self) {
        for event in self.runner.poll() {
            match event {
                RunnerEvent::Finished(r, secs) => {
                    self.run_finished += 1;
                    let (cfg, target) = self.run_cfg.clone().unwrap_or_else(|| (Rc::new(self.cfg.clone()), Rc::new(self.target.clone())));
                    if r.viable {
                        self.run_successes += 1;
                        self.last_success = Some(Instant::now());
                    }
                    let first = self.results.is_empty();
                    self.results.push(ResultEntry { r: *r, cfg, target, run: self.run_no, secs });
                    if first && self.results[0].r.viable {
                        self.selection = Some(Selection::Result(0));
                    }
                }
                RunnerEvent::Failed { id, message } => {
                    self.run_failed += 1;
                    self.set_status(format!("Experiment {id} failed: {message}"));
                }
            }
        }
    }

    /// Index of the best viable result of the session.
    pub fn best_result(&self) -> Option<usize> {
        self.results
            .iter()
            .enumerate()
            .filter(|(_, e)| e.r.viable)
            .min_by(|a, b| a.1.r.fitness.total_cmp(&b.1.r.fitness))
            .map(|(i, _)| i)
    }

    pub fn open_result(&mut self, i: usize) {
        self.selection = Some(Selection::Result(i));
        self.model.cache = None;
        self.model.hidden.clear();
        self.tab = Tab::Model;
    }

    pub fn open_live(&mut self, id: u32) {
        self.selection = Some(Selection::Live(id));
        self.model.cache = None;
        self.model.hidden.clear();
        self.tab = Tab::Model;
    }

    fn handle_files(&mut self) {
        for (kind, name, text) in self.files.poll() {
            let text = match text {
                Ok(t) => t,
                Err(e) => {
                    self.set_status(format!("Could not read {name}: {e}"));
                    continue;
                }
            };
            match kind {
                OpenKind::Target => match Target::parse(&text) {
                    Ok(t) => {
                        self.target = t;
                        self.target_name = name.clone();
                        self.cfg.objective = ObjectiveKind::TimeCourse;
                        self.set_status(format!("Loaded target {name}"));
                    }
                    Err(e) => self.set_status(format!("{name}: {e}")),
                },
                OpenKind::Patterns => {
                    self.cfg.reaction_patterns = text;
                    self.set_status(format!("Loaded reaction patterns from {name}"));
                }
                OpenKind::PascalIni => {
                    let warnings = self.cfg.apply_pascal_ini(&text);
                    self.set_status(if warnings.is_empty() {
                        format!("Imported {name}")
                    } else {
                        format!("Imported {name} ({} lines ignored: {})", warnings.len(), warnings.join("; "))
                    });
                }
                OpenKind::SettingsJson => match serde_json::from_str::<EvolverConfig>(&text) {
                    Ok(c) => {
                        self.cfg = c;
                        self.set_status(format!("Loaded settings from {name}"));
                    }
                    Err(e) => self.set_status(format!("{name}: {e}")),
                },
            }
            self.revalidate();
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("ModelEvolver").size(22.0).strong().color(ACCENT));
            ui.add_space(16.0);
            let idle = self.is_idle();
            let running = self.runner.state == RunState::Running;
            let can_start = idle && self.config_errors.is_empty();
            let start = egui::Button::new(RichText::new("  Start  ").strong().color(Color32::WHITE)).fill(SUCCESS);
            if ui.add_enabled(can_start, start).on_hover_text("Start a new run with the current settings").clicked() {
                self.start();
            }
            if ui
                .add_enabled(running, egui::Button::new("Stop"))
                .on_hover_text("Start no new experiments; let the running ones finish")
                .clicked()
            {
                self.runner.stop();
                self.set_status("Stopping: waiting for running experiments to finish");
            }
            if ui
                .add_enabled(!idle, egui::Button::new("Cancel"))
                .on_hover_text("Abandon all running experiments now (finished results are kept)")
                .clicked()
            {
                self.runner.cancel();
                self.set_status("Run cancelled");
            }
            ui.separator();
            ui.label("CPU usage").on_hover_text("Share of this computer's processor threads a run may use");
            let mut pct = self.cfg.cpu_usage_percent;
            let slider = egui::Slider::new(&mut pct, 5..=100).suffix("%");
            if ui.add(slider).changed() {
                self.cfg.cpu_usage_percent = pct;
                self.cfg.num_workers = 0;
                self.runner.target_workers = self.workers();
            }
            ui.label(RichText::new(format!("{} of {} threads", self.workers(), self.cpu_threads)).weak());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if self.prefs.dark { "Light" } else { "Dark" };
                if ui.small_button(label).on_hover_text("Switch colour theme").clicked() {
                    self.prefs.dark = !self.prefs.dark;
                    apply_theme(ui.ctx(), self.prefs.dark);
                }
            });
        });
    }

    fn stats_bar(&mut self, ui: &mut egui::Ui) {
        let elapsed = self.runner.elapsed_secs();
        let active = self.runner.active_secs();
        let rate = if active > 0.0 { f64::from(self.run_successes) / active * 60.0 } else { 0.0 };
        let (started, total) = self.runner.counts();
        let running = self.runner.live.len();
        ui.horizontal(|ui| {
            stat(ui, "Successes", self.run_successes.to_string(), Some(SUCCESS));
            stat(ui, "Successes / min", if active > 5.0 { format!("{rate:.1}") } else { "-".into() }, None)
                .on_hover_text("Successes per minute while new experiments were being started (the clock stops at Stop)");
            let per = if self.run_successes > 0 { format_duration(elapsed / f64::from(self.run_successes)) } else { "-".into() };
            stat(ui, "Time per success", per, None)
                .on_hover_text("Total real time of the run divided by the number of successes");
            let since = self.last_success.map_or("-".into(), |t| format!("{:.0} s ago", t.elapsed().as_secs_f64()));
            stat(ui, "Last success", since, None);
            let done = match total {
                _ if started == 0 => "-".to_string(),
                Some(t) => format!("{} / {t}", self.run_finished),
                None => format!("{} (continuous)", self.run_finished),
            };
            stat(ui, "Experiments done", done, None);
            stat(ui, "Running", running.to_string(), None);
            stat(ui, "Elapsed", format_duration(elapsed), None).on_hover_text("Total real time of the run");
            let state = match self.runner.state {
                RunState::Idle if started == 0 => ("Ready", ui.visuals().weak_text_color()),
                RunState::Idle => ("Finished", ui.visuals().weak_text_color()),
                RunState::Running => ("Running", SUCCESS),
                RunState::Stopping => ("Stopping", WARN),
            };
            stat(ui, "Status", state.0.to_string(), Some(state.1));
        });
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let n_viable = self.results.iter().filter(|e| e.r.viable).count();
            for (tab, label) in [
                (Tab::Progress, format!("Progress ({})", self.runner.live.len())),
                (Tab::Results, format!("Results ({})", self.results.len())),
                (Tab::Grid, format!("Grid ({n_viable})")),
                (Tab::Model, "Model".to_string()),
            ] {
                let selected = self.tab == tab;
                let text = if selected { RichText::new(label).strong().color(Color32::WHITE) } else { RichText::new(label) };
                if ui.selectable_label(selected, text).clicked() {
                    self.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(best) = self.best_result() {
                    let e = &self.results[best];
                    if ui
                        .button(format!("Best model: #{} (fitness {:.4})", e.r.id, e.r.fitness))
                        .on_hover_text("Open the best model of this session")
                        .clicked()
                    {
                        self.open_result(best);
                    }
                }
            });
        });
    }
}

fn stat(ui: &mut egui::Ui, label: &str, value: String, color: Option<Color32>) -> egui::Response {
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).small().weak());
            let mut v = RichText::new(value).size(17.0).strong();
            if let Some(c) = color {
                v = v.color(c);
            }
            ui.label(v);
        });
    })
    .response
}

pub fn format_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

pub fn apply_theme(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.all_styles_mut(|style| {
        use egui::{FontFamily, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(19.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        style.spacing.interact_size.y = 24.0;
        style.visuals.selection.bg_fill = ACCENT;
    });
    // Darker text in light mode: egui's default greys are hard to read.
    ctx.style_mut_of(egui::Theme::Light, |style| {
        let v = &mut style.visuals;
        v.widgets.noninteractive.fg_stroke.color = Color32::from_gray(30);
        v.widgets.inactive.fg_stroke.color = Color32::from_gray(25);
        v.widgets.hovered.fg_stroke.color = Color32::BLACK;
        v.widgets.active.fg_stroke.color = Color32::BLACK;
        v.weak_text_color = Some(Color32::from_gray(95));
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if let Some(cpu) = frame.info().cpu_usage {
            self.ui_ms = 0.9 * self.ui_ms + 0.1 * cpu * 1000.0;
        }
        let now = Instant::now();
        self.frames.retain(|t| now.duration_since(*t).as_secs_f64() < 1.0);
        self.frames.push(now);
        self.handle_runner();
        self.handle_files();
        if !self.is_idle() {
            // Keep the elapsed time and "last success" counters moving.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
            self.stats_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                match &self.status {
                    Some((msg, t)) if t.elapsed().as_secs() < 30 => {
                        ui.label(msg);
                    }
                    _ => {
                        ui.label(RichText::new("Ready").weak());
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("ModelEvolver {} - diffsol BDF", evolver_core::export::VERSION)).weak().small(),
                    );
                    ui.separator();
                    ui.label(RichText::new(format!("UI {:.1} ms/frame, {} frames/s", self.ui_ms, self.frames.len())).weak().small())
                        .on_hover_text(
                            "Time the page spends drawing. The evolution runs in separate workers and is not slowed by drawing.",
                        );
                });
            });
        });
        egui::Panel::left("settings").default_size(340.0).min_size(280.0).resizable(true).show(ui, |ui| {
            self.settings_panel(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            self.tabs(ui);
            ui.separator();
            match self.tab {
                Tab::Progress => self.progress_tab(ui),
                Tab::Results => self.results_tab(ui),
                Tab::Grid => self.grid_tab(ui),
                Tab::Model => self.model_tab(ui),
            }
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let put = |storage: &mut dyn eframe::Storage, key: &str, v: Result<String, serde_json::Error>| {
            if let Ok(s) = v {
                storage.set_string(key, s);
            }
        };
        put(storage, "cfg", serde_json::to_string(&self.cfg));
        put(storage, "target", serde_json::to_string(&self.target));
        storage.set_string("target_name", self.target_name.clone());
        put(storage, "prefs", serde_json::to_string(&self.prefs));
    }
}

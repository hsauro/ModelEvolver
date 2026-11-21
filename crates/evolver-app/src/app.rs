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
use crate::session::{RunRecord, SessionFile, SessionResult};
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
    /// Size the grid so every model fits on one page.
    pub grid_fit_all: bool,
    /// Start the grid's y axes at zero.
    pub grid_y_from_zero: bool,
    /// End time of the grid plots (each visible model is re-simulated on 0..this).
    pub grid_t_end: f64,
    /// End time and number of points of the Model tab's time course.
    pub model_t_end: f64,
    pub model_points: usize,
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
            grid_fit_all: false,
            grid_y_from_zero: true,
            grid_t_end: 100.0,
            model_t_end: 100.0,
            model_points: 1000,
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
    pub hidden: BTreeSet<usize>,
    pub cache: Option<(String, Result<Trajectory, String>)>,
}

impl Default for ModelView {
    fn default() -> Self {
        Self { show_reduced: true, hidden: BTreeSet::new(), cache: None }
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
    pub show_about: bool,
    /// Successes not yet saved with "Save all successes (zip)".
    pub unsaved_successes: usize,
    /// True when leaving the page would lose work (read by the beforeunload handler).
    leave_guard: Rc<std::cell::Cell<bool>>,
    /// One record per run (for sessions and the loaded-session figures).
    pub runs: Vec<RunRecord>,
    /// Throughput clock and total time of a loaded session (until a new run starts).
    pub loaded_times: Option<(f64, f64)>,
    /// A session waiting for the user to confirm replacing unsaved results.
    pending_session: Option<(String, SessionFile)>,
    runner_ctx: egui::Context,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let storage = cc.storage;
        let get = |key: &str| storage.and_then(|s| s.get_string(key));
        let cfg = get("cfg").and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_else(EvolverConfig::oscillator_preset);
        let target = get("target").and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        let target_name = get("target_name").unwrap_or_else(|| "built-in square wave".into());
        let prefs: Prefs = get("prefs").and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        install_fonts(&cc.egui_ctx);
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
            show_about: false,
            unsaved_successes: 0,
            leave_guard: Rc::new(std::cell::Cell::new(false)),
            runs: Vec::new(),
            loaded_times: None,
            pending_session: None,
            runner_ctx: cc.egui_ctx.clone(),
        };
        install_leave_warning(app.leave_guard.clone());
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
        let seed = if self.cfg.seed == 0 { evolver_core::rng::random_base_seed() } else { self.cfg.seed };
        self.run_no += 1;
        self.run_cfg = Some((Rc::new(self.cfg.clone()), Rc::new(self.target.clone())));
        self.run_successes = 0;
        self.run_finished = 0;
        self.run_failed = 0;
        self.last_success = None;
        self.progress_selected = None;
        self.runner.start(self.cfg.clone(), self.target.clone(), seed, self.workers());
        self.loaded_times = None;
        self.runs.push(RunRecord {
            run: self.run_no,
            settings: self.cfg.clone(),
            target: self.target.clone(),
            base_seed: seed,
            workers: self.workers(),
            active_secs: 0.0,
            elapsed_secs: 0.0,
            experiments: 0,
            successes: 0,
        });
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
                        self.unsaved_successes += 1;
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
                OpenKind::Session => match SessionFile::from_json(&text) {
                    Ok(file) => {
                        if self.unsaved_successes > 0 {
                            self.pending_session = Some((name.clone(), file));
                        } else {
                            self.apply_session(&name, file);
                        }
                    }
                    Err(e) => self.set_status(format!("{name}: {e}")),
                },
                OpenKind::SettingsJson => match evolver_core::objective::SharedConfig::from_json(&text) {
                    Ok(shared) => {
                        // Keep this machine's CPU choices; take everything else from the file.
                        let (cpu, workers) = (self.cfg.cpu_usage_percent, self.cfg.num_workers);
                        self.cfg = shared.settings;
                        self.cfg.cpu_usage_percent = cpu;
                        self.cfg.num_workers = workers;
                        self.target = shared.target;
                        self.target_name = shared.target_name;
                        let seed = if self.cfg.seed == 0 { "random seed".to_string() } else { format!("seed {}", self.cfg.seed) };
                        self.set_status(format!("Loaded configuration {name} ({}, {seed})", shared.created_by));
                    }
                    Err(e) => self.set_status(format!("{name}: {e}")),
                },
            }
            self.revalidate();
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("ModelEvolver").size(22.0).family(bold()).color(ACCENT));
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
                self.set_status("Stopped: no new experiments; waiting for the running ones to finish");
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
                if ui.small_button("About").clicked() {
                    self.show_about = true;
                }
                let label = if self.prefs.dark { "Light" } else { "Dark" };
                if ui.small_button(label).on_hover_text("Switch colour theme").clicked() {
                    self.prefs.dark = !self.prefs.dark;
                    apply_theme(ui.ctx(), self.prefs.dark);
                }
            });
        });
    }

    fn stats_bar(&mut self, ui: &mut egui::Ui) {
        let (active, elapsed) = match self.loaded_times {
            Some(t) if self.runner.started_at.is_none() => t,
            _ => (self.runner.active_secs(), self.runner.elapsed_secs()),
        };
        let rate = if active > 0.0 { f64::from(self.run_successes) / active * 60.0 } else { 0.0 };
        let (started, total) = self.runner.counts();
        let running = self.runner.live.len();
        ui.horizontal(|ui| {
            stat(ui, "Successes", self.run_successes.to_string(), Some(SUCCESS));
            stat(ui, "Successes / min", if active > 5.0 { format!("{rate:.1}") } else { "-".into() }, None)
                .on_hover_text("Successes per minute of run time (the clock stops when you press Stop)");
            let per = if self.run_successes > 0 { format_seconds(active / f64::from(self.run_successes)) } else { "-".into() };
            stat(ui, "Time per success", per, None)
                .on_hover_text("Run time divided by the number of successes (= 60 / successes per minute)");
            let since = self.last_success.map_or("-".into(), |t| format!("{:.0} s ago", t.elapsed().as_secs_f64()));
            stat(ui, "Last success", since, None);
            let done = match total {
                _ if started == 0 && self.loaded_times.is_some() => self.run_finished.to_string(),
                _ if started == 0 => "-".to_string(),
                Some(t) => format!("{} / {t}", self.run_finished),
                None => format!("{} (continuous)", self.run_finished),
            };
            stat(ui, "Experiments done", done, None);
            stat(ui, "Running", running.to_string(), None);
            stat(ui, "Elapsed", format_duration(elapsed), None).on_hover_text("Total real time of the run");
            // Unambiguous status: a batch completing its last experiments is
            // still "Running (n left)"; after Stop it is "Stopping (n left)".
            let (status, color) = match self.runner.state {
                RunState::Idle if started == 0 && self.loaded_times.is_some() => {
                    ("Loaded session".to_string(), ui.visuals().weak_text_color())
                }
                RunState::Idle if started == 0 => ("Ready".to_string(), ui.visuals().weak_text_color()),
                RunState::Idle => ("Finished".to_string(), ui.visuals().weak_text_color()),
                RunState::Running => ("Running".to_string(), SUCCESS),
                RunState::Stopping if self.runner.stopped_at.is_some() => (format!("Stopping ({running} left)"), WARN),
                RunState::Stopping => (format!("Running ({running} left)"), SUCCESS),
            };
            stat(ui, "Status", status, Some(color));
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
                let text = if selected { RichText::new(label).family(bold()).color(Color32::WHITE) } else { RichText::new(label) };
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

pub const FUNDING: &str = "This project has been funded in whole or in part with Federal funds from the Advanced Research Projects Agency for Health (ARPA-H), Department of Health and Human Services, under the Critical Illness Immunological Reprogramming and Control Point Learning Engine (CIRCLE) program. The views and conclusions contained in this document are those of the authors and should not be interpreted as representing the official policies, either expressed or implied, of the U.S. Government.";

impl App {
    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        const REPO: &str = "https://github.com/hsauro/ModelEvolver";
        let style = ctx.global_style();
        let dark = style.visuals.dark_mode;
        let text = if dark { Color32::from_gray(230) } else { Color32::from_gray(20) };
        let soft = if dark { Color32::from_gray(190) } else { Color32::from_gray(60) };
        let body = |s: &str| RichText::new(s).color(text).size(14.5);
        let card = if dark { Color32::from_gray(40) } else { Color32::WHITE };
        let frame = egui::Frame::popup(&style).fill(card).inner_margin(egui::Margin::same(26)).corner_radius(12);
        let response = egui::Modal::new(egui::Id::unique("about_modal")).frame(frame).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new("ModelEvolver").size(26.0).family(bold()).color(text));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(RichText::new(format!("Version {}", evolver_core::export::VERSION)).color(soft));
                // The commit this build was made from (set by GitHub Actions).
                if let Some(sha) = option_env!("GITHUB_SHA") {
                    let short = &sha[..sha.len().min(7)];
                    ui.label(RichText::new(" (build ").color(soft));
                    ui.hyperlink_to(short, format!("{REPO}/commit/{sha}"));
                    ui.label(RichText::new(")").color(soft));
                }
            });
            ui.add_space(10.0);
            ui.label(body(
                "Evolve mass-action chemical reaction networks with a genetic algorithm, to find networks that \
                 oscillate or that follow a target time course. Every candidate is simulated with an adaptive \
                 stiff (BDF) solver. Everything runs in your browser, on as many processor cores as you allow.",
            ));

            let heading = |ui: &mut egui::Ui, t: &str| {
                ui.add_space(14.0);
                ui.label(RichText::new(t).size(16.5).family(bold()).color(text));
                ui.add_space(2.0);
            };
            let bullet = |ui: &mut egui::Ui, add: &dyn Fn(&mut egui::Ui)| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 18.0), egui::Sense::hover());
                    ui.painter().circle_filled(egui::pos2(rect.left() + 9.0, rect.center().y), 2.6, text);
                    add(ui);
                });
            };

            heading(ui, "Software");
            bullet(ui, &|ui| {
                ui.label(body("Written in Rust, compiled to WebAssembly. Source code: "));
                ui.hyperlink_to("hsauro/ModelEvolver", REPO);
            });
            bullet(ui, &|ui| {
                ui.label(body("ODE solver: "));
                ui.hyperlink_to("diffsol", "https://github.com/martinjrobins/diffsol");
                ui.label(body(" (adaptive BDF)"));
            });
            bullet(ui, &|ui| {
                ui.label(body("User interface: "));
                ui.hyperlink_to("egui / eframe", "https://github.com/emilk/egui");
                ui.label(body(" and egui_plot"));
            });
            bullet(ui, &|ui| {
                ui.label(body("Antimony export checked with the antimony crate; FFT by rustfft"));
            });
            bullet(ui, &|ui| {
                ui.label(body("A port of the Object Pascal OscillatorEvolver"));
            });

            heading(ui, "Funding");
            ui.label(RichText::new(FUNDING).color(soft).size(13.5));

            ui.add_space(18.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close = egui::Button::new(RichText::new("Close").size(15.5).color(Color32::WHITE))
                    .fill(ACCENT)
                    .corner_radius(8)
                    .min_size(egui::vec2(96.0, 34.0));
                if ui.add(close).clicked() {
                    ui.close();
                }
            });
        });
        if response.should_close() {
            self.show_about = false;
        }
    }
}

fn stat(ui: &mut egui::Ui, label: &str, value: String, color: Option<Color32>) -> egui::Response {
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
        ui.vertical(|ui| {
            let label_color = if ui.visuals().dark_mode { Color32::from_gray(175) } else { Color32::from_gray(70) };
            ui.label(RichText::new(label).small().color(label_color));
            let mut v = RichText::new(value).size(17.0).family(bold());
            if let Some(c) = color {
                v = v.color(c);
            }
            ui.label(v);
        });
    })
    .response
}

/// A short span with units: "14 s", "1 min 05 s", "2 h 03 min".
pub fn format_seconds(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min {:02} s", s / 60, s % 60)
    } else {
        format!("{} h {:02} min", s / 3600, (s / 60) % 60)
    }
}

/// A clock-style duration, "m:ss" or "h:mm:ss".
impl App {
    pub fn save_session(&mut self) {
        let used: std::collections::BTreeSet<u32> = self.results.iter().map(|e| e.run).collect();
        let runs: Vec<RunRecord> = self.runs.iter().filter(|r| used.contains(&r.run)).cloned().collect();
        let results: Vec<SessionResult> =
            self.results.iter().map(|e| SessionResult { run: e.run, secs: e.secs, result: e.r.clone() }).collect();
        let file = SessionFile::new(runs, results);
        if let Some(msg) = self.files.save_text("modelevolver-session.json", &file.to_json()) {
            self.unsaved_successes = 0;
            self.set_status(msg);
        }
    }

    pub fn load_session(&mut self) {
        self.files.open(OpenKind::Session, "Load a ModelEvolver session", &["json"]);
    }

    fn apply_session(&mut self, name: &str, file: SessionFile) {
        let rc: std::collections::HashMap<u32, (Rc<EvolverConfig>, Rc<Target>)> =
            file.runs.iter().map(|r| (r.run, (Rc::new(r.settings.clone()), Rc::new(r.target.clone())))).collect();
        self.results = file
            .results
            .into_iter()
            .filter_map(|s| {
                let (cfg, target) = rc.get(&s.run)?.clone();
                Some(ResultEntry { r: s.result, cfg, target, run: s.run, secs: s.secs })
            })
            .collect();
        self.run_no = file.runs.iter().map(|r| r.run).max().unwrap_or(0);
        let active: f64 = file.runs.iter().map(|r| r.active_secs).sum();
        let elapsed: f64 = file.runs.iter().map(|r| r.elapsed_secs).sum();
        self.runs = file.runs;
        self.loaded_times = Some((active, elapsed));
        self.run_finished = self.results.len() as u32;
        self.run_successes = self.results.iter().filter(|e| e.r.viable).count() as u32;
        self.run_failed = 0;
        self.last_success = None;
        self.unsaved_successes = 0;
        self.selection = None;
        self.results_selected = None;
        self.model.cache = None;
        self.grid_cache.clear();
        self.grid_page = 0;
        self.runner = Runner::new(self.runner_ctx.clone());
        self.tab = Tab::Grid;
        self.set_status(format!(
            "Loaded session {name}: {} successes of {} experiments ({})",
            self.run_successes,
            self.run_finished,
            file.created_by
        ));
    }

    /// Ask before a loaded session replaces results that were never saved.
    fn session_confirm(&mut self, ctx: &egui::Context) {
        let Some((name, _)) = &self.pending_session else { return };
        let name = name.clone();
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("session_confirm")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("Replace the current results?").size(17.0).family(bold()));
            ui.add_space(6.0);
            ui.label(format!(
                "Loading {name} replaces the {} current results, {} of them successes not yet saved.",
                self.results.len(),
                self.unsaved_successes
            ));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Save the current session first...").clicked() {
                    choice = Some(0);
                }
                if ui.button("Replace").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(2);
                }
            });
        });
        match choice {
            Some(0) => {
                self.save_session();
                if let Some((name, file)) = self.pending_session.take() {
                    self.apply_session(&name, file);
                }
            }
            Some(1) => {
                if let Some((name, file)) = self.pending_session.take() {
                    self.apply_session(&name, file);
                }
            }
            Some(2) => self.pending_session = None,
            _ => {}
        }
    }
}

/// In the browser, ask before the page is reloaded or closed while `guard`
/// is set (results would be lost). Does nothing on the desktop.
fn install_leave_warning(guard: Rc<std::cell::Cell<bool>>) {
    #[cfg(target_arch = "wasm32")]
    {
        use eframe::wasm_bindgen::JsCast;
        use eframe::wasm_bindgen::prelude::Closure;
        let Some(window) = web_sys::window() else { return };
        let handler = Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(move |e: web_sys::BeforeUnloadEvent| {
            if guard.get() {
                // Browsers show their own generic "Leave site?" message.
                e.prevent_default();
                e.set_return_value("Results that have not been saved will be lost.");
            }
        });
        window.set_onbeforeunload(Some(handler.as_ref().unchecked_ref()));
        handler.forget();
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = guard;
}

/// The bold font family (Noto Sans Bold).
pub fn bold() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

/// Noto Sans (regular, and bold as its own family) instead of egui's thin
/// Ubuntu Light. The fonts are under the SIL Open Font License
/// (assets/fonts/NotoSans-OFL.txt).
fn install_fonts(ctx: &egui::Context) {
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSans-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "noto-bold".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSans-Bold.ttf"))),
    );
    if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        prop.insert(0, "noto".into());
    }
    let mut bold_chain = vec!["noto-bold".to_string()];
    bold_chain.extend(fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(bold(), bold_chain);
    ctx.set_fonts(fonts);
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
        v.override_text_color = Some(Color32::from_gray(25));
        v.hyperlink_color = Color32::from_rgb(0x1d, 0x5f, 0xc4);
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
        self.leave_guard.set(!self.is_idle() || self.unsaved_successes > 0);
        if self.runner.started_at.is_some()
            && let Some(rec) = self.runs.iter_mut().find(|r| r.run == self.run_no)
        {
            rec.active_secs = self.runner.active_secs();
            rec.elapsed_secs = self.runner.elapsed_secs();
            rec.experiments = self.run_finished;
            rec.successes = self.run_successes;
        }
        self.session_confirm(ui.ctx());
        if !self.is_idle() {
            // Keep the elapsed time and "last success" counters moving.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }

        self.about_window(ui.ctx());
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

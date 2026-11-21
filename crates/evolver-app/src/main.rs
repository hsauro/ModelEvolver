//! ModelEvolver app. M0: a solver spike that plots a known oscillator simulated
//! with diffsol BDF, and benchmarks fitness evaluations across all cores.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod executor;

use eframe::egui;
use egui_plot::{Legend, Line, Plot, PlotPoints};
use evolver_app::tasks::{Reply, Request};
use evolver_core::objective::oscillation_fitness;
use evolver_core::samples::{PASCAL_INITIAL, pascal_oscillator};
use evolver_core::simulate::{SimOptions, Trajectory};
use executor::{Executor, hardware_threads};

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 750.0]).with_title("ModelEvolver"),
        ..Default::default()
    };
    eframe::run_native("ModelEvolver", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window().expect("no window").document().expect("no document");
        let canvas = document
            .get_element_by_id("the_canvas_id")
            .expect("no canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("not a canvas");
        let result = eframe::WebRunner::new()
            .start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(App::new(cc)))))
            .await;
        if let Some(loading) = document.get_element_by_id("loading_text") {
            match result {
                Ok(()) => loading.remove(),
                Err(e) => loading.set_inner_html(&format!("The app crashed: {e:?}")),
            }
        }
    });
}

struct SimResult {
    traj: Trajectory,
    scores: Vec<f64>,
    fitness: f64,
    ms: f64,
}

/// One benchmark reply: worker, evals, ok, bad, milliseconds.
type BenchRow = (u32, u32, u32, u32, f64);

#[derive(Default)]
struct BenchRun {
    started: Option<web_time::Instant>,
    expected: usize,
    replies: Vec<BenchRow>,
    wall_ms: f64,
}

struct App {
    executor: Executor,
    n_workers: usize,
    evals_per_worker: u32,
    sim: Option<Result<SimResult, String>>,
    bench: BenchRun,
    errors: Vec<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let n = hardware_threads();
        Self {
            executor: Executor::new(cc.egui_ctx.clone(), n),
            n_workers: n,
            evals_per_worker: 500,
            sim: None,
            bench: BenchRun::default(),
            errors: Vec::new(),
        }
    }

    fn run_oscillator(&mut self) {
        let start = web_time::Instant::now();
        let r = oscillation_fitness(&pascal_oscillator(), &PASCAL_INITIAL, &SimOptions::default());
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        self.sim = Some(
            r.map(|(fitness, scores, traj)| SimResult { traj, scores, fitness, ms }).map_err(|e| e.to_string()),
        );
    }

    fn start_bench(&mut self) {
        self.bench = BenchRun { started: Some(web_time::Instant::now()), expected: self.n_workers, ..Default::default() };
        for w in 0..self.n_workers as u32 {
            self.executor.submit(Request::Bench { worker: w, seed: 1000 + u64::from(w), n_evals: self.evals_per_worker });
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("1. Simulate a known oscillator");
        ui.label("An oscillator evolved by the Pascal version, simulated with diffsol BDF (t = 0..150, 2048 points).");
        if ui.button("Simulate").clicked() {
            self.run_oscillator();
        }
        match &self.sim {
            Some(Ok(r)) => {
                ui.label(format!("Time: {:.1} ms", r.ms));
                ui.label(format!("Fitness (1 - best F): {:.4}", r.fitness));
                let s: Vec<String> = r.scores.iter().map(|f| format!("{f:.3}")).collect();
                ui.label(format!("F per species: {}", s.join(", ")));
            }
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::RED, e);
            }
            None => {}
        }

        ui.separator();
        ui.heading("2. Parallel benchmark");
        ui.label("Each worker evaluates random networks (simulate + oscillation score).");
        ui.label(format!("Hardware threads: {}", hardware_threads()));
        let running = self.bench.started.is_some() && self.bench.replies.len() < self.bench.expected;
        ui.add_enabled_ui(!running, |ui| {
            ui.horizontal(|ui| {
                ui.label("Workers:");
                ui.add(egui::DragValue::new(&mut self.n_workers).range(1..=hardware_threads()));
            });
            ui.horizontal(|ui| {
                ui.label("Evaluations per worker:");
                ui.add(egui::DragValue::new(&mut self.evals_per_worker).range(10..=100_000));
            });
            if ui.button("Run benchmark").clicked() {
                self.start_bench();
            }
        });
        if running {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("{} / {} workers done", self.bench.replies.len(), self.bench.expected));
            });
        }
        if !self.bench.replies.is_empty() {
            let total: u32 = self.bench.replies.iter().map(|r| r.1).sum();
            let n = self.bench.replies.len() as f64;
            let per_worker: f64 = self.bench.replies.iter().map(|r| f64::from(r.1) / (r.4 / 1000.0)).sum::<f64>() / n;
            ui.label(format!("Per worker: {per_worker:.0} evals/s"));
            if !running {
                let secs = self.bench.wall_ms / 1000.0;
                ui.label(format!("Total: {total} evals in {secs:.2} s = {:.0} evals/s", f64::from(total) / secs));
            }
            egui::Grid::new("bench").striped(true).show(ui, |ui| {
                for h in ["worker", "evals", "ok", "bad", "ms"] {
                    ui.label(h);
                }
                ui.end_row();
                let mut rows = self.bench.replies.clone();
                rows.sort_by_key(|r| r.0);
                for (w, e, ok, bad, ms) in rows {
                    ui.label(w.to_string());
                    ui.label(e.to_string());
                    ui.label(ok.to_string());
                    ui.label(bad.to_string());
                    ui.label(format!("{ms:.0}"));
                    ui.end_row();
                }
            });
        }
        for e in &self.errors {
            ui.colored_label(egui::Color32::RED, e);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        for reply in self.executor.poll() {
            match reply {
                Reply::Bench { worker, evals, ok, bad, ms } => {
                    self.bench.replies.push((worker, evals, ok, bad, ms));
                    if self.bench.replies.len() == self.bench.expected
                        && let Some(s) = self.bench.started
                    {
                        self.bench.wall_ms = s.elapsed().as_secs_f64() * 1000.0;
                    }
                }
                Reply::Error(e) => self.errors.push(e),
            }
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.heading("ModelEvolver - M0 solver spike (diffsol BDF)");
        });

        egui::Panel::left("controls").default_size(360.0).min_size(300.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(Ok(r)) = &self.sim {
                Plot::new("timecourse").legend(Legend::default()).x_axis_label("time").show(ui, |plot| {
                    for (j, col) in r.traj.cols.iter().enumerate() {
                        let pts: PlotPoints = r.traj.t.iter().zip(col).map(|(&t, &y)| [t, y]).collect();
                        plot.line(Line::new(format!("S{j} (F = {:.2})", r.scores[j]), pts));
                    }
                });
            } else {
                ui.centered_and_justified(|ui| ui.label("Press Simulate to plot the oscillator."));
            }
        });
    }
}

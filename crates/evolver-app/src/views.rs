//! The four tabs: Progress, Results, Grid and Model.

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use egui_plot::{HLine, Legend, Line, Plot, PlotPoints, Points};

use evolver_core::config::{EvolverConfig, ObjectiveKind};
use evolver_core::export::network_antimony;
use evolver_core::ga::{ExperimentResult, StopReason};
use evolver_core::network::Network;
use evolver_core::objective::{RunContext, Target};
use evolver_core::simulate::{Trajectory, simulate};

use crate::app::{ACCENT, App, ResultEntry, Selection, SortCol, SUCCESS, WARN, format_duration};
use crate::runner::RunState;

/// Distinct colours for species S0, S1, ... (by species index).
const PALETTE: [Color32; 12] = [
    Color32::from_rgb(0x1f, 0x77, 0xb4),
    Color32::from_rgb(0xff, 0x7f, 0x0e),
    Color32::from_rgb(0x2c, 0xa0, 0x2c),
    Color32::from_rgb(0xd6, 0x27, 0x28),
    Color32::from_rgb(0x94, 0x67, 0xbd),
    Color32::from_rgb(0x8c, 0x56, 0x4b),
    Color32::from_rgb(0xe3, 0x77, 0xc2),
    Color32::from_rgb(0x7f, 0x7f, 0x7f),
    Color32::from_rgb(0xbc, 0xbd, 0x22),
    Color32::from_rgb(0x17, 0xbe, 0xcf),
    Color32::from_rgb(0x39, 0x3b, 0x79),
    Color32::from_rgb(0xad, 0x49, 0x4a),
];

pub fn species_color(i: usize) -> Color32 {
    PALETTE[i % PALETTE.len()]
}

fn stop_text(r: StopReason) -> &'static str {
    match r {
        StopReason::Converged => "converged",
        StopReason::MaxGenerations => "max generations",
        StopReason::Stagnated => "stagnated",
        StopReason::Extinct => "extinct",
        StopReason::Abandoned => "given up",
    }
}

/// Species index of each state variable of `net`.
fn state_species(net: &Network) -> Vec<usize> {
    net.floating_species().collect()
}

/// Lines for a trajectory; `only` limits to one state index.
fn add_lines(plot: &mut egui_plot::PlotUi, traj: &Trajectory, net: &Network, best: Option<usize>, only: Option<usize>, hidden: &std::collections::BTreeSet<usize>) {
    let species = state_species(net);
    for (j, col) in traj.cols.iter().enumerate() {
        if only.is_some_and(|o| o != j) {
            continue;
        }
        let sp = species.get(j).copied().unwrap_or(j);
        if hidden.contains(&sp) {
            continue;
        }
        let pts: PlotPoints = traj.t.iter().zip(col).map(|(&t, &y)| [t, y]).collect();
        let width = if best == Some(j) { 2.6 } else { 1.4 };
        plot.line(Line::new(format!("S{sp}"), pts).color(species_color(sp)).width(width));
    }
}

fn target_points(target: &Target) -> Points<'static> {
    let step = target.sim_t_end() / (target.n_points().max(2) - 1) as f64;
    let pts: Vec<[f64; 2]> = target.values.iter().enumerate().map(|(i, v)| [i as f64 * step, *v]).collect();
    Points::new("target", pts).radius(4.0).color(Color32::BLACK).filled(false)
}

impl App {
    // ------------------------------------------------------------------ Progress

    pub fn progress_tab(&mut self, ui: &mut egui::Ui) {
        if self.runner.live.is_empty() && self.runner.state == RunState::Idle {
            self.welcome(ui);
            return;
        }
        let max_gen = self.run_settings().map_or(self.cfg.max_generations, |(c, _)| c.max_generations);
        let live: Vec<u32> = self.runner.live.keys().copied().collect();
        let table_h = (ui.available_height() * 0.45).max(140.0);
        let mut open_live = None;
        TableBuilder::new(ui)
            .id_salt("live")
            .striped(true)
            .sense(egui::Sense::click())
            .max_scroll_height(table_h)
            .column(Column::exact(90.0))
            .column(Column::initial(200.0).at_least(120.0))
            .column(Column::exact(110.0))
            .column(Column::exact(80.0))
            .column(Column::exact(80.0))
            .column(Column::exact(80.0))
            .column(Column::remainder())
            .header(24.0, |mut h| {
                for t in ["Experiment", "Generation", "Best fitness", "Reactions", "Species", "Time", ""] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(26.0, live.len(), |mut row| {
                    let id = live[row.index()];
                    let Some(l) = self.runner.live.get(&id) else { return };
                    row.set_selected(self.progress_selected == Some(id));
                    let p = l.progress.as_ref();
                    row.col(|ui| {
                        ui.label(format!("#{id}"));
                    });
                    row.col(|ui| {
                        let g = p.map_or(0, |p| p.generation);
                        ui.label(format!("{g:>5}"));
                        ui.add(egui::ProgressBar::new(g as f32 / max_gen.max(1) as f32).desired_height(6.0).desired_width(110.0));
                    });
                    row.col(|ui| {
                        ui.label(p.map_or("-".into(), |p| format!("{:.4}", p.best_fitness)));
                    });
                    row.col(|ui| {
                        ui.label(p.map_or("-".into(), |p| p.n_reactions.to_string()));
                    });
                    row.col(|ui| {
                        ui.label(p.map_or("-".into(), |p| p.n_float.to_string()));
                    });
                    row.col(|ui| {
                        ui.label(format_duration(l.started.elapsed().as_secs_f64()));
                    });
                    row.col(|ui| {
                        if ui.add_enabled(l.best_network.is_some(), egui::Button::new("View best").small()).clicked() {
                            open_live = Some(id);
                        }
                    });
                    if row.response().clicked() {
                        self.progress_selected = Some(id);
                    }
                });
            });
        if let Some(id) = open_live {
            self.open_live(id);
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.strong("Best fitness of running experiments");
            ui.checkbox(&mut self.prefs.progress_log_y, "log scale");
            if self.runner.live.is_empty() {
                ui.label(RichText::new("(waiting for running experiments to finish)").weak());
            }
        });
        let log = self.prefs.progress_log_y;
        let tf = |v: f64| if log { v.max(1e-6).log10() } else { v };
        let (stop, viable) = self.run_settings().map_or((0.05, 0.3), |(c, _)| match c.objective {
            ObjectiveKind::Oscillation => (c.osc_stop_threshold, c.osc_viable_threshold),
            ObjectiveKind::TimeCourse => (c.ssd_stop_threshold, c.ssd_viable_threshold),
        });
        let selected = self.progress_selected;
        Plot::new("live_hist")
            .legend(Legend::default())
            .x_axis_label("generation")
            .y_axis_label(if log { "log10 best fitness" } else { "best fitness" })
            .show(ui, |plot| {
                plot.hline(HLine::new("viable", tf(viable)).color(SUCCESS).width(1.0));
                plot.hline(HLine::new("converged", tf(stop)).color(ACCENT).width(1.0));
                for l in self.runner.live.values() {
                    let pts: PlotPoints = l.history.iter().map(|[g, f]| [*g, tf(*f)]).collect();
                    let sel = selected == Some(l.id);
                    let line = Line::new(format!("#{}", l.id), pts).width(if sel { 2.5 } else { 1.0 });
                    plot.line(if sel { line.color(WARN) } else { line });
                }
            });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Evolve chemical reaction networks").size(24.0).strong());
            ui.add_space(8.0);
            ui.label(
                "ModelEvolver uses a genetic algorithm to evolve mass-action reaction networks that oscillate,\n\
                 or that follow a target time course. Each candidate is simulated with an adaptive stiff (BDF) solver.",
            );
            ui.add_space(16.0);
            ui.label(RichText::new("1. Choose the objective and settings on the left (the defaults work well).").size(15.0));
            ui.label(RichText::new("2. Set how much of the computer to use (CPU usage, top).").size(15.0));
            ui.label(RichText::new("3. Press Start. Successful models appear under Results and Grid.").size(15.0));
            ui.add_space(16.0);
            if !self.results.is_empty() {
                ui.label(format!("{} results from earlier runs are under Results and Grid.", self.results.len()));
            }
        });
    }

    // ------------------------------------------------------------------ Results

    fn sorted_results(&self, viable_only: bool) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.results.len()).filter(|&i| !viable_only || self.results[i].r.viable).collect();
        let (col, asc) = self.results_sort;
        let key = |e: &ResultEntry| -> f64 {
            match col {
                SortCol::Id => f64::from(e.run) * 1e9 + f64::from(e.r.id),
                SortCol::Fitness => e.r.fitness,
                SortCol::Reactions => e.r.reduced.reactions.len() as f64,
                SortCol::Generations => f64::from(e.r.generations),
                SortCol::Time => e.secs,
            }
        };
        idx.sort_by(|&a, &b| {
            let o = key(&self.results[a]).total_cmp(&key(&self.results[b]));
            if asc { o } else { o.reverse() }
        });
        idx
    }

    fn export_name(e: &ResultEntry) -> String {
        format!("model_run{}_exp{}_fit{:.4}.txt", e.run, e.r.id, e.r.fitness)
    }

    pub fn results_tab(&mut self, ui: &mut egui::Ui) {
        if self.results.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("No finished experiments yet.").size(16.0).weak()));
            return;
        }
        let n_viable = self.results.iter().filter(|e| e.r.viable).count();
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.prefs.results_viable_only, "Successes only");
            ui.label(RichText::new(format!("{n_viable} successes of {} experiments", self.results.len())).weak());
            ui.separator();
            let sel = self.results_selected;
            if ui.add_enabled(sel.is_some(), egui::Button::new("Open")).clicked()
                && let Some(i) = sel
            {
                self.open_result(i);
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new("Save model...")).clicked()
                && let Some(i) = sel
            {
                let e = &self.results[i];
                if let Some(msg) = self.files.save_text(&Self::export_name(e), &e.r.antimony) {
                    self.set_status(msg);
                }
            }
            if ui
                .add_enabled(n_viable > 0, egui::Button::new("Save all successes..."))
                .on_hover_text("Write every successful model as an Antimony text file")
                .clicked()
            {
                let files: Vec<(String, String)> =
                    self.results.iter().filter(|e| e.r.viable).map(|e| (Self::export_name(e), e.r.antimony.clone())).collect();
                if let Some(msg) = self.files.save_many("modelevolver-models.txt", &files) {
                    self.set_status(msg);
                }
            }
            if ui.button("Save summary (CSV)...").clicked() {
                let mut csv = String::from("run,experiment,viable,fitness,evolved_fitness,reduced_fitness,reactions,reactions_before,floating,generations,stop,seconds\n");
                for e in &self.results {
                    let r = &e.r;
                    csv.push_str(&format!(
                        "{},{},{},{},{},{},{},{},{},{},{},{:.1}\n",
                        e.run, r.id, r.viable, r.fitness, r.evolved_fitness, r.reduced_fitness, r.reduced.reactions.len(),
                        r.best.reactions.len(), r.reduced.n_float, r.generations, stop_text(r.stop_reason), e.secs
                    ));
                }
                if let Some(msg) = self.files.save_text("modelevolver-results.csv", &csv) {
                    self.set_status(msg);
                }
            }
        });
        ui.add_space(4.0);

        let rows = self.sorted_results(self.prefs.results_viable_only);
        let mut clicked_sort = None;
        let mut open = None;
        let sort = self.results_sort;
        let header = |ui: &mut egui::Ui, title: &str, col: Option<SortCol>, clicked: &mut Option<SortCol>| {
            let arrow = match col {
                Some(c) if c == sort.0 => if sort.1 { " ^" } else { " v" },
                _ => "",
            };
            let r = ui.add(egui::Label::new(RichText::new(format!("{title}{arrow}")).strong()).sense(egui::Sense::click()));
            if let Some(c) = col
                && r.clicked()
            {
                *clicked = Some(c);
            }
        };
        TableBuilder::new(ui)
            .id_salt("results")
            .striped(true)
            .sense(egui::Sense::click())
            .column(Column::exact(90.0))
            .column(Column::exact(80.0))
            .column(Column::exact(95.0))
            .column(Column::exact(95.0))
            .column(Column::exact(95.0))
            .column(Column::exact(75.0))
            .column(Column::exact(95.0))
            .column(Column::exact(120.0))
            .column(Column::remainder())
            .header(24.0, |mut h| {
                h.col(|ui| header(ui, "Experiment", Some(SortCol::Id), &mut clicked_sort));
                h.col(|ui| header(ui, "Result", None, &mut clicked_sort));
                h.col(|ui| header(ui, "Fitness", Some(SortCol::Fitness), &mut clicked_sort));
                h.col(|ui| header(ui, "Reduced", None, &mut clicked_sort));
                h.col(|ui| header(ui, "Reactions", Some(SortCol::Reactions), &mut clicked_sort));
                h.col(|ui| header(ui, "Species", None, &mut clicked_sort));
                h.col(|ui| header(ui, "Generations", Some(SortCol::Generations), &mut clicked_sort));
                h.col(|ui| header(ui, "Stopped", None, &mut clicked_sort));
                h.col(|ui| header(ui, "Time", Some(SortCol::Time), &mut clicked_sort));
            })
            .body(|body| {
                body.rows(24.0, rows.len(), |mut row| {
                    let i = rows[row.index()];
                    let e = &self.results[i];
                    let r = &e.r;
                    row.set_selected(self.results_selected == Some(i));
                    row.col(|ui| {
                        ui.label(format!("{}.{}", e.run, r.id));
                    });
                    row.col(|ui| {
                        if r.viable {
                            ui.label(RichText::new("success").color(SUCCESS).strong());
                        } else {
                            ui.label(RichText::new("no").weak());
                        }
                    });
                    row.col(|ui| {
                        ui.label(fmt_fit(r.fitness));
                    });
                    row.col(|ui| {
                        ui.label(fmt_fit(r.reduced_fitness));
                    });
                    row.col(|ui| {
                        ui.label(format!("{} ({})", r.reduced.reactions.len(), r.best.reactions.len()));
                    });
                    row.col(|ui| {
                        ui.label(r.reduced.n_float.to_string());
                    });
                    row.col(|ui| {
                        ui.label(r.generations.to_string());
                    });
                    row.col(|ui| {
                        ui.label(stop_text(r.stop_reason));
                    });
                    row.col(|ui| {
                        ui.label(format_duration(e.secs));
                    });
                    let resp = row.response();
                    if resp.clicked() {
                        self.results_selected = Some(i);
                    }
                    if resp.double_clicked() {
                        open = Some(i);
                    }
                });
            });
        if let Some(c) = clicked_sort {
            self.results_sort = if self.results_sort.0 == c { (c, !self.results_sort.1) } else { (c, true) };
        }
        if let Some(i) = open {
            self.open_result(i);
        }
    }

    // ------------------------------------------------------------------ Grid

    pub fn grid_tab(&mut self, ui: &mut egui::Ui) {
        let rows = {
            let mut idx: Vec<usize> =
                (0..self.results.len()).filter(|&i| self.prefs.grid_include_failed || self.results[i].r.viable).collect();
            idx.sort_by(|&a, &b| self.results[a].r.fitness.total_cmp(&self.results[b].r.fitness));
            idx
        };
        ui.horizontal(|ui| {
            ui.label("Columns");
            ui.add(egui::Slider::new(&mut self.prefs.grid_cols, 1..=8));
            ui.label("Rows per page");
            ui.add(egui::Slider::new(&mut self.prefs.grid_rows, 1..=8));
            ui.checkbox(&mut self.prefs.grid_all_species, "All species")
                .on_hover_text("Show every floating species instead of only the best-scoring one");
            ui.checkbox(&mut self.prefs.grid_include_failed, "Include non-successes");
        });
        if rows.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("Successful models will appear here.").size(16.0).weak()));
            return;
        }
        let per_page = self.prefs.grid_cols * self.prefs.grid_rows;
        let pages = rows.len().div_ceil(per_page);
        self.grid_page = self.grid_page.min(pages - 1);
        ui.horizontal(|ui| {
            if ui.add_enabled(self.grid_page > 0, egui::Button::new("< Previous")).clicked() {
                self.grid_page -= 1;
            }
            ui.label(format!("Page {} of {pages}  ({} models, best first)", self.grid_page + 1, rows.len()));
            if ui.add_enabled(self.grid_page + 1 < pages, egui::Button::new("Next >")).clicked() {
                self.grid_page += 1;
            }
            ui.label(RichText::new("Click a plot to open the model").weak());
        });
        ui.add_space(4.0);

        let cols = self.prefs.grid_cols;
        let spacing = 8.0;
        let cell_w = (ui.available_width() - spacing * (cols as f32 - 1.0)) / cols as f32;
        let cell_h = ((ui.available_height() - 40.0) / self.prefs.grid_rows as f32 - 36.0).clamp(80.0, cell_w * 0.75);
        let page: Vec<usize> = rows.iter().skip(self.grid_page * per_page).take(per_page).copied().collect();
        let mut open = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for chunk in page.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = spacing;
                    for &i in chunk {
                        let e = &self.results[i];
                        ui.allocate_ui(egui::vec2(cell_w, cell_h + 32.0), |ui| {
                            egui::Frame::group(ui.style()).inner_margin(4).show(ui, |ui| {
                                ui.set_width(cell_w - 10.0);
                                ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("#{}.{}", e.run, e.r.id)).strong());
                                    let c = if e.r.viable { SUCCESS } else { WARN };
                                    ui.label(RichText::new(fmt_fit(e.r.fitness)).color(c));
                                    ui.label(RichText::new(format!("{} rxn", e.r.reduced.reactions.len())).weak());
                                });
                                if let Some(traj) = &e.r.display {
                                    let only = (!self.prefs.grid_all_species).then_some(e.r.best_node);
                                    let resp = Plot::new(("grid", e.run, e.r.id))
                                        .height(cell_h)
                                        .show_axes([false, true])
                                        .allow_drag(false)
                                        .allow_zoom(false)
                                        .allow_scroll(false)
                                        .allow_boxed_zoom(false)
                                        .show_x(false)
                                        .show_y(false)
                                        .show(ui, |p| {
                                            add_lines(p, traj, &e.r.reduced, Some(e.r.best_node), only, &Default::default());
                                            if let Some(b) = tail_bounds(traj, only) {
                                                p.set_plot_bounds(b);
                                            }
                                        })
                                        .response;
                                    if resp.clicked() {
                                        open = Some(i);
                                    }
                                    resp.on_hover_text("Open this model");
                                } else {
                                    ui.label("(no trajectory)");
                                }
                                });
                            });
                        });
                    }
                });
                ui.add_space(spacing);
            }
        });
        if let Some(i) = open {
            self.open_result(i);
        }
    }

    // ------------------------------------------------------------------ Model

    pub fn model_tab(&mut self, ui: &mut egui::Ui) {
        // Resolve what to show: network, settings, Antimony text, trajectory source.
        let (net, cfg, target, text, title, result): (Network, std::rc::Rc<EvolverConfig>, std::rc::Rc<Target>, String, String, Option<ExperimentResult>) =
            match self.selection {
                None => {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No model selected.").size(16.0).weak());
                        ui.label(RichText::new("Open one from Results or Grid, or 'View best' on a running experiment.").weak());
                    });
                    return;
                }
                Some(Selection::Result(i)) => {
                    let Some(e) = self.results.get(i) else { return };
                    let net = if self.model.show_reduced { e.r.reduced.clone() } else { e.r.best.clone() };
                    let (ctx, _) = RunContext::new(&e.cfg, (*e.target).clone());
                    let text = if self.model.show_reduced { e.r.antimony.clone() } else { network_antimony(&net, &ctx.init) };
                    let title = format!(
                        "Experiment {}.{}  -  fitness {} (reduced model {})  -  {} reactions, {} floating species  -  {} after {} generations",
                        e.run,
                        e.r.id,
                        fmt_fit(e.r.fitness),
                        fmt_fit(e.r.reduced_fitness),
                        net.reactions.len(),
                        net.n_float,
                        stop_text(e.r.stop_reason),
                        e.r.generations
                    );
                    (net, e.cfg.clone(), e.target.clone(), text, title, Some(e.r.clone()))
                }
                Some(Selection::Live(id)) => {
                    let Some((cfg, target)) = self.run_settings() else { return };
                    let Some(net) = self.runner.live.get(&id).and_then(|l| l.best_network.clone()) else {
                        ui.add_space(20.0);
                        ui.label(RichText::new(format!("Experiment {id} has finished; its result is under Results.")).weak());
                        return;
                    };
                    let (ctx, _) = RunContext::new(&cfg, (*target).clone());
                    let text = network_antimony(&net, &ctx.init);
                    let title = format!("Running experiment {id}  -  current best network ({} reactions)", net.reactions.len());
                    (net, cfg, target, text, title, None)
                }
            };

        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong());
        });
        ui.horizontal(|ui| {
            if result.is_some() {
                let before = self.model.show_reduced;
                ui.selectable_value(&mut self.model.show_reduced, true, "Reduced model");
                ui.selectable_value(&mut self.model.show_reduced, false, "Before reduction");
                if before != self.model.show_reduced {
                    self.model.cache = None;
                }
                ui.separator();
            }
            if ui.button("Copy Antimony").clicked() {
                ui.ctx().copy_text(text.clone());
                self.set_status("Antimony copied to the clipboard");
            }
            if ui.button("Save Antimony...").clicked() {
                let name = match &result {
                    Some(r) => format!("model_exp{}_fit{:.4}.txt", r.id, r.fitness),
                    None => "model.txt".into(),
                };
                if let Some(msg) = self.files.save_text(&name, &text) {
                    self.set_status(msg);
                }
            }
            if let Some(Selection::Result(cur)) = self.selection {
                ui.separator();
                let order: Vec<usize> = self.sorted_results(true);
                let pos = order.iter().position(|&i| i == cur);
                if ui.add_enabled(pos.is_some_and(|p| p > 0), egui::Button::new("< Previous")).clicked()
                    && let Some(p) = pos
                {
                    self.open_result(order[p - 1]);
                }
                if ui.add_enabled(pos.is_some_and(|p| p + 1 < order.len()), egui::Button::new("Next >")).clicked()
                    && let Some(p) = pos
                {
                    self.open_result(order[p + 1]);
                }
            }
        });
        ui.separator();

        let default_window = match cfg.objective {
            ObjectiveKind::Oscillation if cfg.osc_confirm_t_end > 0.0 => cfg.osc_confirm_t_end,
            ObjectiveKind::Oscillation => cfg.osc_t_end,
            ObjectiveKind::TimeCourse => target.sim_t_end(),
        };
        let text_w = (ui.available_width() * 0.38).clamp(280.0, 560.0);
        ui.horizontal_top(|ui| {
            // Antimony text
            ui.allocate_ui(egui::vec2(text_w, ui.available_height()), |ui| {
                ui.vertical(|ui| {
                    ui.strong("Antimony");
                    egui::ScrollArea::both().id_salt("ant").auto_shrink([false, false]).show(ui, |ui| {
                        let mut t = text.as_str();
                        ui.add(egui::TextEdit::multiline(&mut t).code_editor().desired_width(f32::INFINITY));
                    });
                });
            });
            ui.separator();
            // Plot and controls
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("Time course");
                    ui.checkbox(&mut self.model.custom_window, "Custom time span");
                    if self.model.custom_window {
                        let a = ui.add(egui::DragValue::new(&mut self.model.t_end).range(0.1..=1e6).prefix("end time "));
                        let b = ui.add(egui::DragValue::new(&mut self.model.n_points).range(10..=100_000).prefix("points "));
                        if a.changed() || b.changed() {
                            self.model.cache = None;
                        }
                    } else {
                        ui.label(RichText::new(format!("t = 0..{default_window}")).weak());
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Species:");
                    for sp in state_species(&net) {
                        let mut shown = !self.model.hidden.contains(&sp);
                        let label = RichText::new(format!("S{sp}")).color(species_color(sp));
                        if ui.checkbox(&mut shown, label).changed() {
                            if shown {
                                self.model.hidden.remove(&sp);
                            } else {
                                self.model.hidden.insert(sp);
                            }
                        }
                    }
                });

                // Trajectory: the stored display trajectory when possible, else simulate now.
                let stored = result.as_ref().filter(|_| self.model.show_reduced && !self.model.custom_window).and_then(|r| r.display.clone());
                let traj: Result<Trajectory, String> = match stored {
                    Some(t) => Ok(t),
                    None => {
                        let (t_end, n) = if self.model.custom_window { (self.model.t_end, self.model.n_points) } else { (default_window, 1000) };
                        let key = format!("{:?}|{}|{}|{t_end}|{n}", self.selection, self.model.show_reduced, net.id);
                        let live = matches!(self.selection, Some(Selection::Live(_)));
                        match &self.model.cache {
                            Some((k, t)) if *k == key && !live => t.clone(),
                            _ => {
                                let (ctx, _) = RunContext::new(&cfg, (*target).clone());
                                let t = simulate(&net.compile(&ctx.init), &cfg.sim_options(t_end, n)).map_err(|e| e.to_string());
                                self.model.cache = Some((key, t.clone()));
                                t
                            }
                        }
                    }
                };
                let best = result.as_ref().filter(|_| self.model.show_reduced).map(|r| r.best_node);
                match traj {
                    Ok(traj) => {
                        let hidden = self.model.hidden.clone();
                        let show_target = cfg.objective == ObjectiveKind::TimeCourse;
                        Plot::new("model_plot").legend(Legend::default()).x_axis_label("time").y_axis_label("concentration").show(
                            ui,
                            |p| {
                                add_lines(p, &traj, &net, best, None, &hidden);
                                if show_target {
                                    p.points(target_points(&target));
                                }
                            },
                        );
                    }
                    Err(e) => {
                        ui.colored_label(Color32::RED, format!("Simulation failed: {e}"));
                    }
                }
            });
        });
    }
}

/// Plot bounds covering the whole time span and the y-range after the first
/// 40% (the transient), so an early spike does not flatten the oscillation.
fn tail_bounds(traj: &Trajectory, only: Option<usize>) -> Option<egui_plot::PlotBounds> {
    let n = traj.t.len();
    if n < 2 {
        return None;
    }
    let start = n * 2 / 5;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (j, col) in traj.cols.iter().enumerate() {
        if only.is_some_and(|o| o != j) {
            continue;
        }
        for &v in &col[start..] {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    let pad = ((hi - lo) * 0.08).max(1e-9);
    Some(egui_plot::PlotBounds::from_min_max([traj.t[0], lo - pad], [traj.t[n - 1], hi + pad]))
}

fn fmt_fit(f: f64) -> String {
    if f.is_finite() { format!("{f:.4}") } else { "failed".into() }
}

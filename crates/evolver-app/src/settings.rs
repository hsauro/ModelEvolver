//! The Settings panel: every option of the Pascal command line and config.ini,
//! grouped, with tooltips and inline validation.

use eframe::egui::{self, RichText};
use egui_plot::{Plot, Points};

use evolver_core::config::{EvolverConfig, ObjectiveKind};

use crate::app::{ACCENT, App};
use crate::files::OpenKind;

/// A labelled number field; returns true if changed.
fn num<T: egui::emath::Numeric>(ui: &mut egui::Ui, label: &str, v: &mut T, range: std::ops::RangeInclusive<T>, help: &str) -> bool {
    ui.horizontal(|ui| {
        let r = ui.add(egui::DragValue::new(v).range(range).speed(0.1));
        ui.label(label).on_hover_text(help);
        r.on_hover_text(help).changed()
    })
    .inner
}

/// A probability / fraction field with 0..=1 range.
fn frac(ui: &mut egui::Ui, label: &str, v: &mut f64, help: &str) -> bool {
    ui.horizontal(|ui| {
        let r = ui.add(egui::DragValue::new(v).range(0.0..=1.0).speed(0.005).fixed_decimals(3));
        ui.label(label).on_hover_text(help);
        r.on_hover_text(help).changed()
    })
    .inner
}

/// A min..max pair.
fn range_pair(ui: &mut egui::Ui, label: &str, lo: &mut usize, hi: &mut usize, max: usize, help: &str) -> bool {
    ui.horizontal(|ui| {
        let a = ui.add(egui::DragValue::new(lo).range(1..=max).speed(0.1)).changed();
        ui.label("to");
        let b = ui.add(egui::DragValue::new(hi).range(1..=max).speed(0.1)).changed();
        ui.label(label).on_hover_text(help);
        a || b
    })
    .inner
}

/// A small-number field shown in scientific notation.
fn sci(ui: &mut egui::Ui, label: &str, v: &mut f64, help: &str) -> bool {
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::DragValue::new(v)
                .range(1e-15..=1.0)
                .speed(0.0)
                .custom_formatter(|x, _| format!("{x:.0e}"))
                .custom_parser(|s| s.trim().parse::<f64>().ok()),
        );
        let mut changed = r.changed();
        if ui.small_button("x10").on_hover_text("Multiply by 10 (looser)").clicked() {
            *v = (*v * 10.0).min(1.0);
            changed = true;
        }
        if ui.small_button("/10").on_hover_text("Divide by 10 (tighter)").clicked() {
            *v = (*v / 10.0).max(1e-15);
            changed = true;
        }
        ui.label(label).on_hover_text(help);
        changed
    })
    .inner
}

fn section(ui: &mut egui::Ui, title: &str, open: bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).family(crate::app::bold())).default_open(open).show(ui, add);
}

impl App {
    pub fn settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Settings").size(19.0).family(crate::app::bold()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("More", |ui| {
                    if ui.button("Oscillator preset").on_hover_text("Settings that produced the Pascal oscillator corpus").clicked() {
                        self.set_preset(EvolverConfig::oscillator_preset(), "oscillator preset");
                        ui.close();
                    }
                    if ui.button("Pascal defaults").on_hover_text("The Pascal program's built-in defaults").clicked() {
                        self.set_preset(EvolverConfig::default(), "Pascal defaults");
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Import Pascal config.ini...").clicked() {
                        self.files.open(OpenKind::PascalIni, "Import a Pascal config.ini", &["ini"]);
                        ui.close();
                    }
                    if ui.button("Load configuration...").clicked() {
                        self.load_configuration();
                        ui.close();
                    }
                    if ui.button("Save configuration...").clicked() {
                        self.save_configuration();
                        ui.close();
                    }
                });
            });
        });
        ui.horizontal(|ui| {
            let idle = self.is_idle();
            if ui
                .add_enabled(idle, egui::Button::new("Load configuration..."))
                .on_hover_text("Load a configuration file (settings and target time course), e.g. one shared by a colleague")
                .clicked()
            {
                self.load_configuration();
            }
            if ui
                .button("Save configuration...")
                .on_hover_text(
                    "Save all settings and the target time course to a file you can share. With a fixed seed \
                     (untick Random seed) others get exactly the same results.",
                )
                .clicked()
            {
                self.save_configuration();
            }
        });
        if !self.is_idle() {
            ui.label(RichText::new("Settings are locked while a run is in progress (CPU usage can still be changed).").weak().small());
        }
        for e in &self.config_errors {
            ui.colored_label(egui::Color32::RED, format!("- {e}"));
        }
        ui.separator();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let idle = self.is_idle();
            let changed = ui.add_enabled_ui(idle, |ui| self.settings_body(ui)).inner;
            if changed {
                self.revalidate();
            }
        });
    }

    pub fn load_configuration(&mut self) {
        self.files.open(OpenKind::SettingsJson, "Load a ModelEvolver configuration", &["json"]);
    }

    pub fn save_configuration(&mut self) {
        let shared = evolver_core::objective::SharedConfig::new(self.cfg.clone(), self.target.clone(), self.target_name.clone());
        if let Some(msg) = self.files.save_text("modelevolver-configuration.json", &shared.to_json()) {
            self.set_status(msg);
        }
    }

    fn set_preset(&mut self, mut cfg: EvolverConfig, name: &str) {
        // Keep the user's machine-related choices.
        cfg.cpu_usage_percent = self.cfg.cpu_usage_percent;
        cfg.num_workers = self.cfg.num_workers;
        self.cfg = cfg;
        self.revalidate();
        self.set_status(format!("Settings reset to the {name}"));
    }

    fn settings_body(&mut self, ui: &mut egui::Ui) -> bool {
        let mut ch = false;
        let c = &mut self.cfg;

        section(ui, "Objective", true, |ui| {
            ch |= ui
                .radio_value(&mut c.objective, ObjectiveKind::Oscillation, "Sustained oscillation")
                .on_hover_text("Evolve networks with a clean, sustained oscillation in at least one species")
                .changed();
            ch |= ui
                .radio_value(&mut c.objective, ObjectiveKind::TimeCourse, "Match a target time course")
                .on_hover_text("Evolve networks in which one species follows the target time course")
                .changed();
        });
        if c.objective == ObjectiveKind::TimeCourse {
            ui.indent("target", |ui| {
                ui.label(format!(
                    "Target: {} ({} points, t = {}..{})",
                    self.target_name,
                    self.target.n_points(),
                    self.target.t_start,
                    self.target.t_end
                ));
                if ui.button("Load target...").on_hover_text("A file in the Pascal objectivefunction.txt format").clicked() {
                    self.files.open(OpenKind::Target, "Load a target time course", &["txt"]);
                }
                let step = self.target.sim_t_end() / (self.target.n_points().max(2) - 1) as f64;
                let pts: Vec<[f64; 2]> = self.target.values.iter().enumerate().map(|(i, v)| [i as f64 * step, *v]).collect();
                Plot::new("target_preview").height(90.0).show_axes([false, true]).allow_drag(false).allow_zoom(false).allow_scroll(false).show(
                    ui,
                    |p| p.points(Points::new("target", pts).radius(3.0).color(ACCENT)),
                );
            });
        }
        let c = &mut self.cfg;

        section(ui, "Run", true, |ui| {
            ui.horizontal(|ui| {
                ch |= ui.radio_value(&mut c.continuous, false, "Run").on_hover_text("Run a fixed number of experiments").changed();
                let r = ui
                    .add(egui::DragValue::new(&mut c.num_experiments).range(1..=100_000))
                    .on_hover_text("Number of experiments in the batch");
                if r.changed() {
                    c.continuous = false;
                    ch = true;
                }
                ui.label("experiments");
            });
            ch |= ui
                .radio_value(&mut c.continuous, true, "Run until stopped")
                .on_hover_text("Keep starting new experiments until you press Stop (best for producing many models)")
                .changed();
            ui.horizontal(|ui| {
                let mut random = c.seed == 0;
                if ui.checkbox(&mut random, "Random seed").on_hover_text("Pick a new seed for each run").changed() {
                    c.seed = if random { 0 } else { 1 };
                    ch = true;
                }
                if !random {
                    ch |= ui.add(egui::DragValue::new(&mut c.seed).range(1..=u32::MAX)).on_hover_text("Same seed = same results").changed();
                }
            });
            ch |= num(ui, "max generations", &mut c.max_generations, 1..=1_000_000, "Generations per experiment before it stops");
            ch |= num(ui, "population size", &mut c.max_population_size, 2..=100_000, "Networks per generation");
        });

        section(ui, "Give up on unpromising experiments", true, |ui| {
            let mut on = c.abandon_generation > 0;
            if ui
                .checkbox(&mut on, "Enabled")
                .on_hover_text(
                    "Stop an experiment that is still far from the goal after a number of generations, so a fresh \
                     one can start. About 2.7x more successes per minute in continuous mode.",
                )
                .changed()
            {
                c.abandon_generation = if on { 100 } else { 0 };
                ch = true;
            }
            if on {
                ch |= num(ui, "at generation", &mut c.abandon_generation, 1..=1_000_000, "When to check");
                ch |= frac(ui, "if fitness still above", &mut c.abandon_threshold, "Fitness is 1 - F; lower is better");
            }
        });

        match c.objective {
            ObjectiveKind::Oscillation => section(ui, "Oscillation scoring", false, |ui| {
                ch |= num(ui, "evolution window end time", &mut c.osc_t_end, 1.0..=1e6, "Simulated time per evaluation");
                ch |= num(ui, "evolution window points", &mut c.osc_n_points, 32..=65_536, "Samples per evaluation");
                ch |= frac(ui, "stop when fitness below", &mut c.osc_stop_threshold, "An experiment converges below this");
                ch |= frac(ui, "viable when fitness below", &mut c.osc_viable_threshold, "A model counts as a success below this");
                ch |= frac(ui, "reduction margin", &mut c.osc_reduce_margin, "How much worse a reduced model may be");
                ch |= ui
                    .checkbox(&mut c.reject_drift, "Reject species that grow without bound")
                    .on_hover_text(
                        "Reject a network if any species keeps rising through the simulation \
                         (slowly heading to infinity), even while another species oscillates",
                    )
                    .changed();
                if c.reject_drift {
                    ch |= frac(ui, "drift tolerance", &mut c.drift_tolerance, "Growth (fraction of the level) counted as drift");
                }
                ch |= num(ui, "confirmation end time", &mut c.osc_confirm_t_end, 0.0..=1e6, "Longer check of the best model (0 = off)");
                ch |= num(ui, "confirmation points", &mut c.osc_confirm_n_points, 32..=65_536, "Samples in the confirmation check");
            }),
            ObjectiveKind::TimeCourse => section(ui, "Time-course scoring", false, |ui| {
                ch |= num(ui, "stop when fitness below", &mut c.ssd_stop_threshold, 0.0..=1e12, "Sum of squared differences");
                ch |= num(ui, "viable when fitness below", &mut c.ssd_viable_threshold, 0.0..=1e12, "A model counts as a success below this");
                ch |= ui
                    .checkbox(&mut c.ssd_stagnation_rules, "Stagnation rules")
                    .on_hover_text("Pascal heuristics: stop if fitness > 800 after generation 600, or < 30 after 800")
                    .changed();
            }),
        }

        section(ui, "Network size", false, |ui| {
            ch |= range_pair(ui, "species", &mut c.min_species, &mut c.max_species, 200, "Species per random network");
            ch |= range_pair(ui, "reactions", &mut c.min_reactions, &mut c.max_reactions, 500, "Reactions per random network");
            ch |= num(ui, "max initial rate constant", &mut c.max_initial_rate_constant, 1e-6..=1e9, "Rate constants start uniform in 0..this");
            ch |= ui
                .checkbox(&mut c.require_input_boundary, "Require an input boundary species")
                .on_hover_text("Reject networks with no constant source species (off: closed networks allowed)")
                .changed();
        });

        section(ui, "Reaction types", false, |ui| {
            let p = c.type_probabilities();
            for (v, name, pn) in [
                (&mut c.p_uniuni, "A -> B", p[0]),
                (&mut c.p_biuni, "A + B -> C", p[1]),
                (&mut c.p_unibi, "A -> B + C", p[2]),
                (&mut c.p_bibi, "A + B -> C + D", p[3]),
            ] {
                ui.horizontal(|ui| {
                    ch |= ui.add(egui::Slider::new(v, 0.0..=1.0).show_value(false)).changed();
                    ui.label(format!("{name}  ({:.0}%)", pn * 100.0));
                });
            }
        });

        section(ui, "Mutation and selection", false, |ui| {
            ch |= frac(ui, "change a rate constant", &mut c.p_rate_mutation, "Probability per mutation draw");
            ch |= frac(ui, "add a reaction", &mut c.p_add_reaction, "Probability per mutation draw");
            ch |= frac(ui, "delete a reaction", &mut c.p_delete_reaction, "Probability per mutation draw");
            ch |= num(ui, "rate change fraction", &mut c.rate_change_fraction, 0.0..=1.99, "A rate change is up to +/- half this fraction of k");
            ch |= frac(ui, "elite fraction", &mut c.elite_fraction, "Share of the best networks copied unchanged");
            ch |= ui.checkbox(&mut c.add_random_networks, "Add random networks each generation").changed();
            if c.add_random_networks {
                ch |= num(ui, "random networks", &mut c.num_random_networks, 1..=10_000, "Fresh networks added per generation");
            }
        });

        section(ui, "Reaction constraints", false, |ui| {
            ch |= ui
                .checkbox(&mut c.disallow_self_referential, "No species on both sides (no catalysis)")
                .on_hover_text("Reject A + B -> A, A -> A + B, ...")
                .changed();
            ch |= ui
                .checkbox(&mut c.disallow_autocatalysis, "No autocatalysis")
                .on_hover_text("Reject A -> 2A and A + B -> 2B")
                .changed();
            ui.horizontal(|ui| {
                ui.label("Reaction patterns").on_hover_text(
                    "One per line, e.g. 'A + B -> A' or '2A -> A'. Optional 'mode: allow' (only these) or \
                     'mode: disallow' (default). '#' starts a comment.",
                );
                if ui.small_button("Load...").clicked() {
                    self.files.open(OpenKind::Patterns, "Load reaction patterns", &["txt"]);
                }
            });
            ch |= ui
                .add(
                    egui::TextEdit::multiline(&mut c.reaction_patterns)
                        .code_editor()
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text("e.g.\nmode: disallow\nA + B -> A"),
                )
                .changed();
        });
        for e in &self.pattern_errors {
            ui.colored_label(egui::Color32::RED, format!("pattern line {}: {}", e.line, e.message));
        }
        let c = &mut self.cfg;

        section(ui, "Initial concentrations", false, |ui| {
            ui.label(RichText::new("By species index (S0, S1, ...); repeated if there are more species.").small().weak());
            let id = egui::Id::unique("settings_init_text");
            let mut text: String = ui.data_mut(|d| {
                d.get_temp::<String>(id)
                    .unwrap_or_else(|| c.initial_concentrations.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "))
            });
            let r = ui.add(egui::TextEdit::multiline(&mut text).desired_rows(2).desired_width(f32::INFINITY));
            if r.changed() {
                let parsed: Result<Vec<f64>, _> =
                    text.split([',', ' ', '\n']).filter(|s| !s.trim().is_empty()).map(|s| s.trim().parse::<f64>()).collect();
                match parsed {
                    Ok(v) if !v.is_empty() => {
                        c.initial_concentrations = v;
                        ch = true;
                    }
                    _ => {
                        ui.colored_label(egui::Color32::RED, "Enter numbers separated by commas");
                    }
                }
            }
            if r.lost_focus() {
                ui.data_mut(|d| d.remove::<String>(id));
            } else {
                ui.data_mut(|d| d.insert_temp(id, text));
            }
        });

        section(ui, "Solver (advanced)", false, |ui| {
            ui.label(RichText::new("diffsol adaptive BDF (stiff solver)").small().weak());
            ch |= sci(ui, "relative tolerance", &mut c.rtol, "Default 1e-6");
            ch |= sci(ui, "absolute tolerance", &mut c.atol, "Default 1e-10");
            ch |= num(ui, "max steps per output interval", &mut c.max_steps_per_interval, 10..=1_000_000, "Give up beyond this");
            ch |= num(ui, "max total steps", &mut c.max_total_steps, 1000..=100_000_000, "Give up beyond this");
            ch |= num(ui, "max concentration", &mut c.max_concentration, 1.0..=1e15, "Larger values count as a runaway");
            ui.horizontal(|ui| {
                let mut exact = c.num_workers > 0;
                if ui.checkbox(&mut exact, "Exact number of workers").on_hover_text("Override the CPU usage percentage").changed() {
                    c.num_workers = if exact { 4 } else { 0 };
                    ch = true;
                }
                if exact {
                    ch |= ui.add(egui::DragValue::new(&mut c.num_workers).range(1..=1024)).changed();
                }
            });
        });

        section(ui, "Output", false, |ui| {
            ch |= ui
                .checkbox(&mut c.reduce_models, "Reduce models")
                .on_hover_text("Remove reactions that are not needed for the behaviour")
                .changed();
            ch |= ui
                .checkbox(&mut c.add_tellurium_harness, "Add a Tellurium harness to exported models")
                .on_hover_text("Wrap the Antimony in a runnable Python/Tellurium script")
                .changed();
        });
        ch
    }
}

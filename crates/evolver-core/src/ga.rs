//! The genetic algorithm: one experiment evolves one population.
//!
//! An [`Experiment`] is a state machine driven by [`Experiment::step`], so the
//! same code runs in a Web Worker, an OS thread, the CLI and tests. It owns its
//! RNG (seeded from the base seed and experiment id) and touches no shared
//! state, so results do not depend on scheduling or worker count.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::{EvolverConfig, ObjectiveKind};
use crate::export;
use crate::network::{Network, Reaction, ReactionKind};
use crate::objective::{Evaluation, RunContext, Window, evaluate, window_options};
use crate::rng::Rng;
use crate::simulate::{Trajectory, simulate};

/// Maximum attempts to draw one viable random network.
const MAX_CREATE_ATTEMPTS: usize = 10_000;
/// Points in the display trajectory kept with each result (for plots and the grid).
const DISPLAY_POINTS: usize = 600;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationCounts {
    pub rate: u64,
    pub add: u64,
    pub delete: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenStats {
    pub generation: u32,
    pub best: f64,
    pub median: f64,
    pub pop_len: usize,
    pub n_float: usize,
    pub n_reactions: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// Best fitness fell below the stop threshold.
    Converged,
    MaxGenerations,
    /// The SSD stagnation heuristics fired.
    Stagnated,
    /// Every individual failed to simulate.
    Extinct,
    /// Still above the abandon threshold at the abandon generation.
    Abandoned,
}

/// A network plus its cached evaluation (cleared whenever it changes).
#[derive(Clone, Debug)]
struct Individual {
    net: Network,
    fitness: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Progress {
    pub id: u32,
    pub generation: u32,
    pub best_fitness: f64,
    pub pop_len: usize,
    pub n_float: usize,
    pub n_reactions: usize,
    pub counts: MutationCounts,
    pub evaluations: u64,
    /// Sent only when the best network changed since the last Progress.
    pub best_network: Option<Network>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExperimentResult {
    pub id: u32,
    pub base_seed: u32,
    pub generations: u32,
    pub stop_reason: StopReason,
    /// Best network at the end of evolution.
    pub best: Network,
    /// Its fitness on the evolution window.
    pub evolved_fitness: f64,
    /// Its fitness used for decisions: the confirmation window in oscillation
    /// mode (if enabled), else the same as `evolved_fitness`.
    pub fitness: f64,
    /// Below the mode's viable threshold.
    pub viable: bool,
    /// The reduced network (same as `best` if reduction is off or not viable).
    pub reduced: Network,
    pub reduced_fitness: f64,
    /// Node scores of the reduced network, per floating species (state order).
    pub node_scores: Vec<f64>,
    /// The reduced model on the display window, decimated for plotting.
    pub display: Option<Trajectory>,
    /// State index of the best-scoring species in `display`.
    pub best_node: usize,
    pub history: Vec<GenStats>,
    pub counts: MutationCounts,
    pub evaluations: u64,
    pub antimony: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ExperimentEvent {
    Progress(Progress),
    Finished(Box<ExperimentResult>),
    Failed { id: u32, message: String },
}

enum State {
    Running,
    Done,
}

pub struct Experiment {
    cfg: Arc<EvolverConfig>,
    ctx: Arc<RunContext>,
    id: u32,
    base_seed: u32,
    rng: Rng,
    pop: Vec<Individual>,
    generation: u32,
    history: Vec<GenStats>,
    counts: MutationCounts,
    evaluations: u64,
    lineage: u64,
    last_reported: Option<(u64, u64)>,
    state: State,
    pending_failure: Option<String>,
}

/// Viability rule shared by creation and mutation (B13).
pub fn is_viable(net: &Network, cfg: &EvolverConfig) -> bool {
    !net.reactions.is_empty() && net.n_float >= 2 && (!cfg.require_input_boundary || net.n_input >= 1)
}

fn draw_kind(rng: &mut Rng, p: &[f64; 4]) -> ReactionKind {
    let u = rng.next_f64();
    if u < p[0] {
        ReactionKind::UniUni
    } else if u < p[0] + p[1] {
        ReactionKind::BiUni
    } else if u < p[0] + p[1] + p[2] {
        ReactionKind::UniBi
    } else {
        ReactionKind::BiBi
    }
}

impl Experiment {
    pub fn new(cfg: Arc<EvolverConfig>, ctx: Arc<RunContext>, id: u32, base_seed: u32) -> Self {
        let mut exp = Self {
            rng: Rng::for_experiment(base_seed, id),
            cfg,
            ctx,
            id,
            base_seed,
            pop: Vec::new(),
            generation: 0,
            history: Vec::new(),
            counts: MutationCounts::default(),
            evaluations: 0,
            lineage: 0,
            last_reported: None,
            state: State::Running,
            pending_failure: None,
        };
        for _ in 0..exp.cfg.max_population_size {
            match exp.create_viable() {
                Some(net) => exp.pop.push(Individual { net, fitness: None }),
                None => {
                    exp.pending_failure = Some(
                        "could not create a viable random network; check the network size, reaction types and constraints"
                            .into(),
                    );
                    break;
                }
            }
        }
        exp
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.state, State::Done)
    }

    fn new_lineage(&mut self) -> u64 {
        self.lineage += 1;
        (u64::from(self.id) << 32) | self.lineage
    }

    /// A random network (TIndividual.Create). Species and reaction counts are
    /// drawn inclusively from the configured ranges (Pascal excluded the max, B20).
    fn create_random(&mut self) -> Network {
        let cfg = self.cfg.clone();
        let n_species = self.rng.range(cfg.min_species as i32, cfg.max_species as i32 + 1) as usize;
        let n_reactions = self.rng.range(cfg.min_reactions as i32, cfg.max_reactions as i32 + 1) as usize;
        let p = cfg.type_probabilities();
        let mut reactions = Vec::with_capacity(n_reactions);
        for _ in 0..n_reactions {
            let k = self.rng.next_f64() * cfg.max_initial_rate_constant;
            let kind = draw_kind(&mut self.rng, &p);
            if let Some(conn) = self.ctx.patterns.pick(&mut self.rng, kind, n_species) {
                reactions.push(Reaction { kind, k, conn });
            }
        }
        let id = self.new_lineage();
        Network::new(id, n_species, reactions)
    }

    fn create_viable(&mut self) -> Option<Network> {
        for _ in 0..MAX_CREATE_ATTEMPTS {
            let net = self.create_random();
            if is_viable(&net, &self.cfg) {
                return Some(net);
            }
        }
        None
    }

    /// Two mutation draws on a copy (mutateIndividual).
    fn mutate(&mut self, parent: &Network) -> Network {
        let cfg = self.cfg.clone();
        let mut net = parent.clone();
        if net.reactions.is_empty() {
            return net;
        }
        for _ in 0..2 {
            let u = self.rng.next_f64();
            if u < cfg.p_rate_mutation {
                let i = self.rng.index(net.reactions.len());
                let r = &mut net.reactions[i];
                r.k = (r.k + (self.rng.next_f64() - 0.5) * cfg.rate_change_fraction * r.k).max(0.0);
                self.counts.rate += 1;
            } else if u < cfg.p_rate_mutation + cfg.p_add_reaction {
                let k = self.rng.next_f64();
                let kind = draw_kind(&mut self.rng, &cfg.type_probabilities());
                if let Some(conn) = self.ctx.patterns.pick(&mut self.rng, kind, net.n_species) {
                    net.reactions.push(Reaction { kind, k, conn });
                    net.classify();
                    net.id = self.new_lineage();
                    self.counts.add += 1;
                }
            } else if u < cfg.p_rate_mutation + cfg.p_add_reaction + cfg.p_delete_reaction
                && net.reactions.len() > 2
            {
                let i = self.rng.index(net.reactions.len());
                net.reactions.remove(i);
                net.classify();
                net.id = self.new_lineage();
                self.counts.delete += 1;
            }
        }
        net
    }

    /// Evaluate individuals without a cached fitness, drop failures, sort.
    fn evaluate_population(&mut self) {
        let cfg = self.cfg.clone();
        let ctx = self.ctx.clone();
        let mut evaluations = 0;
        self.pop.retain_mut(|ind| {
            if ind.fitness.is_some() {
                return true;
            }
            if !is_viable(&ind.net, &cfg) {
                return false;
            }
            evaluations += 1;
            match evaluate(&ind.net, &cfg, &ctx, Window::Evolve) {
                Ok(e) if e.fitness.is_finite() && !(cfg.objective == ObjectiveKind::TimeCourse && e.fitness > 1e8) => {
                    ind.fitness = Some(e.fitness);
                    true
                }
                _ => false,
            }
        });
        self.evaluations += evaluations;
        self.pop.sort_by(|a, b| a.fitness.unwrap_or(f64::INFINITY).total_cmp(&b.fitness.unwrap_or(f64::INFINITY)));
    }

    fn best_fitness(&self) -> f64 {
        self.pop.first().and_then(|i| i.fitness).unwrap_or(f64::INFINITY)
    }

    fn stop_check(&self) -> Option<StopReason> {
        let best = self.best_fitness();
        let g = self.generation;
        match self.cfg.objective {
            ObjectiveKind::Oscillation => {
                if best < self.cfg.osc_stop_threshold {
                    Some(StopReason::Converged)
                } else if self.cfg.abandon_generation > 0
                    && g >= self.cfg.abandon_generation
                    && best > self.cfg.abandon_threshold
                {
                    Some(StopReason::Abandoned)
                } else {
                    None
                }
            }
            ObjectiveKind::TimeCourse => {
                if self.cfg.ssd_stagnation_rules && ((best > 800.0 && g > 600) || (best < 30.0 && g > 800)) {
                    Some(StopReason::Stagnated)
                } else {
                    (best < self.cfg.ssd_stop_threshold).then_some(StopReason::Converged)
                }
            }
        }
    }

    /// Build the next generation (elites, tournament, optional random networks, mutation).
    fn breed(&mut self) {
        let cfg = self.cfg.clone();
        let len = self.pop.len();
        let n_elite = ((cfg.elite_fraction * len as f64) as usize).clamp(1, len);
        let mut next: Vec<Individual> = self.pop[..n_elite].to_vec();
        while next.len() < cfg.max_population_size {
            let a = self.rng.index(len);
            let b = self.rng.index(len);
            let fa = self.pop[a].fitness.unwrap_or(f64::INFINITY);
            let fb = self.pop[b].fitness.unwrap_or(f64::INFINITY);
            next.push(self.pop[if fa < fb { a } else { b }].clone());
        }
        if cfg.add_random_networks {
            for _ in 0..cfg.num_random_networks {
                if let Some(net) = self.create_viable() {
                    next.push(Individual { net, fitness: None });
                }
            }
        }
        for ind in next.iter_mut().skip(n_elite) {
            let child = self.mutate(&ind.net);
            if is_viable(&child, &cfg) {
                *ind = Individual { net: child, fitness: None };
            }
        }
        self.pop = next;
    }

    fn progress(&mut self) -> Progress {
        let best = &self.pop[0];
        let key = (best.net.id, best.fitness.unwrap_or(f64::INFINITY).to_bits());
        let changed = self.last_reported != Some(key);
        self.last_reported = Some(key);
        Progress {
            id: self.id,
            generation: self.generation,
            best_fitness: self.best_fitness(),
            pop_len: self.pop.len(),
            n_float: best.net.n_float,
            n_reactions: best.net.reactions.len(),
            counts: self.counts,
            evaluations: self.evaluations,
            best_network: changed.then(|| best.net.clone()),
        }
    }

    /// Run up to `max_generations` generations. Returns a Progress event per
    /// generation, and a Finished (or Failed) event when the experiment ends.
    pub fn step(&mut self, max_generations: u32) -> Vec<ExperimentEvent> {
        let mut events = Vec::new();
        if self.is_finished() {
            return events;
        }
        if let Some(message) = self.pending_failure.take() {
            self.state = State::Done;
            events.push(ExperimentEvent::Failed { id: self.id, message });
            return events;
        }
        for _ in 0..max_generations.max(1) {
            self.evaluate_population();
            if self.pop.is_empty() {
                self.state = State::Done;
                events.push(ExperimentEvent::Failed { id: self.id, message: "every network failed to simulate".into() });
                return events;
            }
            let stop = if self.generation >= self.cfg.max_generations {
                Some(StopReason::MaxGenerations)
            } else {
                self.stop_check()
            };
            let best = &self.pop[0];
            let mut fits: Vec<f64> = self.pop.iter().filter_map(|i| i.fitness).collect();
            fits.sort_by(f64::total_cmp);
            self.history.push(GenStats {
                generation: self.generation,
                best: self.best_fitness(),
                median: fits[fits.len() / 2],
                pop_len: self.pop.len(),
                n_float: best.net.n_float,
                n_reactions: best.net.reactions.len(),
            });
            let p = self.progress();
            events.push(ExperimentEvent::Progress(p));
            if let Some(reason) = stop {
                self.state = State::Done;
                events.push(ExperimentEvent::Finished(Box::new(self.finish(reason))));
                return events;
            }
            self.breed();
            self.generation += 1;
        }
        events
    }

    /// Run to completion (tests, CLI).
    pub fn run(mut self) -> Result<ExperimentResult, String> {
        loop {
            for e in self.step(50) {
                match e {
                    ExperimentEvent::Finished(r) => return Ok(*r),
                    ExperimentEvent::Failed { message, .. } => return Err(message),
                    ExperimentEvent::Progress(_) => {}
                }
            }
        }
    }

    /// Confirmation, reduction, display trajectory and export.
    fn finish(&mut self, stop_reason: StopReason) -> ExperimentResult {
        let cfg = self.cfg.clone();
        let ctx = self.ctx.clone();
        let best = self.pop[0].net.clone();
        let evolved_fitness = self.best_fitness();

        // Long-window confirmation (oscillation): rejects slow-damping oscillators.
        // Oscillation: the final check always uses the Confirm window (the same as
        // the evolution window when confirmation is off), which also applies the
        // drift check.
        let window = if cfg.objective == ObjectiveKind::Oscillation { Window::Confirm } else { Window::Evolve };
        self.evaluations += 1;
        let base_eval = evaluate(&best, &cfg, &ctx, window).ok();
        let fitness = base_eval.as_ref().map_or(f64::INFINITY, |e| e.fitness);
        let threshold = match cfg.objective {
            ObjectiveKind::Oscillation => cfg.osc_viable_threshold,
            ObjectiveKind::TimeCourse => cfg.ssd_viable_threshold,
        };
        let viable = fitness < threshold;

        let (reduced, reduced_eval) = match base_eval {
            Some(base) if viable && cfg.reduce_models => {
                let (net, eval, n) = reduce(&best, base, &cfg, &ctx);
                self.evaluations += n;
                (net, Some(eval))
            }
            other => (best.clone(), other),
        };
        let reduced_fitness = reduced_eval.as_ref().map_or(f64::INFINITY, |e| e.fitness);
        let node_scores = reduced_eval.map(|e| e.node_scores).unwrap_or_default();
        let best_node = node_scores
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(i, _)| i);
        let display = display_trajectory(&reduced, &cfg, &ctx);

        let mut result = ExperimentResult {
            id: self.id,
            base_seed: self.base_seed,
            generations: self.generation,
            stop_reason,
            best,
            evolved_fitness,
            fitness,
            viable,
            reduced,
            reduced_fitness,
            node_scores,
            display,
            best_node,
            history: std::mem::take(&mut self.history),
            counts: self.counts,
            evaluations: self.evaluations,
            antimony: String::new(),
        };
        result.antimony = export::to_antimony(&result, &cfg, &ctx);
        result
    }
}

/// Greedy structural reduction (B14 fix): try removing each reaction in turn;
/// keep the removal if the network stays structurally viable, its fitness stays
/// below the viable threshold, and within the tolerance of the unreduced
/// network's. Returns the reduced network, its
/// evaluation and the number of evaluations used.
pub fn reduce(net: &Network, base: Evaluation, cfg: &EvolverConfig, ctx: &RunContext) -> (Network, Evaluation, u64) {
    let base_fitness = base.fitness;
    // A removal is kept only if the model stays viable and within the
    // mode's tolerance of the unreduced fitness.
    let within = |f: f64| match cfg.objective {
        ObjectiveKind::Oscillation => f <= base_fitness + cfg.osc_reduce_margin && f < cfg.osc_viable_threshold,
        ObjectiveKind::TimeCourse => f <= 10.0 * base_fitness && f < cfg.ssd_viable_threshold,
    };
    let window = if cfg.objective == ObjectiveKind::Oscillation { Window::Confirm } else { Window::Evolve };
    let mut current = net.clone();
    let mut current_eval = base;
    let mut evaluations = 0;
    let mut i = 0;
    while i < current.reactions.len() {
        if current.reactions.len() <= 1 {
            break;
        }
        let mut cand = current.clone();
        cand.reactions.remove(i);
        cand.classify();
        let accepted = if is_viable(&cand, cfg) {
            evaluations += 1;
            match evaluate(&cand, cfg, ctx, window) {
                Ok(e) if within(e.fitness) => {
                    current = cand;
                    current_eval = e;
                    true
                }
                _ => false,
            }
        } else {
            false
        };
        if !accepted {
            i += 1;
        }
    }
    (current, current_eval, evaluations)
}

/// The model simulated on the display window, decimated for plotting.
pub fn display_trajectory(net: &Network, cfg: &EvolverConfig, ctx: &RunContext) -> Option<Trajectory> {
    let mut opts = window_options(cfg, ctx, Window::Confirm);
    opts.n_points = DISPLAY_POINTS;
    simulate(&net.compile(&ctx.init), &opts).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objective::Target;

    fn small_cfg() -> EvolverConfig {
        EvolverConfig {
            max_population_size: 30,
            max_generations: 15,
            osc_t_end: 60.0,
            osc_n_points: 512,
            osc_confirm_t_end: 120.0,
            osc_confirm_n_points: 1024,
            ..EvolverConfig::oscillator_preset()
        }
    }

    fn run(cfg: EvolverConfig, id: u32, seed: u32) -> ExperimentResult {
        let (ctx, _) = RunContext::new(&cfg, Target::default());
        Experiment::new(Arc::new(cfg), Arc::new(ctx), id, seed).run().unwrap()
    }

    #[test]
    fn deterministic_for_a_seed() {
        let a = run(small_cfg(), 3, 42);
        let b = run(small_cfg(), 3, 42);
        assert_eq!(a.best, b.best);
        assert_eq!(a.fitness.to_bits(), b.fitness.to_bits());
        assert_eq!(a.history, b.history);
        let c = run(small_cfg(), 4, 42);
        assert_ne!(a.best, c.best);
    }

    #[test]
    fn population_invariants() {
        let cfg = small_cfg();
        let (ctx, _) = RunContext::new(&cfg, Target::default());
        let mut exp = Experiment::new(Arc::new(cfg.clone()), Arc::new(ctx), 0, 7);
        for _ in 0..5 {
            exp.step(1);
            if exp.is_finished() {
                break;
            }
            // After breeding the population is exactly the configured size (B5).
            assert_eq!(exp.pop.len(), cfg.max_population_size);
            for ind in &exp.pop {
                assert!(ind.net.reactions.iter().all(|r| r.k >= 0.0));
                assert!(is_viable(&ind.net, &cfg));
            }
        }
    }

    #[test]
    fn elites_survive_unchanged() {
        let cfg = small_cfg();
        let (ctx, _) = RunContext::new(&cfg, Target::default());
        let mut exp = Experiment::new(Arc::new(cfg.clone()), Arc::new(ctx), 0, 11);
        exp.evaluate_population();
        let n_elite = ((cfg.elite_fraction * exp.pop.len() as f64) as usize).max(1);
        let elites: Vec<Network> = exp.pop[..n_elite].iter().map(|i| i.net.clone()).collect();
        exp.breed();
        for (e, ind) in elites.iter().zip(&exp.pop) {
            assert_eq!(e, &ind.net);
            assert!(ind.fitness.is_some(), "elite fitness should stay cached");
        }
    }

    #[test]
    fn best_fitness_never_worsens() {
        let r = run(small_cfg(), 1, 5);
        for w in r.history.windows(2) {
            assert!(w[1].best <= w[0].best + 1e-12, "{:?}", w);
        }
    }

    #[test]
    fn reduction_keeps_fitness() {
        // The Pascal oscillator plus two inert reactions that reduction should strip.
        let cfg = EvolverConfig { max_species: 7, ..Default::default() };
        let (ctx, _) = RunContext::new(&cfg, Target::default());
        let mut net = crate::samples::pascal_oscillator();
        net.n_species = 7;
        net.reactions.push(Reaction::new(ReactionKind::UniUni, 0.0, &[5], &[6]));
        net.reactions.push(Reaction::new(ReactionKind::UniUni, 1e-9, &[0], &[6]));
        net.classify();
        let base = evaluate(&net, &cfg, &ctx, Window::Confirm).unwrap();
        let (reduced, eval, _) = reduce(&net, base.clone(), &cfg, &ctx);
        assert!(reduced.reactions.len() <= 10, "{} reactions left", reduced.reactions.len());
        assert!(eval.fitness <= base.fitness + cfg.osc_reduce_margin);
        assert!(eval.fitness < cfg.osc_viable_threshold);
        // The exported fitness is exactly that of the returned network.
        let again = evaluate(&reduced, &cfg, &ctx, Window::Confirm).unwrap();
        assert_eq!(again.fitness.to_bits(), eval.fitness.to_bits());
    }
}

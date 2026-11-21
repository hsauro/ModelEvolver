//! Work sent to a worker (an OS thread natively, a Web Worker in the browser)
//! and the replies. Messages cross the Web Worker boundary as JSON.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use evolver_core::config::EvolverConfig;
use evolver_core::ga::{Experiment, ExperimentEvent};
use evolver_core::objective::{RunContext, Target};

/// Run one experiment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRequest {
    pub id: u32,
    pub base_seed: u32,
    pub cfg: EvolverConfig,
    pub target: Target,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reply {
    /// A Web Worker has loaded and is listening (messages sent before this are lost).
    Ready,
    Event(ExperimentEvent),
    /// The worker crashed (reported by the page, not sent by a worker).
    WorkerError(String),
}

/// Encode a message for the Web Worker boundary (postcard binary: compact,
/// and it round-trips infinite fitness values, which JSON cannot).
pub fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    postcard::to_allocvec(v).unwrap_or_default()
}

pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, String> {
    postcard::from_bytes(bytes).map_err(|e| e.to_string())
}

/// Send a Progress event at least every this many generations (and always
/// when the best network changes).
const PROGRESS_EVERY: u32 = 5;

/// Run the experiment to the end, calling `emit` with progress (throttled) and
/// the final Finished/Failed event. `cancelled` is polled between generations.
pub fn run_experiment(req: RunRequest, mut emit: impl FnMut(Reply), cancelled: impl Fn() -> bool) {
    let (ctx, _pattern_errors) = RunContext::new(&req.cfg, req.target);
    let mut exp = Experiment::new(Arc::new(req.cfg), Arc::new(ctx), req.id, req.base_seed);
    while !exp.is_finished() {
        if cancelled() {
            return;
        }
        for event in exp.step(1) {
            let send = match &event {
                ExperimentEvent::Progress(p) => p.best_network.is_some() || p.generation % PROGRESS_EVERY == 0,
                _ => true,
            };
            if send {
                emit(Reply::Event(event));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evolver_core::ga::ExperimentEvent;

    #[test]
    fn results_with_infinite_fitness_survive_encoding() {
        // An experiment that fails confirmation has infinite fitness; JSON turned
        // that into null and the page dropped the result.
        let cfg = EvolverConfig { max_generations: 1, max_population_size: 6, osc_n_points: 64, osc_confirm_t_end: 0.0, ..Default::default() };
        let mut finished = None;
        run_experiment(
            RunRequest { id: 7, base_seed: 3, cfg, target: Target::default() },
            |r| {
                if let Reply::Event(ExperimentEvent::Finished(res)) = r {
                    finished = Some(*res);
                }
            },
            || false,
        );
        let mut res = finished.expect("experiment finished");
        res.fitness = f64::INFINITY;
        res.reduced_fitness = f64::INFINITY;
        let back: Reply = decode(&encode(&Reply::Event(ExperimentEvent::Finished(Box::new(res))))).unwrap();
        match back {
            Reply::Event(ExperimentEvent::Finished(r)) => {
                assert_eq!(r.id, 7);
                assert!(r.fitness.is_infinite() && r.reduced_fitness.is_infinite());
            }
            _ => panic!("wrong message"),
        }
    }
}

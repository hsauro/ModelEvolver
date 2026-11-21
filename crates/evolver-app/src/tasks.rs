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

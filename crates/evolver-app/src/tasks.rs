//! Work sent to a worker (a Web Worker in the browser, a thread natively),
//! and the replies. Messages cross the Web Worker boundary as JSON.

use serde::{Deserialize, Serialize};

use evolver_core::objective::oscillation_fitness;
use evolver_core::rng::Rng;
use evolver_core::samples::{PASCAL_INITIAL, random_network};
use evolver_core::simulate::SimOptions;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    /// Evaluate `n_evals` random GA-like networks (oscillation fitness).
    Bench { worker: u32, seed: u64, n_evals: u32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reply {
    Bench { worker: u32, evals: u32, ok: u32, bad: u32, ms: f64 },
    Error(String),
}

pub fn handle(req: Request) -> Reply {
    match req {
        Request::Bench { worker, seed, n_evals } => {
            let opts = SimOptions::default();
            let mut rng = Rng::new(seed);
            let start = web_time::Instant::now();
            let (mut ok, mut bad, mut evals) = (0, 0, 0);
            while evals < n_evals {
                let net = random_network(&mut rng, 5, 9, 12.3);
                if net.n_float < 2 {
                    continue;
                }
                evals += 1;
                match oscillation_fitness(&net, &PASCAL_INITIAL, &opts) {
                    Ok(_) => ok += 1,
                    Err(_) => bad += 1,
                }
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            Reply::Bench { worker, evals, ok, bad, ms }
        }
    }
}

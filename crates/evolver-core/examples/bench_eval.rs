//! Measure the cost of one oscillation-fitness evaluation (simulate + score)
//! on random GA-like networks. Run with:
//!   cargo run --release -p evolver-core --example bench_eval
use std::time::Instant;

use evolver_core::objective::oscillation_fitness;
use evolver_core::rng::Rng;
use evolver_core::samples::{PASCAL_INITIAL, pascal_oscillator, random_network};
use evolver_core::simulate::SimOptions;

fn main() {
    let opts = SimOptions::default();
    let mut rng = Rng::new(12345);
    let nets: Vec<_> = (0..2000).map(|_| random_network(&mut rng, 5, 9, 12.3)).filter(|n| n.n_float >= 2).collect();

    let t = Instant::now();
    let (mut ok, mut bad) = (0, 0);
    for n in &nets {
        match oscillation_fitness(n, &PASCAL_INITIAL, &opts) {
            Ok(_) => ok += 1,
            Err(_) => bad += 1,
        }
    }
    let secs = t.elapsed().as_secs_f64();
    println!(
        "random networks: {} evals in {:.2}s = {:.0} evals/s/thread ({} ok, {} bad)",
        nets.len(), secs, nets.len() as f64 / secs, ok, bad
    );

    // All cores: each thread evaluates its own 1000 random networks.
    for k in [1usize, 4, 10, 20] {
        let t = Instant::now();
        std::thread::scope(|s| {
            for w in 0..k {
                s.spawn(move || {
                    let mut rng = Rng::new(5000 + w as u64);
                    let mut done = 0;
                    while done < 1000 {
                        let n = random_network(&mut rng, 5, 9, 12.3);
                        if n.n_float < 2 {
                            continue;
                        }
                        done += 1;
                        let _ = oscillation_fitness(&n, &PASCAL_INITIAL, &opts);
                    }
                });
            }
        });
        let secs = t.elapsed().as_secs_f64();
        println!("{k} threads: {:.0} evals/s", (k * 1000) as f64 / secs);
    }

    let osc = pascal_oscillator();
    let t = Instant::now();
    let reps = 200;
    for _ in 0..reps {
        oscillation_fitness(&osc, &PASCAL_INITIAL, &opts).unwrap();
    }
    let secs = t.elapsed().as_secs_f64();
    println!("Pascal oscillator: {:.2} ms/eval", 1000.0 * secs / reps as f64);
}

//! M2 tuning and validation tool.
//!
//!   tune score DIR [T_END POINTS]   score a directory of models (Pascal or Rust
//!                                   output) and compare with the fitness each
//!                                   file reports
//!   tune tol DIR                    accuracy and cost of solver tolerances on
//!                                   the models in DIR

use std::time::Instant;

use evolver_core::import::{ImportedModel, network_from_antimony};
use evolver_core::objective::oscillation_fitness;
use evolver_core::simulate::SimOptions;

struct Model {
    name: String,
    model: ImportedModel,
    /// Best (lowest) node fitness reported in the file, if any.
    reported: Option<f64>,
}

/// The fitness a model file reports: Pascal "Best fitness", or the lowest
/// Rust node score.
fn reported_fitness(text: &str) -> Option<f64> {
    let lines: Vec<&str> = text.lines().collect();
    let nums = |s: &str| -> Option<f64> {
        s.trim_start_matches("//")
            .split(',')
            .filter_map(|x| x.trim().parse::<f64>().ok())
            .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.min(v))))
    };
    // Pascal: "// Best fitness =     0.01" (its "Fitness of nodes" line also lists
    // boundary species, which always score 0, so it cannot be used).
    for l in &lines {
        if let Some((_, rest)) = l.split_once("Best fitness =") {
            return rest.trim().parse::<f64>().ok();
        }
        if let Some((_, rest)) = l.split_once("Node scores (floating species, state order):") {
            return nums(rest);
        }
    }
    None
}

fn load_dir(dir: &str) -> Vec<Model> {
    let mut out = Vec::new();
    let mut failed = 0;
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("cannot read directory").flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        match network_from_antimony(&text) {
            Ok(model) => out.push(Model {
                name: path.file_name().unwrap_or_default().to_string_lossy().into(),
                model,
                reported: reported_fitness(&text),
            }),
            Err(err) => {
                failed += 1;
                eprintln!("{}: {err}", path.display());
            }
        }
    }
    eprintln!("loaded {} models ({failed} could not be imported)", out.len());
    out
}

/// Evaluate every model in parallel; returns (fitness or None, steps) per model and wall seconds.
fn evaluate_all(models: &[Model], opts: &SimOptions) -> (Vec<(Option<f64>, usize)>, f64) {
    // At most 8 threads: leave the rest of the machine free while tuning.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
    let start = Instant::now();
    let chunk = models.len().div_ceil(threads).max(1);
    let results: Vec<(Option<f64>, usize)> = std::thread::scope(|s| {
        let handles: Vec<_> = models
            .chunks(chunk)
            .map(|c| {
                s.spawn(move || {
                    c.iter()
                        .map(|m| match oscillation_fitness(&m.model.net, &m.model.init, opts) {
                            Ok((f, _, t)) => (Some(f), t.steps),
                            Err(_) => (None, 0),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    (results, start.elapsed().as_secs_f64())
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn score(dir: &str, t_end: f64, points: usize) {
    let models = load_dir(dir);
    let opts = SimOptions { t_end, n_points: points, ..Default::default() };
    let (res, wall) = evaluate_all(&models, &opts);
    let mut diffs = Vec::new();
    let (mut failed, mut viable, mut agree) = (0, 0, 0);
    let mut worst: Vec<(f64, &str, f64, f64)> = Vec::new();
    for (m, (f, _)) in models.iter().zip(&res) {
        let Some(f) = *f else {
            failed += 1;
            continue;
        };
        if f < 0.3 {
            viable += 1;
        }
        if let Some(p) = m.reported {
            let d = (f - p).abs();
            diffs.push(d);
            if (f < 0.3) == (p < 0.3) {
                agree += 1;
            }
            worst.push((d, &m.name, f, p));
        }
    }
    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("window t = 0..{t_end}, {points} points; {} models, {wall:.2} s", models.len());
    println!("simulation failed: {failed}; viable (fitness < 0.3): {viable}");
    let n = diffs.len();
    println!(
        "|Rust - reported| over {n} models: median {:.4}, 90% {:.4}, max {:.4}; same viable decision {agree}/{n}",
        percentile(&mut diffs.clone(), 0.5),
        percentile(&mut diffs.clone(), 0.9),
        percentile(&mut diffs, 1.0)
    );
    println!("largest differences:");
    for (d, name, f, p) in worst.iter().take(8) {
        println!("  {name}: Rust {f:.4}, reported {p:.4} (diff {d:.4})");
    }
}

/// How many models have a species that keeps growing (see `drifting_species`).
fn drift(dir: &str, t_end: f64, points: usize) {
    let models = load_dir(dir);
    let opts = SimOptions { t_end, n_points: points, ..Default::default() };
    let mut flagged = Vec::new();
    let mut ok = 0;
    for m in &models {
        if let Ok((f, _, traj)) = oscillation_fitness(&m.model.net, &m.model.init, &opts) {
            ok += 1;
            if let Some(j) = evolver_core::objective::drifting_species(&traj, 0.1) {
                flagged.push((m.name.clone(), j, f));
            }
        }
    }
    println!("window t = 0..{t_end}: {} of {ok} simulated models have a drifting species", flagged.len());
    for (name, j, f) in flagged.iter().take(10) {
        println!("  {name}: floating species {j}, oscillation fitness {f:.4}");
    }
}

fn tolerances(dir: &str) {
    let models = load_dir(dir);
    let base = SimOptions { t_end: 150.0, n_points: 2048, ..Default::default() };
    let reference = SimOptions { rtol: 1e-9, atol: 1e-13, max_steps_per_interval: 5000, max_total_steps: 2_000_000, ..base };
    let (ref_res, ref_wall) = evaluate_all(&models, &reference);
    println!("reference rtol 1e-9, atol 1e-13: {ref_wall:.2} s");
    println!("{:>8} {:>8} {:>9} {:>10} {:>9} {:>9} {:>9} {:>8}", "rtol", "atol", "time s", "speedup", "steps", "med|df|", "max|df|", "flips");
    let mut first_wall = None;
    for (rtol, atol) in [(1e-6, 1e-10), (1e-6, 1e-8), (1e-5, 1e-8), (1e-4, 1e-8), (1e-4, 1e-6), (1e-3, 1e-6)] {
        let opts = SimOptions { rtol, atol, ..base };
        // Three repeats, best time, to reduce noise.
        let mut wall = f64::INFINITY;
        let mut res = Vec::new();
        for _ in 0..3 {
            let (r, w) = evaluate_all(&models, &opts);
            wall = wall.min(w);
            res = r;
        }
        let base_wall = *first_wall.get_or_insert(wall);
        let mut diffs = Vec::new();
        let mut flips = 0;
        let mut steps = 0usize;
        for ((f, s), (fr, _)) in res.iter().zip(&ref_res) {
            steps += s;
            match (f, fr) {
                (Some(f), Some(fr)) => {
                    diffs.push((f - fr).abs());
                    if (*f < 0.3) != (*fr < 0.3) || (*f < 0.05) != (*fr < 0.05) {
                        flips += 1;
                    }
                }
                (None, None) => {}
                _ => flips += 1,
            }
        }
        println!(
            "{rtol:>8.0e} {atol:>8.0e} {wall:>9.2} {:>9.2}x {:>9} {:>9.5} {:>9.5} {flips:>8}",
            base_wall / wall,
            steps / models.len().max(1),
            percentile(&mut diffs.clone(), 0.5),
            percentile(&mut diffs, 1.0)
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("score") if args.len() >= 2 => {
            let t_end = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(150.0);
            let points = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2048);
            score(&args[1], t_end, points);
        }
        Some("tol") if args.len() >= 2 => tolerances(&args[1]),
        Some("drift") if args.len() >= 2 => {
            let t_end = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(300.0);
            let points = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4096);
            drift(&args[1], t_end, points);
        }
        _ => eprintln!("usage: tune score DIR [T_END POINTS] | tune tol DIR"),
    }
}

//! Headless batch runner for large runs: experiments in parallel, results
//! written to disk as they finish, resumable after any interruption.
//!
//! Output folder (default `results`):
//!   run.json      the configuration and base seed of the run (used by --resume)
//!   summary.csv   one line per finished experiment (appended and flushed)
//!   models.jsonl  one JSON object per successful model (Antimony, optionally SBML)
//!   models/       individual model files, with --model-files
//!
//! Each experiment's seed depends only on the base seed and its experiment
//! number, so a resumed (or split) run gives exactly the results of one
//! uninterrupted run.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use evolver_core::config::{EvolverConfig, ObjectiveKind};
use evolver_core::ga::{Experiment, ExperimentResult};
use evolver_core::objective::{RunContext, SharedConfig, Target};

const HELP: &str = "\
evolver-cli - run ModelEvolver experiments headless (large runs, resumable)

Setup:
  --config FILE        configuration saved by the app (settings and target)
  --preset NAME        osc (default: settings of the Pascal oscillator corpus) or pascal
  --pascal-ini FILE    apply a Pascal config.ini
  --objective NAME     osc or timecourse
  --target FILE        target time course (Pascal objectivefunction.txt format)
  --patterns FILE      reaction-pattern file
  -mg N                max generations
  -mp N                max population size
  --osc-window T N     oscillation evolution window (default 150 2048)
  --abandon G T        give up on experiments still above fitness T at generation G
                       (default 100 0.3; G = 0 turns it off)

Which experiments:
  -e N                 number of experiments (default 1)
  --start N            first experiment number (default 0); with -e, runs N .. N+e-1.
                       Use the same seed and different ranges to split a run across machines.
  -s SEED              base seed (default: random from the operating system, printed)
  --duration SECS      instead of -e: keep starting experiments for SECS seconds

Output:
  --out DIR            output folder (default: results)
  --resume             continue an interrupted run in --out DIR (reads run.json and summary.csv;
                       runs only the experiments not yet recorded)
  --sbml               also store SBML for each successful model
  --model-files        also write each successful model as its own file in DIR/models
  --history FILE       append each experiment's best-fitness history (JSON lines)

Running:
  -nt N                worker threads (default: 75% of the hardware threads)
  --progress SECS      seconds between progress lines (default 30)
  -v                   one line per finished experiment
  --dump-config        print the configuration as JSON and exit
  -h, --help           this help
";

struct Args {
    cfg: EvolverConfig,
    target: Target,
    seed_given: bool,
    start: u32,
    duration: Option<f64>,
    out: PathBuf,
    resume: bool,
    sbml: bool,
    model_files: bool,
    history: Option<PathBuf>,
    progress_secs: f64,
    verbose: bool,
    dump: bool,
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args {
        cfg: EvolverConfig::oscillator_preset(),
        target: Target::default(),
        seed_given: false,
        start: 0,
        duration: None,
        out: PathBuf::from("results"),
        resume: false,
        sbml: false,
        model_files: false,
        history: None,
        progress_secs: 30.0,
        verbose: false,
        dump: false,
    };
    let mut i = 0;
    let next = |i: &mut usize| -> Result<String, String> {
        *i += 1;
        argv.get(*i).cloned().ok_or_else(|| format!("{} needs a value", argv[*i - 1]))
    };
    let num = |s: String| s.parse::<f64>().map_err(|_| format!("'{s}' is not a number"));
    while i < argv.len() {
        match argv[i].as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--preset" => {
                a.cfg = match next(&mut i)?.as_str() {
                    "osc" => EvolverConfig::oscillator_preset(),
                    "pascal" => EvolverConfig::default(),
                    p => return Err(format!("unknown preset '{p}'")),
                }
            }
            "--config" => {
                let p = PathBuf::from(next(&mut i)?);
                let shared = SharedConfig::from_json(&read(&p)?).map_err(|e| format!("{}: {e}", p.display()))?;
                a.cfg = shared.settings;
                a.target = shared.target;
                a.seed_given = a.cfg.seed != 0;
            }
            "--pascal-ini" => {
                let p = PathBuf::from(next(&mut i)?);
                for w in a.cfg.apply_pascal_ini(&read(&p)?) {
                    eprintln!("{}: {w}", p.display());
                }
            }
            "--objective" => {
                a.cfg.objective = match next(&mut i)?.as_str() {
                    "osc" => ObjectiveKind::Oscillation,
                    "timecourse" => ObjectiveKind::TimeCourse,
                    o => return Err(format!("unknown objective '{o}'")),
                }
            }
            "--target" => {
                let p = PathBuf::from(next(&mut i)?);
                a.target = Target::parse(&read(&p)?).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            "--patterns" => a.cfg.reaction_patterns = read(Path::new(&next(&mut i)?))?,
            "-e" => a.cfg.num_experiments = num(next(&mut i)?)? as u32,
            "--start" => a.start = num(next(&mut i)?)? as u32,
            "--duration" => {
                a.duration = Some(num(next(&mut i)?)?);
                a.cfg.continuous = true;
            }
            "-s" => {
                a.cfg.seed = num(next(&mut i)?)? as u32;
                a.seed_given = a.cfg.seed != 0;
            }
            "-mg" => a.cfg.max_generations = num(next(&mut i)?)? as u32,
            "-mp" => a.cfg.max_population_size = num(next(&mut i)?)? as usize,
            "-nt" => a.cfg.num_workers = num(next(&mut i)?)? as u32,
            "--out" => a.out = PathBuf::from(next(&mut i)?),
            "--resume" => a.resume = true,
            "--sbml" => a.sbml = true,
            "--model-files" => a.model_files = true,
            "--history" => a.history = Some(PathBuf::from(next(&mut i)?)),
            "--progress" => a.progress_secs = num(next(&mut i)?)?.max(1.0),
            "-v" => a.verbose = true,
            "--osc-window" => {
                a.cfg.osc_t_end = num(next(&mut i)?)?;
                a.cfg.osc_n_points = num(next(&mut i)?)? as usize;
            }
            "--abandon" => {
                a.cfg.abandon_generation = num(next(&mut i)?)? as u32;
                a.cfg.abandon_threshold = num(next(&mut i)?)?;
            }
            "--dump-config" => a.dump = true,
            other => return Err(format!("unknown option '{other}' (try --help)")),
        }
        i += 1;
    }
    Ok(a)
}

/// run.json: what a run is, so that --resume can continue it exactly.
#[derive(serde::Serialize, serde::Deserialize)]
struct RunFile {
    configuration: SharedConfig,
    first_experiment: u32,
    /// Number of experiments (ignored for continuous runs).
    experiments: u32,
}

const CSV_HEADER: &str = "base_seed,experiment,viable,fitness,evolved_fitness,reduced_fitness,reactions,reactions_before,floating,generations,stop,seconds,evaluations";

/// Experiment numbers already recorded in summary.csv.
fn completed_ids(path: &Path) -> Result<BTreeSet<u32>, String> {
    let text = read(path)?;
    Ok(text.lines().skip(1).filter_map(|l| l.split(',').nth(1)).filter_map(|s| s.trim().parse().ok()).collect())
}

fn human(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}h{:02}m", s / 3600, (s / 60) % 60) } else { format!("{}m{:02}s", s / 60, s % 60) }
}

/// The files of a run, written by the main thread only.
struct Output {
    summary: std::fs::File,
    models: std::fs::File,
    models_dir: Option<PathBuf>,
    history: Option<std::fs::File>,
    sbml: bool,
}

impl Output {
    fn record(&mut self, r: &ExperimentResult, secs: f64) -> std::io::Result<()> {
        // Model first, then the summary line: a summary line means the
        // experiment is complete (resume relies on this).
        if r.viable {
            let sbml = if self.sbml { evolver_core::export::antimony_to_sbml(&r.antimony).ok() } else { None };
            let obj = serde_json::json!({
                "base_seed": r.base_seed, "experiment": r.id, "fitness": r.fitness,
                "reduced_fitness": r.reduced_fitness, "generations": r.generations,
                "reactions": r.reduced.reactions.len(), "floating": r.reduced.n_float,
                "antimony": r.antimony, "sbml": sbml,
            });
            writeln!(self.models, "{obj}")?;
            self.models.flush()?;
            if let Some(dir) = &self.models_dir {
                let stem = format!("model_s{}_e{:06}_f{:.4}", r.base_seed, r.id, r.fitness);
                std::fs::write(dir.join(format!("{stem}.txt")), &r.antimony)?;
                if let Some(x) = &sbml {
                    std::fs::write(dir.join(format!("{stem}.xml")), x)?;
                }
            }
        }
        if let Some(h) = self.history.as_mut() {
            let best: Vec<f64> = r.history.iter().map(|g| (g.best * 1e4).round() / 1e4).collect();
            let line = serde_json::json!({"experiment": r.id, "viable": r.viable, "best": best});
            writeln!(h, "{line}")?;
        }
        writeln!(
            self.summary,
            "{},{},{},{},{},{},{},{},{},{},{:?},{:.1},{}",
            r.base_seed,
            r.id,
            r.viable,
            r.fitness,
            r.evolved_fitness,
            r.reduced_fitness,
            r.reduced.reactions.len(),
            r.best.reactions.len(),
            r.reduced.n_float,
            r.generations,
            r.stop_reason,
            secs,
            r.evaluations
        )?;
        self.summary.flush()
    }
}

fn open_append(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))
}

fn run() -> Result<(), String> {
    let mut a = parse_args()?;
    let out = a.out.clone();
    let run_json = out.join("run.json");
    let summary_path = out.join("summary.csv");

    // Resume: settings, target and seed come from the folder, not the command line.
    let mut done = BTreeSet::new();
    let mut first = a.start;
    let mut total = a.cfg.num_experiments;
    if a.resume {
        let saved: RunFile =
            serde_json::from_str(&read(&run_json)?).map_err(|e| format!("{}: {e}", run_json.display()))?;
        let workers = a.cfg.num_workers;
        a.cfg = saved.configuration.settings;
        a.cfg.num_workers = workers; // this machine's choice
        a.target = saved.configuration.target;
        first = saved.first_experiment;
        total = saved.experiments;
        done = completed_ids(&summary_path).unwrap_or_default();
        println!("Resuming run in {}: seed {}, {} experiments already recorded", out.display(), a.cfg.seed, done.len());
    } else {
        if !a.seed_given {
            a.cfg.seed = evolver_core::rng::random_base_seed();
        }
        if a.dump {
            println!("{}", SharedConfig::new(a.cfg.clone(), a.target.clone(), "target").to_json());
            return Ok(());
        }
        if summary_path.exists() {
            return Err(format!(
                "{} already holds a run; use --resume to continue it, or choose another --out folder",
                out.display()
            ));
        }
    }
    a.cfg.validate().map_err(|e| e.join("; "))?;
    std::fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    if !a.resume {
        let saved = RunFile {
            configuration: SharedConfig::new(a.cfg.clone(), a.target.clone(), "target"),
            first_experiment: first,
            experiments: total,
        };
        let json = serde_json::to_string_pretty(&saved).map_err(|e| e.to_string())?;
        std::fs::write(&run_json, json).map_err(|e| e.to_string())?;
        std::fs::write(&summary_path, format!("{CSV_HEADER}\n")).map_err(|e| e.to_string())?;
    }
    let models_dir = a.model_files.then(|| out.join("models"));
    if let Some(d) = &models_dir {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let mut output = Output {
        summary: open_append(&summary_path)?,
        models: open_append(&out.join("models.jsonl"))?,
        models_dir,
        history: a.history.as_ref().map(|p| open_append(p)).transpose()?,
        sbml: a.sbml,
    };

    let continuous = a.cfg.continuous;
    let threads = a.cfg.worker_count(std::thread::available_parallelism().map_or(1, |n| n.get()));
    let end = if continuous { u32::MAX } else { first.saturating_add(total) };
    let todo = if continuous { None } else { Some((first..end).filter(|id| !done.contains(id)).count() as u32) };
    let threads = todo.map_or(threads, |t| threads.min(t.max(1) as usize));
    println!(
        "ModelEvolver CLI {} | objective {:?} | seed {} | {} threads | {} | output {}",
        evolver_core::export::VERSION,
        a.cfg.objective,
        a.cfg.seed,
        threads,
        match (a.duration, todo) {
            (Some(d), _) => format!("continuous for {d} s"),
            (None, Some(t)) => format!("experiments {first}..{} ({t} to run)", end - 1),
            (None, None) => "continuous".into(),
        },
        out.display()
    );

    let (ctx, pattern_errors) = RunContext::new(&a.cfg, a.target.clone());
    for e in &pattern_errors {
        eprintln!("patterns line {}: {}", e.line, e.message);
    }
    let cfg = Arc::new(a.cfg.clone());
    let ctx = Arc::new(ctx);
    let next_id = Arc::new(AtomicU32::new(first));
    let done = Arc::new(Mutex::new(done));
    let start = Instant::now();
    let deadline = a.duration.map(|d| start + Duration::from_secs_f64(d));
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(Result<ExperimentResult, String>, f64)>();
    let mut handles = Vec::new();
    for _ in 0..threads {
        let (cfg, ctx, next_id, done, tx, stop) =
            (cfg.clone(), ctx.clone(), next_id.clone(), done.clone(), tx.clone(), stop.clone());
        handles.push(std::thread::spawn(move || {
            loop {
                if stop.load(Ordering::Relaxed) || deadline.is_some_and(|d| Instant::now() >= d) {
                    break;
                }
                let id = next_id.fetch_add(1, Ordering::Relaxed);
                if id >= end {
                    break;
                }
                if done.lock().is_ok_and(|d| d.contains(&id)) {
                    continue;
                }
                let t = Instant::now();
                let r = Experiment::new(cfg.clone(), ctx.clone(), id, cfg.seed).run();
                if tx.send((r, t.elapsed().as_secs_f64())).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);

    let (mut finished, mut viable, mut failed, mut evaluations) = (0u64, 0u64, 0u64, 0u64);
    let mut last_report = Instant::now();
    let report = |finished: u64, viable: u64, failed: u64, now: Instant| {
        let el = (now - start).as_secs_f64();
        let rate = finished as f64 / el.max(1e-9);
        let eta = todo.filter(|_| finished > 0).map(|t| {
            let left = (u64::from(t)).saturating_sub(finished) as f64;
            human(left / rate)
        });
        println!(
            "[{}] {} done{}, {} successes ({:.1}/min, one every {}), {} failed{}",
            human(el),
            finished,
            todo.map_or(String::new(), |t| format!(" of {t}")),
            viable,
            viable as f64 / el.max(1e-9) * 60.0,
            if viable > 0 { human(el / viable as f64) } else { "-".into() },
            failed,
            eta.map_or(String::new(), |e| format!(", about {e} left")),
        );
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok((r, secs)) => {
                finished += 1;
                match r {
                    Ok(r) => {
                        evaluations += r.evaluations;
                        if r.viable {
                            viable += 1;
                        }
                        if let Err(e) = output.record(&r, secs) {
                            eprintln!("error writing results: {e}; stopping");
                            stop.store(true, Ordering::Relaxed);
                        }
                        if a.verbose {
                            println!(
                                "{} exp {:>7}  gens {:>5}  {:<14} fitness {:>9.4}  reactions {:>2} -> {:>2}  {:>6.1} s",
                                if r.viable { '*' } else { '.' },
                                r.id,
                                r.generations,
                                format!("{:?}", r.stop_reason),
                                r.fitness,
                                r.best.reactions.len(),
                                r.reduced.reactions.len(),
                                secs
                            );
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("experiment failed: {e}");
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if last_report.elapsed().as_secs_f64() >= a.progress_secs {
            report(finished, viable, failed, Instant::now());
            last_report = Instant::now();
        }
    }
    for h in handles {
        let _ = h.join();
    }
    println!("------------------------------------------------------------");
    report(finished, viable, failed, Instant::now());
    println!("{:.0} evaluations/s; results in {}", evaluations as f64 / start.elapsed().as_secs_f64(), out.display());
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(2);
    }
}

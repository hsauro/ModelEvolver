//! Headless batch runner: runs experiments in parallel on all cores and
//! writes the viable models. Used for development, validation against the
//! Pascal version, and throughput measurements.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use evolver_core::config::{EvolverConfig, ObjectiveKind};
use evolver_core::ga::{Experiment, ExperimentResult};
use evolver_core::objective::{RunContext, Target};

const HELP: &str = "\
evolver-cli - run ModelEvolver experiments headless

Options:
  --preset NAME        osc (default: settings of the Pascal oscillator corpus) or pascal (built-in defaults)
  --config FILE        load settings from a JSON file (see --dump-config)
  --pascal-ini FILE    apply a Pascal config.ini
  --objective NAME     osc or timecourse
  --target FILE        target time course (Pascal objectivefunction.txt format)
  --patterns FILE      reaction-pattern file
  -e N                 number of experiments (default 1)
  --duration SECS      continuous mode: keep starting experiments for SECS seconds
  -s SEED              base seed (0 or absent = from the clock)
  -mg N                max generations
  -mp N                max population size
  -nt N                worker threads (default: 75% of the hardware threads)
  --out DIR            write viable models to DIR (default: results)
  --no-write           do not write model files
  --history FILE       append one JSON line per experiment (best fitness per generation)
  --osc-window T N     oscillation evolution window (default 150 2048)
  --abandon G T        abandon experiments still above fitness T at generation G
                       (default 100 0.3; G = 0 turns it off)
  --quiet              one line per experiment only
  --dump-config        print the effective settings as JSON and exit
  -h, --help           this help
";

struct Args {
    cfg: EvolverConfig,
    target: Target,
    duration: Option<f64>,
    out: Option<PathBuf>,
    quiet: bool,
    dump: bool,
    history: Option<PathBuf>,
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg = EvolverConfig::oscillator_preset();
    let mut target = Target::default();
    let mut duration = None;
    let mut out = Some(PathBuf::from("results"));
    let (mut quiet, mut dump) = (false, false);
    let mut history = None;
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
                cfg = match next(&mut i)?.as_str() {
                    "osc" => EvolverConfig::oscillator_preset(),
                    "pascal" => EvolverConfig::default(),
                    p => return Err(format!("unknown preset '{p}'")),
                }
            }
            "--config" => {
                let p = next(&mut i)?;
                cfg = serde_json::from_str(&read(&p)?).map_err(|e| format!("{p}: {e}"))?;
            }
            "--pascal-ini" => {
                let p = next(&mut i)?;
                for w in cfg.apply_pascal_ini(&read(&p)?) {
                    eprintln!("{p}: {w}");
                }
            }
            "--objective" => {
                cfg.objective = match next(&mut i)?.as_str() {
                    "osc" => ObjectiveKind::Oscillation,
                    "timecourse" => ObjectiveKind::TimeCourse,
                    o => return Err(format!("unknown objective '{o}'")),
                }
            }
            "--target" => {
                let p = next(&mut i)?;
                target = Target::parse(&read(&p)?).map_err(|e| format!("{p}: {e}"))?;
            }
            "--patterns" => cfg.reaction_patterns = read(&next(&mut i)?)?,
            "-e" => cfg.num_experiments = num(next(&mut i)?)? as u32,
            "--duration" => {
                duration = Some(num(next(&mut i)?)?);
                cfg.continuous = true;
            }
            "-s" => cfg.seed = num(next(&mut i)?)? as u32,
            "-mg" => cfg.max_generations = num(next(&mut i)?)? as u32,
            "-mp" => cfg.max_population_size = num(next(&mut i)?)? as usize,
            "-nt" => cfg.num_workers = num(next(&mut i)?)? as u32,
            "--out" => out = Some(PathBuf::from(next(&mut i)?)),
            "--no-write" => out = None,
            "--quiet" => quiet = true,
            "--abandon" => {
                cfg.abandon_generation = num(next(&mut i)?)? as u32;
                cfg.abandon_threshold = num(next(&mut i)?)?;
            }
            "--history" => history = Some(PathBuf::from(next(&mut i)?)),
            "--osc-window" => {
                cfg.osc_t_end = num(next(&mut i)?)?;
                cfg.osc_n_points = num(next(&mut i)?)? as usize;
            }
            "--dump-config" => dump = true,
            other => return Err(format!("unknown option '{other}' (try --help)")),
        }
        i += 1;
    }
    Ok(Args { cfg, target, duration, out, quiet, dump, history })
}

/// Seven letters/digits, then '_', then seven more (as the Pascal session ids).
fn session_id(seed: u64) -> String {
    let letters = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = evolver_core::rng::Rng::new(seed);
    let mut s: String = (0..7).map(|_| letters[rng.index(letters.len())] as char).collect();
    s.push('_');
    s.extend((0..7).map(|_| letters[rng.index(letters.len())] as char));
    s
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let mut cfg = args.cfg;
    if args.dump {
        println!("{}", serde_json::to_string_pretty(&cfg).unwrap_or_default());
        return;
    }
    if let Err(errors) = cfg.validate() {
        for e in errors {
            eprintln!("error: {e}");
        }
        std::process::exit(2);
    }
    let clock = SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
    if cfg.seed == 0 {
        cfg.seed = (clock as u32).max(1);
    }
    let (ctx, pattern_errors) = RunContext::new(&cfg, args.target);
    for e in &pattern_errors {
        eprintln!("patterns line {}: {}", e.line, e.message);
    }
    let session = session_id(clock);
    if let Some(dir) = &args.out
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("error: cannot create {}: {e}", dir.display());
        std::process::exit(2);
    }

    let threads = cfg.worker_count(std::thread::available_parallelism().map_or(1, |n| n.get()));
    let total = if cfg.continuous { u32::MAX } else { cfg.num_experiments };
    let threads = threads.min(total as usize).max(1);
    println!(
        "ModelEvolver CLI {} | objective {:?} | seed {} | {} threads | {}",
        evolver_core::export::VERSION,
        cfg.objective,
        cfg.seed,
        threads,
        match args.duration {
            Some(d) => format!("continuous for {d} s"),
            None => format!("{total} experiments"),
        }
    );

    let cfg = Arc::new(cfg);
    let ctx = Arc::new(ctx);
    let next_id = Arc::new(AtomicU32::new(0));
    let start = Instant::now();
    let deadline = args.duration.map(|d| start + Duration::from_secs_f64(d));
    let (tx, rx) = mpsc::channel::<(Result<ExperimentResult, String>, f64)>();
    let mut handles = Vec::new();
    for _ in 0..threads {
        let (cfg, ctx, next_id, tx) = (cfg.clone(), ctx.clone(), next_id.clone(), tx.clone());
        handles.push(std::thread::spawn(move || {
            loop {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    break;
                }
                let id = next_id.fetch_add(1, Ordering::Relaxed);
                if id >= total {
                    break;
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

    let (mut done, mut viable, mut evaluations) = (0u32, 0u32, 0u64);
    let mut viable_times = Vec::new();
    let mut history_file = args.history.as_ref().map(|p| {
        std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap_or_else(|e| {
            eprintln!("error: cannot open {}: {e}", p.display());
            std::process::exit(2)
        })
    });
    for (r, secs) in rx {
        done += 1;
        match r {
            Ok(r) => {
                evaluations += r.evaluations;
                let mark = if r.viable { '*' } else { '.' };
                println!(
                    "{mark} exp {:>4}  gens {:>5}  {:<14} fitness {:>10.4}  reduced {:>10.4}  reactions {:>2} -> {:>2}  floating {}  {:>6.1} s",
                    r.id,
                    r.generations,
                    format!("{:?}", r.stop_reason),
                    r.fitness,
                    r.reduced_fitness,
                    r.best.reactions.len(),
                    r.reduced.reactions.len(),
                    r.reduced.n_float,
                    secs
                );
                if let Some(f) = history_file.as_mut() {
                    use std::io::Write as _;
                    let best: Vec<f64> = r.history.iter().map(|h| (h.best * 1e4).round() / 1e4).collect();
                    let line = serde_json::json!({
                        "id": r.id, "viable": r.viable, "stop": format!("{:?}", r.stop_reason),
                        "generations": r.generations, "fitness": r.fitness, "secs": secs,
                        "evaluations": r.evaluations, "best": best,
                    });
                    let _ = writeln!(f, "{line}");
                }
                if r.viable {
                    viable += 1;
                    viable_times.push(start.elapsed().as_secs_f64());
                    if !args.quiet {
                        println!("{}", r.antimony);
                    }
                    if let Some(dir) = &args.out {
                        let file = dir.join(format!("M{session}_{}_{:.4}.txt", r.id, r.fitness));
                        if let Err(e) = std::fs::write(&file, &r.antimony) {
                            eprintln!("cannot write {}: {e}", file.display());
                        }
                    }
                }
            }
            Err(e) => println!("! experiment failed: {e}"),
        }
    }
    for h in handles {
        let _ = h.join();
    }
    let wall = start.elapsed().as_secs_f64();
    println!("------------------------------------------------------------");
    println!("experiments {done}, viable {viable} ({:.0}%)", 100.0 * f64::from(viable) / f64::from(done.max(1)));
    println!(
        "wall time {wall:.1} s, {:.1} viable models/min, one every {:.1} s, {:.0} evaluations/s",
        f64::from(viable) / wall * 60.0,
        if viable > 0 { wall / f64::from(viable) } else { f64::INFINITY },
        evaluations as f64 / wall
    );
}

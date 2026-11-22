//! Saving and loading a whole session (every result plus the settings of
//! each run), so prepared runs can be shown later without waiting for them.

use serde::{Deserialize, Serialize};

use evolver_core::config::EvolverConfig;
use evolver_core::ga::ExperimentResult;
use evolver_core::objective::Target;

pub const SESSION_FORMAT: &str = "ModelEvolver session";

/// JSON cannot hold infinity (failed experiments have infinite fitness), so
/// non-finite values are stored as f64::MAX and turned back on load.
fn to_file(x: f64) -> f64 {
    if x.is_finite() { x } else { f64::MAX }
}

fn from_file(x: f64) -> f64 {
    if x == f64::MAX { f64::INFINITY } else { x }
}

fn map_fitness(r: &mut ExperimentResult, f: fn(f64) -> f64) {
    r.fitness = f(r.fitness);
    r.evolved_fitness = f(r.evolved_fitness);
    r.reduced_fitness = f(r.reduced_fitness);
    r.node_scores.iter_mut().for_each(|v| *v = f(*v));
}

/// One run (one press of Start) and how it went.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub run: u32,
    pub settings: EvolverConfig,
    pub target: Target,
    pub base_seed: u32,
    pub workers: usize,
    /// Run time used for the throughput figures (stops at Stop).
    pub active_secs: f64,
    /// Total real time of the run.
    pub elapsed_secs: f64,
    pub experiments: u32,
    pub successes: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResult {
    pub run: u32,
    /// How long this experiment took.
    pub secs: f64,
    pub result: ExperimentResult,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionFile {
    pub format: String,
    pub version: u32,
    pub created_by: String,
    pub runs: Vec<RunRecord>,
    pub results: Vec<SessionResult>,
}

impl SessionFile {
    pub fn new(runs: Vec<RunRecord>, mut results: Vec<SessionResult>) -> Self {
        // The per-generation histories and stored trajectories are not used by
        // the restored views (plots are re-simulated), and they make up most of
        // the size, so they are left out.
        for r in &mut results {
            r.result.history.clear();
            r.result.display = None;
            map_fitness(&mut r.result, to_file);
        }
        Self {
            format: SESSION_FORMAT.into(),
            version: 1,
            created_by: format!("ModelEvolver {}", evolver_core::export::VERSION),
            runs,
            results,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        if text.starts_with("PK") {
            return Err("this is a zip file (e.g. from 'Save all successes'), which holds only model files.                         A session is saved with 'Save session...' as a .json file."
                .into());
        }
        let mut s: SessionFile = serde_json::from_str(text).map_err(|e| {
            if text.contains(evolver_core::objective::SHARED_CONFIG_FORMAT) {
                "this is a configuration file; use Load configuration (top of the Settings panel)".to_string()
            } else if !text.contains(SESSION_FORMAT) {
                format!("this file is not a ModelEvolver session (it does not contain '{SESSION_FORMAT}')")
            } else {
                format!("the session file could not be read: {e}")
            }
        })?;
        if s.format != SESSION_FORMAT {
            return Err("this file is not a ModelEvolver session".into());
        }
        if s.version > 1 {
            return Err(format!("session version {} is newer than this program supports", s.version));
        }
        if let Some(r) = s.results.iter().find(|r| !s.runs.iter().any(|run| run.run == r.run)) {
            return Err(format!("session is damaged: result of run {} has no run settings", r.run));
        }
        for r in &mut s.results {
            map_fitness(&mut r.result, from_file);
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evolver_app::tasks::{Reply, RunRequest, run_experiment};
    use evolver_core::ga::ExperimentEvent;

    #[test]
    fn realistic_session_round_trip() {
        let cfg = EvolverConfig {
            max_generations: 30,
            max_population_size: 20,
            osc_t_end: 60.0,
            osc_n_points: 256,
            osc_confirm_t_end: 120.0,
            osc_confirm_n_points: 512,
            ..EvolverConfig::oscillator_preset()
        };
        let mut results = Vec::new();
        for id in 0..12 {
            run_experiment(
                RunRequest { id, base_seed: 77, cfg: cfg.clone(), target: Target::default() },
                |r| {
                    if let Reply::Event(ExperimentEvent::Finished(x)) = r {
                        results.push(SessionResult { run: 1, secs: 2.0, result: *x });
                    }
                },
                || false,
            );
        }
        assert_eq!(results.len(), 12);
        let run = RunRecord {
            run: 1,
            settings: cfg,
            target: Target::default(),
            base_seed: 77,
            workers: 7,
            active_secs: 60.0,
            elapsed_secs: 70.0,
            experiments: 12,
            successes: results.iter().filter(|r| r.result.viable).count() as u32,
        };
        let file = SessionFile::new(vec![run], results);
        let json = file.to_json();
        let back = SessionFile::from_json(&json).expect("a saved session must load");
        assert_eq!(back.results.len(), 12);
        for (a, b) in file.results.iter().zip(&back.results) {
            assert_eq!(a.result.reduced, b.result.reduced);
            assert_eq!(a.result.antimony, b.result.antimony);
            assert_eq!(a.result.viable, b.result.viable);
        }
    }

    #[test]
    fn round_trip_with_infinite_fitness() {
        let cfg = EvolverConfig { max_generations: 1, max_population_size: 6, osc_n_points: 64, osc_confirm_t_end: 0.0, ..Default::default() };
        let mut res = None;
        run_experiment(
            RunRequest { id: 3, base_seed: 9, cfg: cfg.clone(), target: Target::default() },
            |r| {
                if let Reply::Event(ExperimentEvent::Finished(x)) = r {
                    res = Some(*x);
                }
            },
            || false,
        );
        let mut result = res.expect("finished");
        result.fitness = f64::INFINITY;
        let run = RunRecord {
            run: 1,
            settings: cfg,
            target: Target::default(),
            base_seed: 9,
            workers: 7,
            active_secs: 12.0,
            elapsed_secs: 15.0,
            experiments: 1,
            successes: 0,
        };
        let file = SessionFile::new(vec![run], vec![SessionResult { run: 1, secs: 1.5, result }]);
        let back = SessionFile::from_json(&file.to_json()).unwrap();
        assert_eq!(back.results.len(), 1);
        assert_eq!(back.results[0].result.id, 3);
        assert_eq!(back.results[0].result.reduced, file.results[0].result.reduced);
        // Infinite fitness (a failed experiment) must survive the JSON file.
        assert!(back.results[0].result.fitness.is_infinite());
        assert!(SessionFile::from_json("{}").is_err());
    }
}

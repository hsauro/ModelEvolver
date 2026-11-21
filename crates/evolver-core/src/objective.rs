//! Fitness functions (lower is better) and the shared inputs of a run.

use serde::{Deserialize, Serialize};

use crate::config::{EvolverConfig, ObjectiveKind};
use crate::network::Network;
use crate::osc::score_oscillation;
use crate::patterns::{PatternError, PatternSet};
use crate::simulate::{SimError, SimOptions, Trajectory, simulate};

/// A target time course for the time-course objective, in the Pascal
/// `objectivefunction.txt` format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub t_start: f64,
    pub t_end: f64,
    pub values: Vec<f64>,
}

impl Default for Target {
    /// The target shipped with the Pascal version: a 5/30 square wave, 10 points over 0..10.
    fn default() -> Self {
        Self { t_start: 0.0, t_end: 10.0, values: [5.0, 30.0].repeat(5) }
    }
}

impl Target {
    pub fn n_points(&self) -> usize {
        self.values.len()
    }

    /// Sample times; the Pascal program integrates from t = 0 with spacing
    /// `(t_end - t_start) / (n - 1)`, so the target is compared on that grid.
    pub fn sim_t_end(&self) -> f64 {
        self.t_end - self.t_start
    }

    /// Parse the Pascal format: comment lines starting with '#', then
    /// `timeStart v`, `timeEnd v` (or `stepSize`), `numberOfPoints n`, then n values.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (mut t_start, mut t_end, mut n) = (None, None, None);
        let mut values = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }
            let mut parts = line.split_whitespace();
            let first = parts.next().unwrap_or_default();
            let num = |s: Option<&str>| -> Result<f64, String> {
                s.and_then(|v| v.parse::<f64>().ok()).ok_or_else(|| format!("line {}: expected a number after '{first}'", i + 1))
            };
            match first.to_ascii_lowercase().as_str() {
                "timestart" => t_start = Some(num(parts.next())?),
                "timeend" | "stepsize" => t_end = Some(num(parts.next())?),
                "numberofpoints" => n = Some(num(parts.next())? as usize),
                _ => values.push(first.parse::<f64>().map_err(|_| format!("line {}: '{first}' is not a number", i + 1))?),
            }
        }
        let n = n.ok_or("missing 'numberOfPoints'")?;
        let t_start = t_start.unwrap_or(0.0);
        let t_end = t_end.ok_or("missing 'timeEnd'")?;
        if n < 2 {
            return Err("numberOfPoints must be at least 2".into());
        }
        if t_end <= t_start {
            return Err("timeEnd must be larger than timeStart".into());
        }
        if values.len() < n {
            return Err(format!("expected {n} values, found {}", values.len()));
        }
        values.truncate(n);
        Ok(Self { t_start, t_end, values })
    }
}

/// Immutable inputs shared by every experiment of a run.
#[derive(Clone, Debug)]
pub struct RunContext {
    pub target: Target,
    pub patterns: PatternSet,
    /// Initial concentration of each species (length = max species).
    pub init: Vec<f64>,
}

impl RunContext {
    pub fn new(cfg: &EvolverConfig, target: Target) -> (Self, Vec<PatternError>) {
        let (patterns, errors) =
            PatternSet::parse(&cfg.reaction_patterns, cfg.disallow_self_referential, cfg.disallow_autocatalysis);
        let init = cfg.effective_initial(cfg.max_species.max(1));
        (Self { target, patterns, init }, errors)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    /// The window used during evolution.
    Evolve,
    /// The longer oscillation confirmation window (same as Evolve for time-course).
    Confirm,
}

#[derive(Clone, Debug)]
pub struct Evaluation {
    pub fitness: f64,
    /// Per floating species (state order): 1 - F, or the squared deviation.
    pub node_scores: Vec<f64>,
    pub trajectory: Trajectory,
}

/// The simulation settings for a window.
pub fn window_options(cfg: &EvolverConfig, ctx: &RunContext, window: Window) -> SimOptions {
    match cfg.objective {
        ObjectiveKind::TimeCourse => cfg.sim_options(ctx.target.sim_t_end(), ctx.target.n_points()),
        ObjectiveKind::Oscillation => match window {
            Window::Confirm if cfg.osc_confirm_t_end > 0.0 => {
                cfg.sim_options(cfg.osc_confirm_t_end, cfg.osc_confirm_n_points)
            }
            _ => cfg.sim_options(cfg.osc_t_end, cfg.osc_n_points),
        },
    }
}

/// Simulate `net` and score it. Fitness is the best (lowest) node score.
pub fn evaluate(net: &Network, cfg: &EvolverConfig, ctx: &RunContext, window: Window) -> Result<Evaluation, SimError> {
    let opts = window_options(cfg, ctx, window);
    if net.n_species > ctx.init.len() {
        return Err(SimError::Setup(format!(
            "{} species but only {} initial concentrations",
            net.n_species,
            ctx.init.len()
        )));
    }
    let model = net.compile(&ctx.init);
    let trajectory = simulate(&model, &opts)?;
    let node_scores: Vec<f64> = match cfg.objective {
        ObjectiveKind::Oscillation => {
            let dt = opts.t_end / (opts.n_points - 1) as f64;
            trajectory.cols.iter().map(|c| 1.0 - score_oscillation(c, dt)).collect()
        }
        ObjectiveKind::TimeCourse => trajectory
            .cols
            .iter()
            .map(|c| c.iter().zip(&ctx.target.values).map(|(y, t)| (y - t) * (y - t)).sum())
            .collect(),
    };
    let fitness = node_scores.iter().cloned().fold(f64::INFINITY, f64::min);
    Ok(Evaluation { fitness, node_scores, trajectory })
}

/// Oscillation fitness with explicit options (used by the M0 benchmark).
pub fn oscillation_fitness(
    net: &Network,
    init: &[f64],
    opts: &SimOptions,
) -> Result<(f64, Vec<f64>, Trajectory), SimError> {
    let model = net.compile(init);
    let traj = simulate(&model, opts)?;
    let dt = opts.t_end / (opts.n_points.max(2) - 1) as f64;
    let scores: Vec<f64> = traj.cols.iter().map(|c| score_oscillation(c, dt)).collect();
    let fitness = scores.iter().map(|f| 1.0 - f).fold(f64::INFINITY, f64::min);
    Ok((fitness, scores, traj))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::samples::pascal_oscillator;

    #[test]
    fn parse_pascal_target() {
        let text = "# Expected Output\ntimeStart 0.0\ntimeEnd 10\nnumberOfPoints 10\n5\n30.0\n5\n30.0\n5\n30.0\n5\n30.0\n5\n30.0\n\n";
        let t = Target::parse(text).unwrap();
        assert_eq!(t, Target::default());
        assert!(Target::parse("timeEnd 1\nnumberOfPoints 3\n1\n2").is_err());
    }

    #[test]
    fn evaluates_known_oscillator() {
        let cfg = EvolverConfig::default();
        let (ctx, _) = RunContext::new(&cfg, Target::default());
        let e = evaluate(&pascal_oscillator(), &cfg, &ctx, Window::Evolve).unwrap();
        assert!(e.fitness < 0.05, "fitness {}", e.fitness);
        let c = evaluate(&pascal_oscillator(), &cfg, &ctx, Window::Confirm).unwrap();
        assert!(c.fitness < 0.05, "confirm fitness {}", c.fitness);
        assert_eq!(c.trajectory.t.len(), cfg.osc_confirm_n_points);
    }

    #[test]
    fn time_course_ssd() {
        let cfg = EvolverConfig { objective: ObjectiveKind::TimeCourse, ..Default::default() };
        let (ctx, _) = RunContext::new(&cfg, Target::default());
        let e = evaluate(&pascal_oscillator(), &cfg, &ctx, Window::Evolve).unwrap();
        assert_eq!(e.trajectory.t.len(), 10);
        assert_eq!(e.node_scores.len(), 5);
        assert!(e.fitness > 0.0);
    }
}

/// A complete, shareable setup: every setting plus the target time course.
/// Saved as JSON so it can be sent to other people and loaded by the app or
/// the CLI. With a fixed (non-zero) seed, a shared setup reproduces the same
/// results exactly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedConfig {
    /// Always "ModelEvolver configuration"; identifies the file.
    pub format: String,
    /// File format version.
    pub version: u32,
    /// Program version that wrote the file.
    pub created_by: String,
    pub settings: EvolverConfig,
    pub target: Target,
    pub target_name: String,
}

pub const SHARED_CONFIG_FORMAT: &str = "ModelEvolver configuration";

impl SharedConfig {
    pub fn new(settings: EvolverConfig, target: Target, target_name: impl Into<String>) -> Self {
        Self {
            format: SHARED_CONFIG_FORMAT.into(),
            version: 1,
            created_by: format!("ModelEvolver {}", env!("CARGO_PKG_VERSION")),
            settings,
            target,
            target_name: target_name.into(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Read a configuration file. Also accepts an older settings-only file
    /// (a bare settings object), which then keeps the default target.
    pub fn from_json(text: &str) -> Result<Self, String> {
        if let Ok(c) = serde_json::from_str::<SharedConfig>(text) {
            if c.format != SHARED_CONFIG_FORMAT {
                return Err(format!("not a ModelEvolver configuration (format '{}')", c.format));
            }
            if c.version > 1 {
                return Err(format!("configuration version {} is newer than this program supports", c.version));
            }
            c.target_is_valid()?;
            return Ok(c);
        }
        match serde_json::from_str::<EvolverConfig>(text) {
            Ok(settings) if text.contains("max_population_size") => {
                Ok(SharedConfig::new(settings, Target::default(), "built-in square wave"))
            }
            _ => Err("this file is not a ModelEvolver configuration".into()),
        }
    }

    fn target_is_valid(&self) -> Result<(), String> {
        let t = &self.target;
        if t.values.len() < 2 || t.t_end <= t.t_start || t.values.iter().any(|v| !v.is_finite()) {
            return Err("the configuration's target time course is invalid".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod shared_config_tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = EvolverConfig::oscillator_preset();
        s.seed = 1234;
        s.reaction_patterns = "mode: disallow\nA + B -> A".into();
        let t = Target { t_start: 0.0, t_end: 20.0, values: vec![1.0, 2.0, 3.0] };
        let c = SharedConfig::new(s, t, "my target");
        let back = SharedConfig::from_json(&c.to_json()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn accepts_old_settings_only_files() {
        let old = serde_json::to_string(&EvolverConfig::default()).unwrap();
        let c = SharedConfig::from_json(&old).unwrap();
        assert_eq!(c.settings, EvolverConfig::default());
        assert_eq!(c.target, Target::default());
    }

    #[test]
    fn rejects_other_files() {
        assert!(SharedConfig::from_json("{\"hello\": 1}").is_err());
        assert!(SharedConfig::from_json("not json").is_err());
        let mut c = SharedConfig::new(EvolverConfig::default(), Target::default(), "x");
        c.format = "something else".into();
        assert!(SharedConfig::from_json(&c.to_json()).is_err());
    }
}

//! Time-course simulation with diffsol's adaptive BDF solver.
//!
//! The solver is stepped manually and the solution is sampled on a uniform
//! output grid by interpolation, so we keep control of the failure rules the
//! GA relies on: solver errors, a cap on internal steps per output interval,
//! and runaway concentrations all end the run early with a [`SimError`].

use diffsol::{NalgebraLU, NalgebraMat, OdeBuilder, OdeSolverMethod, OdeSolverStopReason, VectorCommon};

use crate::compile::CompiledModel;

#[derive(Clone, Copy, Debug)]
pub struct SimOptions {
    pub t_end: f64,
    /// Number of output samples, including t = 0 and t = t_end.
    pub n_points: usize,
    pub rtol: f64,
    pub atol: f64,
    /// Give up if the solver needs more internal steps than this between two
    /// output points (CVODE's `mxstep` default is 500).
    pub max_steps_per_interval: usize,
    /// Give up after this many internal steps in total (bounds the cost of one
    /// evaluation so a pathological model cannot stall a worker).
    pub max_total_steps: usize,
    /// Any species beyond this magnitude, or NaN/Inf, is a runaway.
    pub max_concentration: f64,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            t_end: 150.0,
            n_points: 2048,
            rtol: 1e-6,
            atol: 1e-10,
            max_steps_per_interval: 500,
            max_total_steps: 200_000,
            max_concentration: 1000.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SimError {
    NoStates,
    Setup(String),
    Solver { t: f64, message: String },
    TooManySteps { t: f64 },
    Runaway { t: f64 },
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimError::NoStates => write!(f, "the model has no floating species"),
            SimError::Setup(m) => write!(f, "could not set up the solver: {m}"),
            SimError::Solver { t, message } => write!(f, "solver failed at t = {t:.4}: {message}"),
            SimError::TooManySteps { t } => write!(f, "too many solver steps near t = {t:.4}"),
            SimError::Runaway { t } => write!(f, "a concentration ran away at t = {t:.4}"),
        }
    }
}

/// A sampled solution: `t[i]`, and one column per floating species.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Trajectory {
    pub t: Vec<f64>,
    pub cols: Vec<Vec<f64>>,
}

/// Simulate `model` from t = 0 to `opts.t_end`, sampled at `opts.n_points`
/// evenly spaced times.
pub fn simulate(model: &CompiledModel, opts: &SimOptions) -> Result<Trajectory, SimError> {
    let n = model.n_states();
    if n == 0 {
        return Err(SimError::NoStates);
    }
    let n_points = opts.n_points.max(2);
    let y0 = model.initial().to_vec();

    let problem = OdeBuilder::<NalgebraMat<f64>>::new()
        .t0(0.0)
        .rtol(opts.rtol)
        .atol([opts.atol])
        .rhs_implicit(
            |y, _p, _t, out| model.rhs(y, out),
            |y, _p, _t, v, out| model.jac_mul(y, v, out),
        )
        .init(move |_p, _t, y| y.copy_from_slice(&y0), n)
        .build()
        .map_err(|e| SimError::Setup(e.to_string()))?;
    let mut solver = problem.bdf::<NalgebraLU<f64>>().map_err(|e| SimError::Setup(e.to_string()))?;
    // Our own step limits apply; lift diffsol's default cap of 5000 steps per solve.
    solver.config_mut().maximum_number_timesteps = opts.max_total_steps;
    solver.set_stop_time(opts.t_end).map_err(|e| SimError::Setup(e.to_string()))?;

    let mut traj = Trajectory { t: Vec::with_capacity(n_points), cols: vec![Vec::with_capacity(n_points); n] };
    let grid = |i: usize| opts.t_end * i as f64 / (n_points - 1) as f64;
    let push = |traj: &mut Trajectory, t: f64, y: &[f64]| -> Result<(), SimError> {
        traj.t.push(t);
        for (col, &v) in traj.cols.iter_mut().zip(y) {
            if !v.is_finite() || v.abs() > opts.max_concentration {
                return Err(SimError::Runaway { t });
            }
            col.push(v);
        }
        Ok(())
    };
    push(&mut traj, 0.0, model.initial())?;

    let mut buf = solver.state().y.clone();
    let mut next = 1;
    let mut steps_since_output = 0usize;
    while next < n_points {
        let reason = solver.step().map_err(|e| SimError::Solver { t: solver.state().t, message: e.to_string() })?;
        steps_since_output += 1;
        let t_now = solver.state().t;
        let at_end = matches!(reason, OdeSolverStopReason::TstopReached);
        while next < n_points && (grid(next) <= t_now || (at_end && next == n_points - 1)) {
            let t = grid(next);
            solver
                .interpolate_inplace(t.min(t_now), &mut buf)
                .map_err(|e| SimError::Solver { t, message: e.to_string() })?;
            push(&mut traj, t, buf.inner().as_slice())?;
            next += 1;
            steps_since_output = 0;
        }
        if steps_since_output > opts.max_steps_per_interval {
            return Err(SimError::TooManySteps { t: t_now });
        }
        if at_end && next < n_points {
            return Err(SimError::Solver { t: t_now, message: "stopped before the last output time".into() });
        }
    }
    Ok(traj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Network, Reaction, ReactionKind::*};

    #[test]
    fn exponential_decay() {
        // S0 -> S1 -> $S2 with k = 0.7 on the first step; S0(0) = 2.
        let net = Network::new(
            0,
            3,
            vec![Reaction::new(UniUni, 0.7, &[0], &[1]), Reaction::new(UniUni, 0.3, &[1], &[2]), Reaction::new(UniUni, 0.1, &[1], &[0])],
        );
        let m = net.compile(&[2.0, 0.0, 0.0]);
        let opts = SimOptions { t_end: 5.0, n_points: 51, ..Default::default() };
        let tr = simulate(&m, &opts).unwrap();
        assert_eq!(tr.t.len(), 51);
        assert!((tr.t[50] - 5.0).abs() < 1e-12);
        // Check conservation-free sanity: S0 + S1 decays at rate 0.3*S1 only.
        // Compare against a fine explicit reference integration (test only).
        let (mut a, mut b) = (2.0f64, 0.0f64);
        let h = 1e-5;
        for _ in 0..(5.0 / h) as usize {
            let da = -0.7 * a + 0.1 * b;
            let db = 0.7 * a - 0.3 * b - 0.1 * b;
            a += h * da;
            b += h * db;
        }
        assert!((tr.cols[0][50] - a).abs() < 1e-3, "{} vs {a}", tr.cols[0][50]);
        assert!((tr.cols[1][50] - b).abs() < 1e-3, "{} vs {b}", tr.cols[1][50]);
    }

    #[test]
    fn robertson_stiff() {
        // A -> B (0.04); B + B -> C + B (3e7); B + C -> A + C (1e4). Classic stiff test.
        let net = Network::new(
            0,
            3,
            vec![
                Reaction::new(UniUni, 0.04, &[0], &[1]),
                Reaction::new(BiBi, 3e7, &[1, 1], &[2, 1]),
                Reaction::new(BiBi, 1e4, &[1, 2], &[0, 2]),
            ],
        );
        let m = net.compile(&[1.0, 0.0, 0.0]);
        let opts = SimOptions { t_end: 40.0, n_points: 5, atol: 1e-10, ..Default::default() };
        let tr = simulate(&m, &opts).unwrap();
        // Reference values at t = 40 (Hairer & Wanner): A = 0.7158, B = 9.185e-6, C = 0.2842.
        assert!((tr.cols[0][4] - 0.7158).abs() < 1e-3);
        assert!((tr.cols[1][4] - 9.185e-6).abs() < 1e-7);
        assert!((tr.cols[2][4] - 0.2842).abs() < 1e-3);
    }

    #[test]
    fn runaway_is_detected() {
        // S0 -> 2 S0 grows without bound (S0 is consumed and produced, so floating).
        let net = Network::new(0, 1, vec![Reaction::new(UniBi, 1.0, &[0], &[0, 0])]);
        let m = net.compile(&[1.0]);
        let opts = SimOptions { t_end: 50.0, n_points: 100, ..Default::default() };
        assert!(matches!(simulate(&m, &opts), Err(SimError::Runaway { .. })));
    }
}

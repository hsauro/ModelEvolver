//! Fitness functions (lower is better).

use crate::network::Network;
use crate::osc::score_oscillation;
use crate::simulate::{SimError, SimOptions, Trajectory, simulate};

/// Oscillation fitness: `min over floating species of (1 - F)`, plus the
/// per-species scores F (in state order) and the trajectory.
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

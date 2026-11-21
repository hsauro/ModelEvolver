//! M0 checks: diffsol BDF on standard problems and on a known evolved oscillator.

use diffsol::{NalgebraLU, NalgebraMat, OdeBuilder, OdeSolverMethod};
use evolver_core::osc::score_oscillation;
use evolver_core::samples::{PASCAL_INITIAL, pascal_oscillator};
use evolver_core::simulate::{SimOptions, simulate};

/// Brusselator (A = 1, B = 3): a limit cycle. Solved directly with diffsol BDF
/// because it has a trimolecular step our mass-action terms do not cover.
#[test]
fn brusselator_bdf_oscillates() {
    let (a, b) = (1.0, 3.0);
    let problem = OdeBuilder::<NalgebraMat<f64>>::new()
        .rtol(1e-6)
        .atol([1e-10])
        .rhs_implicit(
            move |y: &[f64], _p: &[f64], _t: f64, f: &mut [f64]| {
                f[0] = a + y[0] * y[0] * y[1] - (b + 1.0) * y[0];
                f[1] = b * y[0] - y[0] * y[0] * y[1];
            },
            move |y: &[f64], _p: &[f64], _t: f64, v: &[f64], jv: &mut [f64]| {
                jv[0] = (2.0 * y[0] * y[1] - (b + 1.0)) * v[0] + y[0] * y[0] * v[1];
                jv[1] = (b - 2.0 * y[0] * y[1]) * v[0] - y[0] * y[0] * v[1];
            },
        )
        .init(|_p, _t, y| y.copy_from_slice(&[1.5, 3.0]), 2)
        .build()
        .unwrap();
    let mut solver = problem.bdf::<NalgebraLU<f64>>().unwrap();
    let n = 2048;
    let t_eval: Vec<f64> = (0..n).map(|i| 150.0 * i as f64 / (n - 1) as f64).collect();
    let (soln, _) = solver.solve_dense(&t_eval).unwrap();
    use diffsol::MatrixCommon;
    let x: Vec<f64> = (0..n).map(|i| soln.inner()[(0, i)]).collect();
    let f = score_oscillation(&x, t_eval[1]);
    assert!(f > 0.7, "Brusselator F = {f}");
}

#[test]
fn pascal_oscillator_still_oscillates() {
    let net = pascal_oscillator();
    assert_eq!(net.n_float, 5);
    let model = net.compile(&PASCAL_INITIAL);
    let opts = SimOptions::default(); // t = 150, 2048 points, as the Pascal osc window
    let tr = simulate(&model, &opts).unwrap();
    let dt = tr.t[1];
    let scores: Vec<f64> = tr.cols.iter().map(|c| score_oscillation(c, dt)).collect();
    let best = scores.iter().cloned().fold(0.0, f64::max);
    // Pascal reported node fitness ~0.01 (F ~ 0.99) with CVODE.
    assert!(best > 0.9, "scores = {scores:?}");
}

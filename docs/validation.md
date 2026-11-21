# Validation and performance log

## M0 - solver spike (2026-11-21)

Machine: development desktop, 20 hardware threads, Windows 10, Chrome.
Builds: `--release` (lto = fat, codegen-units = 1), wasm via trunk with
`wasm-opt -O3`. diffsol 0.20.0, nalgebra dense LU, BDF only.

### Correctness

| Check | Result |
|---|---|
| Exponential decay chain vs fine reference | max error < 1e-3 at t = 5 |
| Robertson (stiff), t = 40 | A = 0.7158, B = 9.185e-6, C = 0.2842 (Hairer & Wanner values), within 1e-3 / 1e-7 |
| Brusselator (A = 1, B = 3), BDF via `solve_dense` | oscillation score F > 0.7 |
| Pascal-evolved oscillator (results_osc_1, seed 10003) | F = 0.981, 0.980, 0.983, 0.983, 0.978 per species (Pascal/CVODE reported node fitness ~0.01, i.e. F ~ 0.99) |
| Runaway detection (`S0 -> 2 S0`) | `SimError::Runaway`, no panic |
| Analytic Jacobian vs finite differences, 200 random networks | agree to 1e-6 relative |
| Oscillation score vs Python reference (`objective_prototype.py`) | sine 0.886245, spiky 0.729133, damped 0.001793, ramp 0, flat 0: all match to 1e-5 |
| Same oscillator simulated in the browser (eframe, main thread) | same F values; 24 ms |
| `antimony-rs` (vendored copy) | 455 tests pass natively; builds for `wasm32-unknown-unknown` with the `sbml` feature |

### Cost of one fitness evaluation

Oscillation objective: simulate t = 0..150 with 2048 output points, then score
every floating species. "Random" = GA-like random networks (5 species, 9
reactions, k up to 12.3); about 40% fail (runaway or solver failure) and stop early.

| | Native | Browser (wasm, Web Workers) |
|---|---|---|
| Random networks, 1 thread | 2,235 evals/s (0.45 ms) | 1,136 evals/s (0.88 ms) |
| 4 threads | 7,624 | 4,194 |
| 10 threads | 16,792 | 8,989 |
| 20 threads | 20,810 | 12,278 (10,485 through the app UI) |
| A real oscillator, 1 thread | 8 ms/eval | ~24 ms on the main thread (single run, includes first-use cost) |

Observations:

- Browser parallel scaling is good (10.8x on 20 hardware threads, about 10
  physical cores); wasm is about 2x slower than native per thread.
- The first benchmark after page load ran about 3.5x slower: the browser runs
  baseline wasm code until its optimising compiler finishes. Workers are
  long-lived, so real runs pay this once.
- Oscillating networks cost about 18x more than the average random network
  (many more solver steps over 150 time units). Late in a run, when most of the
  population oscillates, evaluation cost is dominated by them. The biggest
  remaining levers are elite fitness caching (spec 8.3), and checking whether
  `atol = 1e-10` (inherited from the Pascal CVODE setup) is tighter than the
  objective needs. Both will be measured in M2.

### Build sizes

| Artifact | Size |
|---|---|
| App wasm (eframe + glow renderer) | 4.9 MB (7.1 MB with the default wgpu renderer) |
| Worker wasm (core + diffsol only) | 0.46 MB |

### Findings that changed the plan

- diffsol caps a solve at 5000 steps by default (`BdfConfig::maximum_number_timesteps`).
  A 150-time-unit oscillator needs more, so `simulate` lifts that cap to its
  own `max_total_steps` (default 200,000) and relies on the per-interval step limit.
- eframe 0.37 defaults to the wgpu renderer. We use glow (WebGL) instead: smaller,
  and it works in every browser.
- The Pascal oscillation score differs from the Python reference it was derived
  from (see objectpascal_bugs.md, B19). The Rust port follows Python.

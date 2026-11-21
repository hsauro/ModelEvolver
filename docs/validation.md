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

## M1 - GA core and CLI (2026-11-21)

Native, release build, 20 threads, `evolver-cli` with the oscillator preset
(Pascal config.ini corpus settings: population 130, 5 species, 9 reactions;
oscillation window 150 / 2048 points, confirmation 300 / 4096).

| Run | Experiments | Viable | Rate |
|---|---|---|---|
| Fixed batch, `-e 20 -s 1` | 20 | 15 (75%) | one every 17.0 s (idle cores at the end of the batch) |
| Continuous, `--duration 180 -s 77` | 47 | 34 (72%) | one every 11.5 s (5.2/min), 8,660 evaluations/s |
| Time-course mode, Pascal square-wave target, `-e 20 -s 3` | 20 | 9 (45%) | one every 17.6 s |

- Experiments that converge take 5-250 s; non-converging ones run the full 1600
  generations (about 220-260 s). The confirmation window rejected several
  candidates that passed on the evolution window (fitness `inf`: the long
  simulation failed or ran away).
- Model reduction typically removes a third to two thirds of the reactions
  (for example 16 -> 9, 41 -> 11).

### Independent check in Tellurium

`results_m1/MlzYbYdX_3jIr1GL_10_0.0277.txt` (9 reactions, 5 floating species)
loaded unchanged with `te.loada`, simulated with CVODE (rtol 1e-6, atol 1e-10) on
t = 0..300 (4096 points), scored with the Python reference: per-species F = 0.965,
0.957, 0.957, 0.964, 0.959, fitness 0.0351, identical to the 0.035140 the Rust
version reported for the reduced model.

### Projection for the browser

wasm runs at about half the native speed (M0), so the web version would produce a
viable model about every 20-25 s on this machine. That is slightly outside the 15-20 s
target. M2 candidates: tolerance study (atol 1e-10 is inherited from Pascal),
the shorter Pascal `config.ini` window (80 / 1024), and early abandonment of
hopeless experiments.

### Open issue

Time-course mode: the Pascal reduction criterion (keep removing reactions while
the fitness stays within 10x of the unreduced model) lets reduced models degrade a
lot (46 -> 186, beyond the viable threshold of 100). Decided: a reduced model must
also stay viable, in both modes.

## M2 - validation against Pascal and throughput tuning (2026-11-21)

Headline metric: **viable models (successes) per minute** in continuous mode.
The share of experiments that succeed is secondary.

### Agreement with the Pascal corpus

All 445 models in the Pascal `results_osc_1` corpus were imported (via the
`antimony` crate, `import.rs`) and re-scored by the Rust version (diffsol BDF,
t = 0..150, 2048 points). Tool: `tune score DIR`.

- 441 simulate; Rust and Pascal agree on viability (fitness < 0.3) for 416 of 441
  (94%). Median |fitness difference| is 0.039, mostly from B19 (Pascal scores
  oscillators lower) and from Pascal reporting the pre-reduction fitness.
- 26 of the 445 Pascal files contain a species written as floating that no
  reaction produces (B14: deleted reactions without reclassifying). The Rust
  importer derives boundaries from the structure, so these are scored as a
  different model; they account for the disagreements. Tellurium (which keeps the
  file's markers) scores 5 of 6 checked ones as oscillating, but only while that
  species slowly drains.

### Solver tolerances (`tune tol DIR`, 445 Pascal oscillators, 8 threads)

| rtol | atol | speed-up | mean steps | flips vs reference |
|---|---|---|---|---|
| 1e-6 | 1e-10 | 1.00x | 9,453 | 11 |
| 1e-6 | 1e-8 | 1.11x | 8,763 | 16 |
| 1e-5 | 1e-8 | 1.44x | 6,064 | 19 |
| 1e-4 | 1e-8 | 1.73x | 4,073 | 27 |
| 1e-3 | 1e-6 | 2.34x | 2,551 | 58 |

Reference: rtol 1e-9, atol 1e-13. "Flips" = models whose viable (0.3) or
converged (0.05) decision changes. The cost is in the number of steps
(~1.5 microseconds each), not in solver overhead. Relaxing tolerances buys
little and changes decisions, so the Pascal tolerances (1e-6 / 1e-10) stay.

### Abandoning hopeless experiments

Replaying run A's histories (49 experiments, 20 threads) showed that
experiments still above fitness 0.3 at generation 100 rarely pay off soon enough;
continuous mode does better by starting a fresh experiment. Measured on
8 cores, 5 minutes each, seed 202:

| Setting | Experiments | Viable | **Successes/min** | CPU-s per success |
|---|---|---|---|---|
| Baseline (no abandoning) | 41 | 31 (76%) | **3.7** | 101 |
| Abandon at gen 100 if fitness > 0.3 | 192 | 52 (27%) | **10.0** | 47 |
| Abandon + 80/1024 evolution window | 318 | 66 (21%) | **9.5** | 40 |

Decisions:

- Abandoning is **on by default** (`abandon_generation = 100`,
  `abandon_threshold = 0.3`): 2.7x more successes per minute. It is a visible
  setting so a fixed batch can switch it off, letting every experiment run to the end
  (76% success per experiment, but slower).
- The evolution window stays at 150 / 2048. The 80 / 1024 window gives no more
  successes per minute, and its candidates fail confirmation far more often
  (19 of 84 versus 2 of 54).
- Expected browser rate: on this machine, the default 75% CPU setting (15 workers)
  is about the throughput of 7.5 native threads (wasm is ~2x slower), so about one
  success every 6-7 s. On a typical 8-thread laptop (6 workers, ~3 native-thread
  equivalents), about one every 15-16 s. Both are within the 15-20 s target.

## M3/M4 - web app and deployment (2026-11-21)

- Published at https://hsauro.github.io/ModelEvolver/ by `.github/workflows/pages.yml`
  (core tests, `trunk build --release`, deploy). First deployment succeeded; a
  1-experiment smoke test ran end to end on the live site (Web Worker loads,
  evolves, result returned).
- Browser runs on this machine, 7 workers (38% CPU): about 4 successes per minute
  while experiments are being started (e.g. 11 successes in 3:11 including the
  drain after Stop).
- UI cost while running: about 1.4 ms per frame at 14-19 frames/s, i.e. 2-3% of
  one core; workers are unaffected.
- Bugs found in browser testing and fixed: workers lost their first message
  (now a Ready handshake); results with infinite fitness were dropped because JSON
  cannot encode infinity (worker messages now use postcard); a crashed worker
  hung its slot (now reported as a failed experiment); a stopped run never
  reached Finished.

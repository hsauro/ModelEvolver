# ModelEvolver (Rust) - Specification

Status: draft v0.1 (2026-11-21)
Repository: https://github.com/hsauro/ModelEvolver
Reference implementation: Delphi "OscillatorEvolver" v1.72a
(`D:\Documents\Embarcadero\Studio\Projects\OscillatorEvolver_Latest_CVODE`).
Known bugs in the reference are listed in [objectpascal_bugs.md](objectpascal_bugs.md).
They are fixed in this port, and each fix is referenced below by its B-number.

---

## 1. Purpose

ModelEvolver uses a genetic algorithm (GA) to evolve mass-action chemical reaction
networks that either:

1. **match a target time course** (sum of squared differences, "SSD mode"), or
2. **oscillate in a sustained way** ("oscillation mode"), scored with a phase- and
   amplitude-invariant oscillation measure.

The Rust port must:

- reproduce the algorithm of the Pascal version, with the bugs fixed;
- run as a **web app on GitHub Pages** (WebAssembly) and as a **native desktop app**,
  both from one egui/eframe code base;
- use **diffsol's adaptive BDF solver** for all ODE integration. Explicit methods
  (RK45 and similar) are not permitted anywhere, including plotting;
- run **many experiments in parallel**: Web Workers in the browser, OS threads on
  desktop;
- turn every command-line option of the Pascal app into a UI control;
- plot the time course of any successful model, and show a **grid of time courses**
  for all models produced by a batch.

Non-goals (v1): rate laws other than mass action, SBML import/export, a LSODA fallback,
and bit-exact reproduction of Pascal results (the solvers differ).

---

## 2. Technology choices

| Concern | Choice | Notes |
|---|---|---|
| Language | Rust (stable, edition 2024) | toolchain 1.99 present on dev box |
| ODE solver | `diffsol` (latest, currently 0.18.x), **BDF** method | closure-based `OdeBuilder`, dense nalgebra LU. No DiffSL/LLVM (not available on wasm). |
| Linear algebra | `nalgebra` via diffsol (`NalgebraMat<f64>`, `NalgebraLU<f64>`) | systems are small (<= ~30 states); dense is fastest |
| GUI | `eframe` / `egui` | same code native + web |
| Plotting | `egui_plot` | |
| Web build | `trunk`, target `wasm32-unknown-unknown` | |
| Web parallelism | Web Workers, one wasm instance per worker | no SharedArrayBuffer needed (GitHub Pages cannot set COOP/COEP) |
| Native parallelism | `std::thread` worker pool + `crossbeam-channel` (or `rayon`) | |
| Serialization | `serde`, `serde_json` | config, worker messages, saved sessions |
| File dialogs | `rfd` (native + web) | |
| Zip export | `zip` (pure Rust, deflate via `miniz_oxide`) | works on wasm |
| RNG | own xorshift64* port (bit-compatible with `uRandom.pas`) | deterministic per seed, no global state |
| CI/CD | GitHub Actions: test, clippy, trunk build, deploy to Pages | |

### 2.1 Solver requirement (hard)

- Method: diffsol **BDF** (variable order, variable step, Newton iteration with a
  dense LU of the iteration matrix). This replaces CVODE BDF (fitness) and LSODA
  (fallback) in the Pascal version.
- Tolerances: `rtol = 1e-6`, `atol = 1e-10` (the same as the Pascal CVODE setup).
  Both are exposed under Advanced settings.
- An **analytic Jacobian** is supplied (mass action makes it trivial, see 4.4).
  diffsol needs the Jacobian-vector product `J*v`. We implement it directly from the
  compiled reaction terms.
- Output is sampled on a fixed time grid. The solver is stepped manually
  (`step()` + `interpolate(t)`), so we keep control over failure detection and step
  limits (4.5).

### 2.2 Technical risk to retire first (Milestone M0)

diffsol's README does not list wasm as a supported target. The closure/nalgebra path is
pure Rust and is expected to compile to `wasm32-unknown-unknown`, but this must be proven
before anything else is built: a Brusselator solved with BDF inside eframe in the
browser. If it fails, the fallback order is: (a) patch or feature-gate the offending
dependency upstream; (b) vendor the diffsol subset we need. Switching to an explicit
solver is **not** an option.

---

## 3. Repository layout

```
ModelEvolver/
  Cargo.toml                 workspace
  crates/
    evolver-core/            pure logic, no UI, no threads, wasm-safe
      src/rng.rs             xorshift64* (port of uRandom.pas)
      src/network.rs         Network, Species, Reaction, boundary classification
      src/patterns.rs        reaction-pattern constraints (port of uReactionPatterns.pas)
      src/compile.rs         Network -> CompiledModel (flat RHS/Jacobian terms)
      src/simulate.rs        diffsol BDF wrapper, sampling, failure detection
      src/objective/ssd.rs   target time-course objective + file parser
      src/objective/osc.rs   oscillation score (port of uOscillationObjective.pas)
      src/fft.rs             radix-2 FFT (or `rustfft` if it builds small on wasm)
      src/ga.rs              population, selection, mutation, main loop (step-able)
      src/reduce.rs          model reduction (B14 fix)
      src/export.rs          Antimony / Tellurium text output
      src/config.rs          EvolverConfig (+ Pascal config.ini importer)
      src/experiment.rs      Experiment state machine + events
    evolver-cli/             headless batch runner (dev/validation, parity tests)
    evolver-app/             eframe app (native bin + web entry + worker entry)
      index.html             trunk entry
      src/worker.rs          Web Worker entry point
  docs/
    SPECIFICATION.md
    objectpascal_bugs.md
  .github/workflows/pages.yml
```

`evolver-core` must not depend on eframe, web-sys or threads. It is driven entirely
through a step-able `Experiment` API (section 7.4), so the same code runs in a Web
Worker, a native thread, the CLI and tests.

---

## 4. Model and simulation

### 4.1 Network data model

```rust
enum ReactionKind { UniUni, BiUni, UniBi, BiBi }   // 1->1, 2->1, 1->2, 2->2

struct Reaction {
    kind: ReactionKind,
    k: f64,                        // rate constant, always >= 0
    conn: [u16; 4],                // species indices in Pascal connection order:
                                   // UniUni [s,p,-,-]  BiUni [s1,s2,p,-]
                                   // UniBi  [s,p1,p2,-] BiBi [s1,s2,p1,p2]
}

enum SpeciesStatus { Float, BoundaryInput, BoundaryOutput, Orphan }

struct Species { leavings: u32, enterings: u32, status: SpeciesStatus, y_index: Option<u16> }

struct Network {
    id: u64,                       // "speciesId" in Pascal: lineage id, new on topology change
    species: Vec<Species>,
    reactions: Vec<Reaction>,
    n_float: usize, n_input: usize, n_output: usize,
}
```

- Rate law: mass action, `v = k * prod(reactants)`.
- Classification (`classify()`, a port of `determineBoundaryNodes`):
  no leavings and no enterings -> Orphan; no leavings -> BoundaryOutput (conc 0);
  no enterings -> BoundaryInput (conc = initial value, held constant); otherwise
  Float, with `y_index` assigned in species order.
- The `leavings`/`enterings` counts are **derived** in `classify()` from the reaction
  list. They are not maintained incrementally, which removes a whole class of
  bookkeeping bugs (B8).
- Species indices are stable. Species are never deleted, they only become Orphan.

### 4.2 Initial concentrations

- A user-editable list. The default is the Pascal table:
  `2, 5, 7, 10, 1, 0.5, 3, 7, 12, 4, 1, 3, 7, 9, 1, 7, 2, 8, 5, 10`.
- Indexed by **species index**, for both floating initial conditions and input-boundary
  levels.
- If `max_species > len`, the table is extended by cycling (B10). The UI shows the
  effective values.

### 4.3 Compiled model (hot path)

Before integration, a `Network` is compiled into a flat form:

```rust
enum Operand { State(u16), Const(f64) }      // floating -> y[idx]; boundary -> fixed value
struct Term { k: f64, a: Operand, b: Option<Operand>, deltas: SmallVec<[(u16, f64); 4]> }
```

`deltas` holds the net stoichiometric change for each **floating** species, with
repeated species merged. For example, `A + B -> 2B` gives `{A:-1, B:+1}`. The RHS is a
single pass over the terms with no branching on species status. This replaces the
four-way `case` blocks in `uMainLoop.pas`.

### 4.4 Jacobian

For term `v = k*a*b`: `dv/dy_a = k*b` and `dv/dy_b = k*a` (for `a == b`, `dv/dy_a = 2*k*a`).
`J*v` and the dense Jacobian (if diffsol requests a sparsity/colouring pass) are
assembled from the same terms. A unit test checks the analytic Jacobian against a
finite-difference Jacobian on random networks.

### 4.5 Integration and failure rules

`simulate(model, t_end, n_points) -> Result<Trajectory, SimError>`:

- Uniform grid `t_i = i * t_end / (n_points - 1)`, `i = 0..n_points-1`, starting at
  t = 0. Sample 0 is the initial condition.
- Step BDF until `t >= t_i`, then interpolate.
- Failure (-> `badModel`):
  - a diffsol error (Newton or step failure, step size underflow);
  - more than `max_steps_per_interval` internal steps between two output points
    (default 500, the CVODE `mxstep` default) - guards against stiff stalls;
  - any floating species with `|y| > max_concentration_cap` (default 1000), NaN or Inf
    at any sample point (runaway rule from the Pascal code);
  - a total wall-clock/step budget per evaluation (Advanced setting), so a single
    pathological model cannot freeze a web worker.
- No panics may escape the solver path. diffsol `Result`s are mapped to `SimError`, and
  any internal `unwrap` in our code is forbidden (`clippy::unwrap_used` is denied in
  `evolver-core`).

The `Trajectory` holds the time vector plus one column per floating species, and keeps
the species-index mapping so plots can label `S{i}`.

---

## 5. Objectives (fitness, lower is better)

### 5.1 SSD mode (time-course match)

- The target comes from an "objective function" file in the Pascal format:
  ```
  # comment line
  timeStart 0.0
  timeEnd 10
  numberOfPoints 10
  <numberOfPoints values, one per line>
  ```
  `stepSize = (timeEnd - timeStart) / (numberOfPoints - 1)`. Loaded through a file
  picker; a built-in default is supplied (the shipped 5/30 square wave).
- Simulate on exactly that grid.
- `deviation_j = sum_i (y_j(t_i) - target_i)^2` for each floating species j.
  **Fitness = min_j deviation_j** (best-matching node). Per-node scores are kept and
  shown in the export header.

### 5.2 Oscillation mode

Simulate on `[0, osc_t_end]` with `osc_n_points` points (defaults 150 and 2048; the FFT
pads to a power of two). For each floating species, compute
`F = rho * harm * sus * amp` in [0,1], exactly as in `uOscillationObjective.pas`:

1. Drop the first 40% (transient). If fewer than 16 samples remain, F = 0.
2. `xr` = raw tail; `xd` = linear-least-squares detrended tail. If `std(xd) < 1e-12`, F = 0.
3. **rho**: autocorrelation through FFT (Wiener-Khinchin, zero-padded to
   `nextpow2(2m)`), normalised by acf[0]. Find the first lag where acf < 0, then the
   maximum acf at or after it. `rho = max(0, peak)`, `lag = argmax`.
4. **harm**: Hann-windowed power spectrum (`nextpow2(m)`), with DC and bin 1 zeroed.
   The fundamental is anchored at the ACF period `f0b = L / lag` bins, snapped to the
   largest bin within `+/- max(2, round(0.15*f0b))`. Sum the power in harmonics
   k = 1..10 with half-window `max(1, round(0.03*k*f0))`. `harm = min(1, sum/total)`.
   If there is no ACF lag, use the global argmax.
5. **sus**: `min(std_early, std_late) / max(...)` over the two halves of `xr`
   (0 if both < 1e-12).
6. **amp**: `clamp((std/|mean|) / 0.02, 0, 1)` (use 1 as the denominator if
   |mean| < 1e-9).

Fitness = `min_j (1 - F_j)`.

**Confirmation pass:** after evolution, the best model is re-scored on
`[0, osc_confirm_t_end]` with `osc_confirm_n_points` points (defaults 300 / 4096). This
rejects slow-damping false positives. Its fitness replaces the evolved fitness for all
"viable / success" decisions. Set `osc_confirm_t_end <= 0` to disable it.

Unit tests: a pure sine gives F ~ 1; a damped sine gives F << 1; a constant gives 0; a
ramp gives ~0; a relaxation (spiky) oscillation gives a high F. A Python reference
(`analysis/objective_prototype.py` in the Pascal repo) is used to generate golden values.

---

## 6. Reaction-pattern constraints

This is a port of `uReactionPatterns.pas`, unchanged in behaviour:

- The null reaction `A -> A` is always rejected.
- `disallow_self_referential`: reject if any reactant is also a product.
- `disallow_autocatalysis`: reject `A -> 2A` and `A + B -> 2B`.
- Optional pattern list (text, the same file format: `mode: allow|disallow`, `#`
  comments, `A + B -> A`, `2A -> A`). Each reaction is reduced to a canonical
  signature (type << 8 | min first-appearance 2-bit label code over the reactant and
  product swaps), and checked against a 1024-entry reject table.
- `pick_allowed_reaction`: up to 30 random draws; if none is allowed, the reaction is
  skipped.
- In the UI, patterns can be loaded from a file **or** edited in a text box, with
  live parse errors shown per line (the Pascal version only printed a warning).

---

## 7. Genetic algorithm

### 7.1 Configuration (`EvolverConfig`, serde)

| Field | Default | Pascal source |
|---|---|---|
| `seed: u32` (0 = random) | 0 | `-s` |
| `num_experiments` | 1 | `-e` |
| `num_workers` (0 = auto) | 0 | `-nt` |
| `max_population_size` | 100 | `-mp`, `maxPopulationSize` |
| `max_generations` | 1600 | `-mg`, `maxNumberOfGenerations` |
| `min_species`, `max_species` | 5, 5 | config |
| `min_reactions`, `max_reactions` | 4, 4 | config |
| `max_initial_rate_constant` | 50 | config |
| `p_uniuni, p_biuni, p_unibi, p_bibi` | 0.4, 0.15, 0.15, 0.3 | config (normalised on validate) |
| `elite_fraction` | 0.4 | `percentageClone` |
| `p_rate_mutation, p_add_reaction, p_delete_reaction` | 0.5, 0.225, 0.275 | config |
| `rate_change_fraction` | 0.2 (must be < 2, B16) | `percentageByWhichRateConstantChanges` |
| `objective` | `Ssd` / `Oscillation` | `-osc` |
| `ssd_stop_threshold` | 50.0 (B11: single default) | `fitnessThresholdToStop` |
| `ssd_viable_threshold` | 100 | `FittnessThresholdForViableModel` |
| `ssd_stagnation_rules` | on (fitness>800 after gen 600; <30 after gen 800) | hard-coded in loop |
| `osc_t_end, osc_n_points` | 150, 2048 | config |
| `osc_stop_threshold` | 0.05 | config |
| `osc_viable_threshold` | 0.3 | config |
| `osc_reduce_margin` | 0.05 | config |
| `osc_confirm_t_end, osc_confirm_n_points` | 300, 4096 | config |
| `max_concentration_cap` | 1000 | config |
| `rtol, atol, max_steps_per_interval` | 1e-6, 1e-10, 500 | CVODE wrapper |
| `initial_concentrations` | Pascal table | `uGlobal.pas` |
| `require_input_boundary` | true | B13 |
| `add_random_networks, num_random_networks` | false, 10 | globals |
| pattern constraints | off | config |
| `add_tellurium_harness` | false | `-addTellHarness` |
| `reduce_models` | true | (always on in Pascal) |

`validate()` rejects `min > max`, probabilities that are negative or all zero,
`elite_fraction` outside [0,1], `rate_change_fraction >= 2`, and `n_points < 2`.

Importer: reads a Pascal `config.ini` `[ConfigSettings]`, maps the keys, and lists
unknown or malformed lines (B11). Export/import of the native format is JSON.

### 7.2 Random numbers and determinism

- `Rng` = xorshift64* with a splitmix64 seed finaliser, bit-identical to `uRandom.pas`
  (`next_f64`, `next_int(range)`, `range(lo, hi)`). A unit test checks the first N
  outputs against values captured from the Pascal build.
- Experiment seed: `(base_seed as u64) << 32 | experiment_id`, as in Pascal. Results are
  independent of worker count and scheduling.
- If `seed == 0`, a base seed is taken from the clock (`js_sys::Date::now` on web) and
  recorded in the output so the run can be replayed.
- The lineage `id` counter is per experiment (`experiment_id << 32 | counter`) instead
  of a global atomic. It stays unique without shared state across workers.

### 7.3 Algorithm (per experiment)

```
pop = create_population(max_population_size)      # resample until viable
evaluate(pop); sort(pop)
for gen in 0..max_generations:
    evaluate(pop)              # sims; bad models removed
    sort(pop)                  # stable sort by fitness, ascending (B4)
    record history (best, median, pop size, mutation counts)
    stop if best < stop_threshold (mode-specific)
    stop if SSD stagnation rules fire (SSD mode only)
    n_elite = floor(elite_fraction * len(pop))
    new = clone(pop[0..n_elite])                       # untouched elites (B6)
    while len(new) < max_population_size:              # exact size (B5)
        a, b = two uniform random picks from pop
        new.push(clone(better of a, b))
    if add_random_networks: append num_random_networks fresh networks
    for i in n_elite..len(new):                        # each non-elite mutated once (B6)
        m = mutate(new[i])
        if m.is_viable(): new[i] = m                   # else keep parent
    pop = new
final: evaluate, sort; oscillation confirmation pass; reduce; export
```

Elitism note: with `elite_fraction = 0`, index 0 is still protected (n_elite = max(1, ...)),
which preserves the Pascal "never mutate the best" rule.

Mutation (`mutate`): clone, then perform 2 mutation draws. Each draw:

- `u < p_rate`: choose a random reaction, `k += (U - 0.5) * rate_change_fraction * k`,
  clamped to `>= 0`;
- `u < p_rate + p_add`: add a reaction. Its type is drawn from the type probabilities,
  its species come from `pick_allowed_reaction`, its rate constant is `U(0,1)` (as in
  Pascal), and it gets a new lineage id;
- `u < p_rate + p_add + p_del`: delete a random reaction (only if there are more than
  2 reactions) and give it a new lineage id;
- otherwise: no-op.

After each structural change, run `classify()`.

Viability (`is_viable`, B13): at least 1 reaction, `n_float >= 2` (the Pascal fitness
loop requires > 1), and `n_input >= 1` when `require_input_boundary` is set.

### 7.4 Step-able experiment API

```rust
struct Experiment { /* config, rng, pop, gen, history, state */ }
impl Experiment {
    fn new(cfg: Arc<EvolverConfig>, ctx: Arc<RunContext>, id: u32, base_seed: u32) -> Self;
    fn step(&mut self, budget: StepBudget) -> Vec<ExperimentEvent>;   // runs <= budget generations
    fn is_finished(&self) -> bool;
    fn finish(self) -> ExperimentResult;  // confirmation, reduction, export text
}
enum ExperimentEvent { Progress { id, gen, best, pop_len, counts }, Finished { .. } }
```

`RunContext` holds the immutable shared inputs: the SSD target, the compiled pattern
table and the initial concentrations.

### 7.5 Model reduction (B14 fix)

Greedy, structural, on the confirmation window (oscillation mode) or the SSD grid:

```
best = evaluate(model)
for each reaction r (in order):
    cand = model without r; classify(cand)
    if cand viable and fitness(cand) within tolerance of best:
        model = cand
```

The tolerance is mode-specific: oscillation uses `fitness <= best + osc_reduce_margin`;
SSD uses `fitness <= 10 * best` (the Pascal criterion). The exported model is exactly
the network that was scored. Both the original and the reduced fitness are reported.

### 7.6 Experiment result

```rust
struct ExperimentResult {
    id: u32, seed: u64, generations: u32, stop_reason: StopReason,
    best: Network, reduced: Network, fitness: f64, reduced_fitness: f64,
    node_scores: Vec<f64>, history: Vec<GenStats>,
    viable: bool,                 // below the mode's viable threshold
    antimony: String,
}
```

---

## 8. Parallel execution

A common trait in `evolver-app`:

```rust
trait Executor {
    fn start(&mut self, cfg: EvolverConfig, ctx: RunContext);
    fn poll(&mut self) -> Vec<RunEvent>;   // called every frame by the UI
    fn cancel(&mut self);
}
```

### 8.1 Native

- `num_workers` threads (default `available_parallelism()`), each pulling experiment ids
  from an `AtomicU32` counter (the same design as the Pascal workers).
- Each thread runs `Experiment::step` in a loop and sends events over a channel.
  Cancellation uses an `AtomicBool` checked between generations.
- No custom allocator is needed initially. The default Rust allocator is per-thread
  friendly on Windows (HeapAlloc is low-fragmentation); profile before adding mimalloc.

### 8.2 Web (GitHub Pages)

- A pool of `num_workers` dedicated Web Workers (default
  `navigator.hardwareConcurrency - 1`, minimum 1). Each worker loads the same wasm
  module through a worker entry point (`evolver-app/src/worker.rs`, built by trunk as a
  separate worker bundle).
- The main thread acts as the dispatcher. It sends `Start { id, base_seed, cfg, ctx }`
  (serde JSON through `postMessage`) to an idle worker. When that worker posts
  `Finished`, the dispatcher sends it the next id.
- The worker calls `Experiment::step(StepBudget::Generations(k))` in a loop, posting
  `Progress` every k generations (default 10) and `Finished { ExperimentResult }` at
  the end.
- Cancel: `worker.terminate()` on all workers and respawn the pool lazily.
- No SharedArrayBuffer or COOP/COEP is required.
- With `num_workers = 0`/single mode (debug), experiments run on the main thread with a
  small per-frame budget.

---

## 9. User interface (egui)

Layout: left side panel **Settings**, top bar **Run controls**, central area with tabs
**Progress**, **Results**, **Grid**, **Model**.

### 9.1 Settings panel (every CLI option becomes a control)

| Pascal CLI / config | UI control |
|---|---|
| `-s seed` | numeric field + "random" checkbox; the last used seed is shown |
| `-e` | number of experiments (DragValue) |
| `-nt` | workers (DragValue, 0 = auto) |
| `-mp`, `-mg` | population size, max generations |
| `-osc` | objective radio: Time-course match / Sustained oscillation |
| objectivefunction.txt | "Load target..." button + inline preview plot of the target |
| `-lsoda` / `-cvode` | (removed: diffsol BDF only) |
| `-v`, `-silent`, `-nds` | replaced by the Progress tab (verbosity selector for the log) |
| `-sg` | "Record fitness history" checkbox (always kept in memory; export as CSV) |
| `-pop file` | "Record population lineage ids" checkbox; export as CSV |
| `-res`, `-pre` | export file-name prefix; native: output folder picker |
| `-addTellHarness` | checkbox |
| `-clipboard` | "Copy" button on the Model tab |
| `-printDefaults` | "Reset to defaults" and "Export settings (JSON)" buttons |
| `-w`, `-h`, `-version`, `-stress`, `-warnings` | not applicable (About dialog shows the version) |
| config.ini values | collapsible groups: Network size, Reaction types, GA, Mutation, Oscillation, Solver (advanced), Constraints, Initial concentrations |

Validation errors are shown inline, and Start is disabled until the config is valid.
Settings persist across sessions (eframe storage; localStorage on web). There is also an
"Import Pascal config.ini" button.

### 9.2 Run controls

Start / Cancel, an overall progress bar (experiments finished / total), elapsed time, and
counts of converged / promising / failed (the Pascal `* o .` markers).

### 9.3 Progress tab

A table with one row per running or finished experiment: id, generation, best fitness,
population size, #species, #reactions, mutation counts, status. A live fitness-history
plot for the selected experiment, plus a log pane.

### 9.4 Results tab

A sortable table of finished experiments: id, seed, fitness, reduced fitness, #reactions
(reduced), #floating species, generations, stop reason, viable flag. A filter "viable
only" is on by default. Selecting a row opens the Model tab. Actions: export the selected
model, or export all viable models as a zip (`M<session>_<id>_<fitness>.txt`, the same
naming as Pascal), plus a results CSV.

### 9.5 Model tab (single time course)

- An Antimony text view (read-only, with copy and download buttons).
- A time-course plot (`egui_plot`) of all floating species, with a legend and per-species
  toggles. The best-scoring node is highlighted.
- Controls: simulation end time and number of points (default: the objective's window;
  for oscillation, the confirmation window), plus a toggle between the original and the
  reduced model.
- SSD mode: the target points are overlaid.
- The plot is re-simulated with the same diffsol BDF path (on the main thread; one
  simulation is cheap).

### 9.6 Grid tab (all models)

- A grid of small plots, one per viable (or all, by toggle) experiment result, ordered
  by fitness. The column count is adjustable (default 4) and the grid is paginated or
  virtualised so hundreds of models stay responsive.
- Each cell shows one of: the best node only (default) or all floating species (toggle).
  The title shows the experiment id, fitness and #reactions.
- Shared or independent y-axis toggle. Clicking a cell opens it in the Model tab.
- Trajectories for the grid are computed once, when the result arrives, inside the
  worker, and cached in `ExperimentResult` (the decimated trajectory, about 500 points).
  The UI thread never re-simulates hundreds of models.
- Export the grid as PNG (native); on web, via a canvas screenshot if feasible (stretch
  goal).

---

## 10. Export format

The Antimony text matches the Pascal `outputResults` + `convertToString` output:
a header of comment lines (version, date, seed, solver "diffsol BDF", objective and
window, all GA parameters, initial concentrations), then reactions
`S0 + $S1 -> S2; S0*S1*k0;`, species initialisations, and rate constants
`k0 = ...;`, then a footer with the fitness, the number of reactions and floating
species, and the generations run. `$` marks boundary species. An optional Tellurium
harness wraps the model in `te.loada("""...""")` plus `r.simulate(0, T, N)`
and `r.plot()`, using the objective's T instead of the hard-coded 15.

The export contains only the species that appear in the reduced network (orphans are
omitted), and the initial-concentration line uses the effective table (B10).

---

## 11. Testing and validation

Unit tests (`evolver-core`, run natively and under `wasm-bindgen-test` for the solver):

- RNG sequence parity with Pascal.
- Pattern signatures: the canonical-form equivalences in the Pascal
  `analysis/TestNetworkIdentity` cases, plus the allow/disallow table.
- `classify()` on hand-built networks, including the corrected Pascal test network.
- RHS against hand-computed derivatives; Jacobian against finite differences
  (random networks, property test with `proptest`).
- Solver: exponential decay (exact solution), Robertson (stiff; checks that BDF
  completes within the step limits), Brusselator (oscillation score > 0.9).
- Failure rules: a blow-up network is flagged bad; a step-limit network is flagged bad,
  without panic.
- Oscillation score golden values from the Python prototype.
- GA invariants: population size is exactly `max_population_size` (+ random networks),
  elites are unchanged, rate constants stay >= 0, and the same seed gives the same
  result regardless of worker count.

Validation against Pascal (manual, via `evolver-cli`): run 200 experiments in each mode
with matching settings and compare success rates, the distribution of the number of
generations, and the reaction-count distribution. Exact matches are not expected
(different solver, B5/B6 fixes); the rates should be the same order of magnitude.
Spot-check exported models in Tellurium.

CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, and
`trunk build --release` on every push. Deploy to GitHub Pages from `main`.

---

## 12. Milestones

| # | Deliverable | Exit criterion |
|---|---|---|
| M0 | Workspace skeleton; diffsol BDF solves Robertson + Brusselator natively **and** in an eframe page in the browser | spike page works in Chrome/Firefox; record the wasm size |
| M1 | `evolver-core`: rng, network, patterns, compile, simulate, objectives, GA, reduce, export, with unit tests | `cargo test` green; CLI finds oscillators |
| M2 | `evolver-cli` batch + validation against Pascal | comparison table written up in `docs/validation.md` |
| M3 | Native egui app: settings, run, progress, results, model plot, grid | full workflow on desktop |
| M4 | Web: worker pool, trunk config, Pages workflow | public URL runs a 20-experiment batch in parallel |
| M5 | Polish: config import, zip/CSV export, persistence, About, README | release v0.1.0 |

---

## 13. Open items (decide during implementation)

1. Grid default cell content: best node only vs all species. Draft: best node only.
2. Hard cap on `max_species` (affects the pattern table and UI). Draft: 50.
3. Whether to also expose diffsol's TR-BDF2/ESDIRK solvers as an alternative stiff option
   under Advanced. Draft: no; BDF only, per requirement.
4. Whether to keep the SSD stagnation heuristics (gens 600/800) on by default. Draft:
   yes, as a checkbox.

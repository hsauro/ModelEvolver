# ModelEvolver

Evolves mass-action chemical reaction networks with a genetic algorithm, either
to produce sustained oscillations or to match a target time course. Every
candidate network is simulated with diffsol's adaptive BDF solver (for stiff
systems). The app runs in the browser (WebAssembly, using Web Workers on several
cores) and on the desktop. It is a Rust port of the Object Pascal
"OscillatorEvolver".

**Try it in the browser: https://hsauro.github.io/ModelEvolver/**

## Using the app

1. **Choose the objective** (left panel): *Sustained oscillation* (default) or
   *Match a target time course*. For a time course, press **Load target...** and pick
   a file in the Pascal `objectivefunction.txt` format:
   ```
   # comment
   timeStart 0
   timeEnd 10
   numberOfPoints 10
   5
   30
   ...
   ```
2. **Set how much of the computer to use** with the **CPU usage** slider (top bar,
   default 75% of the processor threads). Each worker runs one experiment at a time.
3. **Choose a batch size** (*experiments*), or tick **Run until stopped** to keep
   producing models.
4. Press **Start**.
   - **Stop** starts no new experiments and lets the running ones finish.
   - **Cancel** abandons the running experiments; finished results are kept.
5. Watch the **Progress** tab: one row per running experiment, plus the best
   fitness curves. The dashed orange line is the give-up threshold, green is viable,
   blue is converged. *View best* shows a running experiment's current best network.
6. Successful models appear under **Results** (a sortable table) and **Grid**
   (time courses of every success, best first). In the Grid, **Max time** sets the
   plotted time span; each model is re-simulated at 1000 points on that span.
7. Click a model to open it in the **Model** tab: its Antimony (copy or save),
   and a time-course plot with toggles for each species and an optional custom
   time span.

The numbers in the top bar:

- **Successes / min**: successful models per minute while new experiments are
  being started. This is the main measure of throughput.
- **Time per success**: total run time divided by the number of successes.

Exporting: in **Results**, *Save model...* saves one model as Antimony text,
*Save all successes (zip)...* saves every success in one zip file, and
*Save summary (CSV)...* saves a table of all experiments. Settings can be saved,
loaded, reset to a preset, or imported from a Pascal `config.ini` (**More**, top of
the Settings panel). Settings are remembered between visits.

Key settings:

- **Give up on unpromising experiments** (on by default): an experiment whose best
  fitness is still above 0.3 at generation 100 is stopped, and a fresh one starts.
  This gives about 2.7x more successes per minute. Turn it off if each experiment
  in a fixed batch should run to the end.
- **Reaction constraints**: forbid catalysis or autocatalysis, or allow or
  disallow whole reaction patterns (for example `A + B -> A`).
- **Solver (advanced)**: tolerances (default rtol 1e-6, atol 1e-10) and step limits.

Fitness is "lower is better". For oscillation it is 1 - F, where
F = periodicity x harmonic purity x sustained amplitude x minimum amplitude
(each in 0..1). A model counts as a success below 0.3, after a longer
confirmation simulation that rejects slowly damping oscillators.

## Documentation

- [docs/SPECIFICATION.md](docs/SPECIFICATION.md): design and requirements
- [docs/validation.md](docs/validation.md): measured results (solver checks,
  agreement with the Pascal corpus, throughput)
- [docs/objectpascal_bugs.md](docs/objectpascal_bugs.md): bugs found in the Pascal
  version (fixed here)

## Building

```
cargo test -p evolver-core                                  # core tests
cargo run --release -p evolver-cli -- --help                # headless batch runner
cargo run --release -p evolver-core --example bench_eval    # evaluation benchmark
cargo run --release -p evolver-app                          # desktop app
cd crates/evolver-app && trunk serve --release              # web app on a local server
```

The web app is published to GitHub Pages on every push to `main`
(`.github/workflows/pages.yml`).

Crates: `evolver-core` (networks, simulation, objectives, GA), `evolver-app`
(egui app and the Web Worker entry point), `evolver-cli` (headless runner and
the `tune` tool), and our own copies of `antimony` and `sbml-io`.

## Funding

This project has been funded in whole or in part with Federal funds from the
Advanced Research Projects Agency for Health (ARPA-H), Department of Health and
Human Services, under the Critical Illness Immunological Reprogramming and Control
Point Learning Engine (CIRCLE) program. The views and conclusions contained in this
document are those of the authors and should not be interpreted as representing
the official policies, either expressed or implied, of the U.S. Government.

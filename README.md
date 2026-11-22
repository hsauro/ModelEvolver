# ModelEvolver

Evolves mass-action chemical reaction networks with a genetic algorithm, either
to produce sustained oscillations or to match a target time course. Every
candidate network is simulated with diffsol's adaptive BDF solver (for stiff
systems). The app runs in the browser (WebAssembly, using Web Workers on several
cores) and on the desktop. It is a Rust port of the Object Pascal
"OscillatorEvolver".

**Try it in the browser: https://hsauro.github.io/ModelEvolver/**

Current version: **1.0.0**.

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
3. Under **Run**, choose **Run N experiments** (a fixed batch) or **Run until
   stopped** (keep producing models until you press Stop).
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

- **Successes / min**: successful models per minute of run time. This is the main
  measure of throughput. The clock stops when you press Stop, so experiments still
  finishing after Stop do not drag the figure down.
- **Time per success**: the same clock divided by the number of successes
  (60 / successes per minute). **Elapsed** is the true total time.

Exporting: the **Model** tab saves the shown model as Antimony (*Save Antimony...*)
or SBML Level 3 Version 2 (*Save SBML...*). In **Results**, *Save all successes
(zip)...* saves every success as both Antimony (.txt) and SBML (.xml) in one zip
file, and *Save summary (CSV)...* saves a table of all experiments.

Sessions (for demos): in **Results**, *Save session...* writes every result and
each run's settings to one file; *Load session...* reopens it later (Results, Grid and
Model are restored, and the top bar shows the session's successes and run time),
without running anything. The Grid's *Fit all on one page* shows every model at once.

Sharing a setup: **Save configuration...** (top of the Settings panel) writes one
JSON file with every setting and the target time course. Anyone can open it with
**Load configuration...** in the app, or pass it to the command-line runner with
`--config`. Loading keeps the receiver's own CPU usage. With a fixed seed (untick
*Random seed*), a shared configuration reproduces exactly the same results.
Settings can also be reset to a preset or imported from a Pascal `config.ini`
(**More**). Settings are remembered between visits.

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

## Large runs: the command-line tool

For thousands of experiments, use `evolver-cli` (same evolution code, no UI).
Design a setup in the app, **Save configuration...**, then:

```
evolver-cli --config my-setup.json -e 100000 --out run1 --sbml
```

- **Output** (folder `run1`): `run.json` (configuration and base seed),
  `summary.csv` (one line per experiment), `models.jsonl` (one JSON object per
  successful model, with Antimony and, with `--sbml`, SBML). Add `--model-files` to
  also write each model as its own `.txt`/`.xml` file in `run1/models`.
- **Progress**: one status line every 30 s (`--progress SECS`): experiments done,
  successes, successes per minute and estimated time left. `-v` adds a line per
  experiment.
- **Resume after any interruption** (crash, reboot, Ctrl+C): run
  `evolver-cli --resume --out run1`. Every finished experiment is written to disk at
  once; only experiments not yet recorded are run, with the saved configuration and
  seed. Results are identical to an uninterrupted run.
- **Split across machines**: give each machine the same seed and a different
  range, e.g. `-s 7 --start 0 -e 50000` and `-s 7 --start 50000 -e 50000`. The
  combined results equal one run of 100,000.
- **Seed**: without `-s` (or a fixed seed in the configuration) a new base seed is
  taken from the operating system's random-number generator and printed.
- **CPU**: 75% of the hardware threads by default; `-nt N` sets the number of workers.

`evolver-cli --help` lists every option.

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

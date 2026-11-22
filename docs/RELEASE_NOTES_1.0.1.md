# ModelEvolver 1.0.1

First public release of ModelEvolver, a Rust port of the Object Pascal
"OscillatorEvolver". It uses a genetic algorithm to evolve mass-action chemical
reaction networks that oscillate or that follow a target time course.

**Try it in the browser: https://hsauro.github.io/ModelEvolver/**

(Version 1.0.0 was an internal tag made before the drift and negative-concentration
checks below; 1.0.1 is the first release to use.)

## Highlights

- **Runs in the browser**, on as many processor cores as you allow (Web Workers;
  default 75% of the computer's threads, adjustable). The same code also builds as
  a desktop app.
- **Stiff ODE solver**: every candidate network is simulated with the adaptive BDF
  method from the `diffsol` crate, with an exact Jacobian.
- **Fast**: about 10 successful oscillators per minute on 8 cores (native), and
  roughly half that per core in the browser.
- **Headless command-line tool** (`evolver-cli`) for very large runs: resumable
  after any interruption, splittable across machines, with identical results
  either way.

## Features

### Evolution

- Objectives: **sustained oscillation** (a phase- and amplitude-invariant score
  combining periodicity, harmonic purity, sustained amplitude and minimum amplitude)
  or **matching a target time course** (Pascal `objectivefunction.txt` format).
- Every setting of the Pascal program is available, grouped in a settings panel with
  tooltips and validation. Pascal `config.ini` files can be imported.
- Reaction constraints: no catalysis, no autocatalysis, or allow/disallow whole
  reaction patterns (for example `A + B -> A`).
- **Give up on unpromising experiments** (default on): an experiment still above
  fitness 0.3 at generation 100 is stopped and a fresh one starts. This gives about
  2.7x more successes per minute.
- Every success passes a **longer confirmation simulation**, which rejects slowly
  damping oscillators, species that **grow without bound**, and simulations whose
  concentrations go **negative**.
- **Model reduction** removes reactions that are not needed, as long as the model
  stays a success.

### The app

- **Run controls**: Start, Stop (let running experiments finish), Cancel; run a
  fixed number of experiments or run until stopped.
- **Live figures**: successes, successes per minute, time per success, elapsed time.
- **Progress**: every running experiment, with live best-fitness curves.
- **Results**: sortable table of all experiments.
- **Grid**: time courses of all successes, best first; adjustable size, max time,
  y axis from zero, or fit all on one page.
- **Model**: Antimony text beside a time-course plot, with species toggles and an
  adjustable time span.
- **Export**: Antimony, SBML Level 3 Version 2, a zip of all successes, and a CSV
  summary of every experiment. Exported Antimony is checked by reading it back with
  the `antimony` crate.
- **Shareable configurations**: one file with every setting and the target. With a
  fixed seed, others reproduce the exact same results.
- **Sessions**: save all results and reopen them later (for demos), without
  re-running.
- Light and dark themes; settings are remembered between visits; the browser asks
  before leaving the page with unsaved results.

### Command-line tool

```
evolver-cli --config my-setup.json -e 100000 --out run1 --sbml
evolver-cli --resume --out run1        # after an interruption
```

Results are written as experiments finish (`summary.csv`, `models.jsonl`, optional
SBML and per-model files), with a progress line every 30 seconds. See
`evolver-cli --help`.

## Differences from the Pascal version

Twenty-one bugs found in the Pascal version are fixed here; they are listed in
[docs/objectpascal_bugs.md](docs/objectpascal_bugs.md). The ones that change results:

- Configured species and reaction ranges are now honoured (B1, B2, B20).
- The GA keeps an exact population size, never mutates elites, and mutates every
  other network exactly once (B4-B6).
- Reduced models are exactly the network that was scored, and must stay a success
  (B14).
- The oscillation score follows the Python reference implementation. The Pascal
  spectral padding made a pure sine score 0.79 instead of 0.89 (B19).
- Concentrations can no longer go negative. In the Pascal version, round-off below
  zero could be amplified by autocatalytic reactions into spurious oscillations,
  which affected about 3% of its evolved oscillators (B21).

## Validation

Recorded in [docs/validation.md](docs/validation.md):

- diffsol BDF reproduces the Robertson stiff benchmark and the Brusselator limit
  cycle, natively and in the browser.
- The oscillation score matches the Python reference to 1e-5.
- Of the 445 oscillators evolved by the Pascal version, the Rust version agrees on
  viability for 94%. The differences are traced to Pascal bugs B14 and B21.
- Exported models give the same fitness in Tellurium (CVODE).
- Interrupted-and-resumed and split CLI runs give results identical to a single run.

## Known limitations

- In the browser, results live in the page until saved (Save session, or the zip /
  CSV exports). The page warns before closing with unsaved results.
- The desktop build compiles but has had little testing.
- Only mass-action kinetics with up to two reactants and two products.

## Acknowledgement

This project has been funded in whole or in part with Federal funds from the
Advanced Research Projects Agency for Health (ARPA-H), Department of Health and
Human Services, under the Critical Illness Immunological Reprogramming and Control
Point Learning Engine (CIRCLE) program. The views and conclusions contained in this
document are those of the authors and should not be interpreted as representing the
official policies, either expressed or implied, of the U.S. Government.

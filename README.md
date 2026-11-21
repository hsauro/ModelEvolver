# ModelEvolver

Evolves mass-action chemical reaction networks with a genetic algorithm, either
to match a target time course or to produce sustained oscillations. This is a
Rust port of the Delphi "OscillatorEvolver". It runs natively and in the browser
(egui, WebAssembly), and uses diffsol's adaptive BDF solver.

**Try it in the browser: https://hsauro.github.io/ModelEvolver/**

Status: milestones M0-M4 done; the web app is published on GitHub Pages and redeployed on every push to `main`. See
[docs/SPECIFICATION.md](docs/SPECIFICATION.md) for the plan,
[docs/validation.md](docs/validation.md) for measured results, and
[docs/objectpascal_bugs.md](docs/objectpascal_bugs.md) for bugs found in the Pascal version.

## Build

```
cargo test -p evolver-core                                  # core tests
cargo run --release -p evolver-core --example bench_eval    # evaluation benchmark
cargo run --release -p evolver-app                          # desktop app
cd crates/evolver-app && trunk build --release              # web build into dist/
cd crates/evolver-app && trunk serve --release              # web build + local server
```

Crates: `evolver-core` (networks, simulation, objectives, GA), `evolver-app`
(egui app plus the Web Worker entry), and vendored copies of `antimony` and `sbml-io`.

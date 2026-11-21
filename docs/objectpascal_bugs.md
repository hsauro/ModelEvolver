# Object Pascal version: bug record

Source reviewed: `D:\Documents\Embarcadero\Studio\Projects\OscillatorEvolver_Latest_CVODE`
(Delphi 13, program version string `1.72a` in `uGlobal.pas`). Review date: 2026-11-21.

These bugs are **not** fixed in the Pascal code yet. Each one is fixed in the Rust
port; the "Rust fix" line says how. Line numbers refer to the files as they were
when reviewed.

Severity key: **High** = wrong results or silently ignored settings;
**Medium** = skews the GA or leaks; **Low** = cosmetic, dead code or latent.

---

## B1 (High) Species/reaction ranges copied from the wrong defaults

`oscillatorEvolverProject.dpr`, `setupAndRunExperiment`:

```pascal
591  experimentInfo.maxNumberOfSpecies   := default_minNumberOfSpecies;    // should be max
595  experimentInfo.minNumberOfReactions := default_maxNumberOfReactions;  // should be min
```

The built-in defaults have min = max (species 5/5, reactions 4/4), which hides the bug.
Any `config.ini` that sets a range (for example `minNumberOfReactions=3`,
`maxNumberOfReactions=9`) is silently ignored. Species count is pinned to the minimum
and reaction count to the maximum.

Rust fix: one `EvolverConfig` struct is the single source of truth, with no copy step.
`validate()` checks `min <= max`.

## B2 (High) `readIniFile` uses the wrong fallback for maxNumberOfSpecies

`oscillatorEvolverProject.dpr:108`

```pascal
default_maxNumberOfSpecies := appINI.ReadInteger('ConfigSettings', 'maxNumberOfSpecies', default_minNumberOfSpecies);
```

The fallback should be `default_maxNumberOfSpecies`. If the key is missing, the max is
reset to the min.

Rust fix: config defaults come from `EvolverConfig::default()`, so each key has its own fallback.

## B3 (Low) `printDefaultValues` prints the min as the max

`oscillatorEvolverProject.dpr:161`: the "Max Number Of Species" line prints
`default_minNumberOfSpecies`.

Rust fix: the settings panel binds every field directly.

## B4 (Medium) `sortPopulation` swaps individuals incompletely

`uPopulation.pas:52-63`. The manual field-by-field swap:

- saves `numberOfInputBoundarySpecies` but never writes it back; lines 54 and 63 assign
  `numberOfFloatingSpecies` a second time instead;
- never swaps `badModel` or `ErrorMessage`.

After sorting, an individual can carry another individual's input-boundary count. The
sort is also an O(n^2) exchange sort.

Rust fix: `population.sort_by(|a, b| a.fitness.total_cmp(&b.fitness))`. Whole structs
move, so no field can be left behind.

## B5 (Medium) The population grows to maxPopulationSize + 2

`uMainLoop.pas:833`

```pascal
for i := experimentInfo.maxPopulationSize downto newPopulation.Count - 1 do
```

After `n` elites have been copied, this loop runs `maxPop - n + 2` times, so the new
population holds `maxPop + 2` individuals instead of `maxPop`.

Rust fix: fill with tournament winners while `new_pop.len() < max_population_size`.

## B6 (Medium) The mutation loop is unevenly distributed and mutates elites

`uMainLoop.pas:851-860`

```pascal
for i := 1 to newPopulation.Count - 1 do
    p1 := rndRange(1, newPopulation.Count);
    individual := mutateIndividual (newPopulation[p1], ...);
    if not individual.badModel then
       begin newPopulation.Delete (p1); newPopulation.Add (individual); end;
```

- `p1` is drawn with replacement, and each mutant is moved to the end of the list. Some
  individuals are mutated several times (mutations stack) and others not at all.
- Only index 0 is protected. The rest of the "elite" fraction (40% by default) can be
  mutated and lost, which contradicts the comment and the meaning of "elite".

Rust fix: every individual at index `>= n_elite` is mutated exactly once, in place.
Elites at indices `0 .. n_elite` are copied unchanged. This deliberately changes the GA
dynamics; it is recorded in the spec.

## B7 (Medium) Bad mutants leak memory

`uMainLoop.pas:855-860`. When `mutateIndividual` returns an individual with
`badModel = True`, the clone is neither freed nor added to the population. It leaks.

Rust fix: ownership makes this impossible.

## B8 (Low) `CreateTestNetwork` increments the wrong counter

`uIndividual.pas:154`

```pascal
rt.connections[1] := 4;  inc (listOfSpecies[6].enterings);   // should be [4]
```

This only affects the test network (`InjectTestingNetwork`), but it gives S6 a spurious
entering count, so its boundary classification can be wrong.

Rust fix: the test network is built through the same `add_reaction` helper as everything
else, which keeps the counts consistent.

## B9 (Low) A stale or uninitialised value is tested in computeFitnessOfPopulation

`uMainLoop.pas:682-692`. For individuals flagged bad up front (`numberOfFloatingSpecies <= 1`
or no reactions), `d` is not assigned, yet `if d > 1E8` is still evaluated with the
previous iteration's value, or garbage on the first iteration. It is harmless today
because the model is already bad.

Rust fix: `evaluate()` returns `Result<Fitness, BadModel>`, so there is no shared scratch
variable.

## B10 (High, latent) initialConcentrations has only 20 entries

`uGlobal.pas`, `InitializeConcentrations`. The array has a fixed length of 20 and is
indexed by species index in the fitness routines and in `determineBoundaryNodes`. With
`maxNumberOfSpecies > 20` it reads out of bounds, and range checking is off, so the
result is garbage. `convertToString` and `outputResults` also hard-code `0 to 19`.

Rust fix: initial concentrations are a user-editable `Vec<f64>`. If it is shorter than
`max_species`, it is extended by cycling the table, and the UI shows the effective values.

## B11 (Medium) config.ini handling is inconsistent

- `fitnessThresholdToStop` defaults to 50.0 in `uGlobal.pas`, but `readIniFile` uses a
  fallback of 10 (`.dpr:118`). Whether `config.ini` exists therefore changes the
  threshold even when the key is absent. `generationIntervalForDetailedOutput` has the
  same pattern.
- The shipped `Win64\Debug\config.ini` uses keys the program never reads
  (`intervalForOutput`, `initialConcentrations`). It also contains a malformed line,
  `minNumberOfSpecies 8` (no `=`), which is silently ignored.

Rust fix: defaults live in one place. The config importer reports unknown keys and
malformed lines instead of ignoring them.

## B12 (Low) The comment in deleteReaction does not match the code

`uMutate.pas:180`: the comment says "Only delete reactions that don't result in a
boundary node", but no such check exists. Any reaction may be deleted.

Rust fix: the behaviour is kept (any reaction may be deleted) and documented accurately.
The misleading comment is not carried over.

## B13 (Low) Creation and mutation apply different validity rules

`TIndividual.Create` rejects models with no input boundary species
(`numberOfInputBoundarySpecies = 0 -> badModel`). Mutation does not re-check this rule,
so the population can drift into models that the initial generator would have rejected.

Rust fix: one `is_viable()` predicate applied at creation and after mutation.
Requiring an input boundary species is a config option (`require_input_boundary`),
**default off**. Many oscillators the Pascal version actually evolved are closed
networks with no input boundary (for example the test oscillator from
`results_osc_1`, seed 10003), which mutation could reach only because the rule was
not re-checked. Enforcing it everywhere would exclude them.

## B14 (Medium) `reduceModel` output can be inconsistent

`oscillatorEvolverProject.dpr:212-264`

- Reduction scores a candidate by setting a rate constant to 0, then deletes the
  reaction with `removeNthReaction` **without** calling `determineBoundaryNodes`. The
  written model's `$` boundary markers and its "Number of floating species" line can
  therefore be stale.
- If you do recompute boundaries after deletion, a species that lost its only producer
  becomes a fixed boundary species. That changes the dynamics compared with the
  zero-rate network that was actually scored, so the written model may not reproduce
  the reported fitness.
- `removeNthReaction` refuses to act when the count is `<= 2`, so reactions with
  `k = 0` can remain in the output.
- Only a single pass is made.

Rust fix: greedy reduction on the real structure. For each reaction in turn, build the
network with it removed, recompute boundaries, re-score it on the confirmation window, and
accept the removal only if the fitness stays within the margin. The exported model is
always the exact network that was scored. The reduced model's fitness is reported next to
the original's.

## B15 (Low) `Simulate()` is dead code and inconsistent

`uMainLoop.pas:315`. This function is unused. It uses Adams instead of BDF, and indexes
initial concentrations by state index instead of species index, which differs from the
fitness routines.

Rust fix: a single `simulate()` used by fitness evaluation, plotting and export.

## B16 (Low, latent) Rate constants can become negative

`uMutate.pas`, `mutateRateConstant`: `k += (u - 0.5) * pct * k`. If
`percentageByWhichRateConstantChanges > 2`, `k` can become negative. Nothing guards this,
and mass action requires `k >= 0`.

Rust fix: config validation caps `pct` at below 2, and the mutated value is clamped to
`>= 0`.

## B17 (Low) Per-node fitness is rewritten inside the species loop

`uMainLoop.pas:643-651` (and the LSODA twin). The loop that copies `deviation[]` into
`nodefitscore` sits inside the per-species `j` loop. It runs n times, and in the early
passes it writes entries that have not been computed yet. The final result is correct,
but the work is O(n^2).

Rust fix: assign once after all deviations are computed.

## B18 (Low) The vestigial C link and stress test

`{$LINK mathlibrary}` / `external add` is unused demo code (`.dpr:63-65`). This is not a
bug, but it blocks a FreePascal build.

Rust fix: not ported.

## B19 (Medium) The oscillation score differs from its Python reference

`uOscillationObjective.pas`, `harmonicFraction`. The Pascal code zero-pads the
Hann-windowed signal to the next power of two before the FFT. The Python reference
(`analysis/objective_prototype.py`, `spectral_harmonic_fraction`) uses the exact
signal length. Padding refines the bin spacing, so each spectral peak spreads over
more bins than the harmonic windows (`+/- max(1, round(0.03*k*f0))` bins) capture.
For a pure sine (t = 0..150, 2048 points, period 10), the harmonic fraction is 0.896
in Pascal versus 0.9999 in Python, so F = 0.794 versus 0.886. Spiky waveforms lose
more (F = 0.654 versus 0.729).

The fundamental window also uses `Round` where Python truncates (`int(f0b)`), and
Delphi's `Round` is banker's rounding.

Effect: every oscillator scores lower than intended, which shifts the meaning of the
thresholds (`oscViableThreshold = 0.3`, `oscFitnessThresholdToStop = 0.05`).

Rust fix: the harmonic fraction is computed exactly as in Python (exact-length FFT
via `rustfft`, the same window bounds). Unit tests check the Python golden values to 1e-5.

## B20 (Medium) The upper bounds of the species and reaction ranges are never drawn

`uIndividual.pas`, `TIndividual.Create`:

```pascal
nSpecies   := rndRange (experimentInfo.minNumberOfSpecies,   experimentInfo.maxNumberOfSpecies);
nReactions := rndRange (experimentInfo.minNumberOfReactions, experimentInfo.maxNumberOfReactions);
```

`rndRange(lo, hi)` (like `Math.RandomRange`) returns `lo .. hi-1`, so a network never
has `maxNumberOfSpecies` species or `maxNumberOfReactions` reactions unless min = max
(then it returns `lo`). With `minNumberOfReactions=3`, `maxNumberOfReactions=9`, a
network gets 3..8 reactions. B1 currently hides this for species.

Rust fix: both ranges are inclusive (`range(min, max + 1)`).

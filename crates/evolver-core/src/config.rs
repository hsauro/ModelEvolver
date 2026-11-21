//! Run configuration: every setting of the Pascal command line and config.ini.

use serde::{Deserialize, Serialize};

use crate::simulate::SimOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectiveKind {
    /// Match the target time course (sum of squared differences).
    TimeCourse,
    /// Sustained oscillation, phase- and amplitude-invariant.
    Oscillation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EvolverConfig {
    // Run
    /// Base seed; 0 = pick one from the clock (the chosen value is recorded).
    pub seed: u32,
    pub num_experiments: u32,
    /// Keep starting new experiments until stopped.
    pub continuous: bool,
    /// Parallel workers; 0 = all hardware threads.
    pub num_workers: u32,

    // GA
    pub max_population_size: usize,
    pub max_generations: u32,
    pub elite_fraction: f64,
    pub add_random_networks: bool,
    pub num_random_networks: usize,

    // Network size (inclusive ranges)
    pub min_species: usize,
    pub max_species: usize,
    pub min_reactions: usize,
    pub max_reactions: usize,
    pub max_initial_rate_constant: f64,
    /// Require at least one input boundary species (B13).
    pub require_input_boundary: bool,

    // Reaction types (normalised when used)
    pub p_uniuni: f64,
    pub p_biuni: f64,
    pub p_unibi: f64,
    pub p_bibi: f64,

    // Mutation
    pub p_rate_mutation: f64,
    pub p_add_reaction: f64,
    pub p_delete_reaction: f64,
    /// A rate mutation changes k by up to +/- half this fraction (must be < 2).
    pub rate_change_fraction: f64,

    // Objective
    pub objective: ObjectiveKind,
    pub ssd_stop_threshold: f64,
    pub ssd_viable_threshold: f64,
    /// Pascal heuristics: stop if best > 800 after gen 600, or < 30 after gen 800.
    pub ssd_stagnation_rules: bool,
    pub osc_t_end: f64,
    pub osc_n_points: usize,
    pub osc_stop_threshold: f64,
    pub osc_viable_threshold: f64,
    pub osc_reduce_margin: f64,
    /// Confirmation window; `osc_confirm_t_end <= 0` disables confirmation.
    pub osc_confirm_t_end: f64,
    pub osc_confirm_n_points: usize,

    // Solver
    pub max_concentration: f64,
    pub rtol: f64,
    pub atol: f64,
    pub max_steps_per_interval: usize,
    pub max_total_steps: usize,

    // Initial concentrations by species index (cycled if too short, B10)
    pub initial_concentrations: Vec<f64>,

    // Constraints
    pub disallow_self_referential: bool,
    pub disallow_autocatalysis: bool,
    /// Pattern text (see `patterns`); empty = no pattern list.
    pub reaction_patterns: String,

    // Output
    pub reduce_models: bool,
    pub add_tellurium_harness: bool,
}

pub const PASCAL_INITIAL_CONCENTRATIONS: [f64; 20] =
    [2.0, 5.0, 7.0, 10.0, 1.0, 0.5, 3.0, 7.0, 12.0, 4.0, 1.0, 3.0, 7.0, 9.0, 1.0, 7.0, 2.0, 8.0, 5.0, 10.0];

impl Default for EvolverConfig {
    /// The Pascal program's built-in defaults (uGlobal.pas), objective set to
    /// oscillation.
    fn default() -> Self {
        Self {
            seed: 0,
            num_experiments: 1,
            continuous: false,
            num_workers: 0,
            max_population_size: 100,
            max_generations: 1600,
            elite_fraction: 0.4,
            add_random_networks: false,
            num_random_networks: 10,
            min_species: 5,
            max_species: 5,
            min_reactions: 4,
            max_reactions: 4,
            max_initial_rate_constant: 50.0,
            require_input_boundary: false,
            p_uniuni: 0.4,
            p_biuni: 0.15,
            p_unibi: 0.15,
            p_bibi: 0.3,
            p_rate_mutation: 0.5,
            p_add_reaction: 0.225,
            p_delete_reaction: 0.275,
            rate_change_fraction: 0.2,
            objective: ObjectiveKind::Oscillation,
            ssd_stop_threshold: 50.0,
            ssd_viable_threshold: 100.0,
            ssd_stagnation_rules: true,
            osc_t_end: 150.0,
            osc_n_points: 2048,
            osc_stop_threshold: 0.05,
            osc_viable_threshold: 0.3,
            osc_reduce_margin: 0.05,
            osc_confirm_t_end: 300.0,
            osc_confirm_n_points: 4096,
            max_concentration: 1000.0,
            rtol: 1e-6,
            atol: 1e-10,
            max_steps_per_interval: 500,
            max_total_steps: 200_000,
            initial_concentrations: PASCAL_INITIAL_CONCENTRATIONS.to_vec(),
            disallow_self_referential: false,
            disallow_autocatalysis: false,
            reaction_patterns: String::new(),
            reduce_models: true,
            add_tellurium_harness: false,
        }
    }
}

impl EvolverConfig {
    /// The settings that actually produced the Pascal oscillator corpus
    /// (`Win64\Debug\config.ini`, as applied by v1.71 including bug B1: 5
    /// species, 9 reactions).
    pub fn oscillator_preset() -> Self {
        Self {
            max_population_size: 130,
            min_species: 5,
            max_species: 5,
            min_reactions: 9,
            max_reactions: 9,
            max_initial_rate_constant: 12.323,
            p_uniuni: 0.2981,
            p_biuni: 0.2542,
            p_unibi: 0.2992,
            p_bibi: 0.1485,
            elite_fraction: 0.107,
            p_rate_mutation: 0.642,
            p_add_reaction: 0.1445,
            p_delete_reaction: 0.1375,
            rate_change_fraction: 0.3489,
            ..Self::default()
        }
    }

    /// Check the settings; returns every problem found.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut e = Vec::new();
        if self.min_species < 2 {
            e.push("at least 2 species are needed".into());
        }
        if self.min_species > self.max_species {
            e.push("min species is larger than max species".into());
        }
        if self.max_species > 200 {
            e.push("at most 200 species are supported".into());
        }
        if self.min_reactions < 1 {
            e.push("at least 1 reaction is needed".into());
        }
        if self.min_reactions > self.max_reactions {
            e.push("min reactions is larger than max reactions".into());
        }
        if self.max_population_size < 2 {
            e.push("the population needs at least 2 individuals".into());
        }
        if self.max_generations < 1 {
            e.push("at least 1 generation is needed".into());
        }
        let types = [self.p_uniuni, self.p_biuni, self.p_unibi, self.p_bibi];
        if types.iter().any(|p| *p < 0.0 || !p.is_finite()) || types.iter().sum::<f64>() <= 0.0 {
            e.push("reaction-type probabilities must be >= 0 and not all zero".into());
        }
        let muts = [self.p_rate_mutation, self.p_add_reaction, self.p_delete_reaction];
        if muts.iter().any(|p| *p < 0.0 || !p.is_finite()) {
            e.push("mutation probabilities must be >= 0".into());
        }
        if !(0.0..=1.0).contains(&self.elite_fraction) {
            e.push("elite fraction must be between 0 and 1".into());
        }
        if !(0.0..2.0).contains(&self.rate_change_fraction) {
            e.push("rate change fraction must be >= 0 and < 2 (B16)".into());
        }
        if self.max_initial_rate_constant <= 0.0 {
            e.push("max initial rate constant must be > 0".into());
        }
        if self.osc_t_end <= 0.0 || self.osc_n_points < 32 {
            e.push("oscillation window needs t_end > 0 and at least 32 points".into());
        }
        if self.osc_confirm_t_end > 0.0 && self.osc_confirm_n_points < 32 {
            e.push("confirmation window needs at least 32 points".into());
        }
        if self.rtol <= 0.0 || self.atol <= 0.0 {
            e.push("solver tolerances must be > 0".into());
        }
        if self.max_concentration <= 0.0 {
            e.push("max concentration must be > 0".into());
        }
        if self.initial_concentrations.is_empty() || self.initial_concentrations.iter().any(|c| *c < 0.0 || !c.is_finite()) {
            e.push("initial concentrations must be a non-empty list of values >= 0".into());
        }
        if e.is_empty() { Ok(()) } else { Err(e) }
    }

    /// Initial concentrations for `n` species: the configured list, cycled (B10).
    pub fn effective_initial(&self, n: usize) -> Vec<f64> {
        let table = if self.initial_concentrations.is_empty() {
            &PASCAL_INITIAL_CONCENTRATIONS[..]
        } else {
            &self.initial_concentrations[..]
        };
        (0..n).map(|i| table[i % table.len()]).collect()
    }

    /// Reaction-type probabilities, normalised to sum to 1.
    pub fn type_probabilities(&self) -> [f64; 4] {
        let p = [self.p_uniuni, self.p_biuni, self.p_unibi, self.p_bibi];
        let s: f64 = p.iter().sum();
        if s > 0.0 { p.map(|x| x / s) } else { [0.25; 4] }
    }

    pub fn sim_options(&self, t_end: f64, n_points: usize) -> SimOptions {
        SimOptions {
            t_end,
            n_points,
            rtol: self.rtol,
            atol: self.atol,
            max_steps_per_interval: self.max_steps_per_interval,
            max_total_steps: self.max_total_steps,
            max_concentration: self.max_concentration,
        }
    }

    /// Apply a Pascal `config.ini` (`[ConfigSettings]` section). Returns
    /// warnings for unknown keys and malformed lines (B11).
    pub fn apply_pascal_ini(&mut self, text: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        let mut in_settings = false;
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_settings = line.eq_ignore_ascii_case("[ConfigSettings]");
                continue;
            }
            if !in_settings {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("line {}: no '=' in '{line}', ignored", i + 1));
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let num = || value.parse::<f64>().map_err(|_| format!("line {}: '{value}' is not a number", i + 1));
            let boolean = || match value.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" => Ok(true),
                "0" | "false" | "no" => Ok(false),
                _ => Err(format!("line {}: '{value}' is not true/false", i + 1)),
            };
            let result: Result<(), String> = (|| {
                match key.to_ascii_lowercase().as_str() {
                    "maxnumberofgenerations" => self.max_generations = num()? as u32,
                    "maxpopulationsize" => self.max_population_size = num()? as usize,
                    "probabilityofuniuni" => self.p_uniuni = num()?,
                    "probabilityofbiuni" => self.p_biuni = num()?,
                    "probabilityofunibi" => self.p_unibi = num()?,
                    "probabilityofbibi" => self.p_bibi = num()?,
                    "percentageclone" => self.elite_fraction = num()?,
                    "maxnumberofspecies" => self.max_species = num()? as usize,
                    "minnumberofspecies" => self.min_species = num()? as usize,
                    "maxnumberofreactions" => self.max_reactions = num()? as usize,
                    "minnumberofreactions" => self.min_reactions = num()? as usize,
                    "maxinitialrateconstant" => self.max_initial_rate_constant = num()?,
                    "rateconstantmutation" => self.p_rate_mutation = num()?,
                    "percentagebywhichrateconstantchanges" => self.rate_change_fraction = num()?,
                    "addreactionmutation" => self.p_add_reaction = num()?,
                    "deletereactionmutation" => self.p_delete_reaction = num()?,
                    "fitnessthresholdtostop" => self.ssd_stop_threshold = num()?,
                    "maxconcentrationcap" => self.max_concentration = num()?,
                    "disallowselfreferential" => self.disallow_self_referential = boolean()?,
                    "disallowautocatalysis" => self.disallow_autocatalysis = boolean()?,
                    "useoscillationobjective" => {
                        self.objective = if boolean()? { ObjectiveKind::Oscillation } else { ObjectiveKind::TimeCourse }
                    }
                    "osctimeend" => self.osc_t_end = num()?,
                    "oscnumberofpoints" => self.osc_n_points = num()? as usize,
                    "oscfitnessthresholdtostop" => self.osc_stop_threshold = num()?,
                    "oscreducemargin" => self.osc_reduce_margin = num()?,
                    "oscviablethreshold" => self.osc_viable_threshold = num()?,
                    "oscconfirmtimeend" => self.osc_confirm_t_end = num()?,
                    "oscconfirmnumberofpoints" => self.osc_confirm_n_points = num()? as usize,
                    "initialconcentrations" => {
                        let v: Result<Vec<f64>, _> = value.split(',').map(|s| s.trim().parse::<f64>()).collect();
                        self.initial_concentrations = v.map_err(|_| format!("line {}: bad concentration list", i + 1))?;
                    }
                    "generationintervalfordetailedoutput" | "numberofdotsperlineinsilentmode" | "intervalforoutput" => {
                        return Err(format!("line {}: '{key}' controls console output only, ignored", i + 1));
                    }
                    "reactionpatternfile" => {
                        return Err(format!(
                            "line {}: reactionPatternFile = '{value}': load the pattern file separately",
                            i + 1
                        ));
                    }
                    _ => return Err(format!("line {}: unknown key '{key}', ignored", i + 1)),
                }
                Ok(())
            })();
            if let Err(w) = result {
                warnings.push(w);
            }
        }
        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        EvolverConfig::default().validate().unwrap();
        EvolverConfig::oscillator_preset().validate().unwrap();
    }

    #[test]
    fn validation_catches_problems() {
        let cfg = EvolverConfig { min_species: 6, max_species: 5, rate_change_fraction: 2.5, ..Default::default() };
        assert_eq!(cfg.validate().unwrap_err().len(), 2);
    }

    #[test]
    fn initial_cycles() {
        let cfg = EvolverConfig { initial_concentrations: vec![1.0, 2.0, 3.0], ..Default::default() };
        assert_eq!(cfg.effective_initial(7), vec![1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0]);
    }

    #[test]
    fn pascal_ini_import() {
        // The shipped Win64\Debug\config.ini (abridged), including its malformed line.
        let ini = "[ConfigSettings]\nmaxNumberOfGenerations=341\nmaxPopulationSize=130\npercentageClone=0.107\n\
                   initialConcentrations=5.0\nmaxNumberOfSpecies=8\nminNumberOfSpecies 8\nmaxNumberOfReactions=9\n\
                   intervalForOutput=10\noscTimeEnd=80\noscNumberOfPoints=1024\n\n[Other]\ntestSettings=-s 1 -sg\n";
        let mut cfg = EvolverConfig::default();
        let w = cfg.apply_pascal_ini(ini);
        assert_eq!(cfg.max_generations, 341);
        assert_eq!(cfg.max_population_size, 130);
        assert_eq!(cfg.max_species, 8);
        assert_eq!(cfg.min_species, 5); // malformed line ignored
        assert_eq!(cfg.initial_concentrations, vec![5.0]);
        assert_eq!(cfg.osc_n_points, 1024);
        assert_eq!(w.len(), 2, "{w:?}"); // missing '=' and intervalForOutput
    }
}

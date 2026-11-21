//! Fixed networks and a simple random-network generator, used by tests,
//! benchmarks and the M0 solver spike.

use crate::network::{Network, Reaction, ReactionKind, ReactionKind::*};
use crate::rng::Rng;

/// The Pascal version's default initial-concentration table.
pub const PASCAL_INITIAL: [f64; 20] =
    [2.0, 5.0, 7.0, 10.0, 1.0, 0.5, 3.0, 7.0, 12.0, 4.0, 1.0, 3.0, 7.0, 9.0, 1.0, 7.0, 2.0, 8.0, 5.0, 10.0];

/// An oscillator evolved by the Pascal version (results_osc_1, seed 10003,
/// node fitness ~0.01, i.e. F ~ 0.99).
pub fn pascal_oscillator() -> Network {
    let r = |kind, k, re: &[u16], pr: &[u16]| Reaction::new(kind, k, re, pr);
    Network::new(
        0,
        5,
        vec![
            r(BiBi, 2.7655719973597, &[4, 2], &[0, 2]),
            r(BiUni, 3.77762207066864, &[0, 0], &[2]),
            r(UniUni, 0.456113405417214, &[3], &[1]),
            r(BiBi, 0.283401144043187, &[1, 4], &[4, 4]),
            r(UniBi, 0.915381746208189, &[2], &[2, 4]),
            r(BiUni, 0.689777862293152, &[1, 2], &[3]),
            r(UniBi, 1.08971029949999, &[4], &[3, 1]),
            r(BiUni, 0.370448401998388, &[2, 4], &[3]),
            r(UniBi, 0.819196505319224, &[3], &[4, 1]),
            r(BiUni, 0.473105676120906, &[0, 1], &[3]),
        ],
    )
}

/// A random network shaped like the GA's initial population (no pattern
/// constraints; used for benchmarking the evaluation cost).
pub fn random_network(rng: &mut Rng, n_species: usize, n_reactions: usize, max_k: f64) -> Network {
    let kinds = [UniUni, BiUni, UniBi, BiBi];
    let mut reactions = Vec::with_capacity(n_reactions);
    while reactions.len() < n_reactions {
        let kind: ReactionKind = kinds[rng.index(4)];
        let re: Vec<u16> = (0..kind.n_reactants()).map(|_| rng.index(n_species) as u16).collect();
        let pr: Vec<u16> = (0..kind.n_products()).map(|_| rng.index(n_species) as u16).collect();
        if kind == UniUni && re[0] == pr[0] {
            continue;
        }
        reactions.push(Reaction::new(kind, rng.next_f64() * max_k, &re, &pr));
    }
    Network::new(0, n_species, reactions)
}

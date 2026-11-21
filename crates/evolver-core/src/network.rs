//! Mass-action reaction networks: the genome the GA evolves.
//!
//! Species are identified by index. Each species is classified from the reaction
//! list (see [`Network::classify`]): a species that is consumed and produced is
//! floating (an ODE state variable); one that is only produced is an output
//! boundary; one that is only consumed is an input boundary held at its initial
//! concentration; one that appears in no reaction is an orphan.

use serde::{Deserialize, Serialize};

use crate::compile::{CompiledModel, Operand};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReactionKind {
    /// S -> P
    UniUni,
    /// S1 + S2 -> P
    BiUni,
    /// S -> P1 + P2
    UniBi,
    /// S1 + S2 -> P1 + P2
    BiBi,
}

impl ReactionKind {
    pub fn n_reactants(self) -> usize {
        match self {
            ReactionKind::UniUni | ReactionKind::UniBi => 1,
            ReactionKind::BiUni | ReactionKind::BiBi => 2,
        }
    }

    pub fn n_products(self) -> usize {
        match self {
            ReactionKind::UniUni | ReactionKind::BiUni => 1,
            ReactionKind::UniBi | ReactionKind::BiBi => 2,
        }
    }
}

/// One mass-action reaction. `conn` holds species indices in the Pascal
/// connection order: reactants first, then products, unused slots 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub kind: ReactionKind,
    pub k: f64,
    pub conn: [u16; 4],
}

impl Reaction {
    pub fn new(kind: ReactionKind, k: f64, reactants: &[u16], products: &[u16]) -> Self {
        debug_assert_eq!(reactants.len(), kind.n_reactants());
        debug_assert_eq!(products.len(), kind.n_products());
        let mut conn = [0u16; 4];
        for (slot, s) in conn.iter_mut().zip(reactants.iter().chain(products)) {
            *slot = *s;
        }
        Self { kind, k, conn }
    }

    pub fn reactants(&self) -> &[u16] {
        &self.conn[..self.kind.n_reactants()]
    }

    pub fn products(&self) -> &[u16] {
        let r = self.kind.n_reactants();
        &self.conn[r..r + self.kind.n_products()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeciesStatus {
    Float,
    BoundaryInput,
    BoundaryOutput,
    Orphan,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Network {
    /// Lineage id; renewed whenever the topology changes.
    pub id: u64,
    pub n_species: usize,
    pub reactions: Vec<Reaction>,
    /// Derived by `classify`; one entry per species.
    pub status: Vec<SpeciesStatus>,
    /// State-vector index of each floating species (None for the others).
    pub y_index: Vec<Option<u16>>,
    pub n_float: usize,
    pub n_input: usize,
    pub n_output: usize,
}

impl Network {
    pub fn new(id: u64, n_species: usize, reactions: Vec<Reaction>) -> Self {
        let mut net = Self {
            id,
            n_species,
            reactions,
            status: Vec::new(),
            y_index: Vec::new(),
            n_float: 0,
            n_input: 0,
            n_output: 0,
        };
        net.classify();
        net
    }

    /// Recompute every species' status from the reaction list (the port of
    /// `determineBoundaryNodes`). The consumption/production counts are derived
    /// here rather than maintained incrementally, so they cannot drift.
    pub fn classify(&mut self) {
        let n = self.n_species;
        let mut leavings = vec![0u32; n];
        let mut enterings = vec![0u32; n];
        for r in &self.reactions {
            for &s in r.reactants() {
                leavings[s as usize] += 1;
            }
            for &p in r.products() {
                enterings[p as usize] += 1;
            }
        }
        self.status.clear();
        self.y_index.clear();
        self.n_float = 0;
        self.n_input = 0;
        self.n_output = 0;
        for i in 0..n {
            let (status, y) = match (leavings[i], enterings[i]) {
                (0, 0) => (SpeciesStatus::Orphan, None),
                (0, _) => {
                    self.n_output += 1;
                    (SpeciesStatus::BoundaryOutput, None)
                }
                (_, 0) => {
                    self.n_input += 1;
                    (SpeciesStatus::BoundaryInput, None)
                }
                _ => {
                    let idx = self.n_float as u16;
                    self.n_float += 1;
                    (SpeciesStatus::Float, Some(idx))
                }
            };
            self.status.push(status);
            self.y_index.push(y);
        }
    }

    /// Indices of the floating species, in state-vector order.
    pub fn floating_species(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.n_species).filter(|&i| self.status[i] == SpeciesStatus::Float)
    }

    /// Compile into the flat form the solver evaluates. `init` gives each
    /// species' initial concentration by species index (and the fixed level of
    /// input boundary species); it must have at least `n_species` entries.
    pub fn compile(&self, init: &[f64]) -> CompiledModel {
        let operand = |s: u16| -> Operand {
            match self.y_index[s as usize] {
                Some(y) => Operand::State(y),
                // Output boundary species never appear as reactants, so only
                // input boundaries reach here; they are held at their initial value.
                None => Operand::Const(init[s as usize]),
            }
        };
        let mut model = CompiledModel::new(self.n_float);
        for r in &self.reactions {
            let reactants: Vec<Operand> = r.reactants().iter().map(|&s| operand(s)).collect();
            let consumed: Vec<u16> = r.reactants().iter().filter_map(|&s| self.y_index[s as usize]).collect();
            let produced: Vec<u16> = r.products().iter().filter_map(|&s| self.y_index[s as usize]).collect();
            model.add_term(r.k, &reactants, &consumed, &produced);
        }
        let y0 = self.floating_species().map(|i| init[i]).collect();
        model.set_initial(y0);
        model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ReactionKind::*;

    #[test]
    fn classification() {
        // $S0 -> S1; S1 -> S2 + S3; S2 + S1 -> S1   (S3 is output, S4 orphan)
        let net = Network::new(
            0,
            5,
            vec![
                Reaction::new(UniUni, 1.0, &[0], &[1]),
                Reaction::new(UniBi, 1.0, &[1], &[2, 3]),
                Reaction::new(BiUni, 1.0, &[2, 1], &[1]),
            ],
        );
        use SpeciesStatus::*;
        assert_eq!(net.status, vec![BoundaryInput, Float, Float, BoundaryOutput, Orphan]);
        assert_eq!(net.y_index, vec![None, Some(0), Some(1), None, None]);
        assert_eq!((net.n_float, net.n_input, net.n_output), (2, 1, 1));
    }
}

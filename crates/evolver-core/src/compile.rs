//! The flat, allocation-free form of a network that the ODE solver evaluates.
//!
//! Each reaction becomes a [`Term`]: a rate `k * a * b` (with `a`, `b` either a
//! state variable or a constant boundary level) and the net stoichiometric change
//! it causes in each floating species. Repeated species are merged, so
//! `A + B -> 2B` contributes `{A: -1, B: +1}`. The RHS is then one pass over the
//! terms with no branching on species status.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Operand {
    /// Index into the state vector.
    State(u16),
    /// A boundary species held at a fixed concentration.
    Const(f64),
}

impl Operand {
    #[inline]
    fn value(self, y: &[f64]) -> f64 {
        match self {
            Operand::State(i) => y[i as usize],
            Operand::Const(c) => c,
        }
    }

    #[inline]
    fn tangent(self, v: &[f64]) -> f64 {
        match self {
            Operand::State(i) => v[i as usize],
            Operand::Const(_) => 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Term {
    pub k: f64,
    pub a: Operand,
    pub b: Option<Operand>,
    /// Range into `CompiledModel::delta_idx` / `delta_val`.
    deltas: (u32, u32),
}

#[derive(Clone, Debug)]
pub struct CompiledModel {
    n: usize,
    terms: Vec<Term>,
    delta_idx: Vec<u16>,
    delta_val: Vec<f64>,
    y0: Vec<f64>,
}

impl CompiledModel {
    pub fn new(n_states: usize) -> Self {
        Self { n: n_states, terms: Vec::new(), delta_idx: Vec::new(), delta_val: Vec::new(), y0: vec![0.0; n_states] }
    }

    pub fn n_states(&self) -> usize {
        self.n
    }

    pub fn initial(&self) -> &[f64] {
        &self.y0
    }

    pub fn set_initial(&mut self, y0: Vec<f64>) {
        assert_eq!(y0.len(), self.n);
        self.y0 = y0;
    }

    /// Add a mass-action term. `reactants` (1 or 2) give the rate law;
    /// `consumed` / `produced` list the floating-species state indices that the
    /// reaction removes / creates (boundary species already filtered out).
    pub fn add_term(&mut self, k: f64, reactants: &[Operand], consumed: &[u16], produced: &[u16]) {
        assert!(matches!(reactants.len(), 1 | 2), "mass-action terms take 1 or 2 reactants");
        let start = self.delta_idx.len() as u32;
        let mut net: Vec<(u16, f64)> = Vec::with_capacity(4);
        let mut bump = |i: u16, d: f64| match net.iter_mut().find(|(j, _)| *j == i) {
            Some((_, v)) => *v += d,
            None => net.push((i, d)),
        };
        for &i in consumed {
            bump(i, -1.0);
        }
        for &i in produced {
            bump(i, 1.0);
        }
        for (i, d) in net {
            if d != 0.0 {
                self.delta_idx.push(i);
                self.delta_val.push(d);
            }
        }
        let end = self.delta_idx.len() as u32;
        self.terms.push(Term { k, a: reactants[0], b: reactants.get(1).copied(), deltas: (start, end) });
    }

    #[inline]
    fn scatter(&self, term: &Term, rate: f64, out: &mut [f64]) {
        let (s, e) = (term.deltas.0 as usize, term.deltas.1 as usize);
        for (i, d) in self.delta_idx[s..e].iter().zip(&self.delta_val[s..e]) {
            out[*i as usize] += d * rate;
        }
    }

    /// dy/dt = f(y).
    pub fn rhs(&self, y: &[f64], out: &mut [f64]) {
        out.fill(0.0);
        for t in &self.terms {
            let mut rate = t.k * t.a.value(y);
            if let Some(b) = t.b {
                rate *= b.value(y);
            }
            self.scatter(t, rate, out);
        }
    }

    /// out = J(y) * v, with J the exact Jacobian of `rhs`.
    pub fn jac_mul(&self, y: &[f64], v: &[f64], out: &mut [f64]) {
        out.fill(0.0);
        for t in &self.terms {
            let a = t.a.value(y);
            let da = t.a.tangent(v);
            let drate = match t.b {
                None => t.k * da,
                Some(b) => t.k * (da * b.value(y) + a * b.tangent(v)),
            };
            if drate != 0.0 {
                self.scatter(t, drate, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::network::{Network, Reaction, ReactionKind::*};
    use crate::rng::Rng;

    fn random_network(rng: &mut Rng) -> Network {
        let n = 6;
        let kinds = [UniUni, BiUni, UniBi, BiBi];
        let reactions = (0..10)
            .map(|_| {
                let kind = kinds[rng.index(4)];
                let mut pick = || rng.index(n) as u16;
                let r: Vec<u16> = (0..kind.n_reactants()).map(|_| pick()).collect();
                let p: Vec<u16> = (0..kind.n_products()).map(|_| pick()).collect();
                Reaction::new(kind, 0.1 + 5.0 * rng.next_f64(), &r, &p)
            })
            .collect();
        Network::new(0, n, reactions)
    }

    #[test]
    fn rhs_by_hand() {
        // $S0 -> S1 (k=2); S1 + S1 -> S2 (k=3); S2 -> S1 + S3 (k=0.5)
        let net = Network::new(
            0,
            4,
            vec![
                Reaction::new(UniUni, 2.0, &[0], &[1]),
                Reaction::new(BiUni, 3.0, &[1, 1], &[2]),
                Reaction::new(UniBi, 0.5, &[2], &[1, 3]),
            ],
        );
        let init = [4.0, 1.0, 2.0, 0.0];
        let m = net.compile(&init);
        assert_eq!(m.n_states(), 2); // S1, S2 float; S0 input; S3 output
        let y = [1.5, 2.0];
        let mut f = [0.0; 2];
        m.rhs(&y, &mut f);
        let v1 = 2.0 * 4.0;
        let v2 = 3.0 * 1.5 * 1.5;
        let v3 = 0.5 * 2.0;
        assert!((f[0] - (v1 - 2.0 * v2 + v3)).abs() < 1e-12);
        assert!((f[1] - (v2 - v3)).abs() < 1e-12);
    }

    #[test]
    fn jacobian_matches_finite_differences() {
        let mut rng = Rng::new(99);
        let init: Vec<f64> = (0..6).map(|i| 1.0 + i as f64).collect();
        for _ in 0..200 {
            let net = random_network(&mut rng);
            let m = net.compile(&init);
            let n = m.n_states();
            if n == 0 {
                continue;
            }
            let y: Vec<f64> = (0..n).map(|_| 0.5 + rng.next_f64()).collect();
            let v: Vec<f64> = (0..n).map(|_| rng.next_f64() - 0.5).collect();
            let mut jv = vec![0.0; n];
            m.jac_mul(&y, &v, &mut jv);
            let h = 1e-6;
            let yp: Vec<f64> = y.iter().zip(&v).map(|(a, b)| a + h * b).collect();
            let ym: Vec<f64> = y.iter().zip(&v).map(|(a, b)| a - h * b).collect();
            let (mut fp, mut fm) = (vec![0.0; n], vec![0.0; n]);
            m.rhs(&yp, &mut fp);
            m.rhs(&ym, &mut fm);
            for i in 0..n {
                let fd = (fp[i] - fm[i]) / (2.0 * h);
                assert!((fd - jv[i]).abs() < 1e-6 * (1.0 + fd.abs()), "J*v mismatch: {fd} vs {}", jv[i]);
            }
        }
    }
}

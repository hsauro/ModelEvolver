//! Read a mass-action model written by this program or by the Pascal version
//! (Antimony, species named `S<i>`) back into a [`Network`], via the
//! `antimony` crate.

use crate::network::{Network, Reaction, ReactionKind};

#[derive(Clone, Debug)]
pub struct ImportedModel {
    pub net: Network,
    /// Initial concentration by species index (0 where none is given).
    pub init: Vec<f64>,
}

fn species_index(name: &str) -> Result<usize, String> {
    name.strip_prefix('S')
        .and_then(|n| n.parse::<usize>().ok())
        .ok_or_else(|| format!("species '{name}' is not named S<number>"))
}

/// Import an Antimony model whose reactions are mass action with 1-2
/// reactants and 1-2 products, and whose species are named `S0`, `S1`, ...
pub fn network_from_antimony(text: &str) -> Result<ImportedModel, String> {
    let loaded = antimony::parse(text);
    if loaded.has_errors() {
        let errs: Vec<String> = loaded.diagnostics.iter().map(|d| d.to_string()).collect();
        return Err(errs.join("; "));
    }
    let m = loaded.document.main_module();
    let mut reactions = Vec::new();
    let mut n_species = 0;
    for r in &m.reactions {
        let expand = |ps: &[antimony::model::Participant]| -> Result<Vec<u16>, String> {
            let mut out = Vec::new();
            for p in ps {
                let st = m.stoichiometry(p);
                if st.fract() != 0.0 || st < 1.0 {
                    return Err(format!("{}: stoichiometry {st} is not supported", r.name));
                }
                for _ in 0..st as usize {
                    out.push(species_index(&p.species)? as u16);
                }
            }
            Ok(out)
        };
        let re = expand(&r.reactants)?;
        let pr = expand(&r.products)?;
        let kind = match (re.len(), pr.len()) {
            (1, 1) => ReactionKind::UniUni,
            (2, 1) => ReactionKind::BiUni,
            (1, 2) => ReactionKind::UniBi,
            (2, 2) => ReactionKind::BiBi,
            (a, b) => return Err(format!("{}: {a} reactants and {b} products are not supported", r.name)),
        };
        // Rate constant: the factors of the law that are not species.
        let law = r.kinetic_law.as_ref().ok_or_else(|| format!("{} has no rate law", r.name))?;
        let mut k = 1.0;
        for factor in law.to_string().split('*') {
            let f = factor.trim();
            if species_index(f).is_ok() && m.symbol(f).is_some_and(|s| s.kind == antimony::model::SymbolKind::Species) {
                continue;
            }
            k *= match f.parse::<f64>() {
                Ok(v) => v,
                Err(_) => m.value(f).ok_or_else(|| format!("{}: cannot evaluate '{f}'", r.name))?,
            };
        }
        for &s in re.iter().chain(&pr) {
            n_species = n_species.max(s as usize + 1);
        }
        reactions.push(Reaction::new(kind, k, &re, &pr));
    }
    if reactions.is_empty() {
        return Err("the model has no reactions".into());
    }
    let init = (0..n_species).map(|i| m.concentration(&format!("S{i}")).unwrap_or(0.0)).collect();
    Ok(ImportedModel { net: Network::new(0, n_species, reactions), init })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::network_antimony;
    use crate::samples::{PASCAL_INITIAL, pascal_oscillator};

    #[test]
    fn pascal_file_format() {
        let text = "// Fitness of nodes:\n//    0.009, \n\nS4 + S2 -> S0 + S2; S4*S2*k0;\nS0 + S0 -> S2;  S0*S0*k1;\n$S3 -> S1;  S3*k2;\n\nS0 = 2; S1 = 5; S2 = 7; S3 = 10; S4 = 1; \n\nk0 = 2.5;\nk1 = 3;\nk2 = 0.5;\n";
        let m = network_from_antimony(text).unwrap();
        assert_eq!(m.net.reactions.len(), 3);
        assert_eq!(m.net.reactions[1].kind, ReactionKind::BiUni);
        assert_eq!(m.net.reactions[1].reactants(), &[0, 0]);
        assert_eq!(m.net.reactions[0].k, 2.5);
        assert_eq!(m.init, vec![2.0, 5.0, 7.0, 10.0, 1.0]);
    }

    #[test]
    fn round_trip_with_export() {
        let net = pascal_oscillator();
        let m = network_from_antimony(&network_antimony(&net, &PASCAL_INITIAL)).unwrap();
        assert_eq!(m.net.reactions, net.reactions);
    }
}

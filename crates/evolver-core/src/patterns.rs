//! Constraints on which reaction "scheme types" may be generated.
//!
//! A port of `uReactionPatterns.pas`. A pattern describes the identity
//! structure of a reaction, independent of the actual species and of reactant
//! or product order: `A + B -> A` also matches `C + D -> C` and `B + A -> B`.
//!
//! Pattern text (one pattern per line):
//!
//! ```text
//! # comment
//! mode: disallow            (or 'mode: allow'; optional, default disallow)
//! A + B -> A
//! 2A -> A                   ('2A' is shorthand for 'A + A')
//! ```
//!
//! Each reaction reduces to a small canonical signature (reaction type in bits
//! 8..9, plus the smallest first-appearance 2-bit label code over the allowed
//! reactant/product swaps), so checking a reaction is one table lookup.

use serde::{Deserialize, Serialize};

use crate::network::ReactionKind;
use crate::rng::Rng;

const SIG_COUNT: usize = 1024;
const MAX_PATTERN_RETRIES: usize = 30;

/// A problem in one line of a pattern text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatternError {
    /// 1-based line number.
    pub line: usize,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct PatternSet {
    /// True when a pattern list is in force (as opposed to only the class toggles).
    pub enabled: bool,
    /// Reject any reaction where a reactant also appears as a product.
    pub no_self_referential: bool,
    /// Reject net self-production (A -> 2A, A + B -> 2B).
    pub no_autocatalysis: bool,
    reject: Box<[bool; SIG_COUNT]>,
}

impl Default for PatternSet {
    fn default() -> Self {
        Self { enabled: false, no_self_referential: false, no_autocatalysis: false, reject: Box::new([false; SIG_COUNT]) }
    }
}

fn kind_code(kind: ReactionKind) -> usize {
    match kind {
        ReactionKind::UniUni => 0,
        ReactionKind::BiUni => 1,
        ReactionKind::UniBi => 2,
        ReactionKind::BiBi => 3,
    }
}

/// First-appearance labelling of `v`, packed 2 bits per slot.
fn pack_labels(v: &[u32]) -> usize {
    let mut seen: [u32; 4] = [0; 4];
    let mut n_seen = 0;
    let mut code = 0usize;
    for &x in v {
        let label = match seen[..n_seen].iter().position(|&s| s == x) {
            Some(l) => l,
            None => {
                seen[n_seen] = x;
                n_seen += 1;
                n_seen - 1
            }
        };
        code = (code << 2) | label;
    }
    code
}

/// Canonical signature of a reaction; `c` in Pascal connection order.
fn canonical_sig(kind: ReactionKind, c: [u32; 4]) -> usize {
    let [c0, c1, c2, c3] = c;
    let best = match kind {
        ReactionKind::UniUni => pack_labels(&[c0, c1]),
        ReactionKind::BiUni => pack_labels(&[c0, c1, c2]).min(pack_labels(&[c1, c0, c2])),
        ReactionKind::UniBi => pack_labels(&[c0, c1, c2]).min(pack_labels(&[c0, c2, c1])),
        ReactionKind::BiBi => pack_labels(&[c0, c1, c2, c3])
            .min(pack_labels(&[c1, c0, c2, c3]))
            .min(pack_labels(&[c0, c1, c3, c2]))
            .min(pack_labels(&[c1, c0, c3, c2])),
    };
    (kind_code(kind) << 8) | best
}

fn is_self_referential(kind: ReactionKind, c: [u16; 4]) -> bool {
    let [c0, c1, c2, c3] = c;
    match kind {
        ReactionKind::UniUni => c0 == c1,
        ReactionKind::BiUni => c2 == c0 || c2 == c1,
        ReactionKind::UniBi => c0 == c1 || c0 == c2,
        ReactionKind::BiBi => c2 == c0 || c2 == c1 || c3 == c0 || c3 == c1,
    }
}

fn is_autocatalytic(kind: ReactionKind, c: [u16; 4]) -> bool {
    let [c0, c1, c2, c3] = c;
    match kind {
        ReactionKind::UniBi => c1 == c0 && c2 == c0,
        ReactionKind::BiBi => c2 == c3 && c0 != c1 && (c2 == c0 || c2 == c1),
        _ => false,
    }
}

/// Parse one side ("A + B", "2A", "A + A") into one or two species letters.
fn parse_side(s: &str) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for term in s.split('+') {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        let mut chars = term.chars().peekable();
        let mut reps = 1;
        if let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
            reps = d as usize;
            chars.next();
        }
        let name: String = chars.collect::<String>().trim().to_string();
        let letter = name.chars().next().ok_or_else(|| format!("missing species in '{term}'"))?;
        if name.chars().count() != 1 || !letter.is_alphabetic() {
            return Err(format!("species must be a single letter, found '{name}'"));
        }
        for _ in 0..reps {
            out.push(letter.to_ascii_uppercase() as u32);
        }
    }
    if out.is_empty() || out.len() > 2 {
        return Err(format!("each side needs 1 or 2 species, found {}", out.len()));
    }
    Ok(out)
}

impl PatternSet {
    /// Only the class toggles, no pattern list.
    pub fn with_toggles(no_self_referential: bool, no_autocatalysis: bool) -> Self {
        Self { no_self_referential, no_autocatalysis, ..Default::default() }
    }

    /// Parse a pattern text. An empty text (only blanks/comments) leaves the
    /// pattern list disabled. Malformed lines are reported and skipped.
    pub fn parse(text: &str, no_self_referential: bool, no_autocatalysis: bool) -> (Self, Vec<PatternError>) {
        let mut set = Self::with_toggles(no_self_referential, no_autocatalysis);
        let mut errors = Vec::new();
        let mut listed = [false; SIG_COUNT];
        let mut allow_mode = false;
        let mut any = false;
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let err = |message: String| PatternError { line: i + 1, message };
            if line.len() >= 5 && line[..5].eq_ignore_ascii_case("mode:") {
                match line[5..].trim().to_ascii_lowercase().as_str() {
                    "allow" => allow_mode = true,
                    "disallow" => allow_mode = false,
                    other => errors.push(err(format!("unknown mode '{other}' (use allow or disallow)"))),
                }
                continue;
            }
            let Some((lhs, rhs)) = line.split_once("->") else {
                errors.push(err("no '->' in pattern".into()));
                continue;
            };
            let (r, p) = match (parse_side(lhs), parse_side(rhs)) {
                (Ok(r), Ok(p)) => (r, p),
                (Err(e), _) | (_, Err(e)) => {
                    errors.push(err(e));
                    continue;
                }
            };
            let (kind, c) = match (r.len(), p.len()) {
                (1, 1) => (ReactionKind::UniUni, [r[0], p[0], 0, 0]),
                (2, 1) => (ReactionKind::BiUni, [r[0], r[1], p[0], 0]),
                (1, 2) => (ReactionKind::UniBi, [r[0], p[0], p[1], 0]),
                _ => (ReactionKind::BiBi, [r[0], r[1], p[0], p[1]]),
            };
            listed[canonical_sig(kind, c)] = true;
            any = true;
        }
        if any || allow_mode {
            for (rej, &l) in set.reject.iter_mut().zip(listed.iter()) {
                *rej = if allow_mode { !l } else { l };
            }
            set.enabled = true;
        }
        (set, errors)
    }

    /// True if the reaction is permitted. The null `A -> A` is always rejected.
    pub fn allowed(&self, kind: ReactionKind, c: [u16; 4]) -> bool {
        if kind == ReactionKind::UniUni && c[0] == c[1] {
            return false;
        }
        if self.no_self_referential && is_self_referential(kind, c) {
            return false;
        }
        if self.no_autocatalysis && is_autocatalytic(kind, c) {
            return false;
        }
        if self.enabled {
            let c32 = [c[0] as u32, c[1] as u32, c[2] as u32, c[3] as u32];
            if self.reject[canonical_sig(kind, c32)] {
                return false;
            }
        }
        true
    }

    /// Draw random species for a reaction of `kind` until an allowed one is
    /// found (up to 30 tries). Returns connections in Pascal order.
    pub fn pick(&self, rng: &mut Rng, kind: ReactionKind, n_species: usize) -> Option<[u16; 4]> {
        let n = n_species as i32;
        for _ in 0..MAX_PATTERN_RETRIES {
            let mut c = [0u16; 4];
            let slots = kind.n_reactants() + kind.n_products();
            for slot in c.iter_mut().take(slots) {
                *slot = rng.range(0, n) as u16;
            }
            if self.allowed(kind, c) {
                return Some(c);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ReactionKind::*;

    fn disallow(text: &str) -> PatternSet {
        let (set, errors) = PatternSet::parse(text, false, false);
        assert!(errors.is_empty(), "{errors:?}");
        set
    }

    #[test]
    fn null_reaction_always_rejected() {
        let set = PatternSet::default();
        assert!(!set.allowed(UniUni, [3, 3, 0, 0]));
        assert!(set.allowed(UniUni, [3, 4, 0, 0]));
    }

    #[test]
    fn pattern_is_species_and_order_invariant() {
        let set = disallow("A + B -> A");
        assert!(!set.allowed(BiUni, [0, 1, 0, 0])); // S0 + S1 -> S0
        assert!(!set.allowed(BiUni, [5, 2, 5, 0])); // relabelled
        assert!(!set.allowed(BiUni, [2, 5, 5, 0])); // reactants swapped: B + A -> A
        assert!(set.allowed(BiUni, [0, 1, 2, 0])); // A + B -> C
        assert!(set.allowed(BiUni, [0, 0, 0, 0])); // A + A -> A is a different pattern
    }

    #[test]
    fn shorthand_and_allow_mode() {
        let set = disallow("2A -> A");
        assert!(!set.allowed(BiUni, [4, 4, 4, 0]));
        let (allow, errors) = PatternSet::parse("mode: allow\nA -> B\nA + B -> C", false, false);
        assert!(errors.is_empty());
        assert!(allow.allowed(UniUni, [0, 1, 0, 0]));
        assert!(allow.allowed(BiUni, [3, 1, 2, 0]));
        assert!(!allow.allowed(UniBi, [0, 1, 2, 0]));
    }

    #[test]
    fn bibi_swaps() {
        let set = disallow("A + B -> C + A");
        assert!(!set.allowed(BiBi, [0, 1, 2, 0]));
        assert!(!set.allowed(BiBi, [1, 0, 0, 2])); // both sides swapped
        assert!(set.allowed(BiBi, [0, 1, 2, 3]));
    }

    #[test]
    fn class_toggles() {
        let s = PatternSet::with_toggles(true, false);
        assert!(!s.allowed(BiUni, [0, 1, 0, 0])); // catalysis
        assert!(!s.allowed(UniBi, [0, 0, 1, 0])); // A -> A + B
        assert!(s.allowed(BiBi, [0, 1, 2, 3]));
        let a = PatternSet::with_toggles(false, true);
        assert!(!a.allowed(UniBi, [0, 0, 0, 0])); // A -> 2A
        assert!(!a.allowed(BiBi, [0, 1, 1, 1])); // A + B -> 2B
        assert!(a.allowed(BiUni, [0, 1, 0, 0])); // catalysis is fine
    }

    #[test]
    fn errors_are_reported_per_line() {
        let (set, errors) = PatternSet::parse("# ok\nA + B\nA + B + C -> A\nAB -> A\nA -> B", false, false);
        assert_eq!(errors.iter().map(|e| e.line).collect::<Vec<_>>(), vec![2, 3, 4]);
        assert!(set.enabled);
        assert!(!set.allowed(UniUni, [0, 1, 0, 0]));
    }

    #[test]
    fn shipped_pascal_pattern_file() {
        let text = "# test\nmode: disallow\nA + B -> A\nA + A -> A\nA -> B + A\nA -> A + A\nA + B -> C + A\nA + B -> C + C\nA + B -> B + B\nA + A -> A + B\nB -> A + A\nA + A -> B\n";
        let set = disallow(text);
        let mut rng = Rng::new(3);
        for kind in [UniUni, BiUni, UniBi, BiBi] {
            if let Some(c) = set.pick(&mut rng, kind, 5) {
                assert!(set.allowed(kind, c));
            }
        }
    }
}

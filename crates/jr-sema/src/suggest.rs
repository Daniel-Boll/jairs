//! "Did you mean …?" — the nearest name to one that does not exist.
//!
//! # Why the suggestion is computed here and not in the editor
//!
//! [ADR-0031](../../../docs/adr/0031-code-actions-and-hints.md) §1. The candidate set is
//! semantic information — "the fields this type has", "the types this file can name" — and
//! only the checker knows it. Computing the guess in `jr-lsp` instead would make `jr check`
//! permanently worse at explaining the same error than an editor is, from two
//! implementations of one guess that drift the first time either learns something.
//!
//! So the suggestion is attached to the diagnostic as a `help:` line, and the code action
//! reads it back off. That also settles a question the cursor cannot answer: a
//! `TypeRef::Name` carries no span (`jr_hir::TypeRef`), so nothing can locate a type
//! annotation from a position — but a diagnostic already points at one.
//!
//! # Why one suggestion rather than a list
//!
//! A `help:` line offering three alternatives is a line the reader has to think about, and
//! the whole value of the line is that it does the thinking. rustc offers one; that
//! practice was arrived at by complaint, which makes it worth copying rather than
//! re-deciding.
//!
//! # Why the threshold is a fraction of the length
//!
//! A fixed distance of 2 makes `x` a suggestion for `y` — every one-character name is
//! within 2 of every other, so a typo'd field on a struct with a field called `w` would
//! confidently suggest `w`. Scaling with length keeps short names strict and lets long
//! ones tolerate a real typo, and the cap keeps a very long name from matching a merely
//! similar one.

/// The nearest candidate to `wanted`, if one is near enough to be worth offering.
///
/// Ties are broken by the order `candidates` yields, which every caller here drives from
/// declaration order — so the suggestion for an ambiguous typo is the one declared first
/// rather than whichever a hash map happened to hold.
///
/// `None` when nothing is close enough, which is the common case and must stay silent: a
/// wrong suggestion is worse than none, because the user acts on it.
pub(crate) fn nearest<'a>(
    wanted: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<&'a str> {
    let limit = threshold(wanted);
    let mut best: Option<(usize, &'a str)> = None;
    for candidate in candidates {
        // An exact match means the caller is reporting an error about a name that does
        // exist, which is a bug in the caller rather than a suggestion opportunity.
        if candidate == wanted {
            return None;
        }
        // A threshold of zero means the name is too short to guess about at all, and the
        // check has to come *after* the exact-match test above rather than as an early
        // return, so that a caller's bug is still caught.
        if limit == 0 {
            continue;
        }
        let distance = edit_distance(wanted, candidate, limit);
        if distance > limit {
            continue;
        }
        if best.is_none_or(|(current, _)| distance < current) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, name)| name)
}

/// How far apart two names may be and still be the same intent.
///
/// **Zero below three characters**, then one, then two. Zero is the interesting case and
/// the reason this is not `len / 3`: at a threshold of 1 every one-character name is within
/// reach of every other, so a typo'd field on a struct with an `x` and a `w` would suggest
/// one of them with total confidence and no information. A name that short carries no
/// evidence of what was meant, so there is nothing honest to offer.
fn threshold(name: &str) -> usize {
    match name.chars().count() {
        0..=2 => 0,
        3..=5 => 1,
        _ => 2,
    }
}

/// Optimal string alignment distance: Levenshtein, plus transposition at cost 1.
///
/// Plain Levenshtein charges **2** for a swapped pair, because it has to delete and
/// reinsert. That rejects `cuont` for `count` at a threshold of 1 — and a transposition is
/// the single most common typing error there is, so a suggester that misses it misses the
/// case it exists for. This variant is the one rustc uses for the same reason.
///
/// `limit` short-circuits on length alone; the full matrix is not worth pruning further,
/// because the candidate lists here are the fields of one struct or the names in one file.
///
/// Two rows are not enough for a transposition — it reads the row *before* the previous
/// one — so this keeps three.
fn edit_distance(a: &str, b: &str, limit: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();

    // A length difference alone already exceeds the limit, so no alignment can help.
    if a.len().abs_diff(b.len()) > limit {
        return limit + 1;
    }

    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current: Vec<usize> = vec![0; b.len() + 1];

    for (i, &ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != cb);
            let delete = previous[j + 1] + 1;
            let insert = current[j] + 1;
            let mut best = substitute.min(delete).min(insert);
            // A transposition: this pair is the previous pair swapped.
            if i > 0 && j > 0 && ca == b[j - 1] && a[i - 1] == cb {
                best = best.min(before[j - 1] + 1);
            }
            current[j + 1] = best;
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }

    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_character_typo_in_a_short_name_is_not_a_suggestion() {
        // The bug a fixed threshold of 2 would have: every one-character name is within
        // 2 of every other, so `x` would confidently suggest `w`.
        assert_eq!(nearest("z", ["x", "y", "w"].into_iter()), None);
    }

    #[test]
    fn a_transposition_is_within_reach() {
        assert_eq!(
            nearest("cuont", ["count", "data"].into_iter()),
            Some("count")
        );
    }

    #[test]
    fn a_missing_character_is_within_reach() {
        assert_eq!(nearest("cout", ["count"].into_iter()), Some("count"));
    }

    #[test]
    fn an_unrelated_name_is_not_suggested() {
        assert_eq!(nearest("height", ["x", "y", "count"].into_iter()), None);
    }

    #[test]
    fn the_nearest_of_several_wins() {
        assert_eq!(
            nearest("widht", ["height", "width", "depth"].into_iter()),
            Some("width")
        );
    }

    #[test]
    fn an_exact_match_is_not_a_suggestion() {
        // The caller is reporting an error about a name that exists, which is its bug.
        assert_eq!(nearest("count", ["count"].into_iter()), None);
    }

    #[test]
    fn a_long_name_tolerates_two_but_not_three() {
        assert_eq!(
            nearest("SHAPES_VERSIN", ["SHAPES_VERSION"].into_iter()),
            Some("SHAPES_VERSION")
        );
        assert_eq!(nearest("SHAPES", ["SHAPES_VERSION"].into_iter()), None);
    }

    #[test]
    fn the_threshold_scales_but_is_capped() {
        // Zero below three: a name that short carries no evidence of intent.
        assert_eq!(threshold("x"), 0);
        assert_eq!(threshold("xy"), 0);
        assert_eq!(threshold("abc"), 1);
        assert_eq!(threshold("abcdef"), 2);
        // Capped: a very long name must not match a merely similar one.
        assert_eq!(threshold("a_very_long_identifier_indeed"), 2);
    }

    #[test]
    fn a_transposition_costs_one_not_two() {
        // Plain Levenshtein charges 2 here, which would reject the single most common
        // typing error there is at a threshold of 1.
        assert_eq!(edit_distance("cuont", "count", 2), 1);
        assert_eq!(edit_distance("ab", "ba", 2), 1);
    }
}

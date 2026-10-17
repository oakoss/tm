//! "Did you mean" candidates by Jaro similarity, the measure clap uses.

/// Up to three candidates more similar than 0.7, best first.
pub(super) fn did_you_mean<'a>(
    input: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let input = input.to_lowercase();
    let mut scored: Vec<(f64, &str)> = candidates
        .into_iter()
        .map(|candidate| (jaro(&input, &candidate.to_lowercase()), candidate))
        .filter(|(score, _)| *score > 0.7)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    scored
        .into_iter()
        .take(3)
        .map(|(_, candidate)| candidate.to_owned())
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn jaro(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let mut used_in_b = vec![false; b.len()];
    let mut a_matches = Vec::new();
    for (i, ca) in a.iter().enumerate() {
        let lo = i.saturating_sub(window);
        let hi = (i + window + 1).min(b.len());
        for j in lo..hi {
            if !used_in_b[j] && b[j] == *ca {
                used_in_b[j] = true;
                a_matches.push(*ca);
                break;
            }
        }
    }
    if a_matches.is_empty() {
        return 0.0;
    }
    let b_matches = b
        .iter()
        .zip(&used_in_b)
        .filter(|(_, matched)| **matched)
        .map(|(c, _)| *c);
    let transpositions = a_matches
        .iter()
        .zip(b_matches)
        .filter(|(x, y)| **x != *y)
        .count();
    let m = a_matches.len() as f64;
    let t = transpositions as f64 / 2.0;
    (m / a.len() as f64 + m / b.len() as f64 + (m - t) / m) / 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jaro_matches_known_values() {
        assert!((jaro("martha", "marhta") - 0.944).abs() < 0.001);
        assert!((jaro("dixon", "dicksonx") - 0.767).abs() < 0.001);
        assert!(jaro("abc", "xyz").abs() < f64::EPSILON);
    }

    #[test]
    fn suggestions_are_close_best_first_and_capped() {
        let fields = ["status", "statuses", "state", "title", "type"];
        assert_eq!(
            did_you_mean("stauts", fields),
            ["status", "statuses", "state"]
        );
        assert_eq!(did_you_mean("zzz", fields), Vec::<String>::new());
    }
}

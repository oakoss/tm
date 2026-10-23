//! Ranks: the ordering keys that place an Issue on its Short-list (ADR 0016).
//!
//! A rank is a base-62 fraction in (0, 1) written as its digits, so ranks
//! compare as plain strings and a new one fits between any two others. No rank
//! ends in `0`, which keeps each fraction's spelling unique.

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: u8 = 62;

/// The longest rank. Respacing keeps ranks far shorter; the cap bounds the
/// recursion a rank read from the log can drive.
pub const MAX_LEN: usize = 128;

fn digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'Z' => Some(c - b'A' + 10),
        b'a'..=b'z' => Some(c - b'a' + 36),
        _ => None,
    }
}

/// Whether `s` is a rank: 1 to [`MAX_LEN`] base-62 digits, not ending in `0`.
#[must_use]
pub fn is_rank(s: &str) -> bool {
    (1..=MAX_LEN).contains(&s.len()) && !s.ends_with('0') && s.bytes().all(|c| digit(c).is_some())
}

/// A rank strictly between `lo` and `hi`, where none stands for the start or
/// the end of the list. None when either is not a rank, `lo` is not below
/// `hi`, or the rank would pass [`MAX_LEN`]: tied or overlong ranks have no
/// gap, and their members are respaced instead.
#[must_use]
pub fn between(lo: Option<&str>, hi: Option<&str>) -> Option<String> {
    if !ordered(lo, hi) {
        return None;
    }
    let rank = match (lo, hi) {
        // Adds at either end step by one digit rather than halving the gap,
        // so a list that only grows at one end gains a digit per ~30 adds.
        (Some(lo), None) => step(lo.as_bytes(), |d| (d + 1 < BASE).then_some(d + 1)),
        (None, Some(hi)) => step(hi.as_bytes(), |d| (d >= 2).then(|| d - 1)),
        _ => None,
    }
    .unwrap_or_else(|| midpoint(lo.unwrap_or("").as_bytes(), hi.map(str::as_bytes)));
    (rank.len() <= MAX_LEN).then(|| rank.into_iter().map(char::from).collect())
}

fn ordered(lo: Option<&str>, hi: Option<&str>) -> bool {
    lo.is_none_or(is_rank) && hi.is_none_or(is_rank) && lo.zip(hi).is_none_or(|(lo, hi)| lo < hi)
}

/// `r` cut after its first digit that `next` can move, with that digit
/// moved: the shortest rank one step past `r` in `next`'s direction.
fn step(r: &[u8], next: impl Fn(u8) -> Option<u8>) -> Option<Vec<u8>> {
    let (i, d) = r
        .iter()
        .enumerate()
        .find_map(|(i, &c)| next(digit(c).expect("checked by between")).map(|d| (i, d)))?;
    let mut out = r[..i].to_vec();
    out.push(DIGITS[usize::from(d)]);
    Some(out)
}

/// `n` ascending ranks strictly between `lo` and `hi`, placed by bisection so
/// they stay short, for respacing a run of tied or overlong ranks. None as
/// for [`between`], or when bisecting would pass [`MAX_LEN`], which can
/// happen before the gap is full; the caller then respaces wider bounds.
#[must_use]
pub fn spread(lo: Option<&str>, hi: Option<&str>, n: usize) -> Option<Vec<String>> {
    if !ordered(lo, hi) {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    fill(lo, hi, n, &mut out)?;
    Some(out)
}

fn fill(lo: Option<&str>, hi: Option<&str>, n: usize, out: &mut Vec<String>) -> Option<()> {
    if n == 0 {
        return Some(());
    }
    let mid = between(lo, hi)?;
    let left = n / 2;
    fill(lo, Some(&mid), left, out)?;
    out.push(mid.clone());
    fill(Some(&mid), hi, n - 1 - left, out)
}

/// The fraction halfway between `a` and `b` (none for 1), digit by digit;
/// `a` is below `b` and neither ends in `0`.
fn midpoint(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    let at = |s: &[u8], i: usize| s.get(i).copied().unwrap_or(b'0');
    if let Some(b) = b {
        // `a` is read as padded with zeros, so a shared prefix may run past it.
        let shared = b
            .iter()
            .enumerate()
            .take_while(|&(i, &c)| at(a, i) == c)
            .count();
        if shared > 0 {
            let mut out = b[..shared].to_vec();
            out.extend(midpoint(a.get(shared..).unwrap_or(&[]), Some(&b[shared..])));
            return out;
        }
    }
    let da = a
        .first()
        .map_or(0, |&c| digit(c).expect("checked by between"));
    let db = b.map_or(BASE, |b| digit(b[0]).expect("checked by between"));
    if db - da > 1 {
        return vec![DIGITS[usize::from(u8::midpoint(da, db))]];
    }
    match b {
        // `b`'s first digit alone sorts above `a` and, `b` going on, below `b`.
        Some(b) if b.len() > 1 => vec![b[0]],
        _ => {
            let mut out = vec![DIGITS[usize::from(da)]];
            out.extend(midpoint(a.get(1..).unwrap_or(&[]), None));
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn rank() -> impl Strategy<Value = String> {
        "[0-9A-Za-z]{0,8}[1-9A-Za-z]"
    }

    #[test]
    fn an_empty_list_gets_a_short_rank() {
        assert_eq!(between(None, None).as_deref(), Some("V"));
        assert_eq!(between(Some("V"), None).as_deref(), Some("W"));
        assert_eq!(between(None, Some("V")).as_deref(), Some("U"));
    }

    #[test]
    fn ends_step_at_the_first_digit_that_can_move() {
        assert_eq!(between(Some("zV"), None).as_deref(), Some("zW"));
        assert_eq!(between(Some("Vz"), None).as_deref(), Some("W"));
        assert_eq!(between(None, Some("1V")).as_deref(), Some("1U"));
        assert_eq!(between(None, Some("V1")).as_deref(), Some("U"));
    }

    #[test]
    fn neighbouring_digits_go_one_digit_deeper() {
        assert_eq!(between(Some("1"), Some("2")).as_deref(), Some("1V"));
        assert_eq!(between(Some("z"), None).as_deref(), Some("zV"));
        assert_eq!(between(None, Some("1")).as_deref(), Some("0V"));
    }

    #[test]
    fn tied_reversed_or_malformed_bounds_have_no_rank_between() {
        assert_eq!(between(Some("V"), Some("V")), None);
        assert_eq!(between(Some("k"), Some("V")), None);
        assert_eq!(between(Some("V0"), None), None);
        assert_eq!(between(Some(""), None), None);
        assert_eq!(between(None, Some("a-b")), None);
        assert_eq!(spread(Some("V"), Some("V"), 0), None);
    }

    #[test]
    fn rank_shape() {
        assert!(is_rank("V"));
        assert!(is_rank("0V"));
        assert!(!is_rank(""));
        assert!(!is_rank("V0"));
        assert!(!is_rank("é"));
        assert!(is_rank(&"z".repeat(MAX_LEN)));
        assert!(!is_rank(&"z".repeat(MAX_LEN + 1)));
    }

    #[test]
    fn a_rank_past_the_cap_is_refused_for_respacing() {
        let longest = "z".repeat(MAX_LEN);
        assert_eq!(between(Some(&longest), None), None);
        assert_eq!(between(Some(&"z".repeat(100_000)), None), None);
    }

    #[test]
    fn spread_fills_a_gap_near_the_cap() {
        let near = format!("{}y", "z".repeat(MAX_LEN - 1));
        assert_eq!(spread(Some(&near), None, 1).map(|r| r.len()), Some(1));
        assert_eq!(spread(Some(&near), None, 0), Some(Vec::new()));
        assert_eq!(spread(Some(&"z".repeat(MAX_LEN)), None, 1), None);
    }

    proptest! {
        #[test]
        fn between_sorts_strictly_inside(a in rank(), b in rank()) {
            prop_assume!(a != b);
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            let mid = between(Some(&lo), Some(&hi)).unwrap();
            prop_assert!(is_rank(&mid));
            prop_assert!(lo < mid && mid < hi, "{lo} < {mid} < {hi}");
        }

        #[test]
        fn open_ends_sort_inside(r in rank()) {
            let below = between(None, Some(&r)).unwrap();
            let above = between(Some(&r), None).unwrap();
            prop_assert!(is_rank(&below) && is_rank(&above));
            prop_assert!(below < r && r < above);
        }

        #[test]
        fn spread_is_ascending_and_inside(
            a in rank(), b in rank(), n in 0usize..200,
        ) {
            prop_assume!(a != b);
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            let ranks = spread(Some(&lo), Some(&hi), n).unwrap();
            prop_assert_eq!(ranks.len(), n);
            prop_assert!(ranks.iter().all(|r| is_rank(r)));
            prop_assert!(ranks.windows(2).all(|w| w[0] < w[1]));
            prop_assert!(ranks.first().is_none_or(|r| lo < *r));
            prop_assert!(ranks.last().is_none_or(|r| *r < hi));
        }

        /// Respacing exists to keep ranks short: a list's worth spread over
        /// the whole range stays a few digits long.
        #[test]
        fn spread_ranks_stay_short(n in 0usize..1000) {
            let ranks = spread(None, None, n).unwrap();
            prop_assert!(ranks.iter().all(|r| r.len() <= 3), "{ranks:?}");
        }

        /// Adds at one end, as adds to a list's end or top do, grow a rank by
        /// about one digit per 30 adds.
        #[test]
        fn adds_at_an_end_grow_slowly(n in 1usize..500, top in any::<bool>()) {
            let mut last = between(None, None).unwrap();
            for _ in 1..n {
                last = if top {
                    between(None, Some(&last))
                } else {
                    between(Some(&last), None)
                }
                .unwrap();
            }
            prop_assert!(last.len() <= 1 + n / 30, "{} after {n}", last.len());
        }
    }
}

//! Durations as people write them: a count and one unit, such as `30m`.

/// The longest duration [`parse_ms`] reads, 100 years, so time arithmetic
/// on one never comes near overflow.
pub const MAX_MS: u64 = 100 * 365 * 86_400_000;

/// `text` in milliseconds, for a count followed by `s`, `m`, `h` or `d`;
/// `None` when it is not one or is over [`MAX_MS`].
#[must_use]
pub fn parse_ms(text: &str) -> Option<u64> {
    let unit_ms = match text.bytes().last()? {
        b's' => 1_000,
        b'm' => 60_000,
        b'h' => 3_600_000,
        b'd' => 86_400_000,
        _ => return None,
    };
    // The unit is one ASCII byte, so cutting it off stays on a char boundary.
    let count = &text[..text.len() - 1];
    if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    count
        .parse::<u64>()
        .ok()?
        .checked_mul(unit_ms)
        .filter(|ms| *ms <= MAX_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_a_count_and_one_unit() {
        assert_eq!(parse_ms("90s"), Some(90_000));
        assert_eq!(parse_ms("2h"), Some(7_200_000));
        assert_eq!(parse_ms("1d"), Some(86_400_000));
        assert_eq!(parse_ms("0m"), Some(0));
        for bad in ["", "h", "2", "2x", "-2h", "2 h", "2.5h", "é"] {
            assert_eq!(parse_ms(bad), None, "{bad:?}");
        }
        assert_eq!(parse_ms(&format!("{}d", u64::MAX)), None, "overflow");
        assert_eq!(parse_ms("36500d"), Some(MAX_MS));
        assert_eq!(parse_ms("36501d"), None, "over 100 years");
    }
}

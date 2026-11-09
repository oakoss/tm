//! Where a cursor lands after a screen reads its Issues again.

/// The row to select among `keys` after a read: `kept`'s row while it is
/// still there, else `was` held to the last row; none when there are no
/// rows.
pub(crate) fn reselect<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    kept: Option<&str>,
    was: usize,
) -> Option<usize> {
    let keys: Vec<&str> = keys.into_iter().collect();
    let found = kept.and_then(|kept| keys.iter().position(|key| *key == kept));
    held(keys.len(), found.unwrap_or(was))
}

/// `row` held within `len` rows, or none when there are none.
pub(crate) fn held(len: usize, row: usize) -> Option<usize> {
    len.checked_sub(1).map(|last| row.min(last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kept_issue_wins_wherever_it_moved() {
        assert_eq!(reselect(["a", "b", "c"], Some("c"), 0), Some(2));
    }

    #[test]
    fn a_gone_issue_leaves_the_row_held_to_the_last() {
        assert_eq!(reselect(["a", "b"], Some("z"), 1), Some(1));
        assert_eq!(reselect(["a", "b"], Some("z"), 5), Some(1));
        assert_eq!(reselect(["a", "b"], None, 5), Some(1));
    }

    #[test]
    fn no_rows_select_nothing() {
        assert_eq!(reselect([], Some("a"), 3), None);
        assert_eq!(held(0, 0), None);
        assert_eq!(held(3, 0), Some(0));
    }
}

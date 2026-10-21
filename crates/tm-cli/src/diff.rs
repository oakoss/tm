//! The diff a person reads before tm writes a file it does not own alone.

use std::fmt::Write as _;

/// `before` turned into `after` as a unified diff of one hunk without
/// context, which is all tm's writes need; `None` is a file tm creates.
pub(crate) fn unified(file: &str, before: Option<&str>, after: &str) -> String {
    let old = before.map_or_else(|| "/dev/null".to_owned(), |_| format!("a/{file}"));
    let (before, after): (Vec<&str>, Vec<&str>) = (
        before.unwrap_or_default().lines().collect(),
        after.lines().collect(),
    );
    let same_start = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let same_end = before[same_start..]
        .iter()
        .rev()
        .zip(after[same_start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let removed = &before[same_start..before.len() - same_end];
    let added = &after[same_start..after.len() - same_end];
    // As `git diff -U0` writes them: an empty side names the line it follows,
    // and a count of one is left out.
    let range = |count: usize| match count {
        0 => format!("{same_start},0"),
        1 => format!("{}", same_start + 1),
        _ => format!("{},{count}", same_start + 1),
    };
    let mut out = format!(
        "--- {old}\n+++ b/{file}\n@@ -{} +{} @@\n",
        range(removed.len()),
        range(added.len())
    );
    for line in removed {
        let _ = writeln!(out, "-{line}");
    }
    for line in added {
        let _ = writeln!(out, "+{line}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::unified;

    #[test]
    fn one_hunk_names_where_it_goes() {
        assert_eq!(
            unified("f", None, "a\nb\n"),
            "--- /dev/null\n+++ b/f\n@@ -0,0 +1,2 @@\n+a\n+b\n"
        );
        assert_eq!(
            unified("f", Some("a\nold\nz\n"), "a\nnew\nz\n"),
            "--- a/f\n+++ b/f\n@@ -2 +2 @@\n-old\n+new\n"
        );
        assert_eq!(
            unified("f", Some("a\n"), "a\n\nb\n"),
            "--- a/f\n+++ b/f\n@@ -1,0 +2,2 @@\n+\n+b\n"
        );
        assert_eq!(
            unified("f", Some(""), "b\n"),
            "--- a/f\n+++ b/f\n@@ -0,0 +1 @@\n+b\n"
        );
    }
}

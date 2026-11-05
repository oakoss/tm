//! The paths a Claim covers: globs relative to the repository root, matched
//! against `/`-separated file paths.

/// The most paths one Claim may name.
pub const MAX_PATHS: usize = 32;

/// The longest path a Claim may name, in characters.
pub const MAX_PATH: usize = 256;

/// `path` in the one spelling a Claim keeps: trimmed, without a leading `./`
/// or trailing `/`.
///
/// # Errors
///
/// Why `path` cannot name files in the repository: blank, absolute, leaving
/// it with `..`, holding a backslash or control character, or too long.
pub fn normalize(path: &str) -> Result<String, &'static str> {
    let trimmed = path.trim();
    let path = trimmed.trim_start_matches("./").trim_end_matches('/');
    if path.is_empty() {
        Err("is blank")
    } else if trimmed.starts_with('/') {
        Err("is absolute; name it from the repository root")
    } else if path.split('/').any(|part| part == "..") {
        Err("leaves the repository with ..")
    } else if path.split('/').any(|part| part.is_empty() || part == ".") {
        Err("has an empty or . part")
    } else if path.contains('\\') || path.chars().any(char::is_control) {
        Err("holds a backslash or control character")
    } else if path.chars().count() > MAX_PATH {
        Err("is longer than 256 characters")
    } else {
        Ok(path.to_owned())
    }
}

/// Whether the file `path` falls under `pattern`: `*` matches within one
/// part of a path, `?` one character, and `**` any number of parts; a
/// pattern without them names that file or every file under that directory.
#[must_use]
pub fn covers(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    let part = |pattern: &&str, part: &&str| {
        let pattern: Vec<char> = pattern.chars().collect();
        let part: Vec<char> = part.chars().collect();
        wild(&pattern, &part, |c| *c == '*', |p, c| *p == '?' || p == c)
    };
    wild(&pattern, &path, |part| *part == "**", part)
        || (!pattern.iter().any(|part| part.contains(['*', '?'])) && path.starts_with(&pattern))
}

/// Whether `text` matches `pattern`, where a `star` element matches any run
/// of elements and any other matches one element `one` accepts. It returns
/// to only the latest star, so it takes at most `pattern` × `text` steps
/// where trying every split could take exponentially many.
fn wild<P, T>(
    pattern: &[P],
    text: &[T],
    star: impl Fn(&P) -> bool,
    one: impl Fn(&P, &T) -> bool,
) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut back: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && star(&pattern[p]) {
            p += 1;
            back = Some((p, t));
        } else if p < pattern.len() && one(&pattern[p], &text[t]) {
            p += 1;
            t += 1;
        } else if let Some((after, from)) = back {
            p = after;
            t = from + 1;
            back = Some((after, t));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(star)
}

/// Whether `paths` is in the one order a Claim keeps them: sorted, with
/// none twice, so two Claims covering the same files compare equal.
#[must_use]
pub fn is_kept_order(paths: &[String]) -> bool {
    paths.windows(2).all(|pair| pair[0] < pair[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_covers_what_it_names() {
        let cases = [
            ("crates/tm-store/**", "crates/tm-store/src/lib.rs", true),
            ("crates/tm-store/**", "crates/tm-store", true),
            ("crates/tm-store/**", "crates/tm-cli/src/main.rs", false),
            ("crates/tm-store", "crates/tm-store/src/lib.rs", true),
            ("crates/tm-store", "crates/tm-store-extra/x.rs", false),
            ("crates/*/src/main.rs", "crates/tm-cli/src/main.rs", true),
            ("crates/*/src/main.rs", "crates/a/b/src/main.rs", false),
            ("**/*.md", "README.md", true),
            ("**/*.md", "docs/specs/design.md", true),
            ("**/*.md", "docs/specs/design.rs", false),
            ("docs/*.md", "docs/specs/design.md", false),
            ("src/lib.r?", "src/lib.rs", true),
            ("src/lib.rs", "src/lib.rs", true),
            ("src/lib.rs", "src/lib.rs.bak", false),
            ("src/*", "src", false),
            ("src/*.rs", "src/.rs", true),
            ("a/**/b/**/c", "a/x/b/y/z/c", true),
            ("a/**/b/**/c", "a/x/c/b", false),
            ("*a*b", "xaxxb", true),
            ("*a*b", "xbxa", false),
        ];
        for (pattern, path, want) in cases {
            assert_eq!(covers(pattern, path), want, "{pattern} over {path}");
        }
    }

    #[test]
    fn a_pattern_built_to_backtrack_is_matched_at_once() {
        let started = std::time::Instant::now();
        let stars = format!("{}b", "*a".repeat(64));
        assert!(!covers(&stars, &"a".repeat(MAX_PATH)));
        let parts = format!("{}/x", "**/a".repeat(30));
        assert!(!covers(&parts, &vec!["a"; 120].join("/")));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn paths_are_kept_sorted_with_none_twice() {
        let kept = |paths: &[&str]| {
            is_kept_order(&paths.iter().map(|&p| p.to_owned()).collect::<Vec<_>>())
        };
        assert!(kept(&[]));
        assert!(kept(&["docs", "src"]));
        assert!(!kept(&["src", "docs"]));
        assert!(!kept(&["src", "src"]));
    }

    #[test]
    fn a_path_is_kept_from_the_repository_root() {
        assert_eq!(
            normalize(" ./crates/tm-store/ "),
            Ok("crates/tm-store".into())
        );
        for bad in [
            "", "  ", "/etc", "../x", "a/../b", "a//b", "a\\b", "a\nb", ".", "a/./b",
        ] {
            assert!(normalize(bad).is_err(), "{bad:?}");
        }
        assert!(normalize(&"a".repeat(257)).is_err());
        assert!(normalize(&"a".repeat(256)).is_ok());
    }
}

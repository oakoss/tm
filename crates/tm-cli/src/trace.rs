//! Tracing, off unless `TM_LOG` asks, and then appended to a file under tm's
//! home, never to stdout or stderr (ADR 0020).

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::Directive;
use tracing_subscriber::fmt::format::FmtSpan;

/// Past this size the log is set aside as `tm.log.1` when tm starts.
const ROLL_BYTES: u64 = 10 * 1024 * 1024;

/// The target of the span every command runs in, which every filter
/// enables, so each line names its command and process.
pub(crate) const COMMAND_TARGET: &str = "tm_command";

/// Starts tracing at `filter`, a `RUST_LOG`-style filter, into `logs/tm.log`
/// under the tm home `home` finds. Nothing starts when `filter` is unset,
/// blank or only turns tracing off. Returns warnings: the parts of `filter`
/// tm cannot read, which are left out, and why the log cannot be written or
/// set aside.
pub(crate) fn start(
    filter: Option<&str>,
    home: impl FnOnce() -> Result<PathBuf, String>,
) -> Vec<String> {
    let Some(filter) = filter.map(str::trim).filter(|filter| !filter.is_empty()) else {
        return Vec::new();
    };
    let (read, unread) = directives(filter);
    let mut warnings: Vec<String> = unread
        .iter()
        .map(|part| format!("TM_LOG's {part:?} is not a filter tm can read, so it is left out"))
        .collect();
    if read.is_empty() {
        warnings.push("TM_LOG names no filter tm can read, so nothing is traced".into());
        return warnings;
    }
    if read
        .iter()
        .all(|(_, directive)| off(&directive.to_string()))
    {
        return warnings;
    }
    let read = read.into_iter().map(|(_, directive)| directive);
    let path = match home() {
        Ok(home) => home.join("logs").join("tm.log"),
        Err(err) => {
            warnings.push(format!(
                "TM_LOG is set, but tm has no home to log in: {err}"
            ));
            return warnings;
        }
    };
    let file = match open(&path) {
        Ok((file, unrolled)) => {
            warnings.extend(unrolled.map(|err| {
                format!(
                    "{} cannot be set aside as tm.log.1, so it keeps growing: {err}",
                    path.display()
                )
            }));
            file
        }
        Err(err) => {
            warnings.push(format!(
                "TM_LOG is set, but {} cannot be written: {err}",
                path.display()
            ));
            return warnings;
        }
    };
    let filter = read
        .chain(format!("{COMMAND_TARGET}=info").parse())
        .fold(EnvFilter::default(), EnvFilter::add_directive);
    let started = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .log_internal_errors(false)
        .with_span_events(FmtSpan::CLOSE)
        .try_init();
    warnings.extend(
        started
            .err()
            .map(|err| format!("TM_LOG is set, but tracing cannot start: {err}")),
    );
    warnings
}

/// The comma-separated parts of `filter` that read as directives, and those
/// that do not, as `EnvFilter` splits them.
fn directives(filter: &str) -> (Vec<(&str, Directive)>, Vec<&str>) {
    let mut read = Vec::new();
    let mut unread = Vec::new();
    for part in filter
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part.parse() {
            Ok(directive) => read.push((part, directive)),
            Err(_) => unread.push(part),
        }
    }
    (read, unread)
}

/// Whether a directive, as it prints, turns its target off: `off`, or ends
/// `=off`; it prints `0` as `off`.
fn off(part: &str) -> bool {
    let level = part.rsplit_once('=').map_or(part, |(_, level)| level);
    level.trim().eq_ignore_ascii_case("off")
}

/// `path` opened to append, after setting it aside past [`ROLL_BYTES`], with
/// why it could not be set aside.
fn open(path: &Path) -> std::io::Result<(File, Option<std::io::Error>)> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let unrolled = fs::metadata(path)
        .is_ok_and(|meta| meta.len() > ROLL_BYTES)
        .then(|| fs::rename(path, path.with_extension("log.1")).err())
        .flatten();
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    Ok((file, unrolled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_past_its_size_is_set_aside_and_a_new_one_begun() {
        let dir = std::env::temp_dir().join(format!("tm-trace-{}", uuid::Uuid::now_v7()));
        let path = dir.join("logs").join("tm.log");
        drop(open(&path).unwrap());
        fs::write(&path, vec![b'x'; usize::try_from(ROLL_BYTES).unwrap() + 1]).unwrap();
        let (_, unrolled) = open(&path).unwrap();
        assert!(unrolled.is_none());
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
        assert_eq!(
            fs::metadata(dir.join("logs").join("tm.log.1"))
                .unwrap()
                .len(),
            ROLL_BYTES + 1
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nothing_starts_without_a_filter() {
        for filter in [
            None,
            Some(""),
            Some("  "),
            Some("off"),
            Some("OFF,tm_store=off"),
            Some("0,tm_store[write]=0"),
        ] {
            assert_eq!(
                start(filter, || Err("unreached".into())),
                Vec::<String>::new()
            );
        }
    }

    #[test]
    fn unreadable_parts_are_named_and_left_out() {
        let (read, unread) = directives("debug, tm_store=nonsense,===,tm::init=trace");
        assert_eq!(
            read.iter().map(|(part, _)| *part).collect::<Vec<_>>(),
            ["debug", "tm::init=trace"]
        );
        assert!(
            read.iter()
                .all(|(_, directive)| !off(&directive.to_string()))
        );
        assert_eq!(unread, ["tm_store=nonsense", "==="]);
        let warnings = start(Some("==="), || Err("unreached".into()));
        assert_eq!(
            warnings,
            [
                "TM_LOG's \"===\" is not a filter tm can read, so it is left out",
                "TM_LOG names no filter tm can read, so nothing is traced",
            ]
        );
    }
}

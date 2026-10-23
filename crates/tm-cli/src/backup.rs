//! The day's backup of a Workspace, and the collection of old ones (ADR 0015).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use jiff::ToSpan as _;
use jiff::civil::Date;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::CliError;
use crate::export;
use crate::here::{self, Here};
use crate::output::Output;

/// `[backups]` in `config.toml` in tm's home.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Policy {
    /// Whether a write takes the day's backup.
    enabled: bool,
    keep_days: u32,
    keep_weeks: u32,
    keep_months: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_days: 14,
            keep_weeks: 8,
            keep_months: 12,
        }
    }
}

impl Policy {
    /// What applies when the config cannot be read: back up, remove nothing.
    fn keep_all() -> Self {
        Self {
            enabled: true,
            keep_days: u32::MAX,
            keep_weeks: u32::MAX,
            keep_months: u32::MAX,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    backups: Policy,
}

/// The policy `home`'s `config.toml` sets, with the defaults where it sets
/// none; one that keeps every backup, with a warning, when it cannot be read.
fn policy(home: &Path) -> (Policy, Option<String>) {
    let path = home.join("config.toml");
    let read = match fs::read_to_string(&path) {
        Ok(text) => toml::from_str::<Config>(&text)
            .map(|config| config.backups)
            .map_err(|err| err.message().to_owned()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Policy::default()),
        Err(err) => Err(err.to_string()),
    };
    match read {
        Ok(policy) => (policy, None),
        Err(cause) => (
            Policy::keep_all(),
            Some(format!(
                "{}: {cause}; no old backup was removed",
                path.display()
            )),
        ),
    }
}

fn dir(home: &Path, workspace: Uuid) -> PathBuf {
    home.join("exports").join(workspace.to_string())
}

/// The backup for `date` in an exports directory.
fn file(dir: &Path, date: Date) -> PathBuf {
    dir.join(format!("{date}.jsonl"))
}

/// The name every staging directory starts with.
const PREFIX: &str = ".tm-import-";

/// How long a write waits for another process's export: past the 0.7 s an
/// export takes at 10,000 Issues, short of a hang.
const WAIT: Duration = Duration::from_secs(5);

/// A Workspace's exports directory, locked while this is held, so one export
/// or collection runs in it at a time.
struct Exports {
    dir: PathBuf,
    _lock: fs::File,
}

impl Exports {
    fn lock(home: &Path, workspace: Uuid) -> Result<Self, CliError> {
        let (dir, path, lock) = Self::open(home, workspace)?;
        lock.lock().map_err(|err| CliError::io(&path, &err))?;
        Ok(Self { dir, _lock: lock })
    }

    /// The lock, waiting at most `wait`; none if another process still holds
    /// it then.
    fn try_lock(home: &Path, workspace: Uuid, wait: Duration) -> Result<Option<Self>, CliError> {
        let (dir, path, lock) = Self::open(home, workspace)?;
        let started = Instant::now();
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok(Some(Self { dir, _lock: lock })),
                Err(fs::TryLockError::WouldBlock) if started.elapsed() < wait => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(fs::TryLockError::WouldBlock) => return Ok(None),
                Err(fs::TryLockError::Error(err)) => return Err(CliError::io(&path, &err)),
            }
        }
    }

    fn open(home: &Path, workspace: Uuid) -> Result<(PathBuf, PathBuf, fs::File), CliError> {
        let dir = dir(home, workspace);
        fs::create_dir_all(&dir).map_err(|err| CliError::io(&dir, &err))?;
        let path = dir.join(".lock");
        let lock = fs::File::create(&path).map_err(|err| CliError::io(&path, &err))?;
        Ok((dir, path, lock))
    }

    fn file(&self, date: Date) -> PathBuf {
        file(&self.dir, date)
    }

    /// Removes staging directories an export left when it was killed, since
    /// any still writing holds the lock, and the backups `policy` does not
    /// keep; a warning for each that could not be removed. Nothing is removed
    /// through a symbolic link, which may lead out of tm's home.
    fn collect(&self, today: Date, policy: &Policy) -> Vec<String> {
        if let Some(dir) = linked(&self.dir) {
            return vec![format!(
                "old backups were not removed: {} is a symbolic link, and tm removes nothing through one",
                dir.display()
            )];
        }
        let failed = |path: &Path, err: &io::Error| {
            format!("old backups were not removed: {}: {err}", path.display())
        };
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) => return vec![failed(&self.dir, &err)],
        };
        let mut warnings = Vec::new();
        let mut backups = Vec::new();
        for entry in entries {
            let entry = match entry.and_then(|entry| Ok((entry.file_type()?, entry))) {
                Ok(entry) => entry,
                Err(err) => {
                    warnings.push(failed(&self.dir, &err));
                    continue;
                }
            };
            let (kind, entry) = entry;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if kind.is_dir() && name.starts_with(PREFIX) {
                if let Err(err) = fs::remove_dir_all(entry.path()) {
                    warnings.push(format!(
                        "old staging was not removed: {}: {err}",
                        entry.path().display()
                    ));
                }
            } else if let Some(date) = kind.is_file().then(|| date_of(name)).flatten() {
                backups.push((date, entry.path()));
            }
        }
        let dates: Vec<Date> = backups.iter().map(|(date, _)| *date).collect();
        let kept = keep(&dates, today, policy);
        for (date, path) in backups {
            if !kept.contains(&date)
                && let Err(err) = fs::remove_file(&path)
            {
                warnings.push(failed(&path, &err));
            }
        }
        warnings
    }
}

/// The exports directory, or `dir` in it, when either is a symbolic link.
fn linked(dir: &Path) -> Option<&Path> {
    let parent = dir.parent().unwrap_or(dir);
    [parent, dir]
        .into_iter()
        .find(|dir| fs::symlink_metadata(dir).is_ok_and(|meta| meta.file_type().is_symlink()))
}

/// The date of a backup by its file name, for the names tm writes only.
fn date_of(name: &str) -> Option<Date> {
    let stem = name.strip_suffix(".jsonl")?;
    let date = Date::strptime("%Y-%m-%d", stem).ok()?;
    (date.to_string() == stem).then_some(date)
}

/// The dates `policy` keeps of `dates`: each in the last `keep_days` days,
/// the newest of each of the last `keep_weeks` ISO weeks and of each of the
/// last `keep_months` months, the newest up to `today`, and each after it,
/// from a clock that ran ahead.
fn keep(dates: &[Date], today: Date, policy: &Policy) -> BTreeSet<Date> {
    let limit = |count: u32| i32::try_from(count).unwrap_or(i32::MAX);
    let (keep_days, keep_weeks, keep_months) = (
        limit(policy.keep_days),
        limit(policy.keep_weeks),
        limit(policy.keep_months),
    );
    let monday = |date: Date| date - i64::from(date.weekday().to_monday_zero_offset()).days();
    let month = |date: Date| i32::from(date.year()) * 12 + i32::from(date.month());
    let (past, future): (Vec<Date>, Vec<Date>) = dates.iter().partition(|&&date| date <= today);
    let mut kept: BTreeSet<Date> = future.into_iter().collect();
    let mut weeks = BTreeMap::new();
    let mut months = BTreeMap::new();
    for &date in &past {
        if (today - date).get_days() < keep_days {
            kept.insert(date);
        }
        let week = monday(date);
        if (monday(today) - week).get_days() / 7 < keep_weeks {
            let newest = weeks.entry(week).or_insert(date);
            *newest = (*newest).max(date);
        }
        if month(today) - month(date) < keep_months {
            let newest = months.entry(month(date)).or_insert(date);
            *newest = (*newest).max(date);
        }
    }
    kept.extend(weeks.into_values());
    kept.extend(months.into_values());
    kept.extend(past.iter().max());
    kept
}

/// Takes `today`'s backup of `here`'s Workspace when it has none, then
/// collects old ones; the warnings of what failed. Run after a write that
/// added to the log, so the log always differs from the last backup's.
pub(crate) fn daily(here: &Here, today: Date) -> Vec<String> {
    if held(&file(&dir(&here.home, here.workspace.id), today)) == Held::Backup {
        return Vec::new();
    }
    let (policy, unread) = policy(&here.home);
    if !policy.enabled {
        return clear_staging(here, today);
    }
    let mut warnings = match back_up(here, today) {
        Ok(Some(exports)) => exports.collect(today, &policy),
        Ok(None) => Vec::new(),
        Err(err) => vec![format!("today's backup was not written: {}", err.message)],
    };
    warnings.extend(unread);
    warnings
}

/// With backups off and no backup today, removes the staging directories a
/// killed export left, taking the lock only when there are some and no
/// export holds it, since one that does collects them itself.
fn clear_staging(here: &Here, today: Date) -> Vec<String> {
    let dir = dir(&here.home, here.workspace.id);
    let not_removed = |cause: &str| vec![format!("old staging was not removed: {cause}")];
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => return not_removed(&format!("{}: {err}", dir.display())),
    };
    // An entry that cannot be read may be one; collect reports it.
    let staging = |entry: io::Result<fs::DirEntry>| match entry {
        Ok(entry) => {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.file_name().to_string_lossy().starts_with(PREFIX)
        }
        Err(_) => true,
    };
    if linked(&dir).is_some() || !entries.into_iter().any(staging) {
        return Vec::new();
    }
    match Exports::try_lock(&here.home, here.workspace.id, Duration::ZERO) {
        Ok(Some(exports)) => exports.collect(today, &Policy::keep_all()),
        Ok(None) => Vec::new(),
        Err(err) => not_removed(&err.message),
    }
}

/// Writes the export to tm's home as `exports/<Workspace>/<local date>.jsonl`,
/// replacing that day's, unless it matches the newest one there, then
/// collects old ones.
pub(crate) fn dated() -> Result<Output, CliError> {
    let here = here::open()?;
    // Held from before the log is read until the rename, so an export that
    // read an older log cannot land after a newer one.
    let exports = Exports::lock(&here.home, here.workspace.id)?;
    let checked = export::stage(&here, &exports.dir)?;
    let today = jiff::Zoned::now().date();
    let newest = newest_export(&exports.dir, today)?;
    let ops = checked.ops();
    let mut warnings = Vec::new();
    let held = match newest.as_ref().map(|newest| (newest, fs::read(newest))) {
        Some((_, Ok(held))) => Some(held),
        Some((_, Err(err))) if err.kind() == io::ErrorKind::NotFound => None,
        Some((newest, Err(err))) => {
            warnings.push(format!(
                "{}: {err}; wrote a new export rather than compare with it",
                newest.display()
            ));
            None
        }
        None => None,
    };
    // Kept only while it would still restore, header included.
    let unchanged = held.is_some_and(|held| {
        tm_store::read_export(&mut held.as_slice()).ok() == Some((here.workspace.id, ops))
            && after_header(&held) == after_header(checked.bytes())
    });
    let file = if let (true, Some(newest)) = (unchanged, newest) {
        newest
    } else {
        let target = exports.file(today);
        checked.persist(&target)?;
        target
    };
    let text = if unchanged {
        format!("Nothing changed since {}; wrote nothing\n", file.display())
    } else {
        format!("Wrote {} ({ops} operations)\n", file.display())
    };
    let mut output = Output::new(
        text,
        json!({ "file": file.display().to_string(), "ops": ops, "written": !unchanged }),
    );
    let (policy, unread) = policy(&here.home);
    warnings.extend(exports.collect(today, &policy));
    warnings.extend(unread);
    output.warnings = warnings;
    Ok(output)
}

/// The newest dated export in `dir` up to `today`, among the regular files
/// tm names; one dated later, left by a clock that ran ahead, is passed over.
fn newest_export(dir: &Path, today: Date) -> Result<Option<PathBuf>, CliError> {
    let entries = fs::read_dir(dir).map_err(|err| CliError::io(dir, &err))?;
    Ok(entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let date = date_of(entry.file_name().to_str()?)?;
            (date <= today).then(|| (date, entry.path()))
        })
        .max()
        .map(|(_, path)| path))
}

/// An export without its header line, which carries the time it was made.
fn after_header(export: &[u8]) -> &[u8] {
    export
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(&[][..], |end| &export[end + 1..])
}

#[derive(Debug, PartialEq, Eq)]
enum Held {
    Backup,
    /// Nothing, or an empty file, which a backup replaces.
    Missing,
    /// Something tm did not write there, which it leaves alone.
    Other,
}

fn held(path: &Path) -> Held {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => Held::Backup,
        Ok(meta) if meta.is_file() => Held::Missing,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Held::Missing,
        Ok(_) | Err(_) => Held::Other,
    }
}

/// Writes today's backup unless another process wrote it first, and returns
/// the still-locked directory for collection. A holder that keeps the lock
/// past [`WAIT`] is an error, since it may write no backup.
fn back_up(here: &Here, today: Date) -> Result<Option<Exports>, CliError> {
    let Some(exports) = Exports::try_lock(&here.home, here.workspace.id, WAIT)? else {
        return Err(CliError::new(
            "io",
            format!(
                "another tm process held {} for {} s; a later write tries again",
                dir(&here.home, here.workspace.id).join(".lock").display(),
                WAIT.as_secs()
            ),
        ));
    };
    let file = exports.file(today);
    match held(&file) {
        Held::Backup => return Ok(None),
        Held::Missing => {}
        Held::Other => {
            return Err(CliError::new(
                "io",
                format!(
                    "{} is not a file tm wrote, so it was left alone and old backups were not removed",
                    file.display()
                ),
            ));
        }
    }
    export::stage(here, &exports.dir)?.persist(&file)?;
    Ok(Some(exports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Date {
        text.parse().unwrap()
    }

    #[test]
    fn collection_keeps_each_tier_of_daily_backups() {
        let today = date("2026-10-22");
        // Oldest first, so the newest of a week or month is not the first seen.
        let dates: Vec<Date> = (0..400).rev().map(|days| today - days.days()).collect();
        let kept = keep(&dates, today, &Policy::default());
        let mut expected: BTreeSet<Date> = (0..14).map(|days| today - days.days()).collect();
        for newest in [
            // Sundays, ending the last 8 ISO weeks.
            "2026-10-04",
            "2026-09-27",
            "2026-09-20",
            "2026-09-13",
            "2026-09-06",
            // Month ends, for the last 12 months.
            "2026-09-30",
            "2026-08-31",
            "2026-07-31",
            "2026-06-30",
            "2026-05-31",
            "2026-04-30",
            "2026-03-31",
            "2026-02-28",
            "2026-01-31",
            "2025-12-31",
            "2025-11-30",
        ] {
            expected.insert(date(newest));
        }
        assert_eq!(kept, expected);
    }

    #[test]
    fn collection_keeps_the_newest_of_a_week_or_month_however_sparse() {
        let today = date("2026-10-22");
        let policy = Policy {
            keep_days: 0,
            ..Policy::default()
        };
        let dates = [
            date("2026-10-20"),
            date("2026-10-19"),
            date("2026-10-02"),
            date("2026-10-01"),
        ];
        let kept = keep(&dates, today, &policy);
        let expected = [date("2026-10-20"), date("2026-10-02")];
        assert_eq!(kept, BTreeSet::from(expected));
    }

    #[test]
    fn collection_always_keeps_the_newest_backup_and_any_dated_later() {
        let none = Policy {
            enabled: true,
            keep_days: 0,
            keep_weeks: 0,
            keep_months: 0,
        };
        let dates = [date("2019-03-01"), date("2020-01-01"), date("2999-01-01")];
        let kept = keep(&dates, date("2026-10-22"), &none);
        assert_eq!(
            kept,
            BTreeSet::from([date("2020-01-01"), date("2999-01-01")])
        );
    }

    #[test]
    fn a_policy_that_could_not_be_read_keeps_every_backup() {
        let today = date("2026-10-22");
        // Old days in one week and one month, which a tier alone would thin.
        let dates: Vec<Date> = (100..130).map(|days| today - days.days()).collect();
        let kept = keep(&dates, today, &Policy::keep_all());
        assert_eq!(kept, dates.into_iter().collect());
    }

    #[test]
    fn only_tm_backup_names_have_a_date() {
        assert_eq!(date_of("2026-10-22.jsonl"), Some(date("2026-10-22")));
        for name in [
            "2026-10-22.json",
            "2026-10-22.jsonl.bak",
            "2026-1-2.jsonl",
            "notes.jsonl",
            "2026-13-01.jsonl",
        ] {
            assert_eq!(date_of(name), None, "{name}");
        }
    }

    #[test]
    fn a_config_sets_what_it_names_and_refuses_unknown_keys() {
        let root = std::env::temp_dir().join(format!("tm-policy-{}", Uuid::now_v7()));
        fs::create_dir_all(&root).unwrap();
        let missing = policy(&root);
        fs::write(root.join("config.toml"), "[backups]\nkeep_days = 3\n").unwrap();
        let partial = policy(&root);
        fs::write(root.join("config.toml"), "[backups]\nkeep_day = 3\n").unwrap();
        let key = policy(&root);
        fs::write(root.join("config.toml"), "[backup]\nenabled = false\n").unwrap();
        let table = policy(&root);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(missing, (Policy::default(), None));
        assert_eq!(
            partial,
            (
                Policy {
                    keep_days: 3,
                    ..Policy::default()
                },
                None
            )
        );
        for (refused, named) in [(key, "keep_day"), (table, "backup")] {
            assert_eq!(refused.0, Policy::keep_all());
            assert!(refused.1.is_some_and(|warning| warning.contains(named)));
        }
    }

    #[test]
    fn only_a_nonempty_file_counts_as_the_days_backup() {
        let root = std::env::temp_dir().join(format!("tm-held-{}", Uuid::now_v7()));
        fs::create_dir_all(root.join("dir")).unwrap();
        fs::write(root.join("full"), "{}\n").unwrap();
        fs::write(root.join("empty"), "").unwrap();
        std::os::unix::fs::symlink(root.join("gone"), root.join("dangling")).unwrap();
        std::os::unix::fs::symlink(root.join("full"), root.join("link")).unwrap();
        let found: Vec<Held> = ["full", "empty", "missing", "dir", "dangling", "link"]
            .iter()
            .map(|name| held(&root.join(name)))
            .collect();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            found,
            [
                Held::Backup,
                Held::Missing,
                Held::Missing,
                Held::Other,
                Held::Other,
                Held::Other
            ]
        );
    }
}
